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

use crate::htab::rapidhashv1;
use kati::bytes::Bytes;
use kati::session::{GroundAnswer, GroundQuestion};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

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
}

/// Everything one invocation recorded.
pub(crate) struct Artifact {
    /// The digest of the graph file this describes.
    pub(crate) graph: u64,
    pub(crate) units: Vec<UnitArtifact>,
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
            })
            .collect();
        // In key order, so that one composition is one file.
        units.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        Self { graph, units }
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
        }
        Ok(())
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
            units.push(UnitArtifact {
                key,
                directory,
                ground,
                off_journal,
                environment,
                sources,
            });
        }
        (r.at == bytes.len()).then_some(Self { graph, units })
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
                },
                UnitArtifact {
                    key: b"sub".to_vec(),
                    directory: PathBuf::from("/src/sub"),
                    ground: Vec::new(),
                    off_journal: Vec::new(),
                    environment: Vec::new(),
                    sources: Vec::new(),
                },
            ],
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
                "unit {:?} {:?} env={:?} sources={:?}",
                unit.key, unit.directory, unit.environment, unit.sources
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
        text
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
