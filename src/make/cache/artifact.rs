//! What one invocation leaves for the next, apart from the graph itself.
//!
//! Per unit, everything a read depended on outside its own text and
//! everything a replay of that read needs to be handed: the ground answers IN
//! ASK ORDER with the nesting count each carries, the answers a recipe
//! expanded at compile time asked outside the journal, the environment names
//! the read looked up, and a digest of every makefile it was given. The record
//! in memory ([`super::super::record::CompositionRecord`]) is keyed by question
//! and exists to refuse a composition that answered one question two ways;
//! this is the sequence, and one sequence serves both checking and replay.
//!
//! Digests rather than bytes for the makefiles: the kernel's are fifty
//! megabytes, and a reader that checks a digest has just read the bytes it
//! would have stored.

use crate::build::LateStep;
use crate::htab::rapidhashv1;
use crate::make::ChildOrigin;
use crate::make::layout::{CommandLayout, SettledSteps};
use crate::subprocess::Launch;
use kati::build_sink::DeferredRecipeId;
use kati::bytes::Bytes;
use kati::session::{GroundAnswer, GroundQuestion};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
#[cfg(test)]
use {crate::subprocess::DirectLaunch, crate::util::BString};

/// What the reader checks before it believes a byte of the rest.
///
/// The version is in the directory name too, so a mismatch here is a file
/// written by something else entirely rather than by an older Ronin.
const MAGIC: &[u8] = b"ronin-make-record\x00";

/// One unit's read, as a later invocation checks and replays it.
pub(crate) struct UnitArtifact {
    /// The unit's compilation key.
    pub(crate) key: Vec<u8>,
    /// Where the process stood while this unit was read, and so where its
    /// questions have to be asked again to be asked at all.
    pub(crate) directory: PathBuf,
    pub(crate) ground: Vec<GroundAnswer>,
    pub(crate) off_journal: Vec<GroundAnswer>,
    pub(crate) environment: Vec<(Bytes, Option<Bytes>)>,
    /// The makefiles the read was given, each with a digest of its bytes. A
    /// digest rather than a date: a file rewritten to the same contents is the
    /// same read, and one restored from a backup with an old date is not.
    pub(crate) sources: Vec<(OsString, u64)>,
    /// How the unit was invoked; `None` for the root.
    pub(crate) origin: Option<ChildOrigin>,
}

/// Everything one invocation recorded.
pub(crate) struct Artifact {
    /// The digest of the graph file this describes.
    pub(crate) graph: u64,
    pub(crate) units: Vec<UnitArtifact>,
    /// `None` for a composition that never settled through the compiler-input
    /// build, which a reader has nothing to build from.
    pub(crate) build: Option<BuildArtifact>,
}

impl Artifact {
    /// What `settled` read, described for the graph whose bytes hash to
    /// `graph`.
    pub(crate) fn of(settled: &crate::make::Groundwork, graph: u64) -> Self {
        let mut units: Vec<UnitArtifact> = settled
            .read_units
            .iter()
            .map(|(key, journal)| UnitArtifact {
                key: key.clone(),
                directory: journal.directory.clone(),
                ground: journal.ground.clone(),
                off_journal: journal.off_journal.clone(),
                environment: journal.environment.clone(),
                sources: journal
                    .sources
                    .iter()
                    .map(|(name, contents)| (name.clone(), rapidhashv1(contents.as_ref())))
                    .collect(),
                origin: journal.origin.as_deref().cloned(),
            })
            .collect();
        // In key order, so that one composition is one file.
        units.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        Self {
            graph,
            units,
            build: settled.build.clone(),
        }
    }

    pub(crate) fn encode(&self, out: &mut dyn Write) -> io::Result<()> {
        let mut w = Writer { out };
        w.raw(MAGIC)?;
        w.u32(super::FORMAT_VERSION)?;
        w.u64(self.graph)?;
        w.len(self.units.len())?;
        for unit in &self.units {
            w.bytes(&unit.key)?;
            w.bytes(unit.directory.as_os_str().as_encoded_bytes())?;
            w.answers(&unit.ground)?;
            w.answers(&unit.off_journal)?;
            w.len(unit.environment.len())?;
            for (name, value) in &unit.environment {
                w.bytes(name)?;
                w.option(value.as_ref(), |w, value| w.bytes(value))?;
            }
            w.len(unit.sources.len())?;
            for (name, digest) in &unit.sources {
                w.bytes(name.as_encoded_bytes())?;
                w.u64(*digest)?;
            }
            w.option(unit.origin.as_ref(), encode_origin)?;
        }
        w.option(self.build.as_ref(), |w, build| build.encode(w))
    }

    /// The artifact `bytes` holds, or `None` for bytes that do not hold one.
    #[cfg(test)]
    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(MAGIC.len())? != MAGIC || r.u32()? != super::FORMAT_VERSION {
            return None;
        }
        let graph = r.u64()?;
        let mut units = Vec::new();
        for _ in 0..r.len()? {
            let key = r.bytes()?.to_vec();
            let directory = PathBuf::from(os_string(r.bytes()?));
            let ground = r.answers()?;
            let off_journal = r.answers()?;
            let mut environment = Vec::new();
            for _ in 0..r.len()? {
                let name = Bytes::from(r.bytes()?.to_vec());
                let value = r.option(|r| r.bytes().map(|value| Bytes::from(value.to_vec())))?;
                environment.push((name, value));
            }
            let mut sources = Vec::new();
            for _ in 0..r.len()? {
                let name = os_string(r.bytes()?);
                sources.push((name, r.u64()?));
            }
            let origin = r.option(decode_origin)?;
            units.push(UnitArtifact {
                key,
                directory,
                ground,
                off_journal,
                environment,
                sources,
                origin,
            });
        }
        let build = r.option(BuildArtifact::decode)?;
        (r.at == bytes.len()).then_some(Self {
            graph,
            units,
            build,
        })
    }
}

#[cfg(test)]
fn os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt as _;
    OsString::from_vec(bytes.to_vec())
}

/// One byte standing for a kind of question.
///
/// Written out rather than derived from the enum's own ordering, so that
/// adding a variant to [`GroundQuestion`] cannot silently change what an
/// already-written record means.
const fn question_tag(question: GroundQuestion) -> u8 {
    match question {
        GroundQuestion::Shell => 1,
        GroundQuestion::Wildcard => 2,
        GroundQuestion::RealPath => 3,
        GroundQuestion::FileRead => 4,
        GroundQuestion::Glob => 5,
        GroundQuestion::Include => 6,
    }
}

#[cfg(test)]
const fn question_of(tag: u8) -> Option<GroundQuestion> {
    Some(match tag {
        1 => GroundQuestion::Shell,
        2 => GroundQuestion::Wildcard,
        3 => GroundQuestion::RealPath,
        4 => GroundQuestion::FileRead,
        5 => GroundQuestion::Glob,
        6 => GroundQuestion::Include,
        _ => return None,
    })
}

struct Writer<'a> {
    out: &'a mut dyn Write,
}

impl Writer<'_> {
    fn raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)
    }

    fn u8(&mut self, value: u8) -> io::Result<()> {
        self.raw(&[value])
    }

    fn u32(&mut self, value: u32) -> io::Result<()> {
        self.raw(&value.to_le_bytes())
    }

    fn u64(&mut self, value: u64) -> io::Result<()> {
        self.raw(&value.to_le_bytes())
    }

    fn len(&mut self, value: usize) -> io::Result<()> {
        self.u64(value as u64)
    }

    fn bytes(&mut self, value: &[u8]) -> io::Result<()> {
        self.len(value.len())?;
        self.raw(value)
    }

    fn option<T>(
        &mut self,
        value: Option<T>,
        put: impl FnOnce(&mut Self, T) -> io::Result<()>,
    ) -> io::Result<()> {
        match value {
            None => self.u8(0),
            Some(value) => {
                self.u8(1)?;
                put(self, value)
            }
        }
    }

    fn answers(&mut self, answers: &[GroundAnswer]) -> io::Result<()> {
        self.len(answers.len())?;
        for answer in answers {
            self.u8(question_tag(answer.question))?;
            self.bytes(&answer.asked)?;
            self.bytes(&answer.answer)?;
            self.option(answer.status, |w, status| w.raw(&status.to_le_bytes()))?;
            self.len(answer.nested())?;
        }
        Ok(())
    }
}

#[cfg(test)]
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

#[cfg(test)]
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let taken = self.bytes.get(self.at..self.at.checked_add(count)?)?;
        self.at += count;
        Some(taken)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|byte| byte[0])
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|bytes| u64::from_le_bytes(bytes.try_into().expect("eight bytes")))
    }

    fn len(&mut self) -> Option<usize> {
        usize::try_from(self.u64()?).ok()
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.len()?;
        self.take(len)
    }

    #[expect(
        clippy::option_option,
        reason = "the outer `None` is a refused file; the inner is the file's own absent value"
    )]
    fn option<T>(&mut self, get: impl FnOnce(&mut Self) -> Option<T>) -> Option<Option<T>> {
        match self.u8()? {
            0 => Some(None),
            1 => get(self).map(Some),
            _ => None,
        }
    }

    fn answers(&mut self) -> Option<Vec<GroundAnswer>> {
        let count = self.len()?;
        if count > self.bytes.len() {
            return None;
        }
        let mut answers = Vec::with_capacity(count);
        for _ in 0..count {
            let question = question_of(self.u8()?)?;
            let asked = Bytes::from(self.bytes()?.to_vec());
            let answer = Bytes::from(self.bytes()?.to_vec());
            let status = self.option(|r| {
                r.take(4)
                    .map(|bytes| i32::from_le_bytes(bytes.try_into().expect("four bytes")))
            })?;
            let nested = self.len()?;
            answers.push(GroundAnswer::recorded(
                question, asked, answer, status, nested,
            ));
        }
        Some(answers)
    }
}

/// How a child unit was invoked, as a run that did not compose it can invoke
/// it again. See [`crate::make::cli::subninja::ChildOrigin`].
fn encode_origin(w: &mut Writer<'_>, origin: &ChildOrigin) -> io::Result<()> {
    w.len(origin.words.len())?;
    for word in &origin.words {
        w.bytes(word)?;
    }
    w.bytes(origin.directory.as_os_str().as_encoded_bytes())?;
    w.bytes(origin.parent_makeflags.as_bytes())?;
    w.option(origin.gnumakeflags.as_deref(), |w, flags| {
        w.bytes(flags.as_bytes())
    })?;
    w.len(origin.environment.len())?;
    for (name, value) in origin.environment.iter() {
        w.bytes(name.as_encoded_bytes())?;
        w.bytes(value.as_encoded_bytes())?;
    }
    w.len(origin.level)?;
    w.recipe_environment(&origin.recipe_environment)
}

#[cfg(test)]
fn decode_origin(r: &mut Reader<'_>) -> Option<ChildOrigin> {
    let mut words = Vec::new();
    for _ in 0..r.len()? {
        words.push(BString::from(r.bytes()?));
    }
    let directory = PathBuf::from(os_string(r.bytes()?));
    let parent_makeflags = String::from_utf8(r.bytes()?.to_vec()).ok()?;
    let gnumakeflags = match r.option(|r| r.bytes().map(<[u8]>::to_vec))? {
        None => None,
        Some(flags) => Some(String::from_utf8(flags).ok()?),
    };
    let mut environment = Vec::new();
    for _ in 0..r.len()? {
        let name = os_string(r.bytes()?);
        environment.push((name, os_string(r.bytes()?)));
    }
    let level = r.len()?;
    let recipe_environment = r.recipe_environment()?;
    Some(ChildOrigin {
        words,
        directory,
        parent_makeflags,
        gnumakeflags,
        environment: std::sync::Arc::new(environment),
        level,
        recipe_environment,
    })
}

/// One unit that left recipes for the build to expand, and what wraps every
/// command it produces.
#[derive(Clone)]
pub(crate) struct RecipeUnitArtifact {
    pub(crate) key: Vec<u8>,
    pub(crate) layout: CommandLayout,
    pub(crate) directory: PathBuf,
}

/// Everything the build over a loaded graph needs that is not in the graph.
///
/// Edges are named by their first output's path rather than by index, so the
/// record does not depend on how the graph numbered them.
#[derive(Clone)]
pub(crate) struct BuildArtifact {
    /// The Makefiles the final read consulted that a rule says how to remake,
    /// in the order the read reached them.
    pub(crate) remakes: Vec<Vec<u8>>,
    pub(crate) forgiven: Vec<Vec<u8>>,
    pub(crate) unread: Vec<Vec<u8>>,
    pub(crate) complaints: Vec<(Vec<u8>, String)>,
    /// Every pass's staged work in pass order, the makefile-phase segments and
    /// the goal-phase ones apart, each name once.
    pub(crate) staged_for_makefiles: Vec<Vec<u8>>,
    pub(crate) staged_for_goals: Vec<Vec<u8>>,
    /// The recursive wrappers the composition settled clean, each with the
    /// staged work its freshness was read from.
    pub(crate) clean_wrappers: Vec<(Vec<u8>, Vec<Vec<u8>>)>,
    pub(crate) units: Vec<RecipeUnitArtifact>,
    /// Each deferred edge: its output, the unit whose evaluator expands it, and
    /// which of that unit's recipes it is.
    pub(crate) deferred: Vec<(Vec<u8>, Vec<u8>, DeferredRecipeId)>,
    pub(crate) settled: Vec<(Vec<u8>, SettledSteps)>,
    /// The root unit's settled `MAKEFLAGS` and the widest budget any unit asked
    /// to run at.
    pub(crate) makeflags: String,
    pub(crate) job_budget: usize,
}

impl BuildArtifact {
    fn encode(&self, w: &mut Writer<'_>) -> io::Result<()> {
        for names in [&self.remakes, &self.forgiven, &self.unread] {
            w.names(names)?;
        }
        w.len(self.complaints.len())?;
        for (name, complaint) in &self.complaints {
            w.bytes(name)?;
            w.bytes(complaint.as_bytes())?;
        }
        w.names(&self.staged_for_makefiles)?;
        w.names(&self.staged_for_goals)?;
        w.len(self.clean_wrappers.len())?;
        for (wrapper, staged) in &self.clean_wrappers {
            w.bytes(wrapper)?;
            w.names(staged)?;
        }
        w.len(self.units.len())?;
        for unit in &self.units {
            w.bytes(&unit.key)?;
            w.layout(&unit.layout)?;
            w.bytes(unit.directory.as_os_str().as_encoded_bytes())?;
        }
        w.len(self.deferred.len())?;
        for (output, unit, recipe) in &self.deferred {
            w.bytes(output)?;
            w.bytes(unit)?;
            w.len(*recipe)?;
        }
        w.len(self.settled.len())?;
        for (output, steps) in &self.settled {
            w.bytes(output)?;
            let (ordinary, while_remaking) = steps.parts();
            w.steps(ordinary)?;
            w.option(while_remaking, Writer::steps)?;
        }
        w.bytes(self.makeflags.as_bytes())?;
        w.len(self.job_budget)
    }

    #[cfg(test)]
    fn decode(r: &mut Reader<'_>) -> Option<Self> {
        let (remakes, forgiven, unread) = (r.names()?, r.names()?, r.names()?);
        let mut complaints = Vec::new();
        for _ in 0..r.len()? {
            let name = r.bytes()?.to_vec();
            complaints.push((name, String::from_utf8(r.bytes()?.to_vec()).ok()?));
        }
        let staged_for_makefiles = r.names()?;
        let staged_for_goals = r.names()?;
        let mut clean_wrappers = Vec::new();
        for _ in 0..r.len()? {
            let wrapper = r.bytes()?.to_vec();
            clean_wrappers.push((wrapper, r.names()?));
        }
        let mut units = Vec::new();
        for _ in 0..r.len()? {
            let key = r.bytes()?.to_vec();
            let layout = r.layout()?;
            let directory = PathBuf::from(os_string(r.bytes()?));
            units.push(RecipeUnitArtifact {
                key,
                layout,
                directory,
            });
        }
        let mut deferred = Vec::new();
        for _ in 0..r.len()? {
            let output = r.bytes()?.to_vec();
            let unit = r.bytes()?.to_vec();
            deferred.push((output, unit, r.len()?));
        }
        let mut settled = Vec::new();
        for _ in 0..r.len()? {
            let output = r.bytes()?.to_vec();
            let ordinary = r.steps()?;
            let while_remaking = r.option(Reader::steps)?;
            settled.push((output, SettledSteps::from_parts(ordinary, while_remaking)));
        }
        let makeflags = String::from_utf8(r.bytes()?.to_vec()).ok()?;
        let job_budget = r.len()?;
        Some(Self {
            remakes,
            forgiven,
            unread,
            complaints,
            staged_for_makefiles,
            staged_for_goals,
            clean_wrappers,
            units,
            deferred,
            settled,
            makeflags,
            job_budget,
        })
    }
}

impl Writer<'_> {
    fn names(&mut self, names: &[Vec<u8>]) -> io::Result<()> {
        self.len(names.len())?;
        names.iter().try_for_each(|name| self.bytes(name))
    }

    fn recipe_environment(
        &mut self,
        environment: &[(OsString, Option<OsString>)],
    ) -> io::Result<()> {
        self.len(environment.len())?;
        for (name, value) in environment {
            self.bytes(name.as_encoded_bytes())?;
            self.option(value.as_ref(), |w, value| w.bytes(value.as_encoded_bytes()))?;
        }
        Ok(())
    }

    fn layout(&mut self, layout: &CommandLayout) -> io::Result<()> {
        self.bytes(layout.command_directory.as_os_str().as_encoded_bytes())?;
        self.len(layout.recipe_environment.len())?;
        for (name, value) in &layout.recipe_environment {
            self.bytes(name)?;
            self.option(value.as_deref(), Writer::bytes)?;
        }
        self.bytes(layout.root_directory.as_os_str().as_encoded_bytes())?;
        self.u8(u8::from(layout.root))?;
        self.option(layout.unreadable.as_deref(), |w, why| {
            w.bytes(why.as_bytes())
        })
    }

    fn steps(&mut self, steps: &[LateStep]) -> io::Result<()> {
        self.len(steps.len())?;
        for step in steps {
            match &step.launch {
                Launch::Shell(command) => {
                    self.u8(0)?;
                    self.bytes(command)?;
                }
                Launch::Direct(direct) => {
                    self.u8(1)?;
                    self.len(direct.argv.len())?;
                    for word in &direct.argv {
                        self.bytes(word)?;
                    }
                    self.bytes(direct.directory.as_os_str().as_encoded_bytes())?;
                    self.recipe_environment(&direct.environment)?;
                    self.bytes(direct.diagnostic_prefix.as_bytes())?;
                    self.u8(u8::from(direct.starts_no_process))?;
                }
                Launch::Refused(why) => {
                    self.u8(2)?;
                    self.bytes(why.as_bytes())?;
                }
            }
            self.u8(u8::from(step.ignore_errors))?;
            self.u8(u8::from(step.runs_while_pretending))?;
        }
        Ok(())
    }
}

#[cfg(test)]
impl Reader<'_> {
    fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    fn string(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }

    fn names(&mut self) -> Option<Vec<Vec<u8>>> {
        let count = self.len()?;
        if count > self.bytes.len() {
            return None;
        }
        (0..count)
            .map(|_| self.bytes().map(<[u8]>::to_vec))
            .collect()
    }

    fn recipe_environment(&mut self) -> Option<Vec<(OsString, Option<OsString>)>> {
        let mut environment = Vec::new();
        for _ in 0..self.len()? {
            let name = os_string(self.bytes()?);
            let value = self.option(|r| r.bytes().map(os_string))?;
            environment.push((name, value));
        }
        Some(environment)
    }

    fn layout(&mut self) -> Option<CommandLayout> {
        let command_directory = PathBuf::from(os_string(self.bytes()?));
        let mut recipe_environment = Vec::new();
        for _ in 0..self.len()? {
            let name = self.bytes()?.to_vec();
            let value = self.option(|r| r.bytes().map(<[u8]>::to_vec))?;
            recipe_environment.push((name, value));
        }
        let root_directory = PathBuf::from(os_string(self.bytes()?));
        let root = self.bool()?;
        let unreadable = self.option(Reader::string)?;
        Some(CommandLayout {
            command_directory,
            recipe_environment,
            root_directory,
            root,
            unreadable,
        })
    }

    fn steps(&mut self) -> Option<Vec<LateStep>> {
        let count = self.len()?;
        if count > self.bytes.len() {
            return None;
        }
        let mut steps = Vec::with_capacity(count);
        for _ in 0..count {
            let launch = match self.u8()? {
                0 => Launch::Shell(BString::from(self.bytes()?)),
                1 => {
                    let mut argv = Vec::new();
                    for _ in 0..self.len()? {
                        argv.push(BString::from(self.bytes()?));
                    }
                    let directory = PathBuf::from(os_string(self.bytes()?));
                    let environment = self.recipe_environment()?;
                    let diagnostic_prefix = self.string()?;
                    let starts_no_process = self.bool()?;
                    Launch::Direct(Box::new(DirectLaunch {
                        argv,
                        directory,
                        environment,
                        diagnostic_prefix,
                        starts_no_process,
                    }))
                }
                2 => Launch::Refused(self.string()?),
                _ => return None,
            };
            steps.push(LateStep {
                launch,
                ignore_errors: self.bool()?,
                runs_while_pretending: self.bool()?,
            });
        }
        Some(steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_journal() -> crate::make::UnitJournal {
        crate::make::UnitJournal {
            ground: Vec::new(),
            sources: Vec::new(),
            substrate: None,
            read: None,
            off_journal: Vec::new(),
            environment: Vec::new(),
            directory: PathBuf::from("/x"),
            origin: None,
        }
    }

    fn answer(question: GroundQuestion, asked: &str, answer: &str, nested: usize) -> GroundAnswer {
        GroundAnswer::recorded(
            question,
            Bytes::from(asked.as_bytes().to_vec()),
            Bytes::from(answer.as_bytes().to_vec()),
            (question == GroundQuestion::Shell).then_some(0),
            nested,
        )
    }

    fn artifact() -> Artifact {
        Artifact {
            graph: 0x1234_5678_9abc_def0,
            units: vec![
                UnitArtifact {
                    key: b"root".to_vec(),
                    directory: PathBuf::from("/src"),
                    ground: vec![
                        answer(GroundQuestion::Shell, "uname", "Linux", 2),
                        answer(GroundQuestion::Wildcard, "*.c", "a.c b.c", 0),
                        answer(GroundQuestion::Include, "gen.mk", "", 0),
                    ],
                    off_journal: vec![answer(GroundQuestion::Shell, "cat stamp", "ready", 0)],
                    environment: vec![
                        (Bytes::from_static(b"CC"), Some(Bytes::from_static(b"gcc"))),
                        (Bytes::from_static(b"UNSET"), None),
                    ],
                    sources: vec![(OsString::from("Makefile"), 42)],
                    origin: None,
                },
                UnitArtifact {
                    key: b"sub".to_vec(),
                    directory: PathBuf::from("/src/sub"),
                    ground: Vec::new(),
                    off_journal: Vec::new(),
                    environment: Vec::new(),
                    sources: Vec::new(),
                    origin: Some(ChildOrigin {
                        words: vec![BString::from("make"), BString::from("-C"), "sub".into()],
                        directory: PathBuf::from("/src/sub"),
                        parent_makeflags: "w".to_owned(),
                        gnumakeflags: None,
                        environment: std::sync::Arc::new(vec![(
                            OsString::from("CC"),
                            OsString::from("gcc"),
                        )]),
                        level: 1,
                        recipe_environment: vec![(OsString::from("MAKELEVEL"), Some("2".into()))],
                    }),
                },
            ],
            build: Some(BuildArtifact {
                remakes: vec![b"gen.mk".to_vec()],
                forgiven: vec![b"gen.mk".to_vec()],
                unread: Vec::new(),
                complaints: vec![(b"gen.mk".to_vec(), "no such file".to_owned())],
                staged_for_makefiles: Vec::new(),
                staged_for_goals: vec![b"gen.txt".to_vec(), b"sub/out".to_vec()],
                clean_wrappers: vec![(b"lib".to_vec(), vec![b"lib/a.o".to_vec()])],
                units: vec![RecipeUnitArtifact {
                    key: b"root".to_vec(),
                    layout: CommandLayout {
                        command_directory: PathBuf::from("/src"),
                        recipe_environment: vec![(b"MAKELEVEL".to_vec(), Some(b"1".to_vec()))],
                        root_directory: PathBuf::from("/src"),
                        root: true,
                        unreadable: Some("bad export".to_owned()),
                    },
                    directory: PathBuf::from("/src"),
                }],
                deferred: vec![(b"a.o".to_vec(), b"root".to_vec(), 3)],
                settled: vec![(
                    b"b.o".to_vec(),
                    SettledSteps::from_parts(
                        vec![LateStep {
                            launch: Launch::Direct(Box::new(DirectLaunch {
                                argv: vec![BString::from("cc"), BString::from("b.c")],
                                directory: PathBuf::new(),
                                environment: vec![(OsString::from("X"), None)],
                                diagnostic_prefix: "make: cc: ".to_owned(),
                                starts_no_process: false,
                            })),
                            ignore_errors: true,
                            runs_while_pretending: false,
                        }],
                        Some(vec![LateStep {
                            launch: Launch::Refused("no value for X".to_owned()),
                            ignore_errors: false,
                            runs_while_pretending: true,
                        }]),
                    ),
                )],
                makeflags: "w -j8".to_owned(),
                job_budget: 8,
            }),
        }
    }

    fn encoded(artifact: &Artifact) -> Vec<u8> {
        let mut bytes = Vec::new();
        artifact.encode(&mut bytes).expect("memory takes the bytes");
        bytes
    }

    fn described(artifact: &Artifact) -> String {
        use std::fmt::Write as _;
        let mut text = format!("graph {:x}\n", artifact.graph);
        for unit in &artifact.units {
            let _ = writeln!(
                text,
                "unit {:?} {:?} env={:?} sources={:?} origin={:?}",
                unit.key, unit.directory, unit.environment, unit.sources, unit.origin
            );
            for (label, answers) in [("ground", &unit.ground), ("off", &unit.off_journal)] {
                for answer in answers {
                    let _ = writeln!(
                        text,
                        "  {label} {:?} {:?} {:?} {:?} nested={}",
                        answer.question,
                        answer.asked,
                        answer.answer,
                        answer.status,
                        answer.nested()
                    );
                }
            }
        }
        if let Some(build) = &artifact.build {
            let _ = writeln!(
                text,
                "build remakes={:?} forgiven={:?} unread={:?} complaints={:?} makefiles={:?} goals={:?} clean={:?} makeflags={:?} budget={}",
                build.remakes,
                build.forgiven,
                build.unread,
                build.complaints,
                build.staged_for_makefiles,
                build.staged_for_goals,
                build.clean_wrappers,
                build.makeflags,
                build.job_budget
            );
            for unit in &build.units {
                let layout = &unit.layout;
                let _ = writeln!(
                    text,
                    "  unit {:?} {:?} {:?} {:?} {:?} root={} {:?}",
                    unit.key,
                    unit.directory,
                    layout.command_directory,
                    layout.recipe_environment,
                    layout.root_directory,
                    layout.root,
                    layout.unreadable
                );
            }
            for (output, unit, recipe) in &build.deferred {
                let _ = writeln!(text, "  deferred {output:?} {unit:?} {recipe}");
            }
            for (output, steps) in &build.settled {
                let (ordinary, remaking) = steps.parts();
                let _ = writeln!(
                    text,
                    "  settled {output:?} {:?} {:?}",
                    ordinary.iter().map(describe_step).collect::<Vec<_>>(),
                    remaking.map(|steps| steps.iter().map(describe_step).collect::<Vec<_>>())
                );
            }
        }
        text
    }

    fn describe_step(step: &LateStep) -> String {
        let launch = match &step.launch {
            Launch::Shell(command) => format!("shell {command:?}"),
            Launch::Direct(direct) => format!(
                "direct {:?} {:?} {:?} {:?} {}",
                direct.argv,
                direct.directory,
                direct.environment,
                direct.diagnostic_prefix,
                direct.starts_no_process
            ),
            Launch::Refused(why) => format!("refused {why:?}"),
        };
        format!(
            "{launch} ignore={} pretending={}",
            step.ignore_errors, step.runs_while_pretending
        )
    }

    #[test]
    fn an_artifact_read_back_is_the_artifact_written() {
        let artifact = artifact();
        let read_back = Artifact::decode(&encoded(&artifact)).expect("the bytes hold a record");
        assert_eq!(described(&read_back), described(&artifact));
    }

    #[test]
    fn the_nesting_count_survives_the_file() {
        let read_back = Artifact::decode(&encoded(&artifact())).expect("a record");
        assert_eq!(read_back.units[0].ground[0].nested(), 2);
    }

    #[test]
    fn every_truncation_is_refused() {
        let bytes = encoded(&artifact());
        for end in 0..bytes.len() {
            assert!(Artifact::decode(&bytes[..end]).is_none(), "cut at {end}");
        }
    }

    #[test]
    fn units_are_written_in_key_order() {
        let mut settled = crate::make::Groundwork::default();
        for key in [b"b".as_slice(), b"a", b"c"] {
            std::sync::Arc::make_mut(&mut settled.read_units).insert(key.to_vec(), empty_journal());
        }
        let artifact = Artifact::of(&settled, 1);
        let keys: Vec<&[u8]> = artifact
            .units
            .iter()
            .map(|unit| unit.key.as_slice())
            .collect();
        assert_eq!(keys, [b"a".as_slice(), b"b", b"c"]);
    }
}
