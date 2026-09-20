//! Rebuilding one composition's evaluators over a graph read from a file.
//!
//! A graph alone cannot decide a kbuild no-op: nearly every edge gets its
//! command by expanding a recipe against the Make evaluator of the unit whose
//! makefile wrote it, and `if_changed` decides out-of-dateness inside that
//! expansion. So a run that loads the graph has to have those evaluators, and
//! it gets them the only honest way: by evaluating each unit again, once,
//! with nothing about the disk carried across from the run that wrote the
//! record.
//!
//! WHAT IS CARRIED AND WHAT IS NOT. What a read is TOLD is carried — the
//! ground answers, in ask order, exactly as the record holds them — and only
//! after every one of them has been put back to the ground and agreed with.
//! What a read DECIDES about the disk is carried nowhere: every freshness
//! question is asked again, by the build, against the disk as it stands.
//!
//! THE TWO CHECKS, AND WHY BOTH ARE NEEDED. Re-asking catches a ground that
//! moved under a replayed answer — a `$(wildcard)` that matches one more file
//! now would otherwise be served the old answer and never noticed, because a
//! replay compares the QUESTION and not what the ground would say. Comparing
//! the journal the read closes with the one the record holds catches the
//! other direction — a read that asked something the record never heard of,
//! which is a read that has left the composition the record describes.
//!
//! A unit that asks a `$(shell)` can only have the second check. Its answer
//! is a launch under an environment nothing records, so [`kati::reask`] will
//! not pretend to re-ask it; that unit is read LIVE, which runs its commands
//! in order, in its own directory, under the environment the read itself
//! builds — because it IS the read — and the journal it closes is then
//! compared with the record whole. If every answer agrees, every input to the
//! read agreed, so the read is the read the record describes and the graph
//! beside it is the graph this read would have emitted.

use crate::htab::rapidhashv1;
use crate::make::cache::artifact::{Artifact, UnitArtifact};
use crate::make::{CarriedRead, Compilation, MakeError};
use kati::build_sink::{BuildSink, SinkEdge, SinkPool, SinkRule};
use kati::ninja::{BuildEvaluation, DeferredRecipes};
use kati::reask::{Reasked, reask};
use kati::session::GroundAnswer;
use kati::symtab::Interner;
use kati::symtab::Symbol;

/// Why a warm run gave up and read the makefiles instead.
///
/// One value rather than a message, because nothing is printed: a run that
/// cannot use the record does what every run did before the record existed,
/// and the user sees a build rather than a complaint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Refused {
    /// A makefile the read was given is not the makefile it was given then.
    Sources,
    /// The record describes a unit this invocation cannot rebuild — a child
    /// whose invocation no longer resolves, or a read that refuses.
    Unit,
    /// The ground answers a recorded question differently now.
    Ground,
}

/// One unit rebuilt: the evaluator a recipe expands against, and the recipes
/// it left unexpanded.
pub(crate) struct RebuiltUnit {
    pub(crate) key: Vec<u8>,
    pub(crate) read: CarriedRead,
    pub(crate) deferred: DeferredRecipes,
    /// What this emission says each of those recipes is FOR, by the output's
    /// own name.
    ///
    /// Held to the record before a single one of them is believed. Two
    /// emissions of one population mint the same numbers only if the two walks
    /// agreed, and a walk that did not is a wrong command rather than a slow
    /// run — this is what turns that into a refusal.
    pub(crate) deferred_outputs: Vec<(Vec<u8>, kati::build_sink::DeferredRecipeId)>,
    /// What this unit's names are qualified by before they reach the graph,
    /// which is what makes a declared name comparable with a graph path. See
    /// [`crate::make::sink::GraphSink`], where the same rule is applied to the
    /// same value while the graph is built.
    pub(crate) path_prefix: std::path::PathBuf,
}

/// A destination that keeps nothing.
///
/// The emission is walked for one thing only: the deferred recipes it hands
/// back, whose numbering is what the record's edge map refers to. The graph
/// the walk would build is the graph already read from the file, so building
/// it again would be work with nowhere to go.
///
/// It answers the same evaluation policy the composing sink asked for,
/// because [`kati::ninja::emit_populated`] refuses a sink that answers
/// differently — which is the check that keeps this from quietly becoming a
/// different emission.
struct Discarded {
    evaluation: BuildEvaluation,
    /// The output each deferred recipe was minted for, as this emission
    /// declared it.
    ///
    /// The one thing kept, because it is what makes a recorded recipe number
    /// checkable rather than merely plausible: the numbers are minted in walk
    /// order, and a walk that disagreed with the recorded one would hand every
    /// edge somebody else's recipe. See [`deferred_outputs`].
    deferred: Vec<(Vec<u8>, kati::build_sink::DeferredRecipeId)>,
    /// The recipe the rule just declared names, waiting for the edge that
    /// runs it.
    awaiting: Option<kati::build_sink::DeferredRecipeId>,
}

impl BuildSink for Discarded {
    fn new_inputs_timing(&self) -> kati::build_sink::NewInputsTiming {
        self.evaluation.new_inputs_timing
    }

    fn shell_evaluation(&self) -> kati::build_sink::ShellEvaluation {
        self.evaluation.shell_evaluation
    }

    fn file_evaluation(&self) -> kati::build_sink::FileEvaluation {
        self.evaluation.file_evaluation
    }

    fn output_evaluation(&self) -> kati::build_sink::OutputEvaluation {
        self.evaluation.output_evaluation
    }

    fn recipe_expansion(&self) -> kati::build_sink::RecipeExpansion {
        self.evaluation.recipe_expansion
    }

    fn start(&mut self, _pools: &[SinkPool<'_>]) -> kati::anyhow::Result<()> {
        Ok(())
    }

    /// A rule is declared immediately before the one edge that runs it, so
    /// the recipe number it names belongs to the edge that arrives next. That
    /// is how the composing sink pairs the two as well.
    fn declare_rule(
        &mut self,
        _names: &dyn Interner,
        rule: &SinkRule<'_>,
    ) -> kati::anyhow::Result<()> {
        self.awaiting = rule.deferred_recipe;
        Ok(())
    }

    fn declare_edge(
        &mut self,
        names: &dyn Interner,
        edge: &SinkEdge<'_>,
    ) -> kati::anyhow::Result<()> {
        if let Some(recipe) = self.awaiting.take() {
            self.deferred
                .push((names.symtab().name(edge.output).to_vec(), recipe));
        }
        Ok(())
    }

    fn set_default_targets(
        &mut self,
        _names: &dyn Interner,
        _targets: &[Symbol],
    ) -> kati::anyhow::Result<()> {
        Ok(())
    }

    fn finish(&mut self) -> kati::anyhow::Result<()> {
        Ok(())
    }
}

/// The makefiles the record says this unit was given, as they stand now.
///
/// `None` where one of them has moved, which is the common miss and the
/// cheapest to find: it is a read of bytes this run would have read anyway,
/// and what comes back is supplied to the session so kati does not read them
/// a second time.
///
/// Digests rather than dates, because a file rewritten to the same contents is
/// the same read and one restored from a backup with an older date is not.
fn sources_as_they_stand(unit: &UnitArtifact) -> Option<Vec<(std::ffi::OsString, Vec<u8>)>> {
    let mut supplied = Vec::with_capacity(unit.sources.len());
    for (name, digest) in &unit.sources {
        let contents = std::fs::read(unit.directory.join(name)).ok()?;
        if rapidhashv1(contents.as_slice()) != *digest {
            return None;
        }
        supplied.push((name.clone(), contents));
    }
    Some(supplied)
}

/// Whether every makefile every unit was given still hashes to what the
/// record holds.
///
/// Asked AGAIN once the staged work has run, and that is the point of it. A
/// cold read composes a unit only after the work its boundary waits for is on
/// the ground, so a generated makefile reaches that read already written; a
/// loaded composition reads everything before any of it is built. Staged work
/// that rewrites one of those texts has therefore composed against the wrong
/// bytes, and the answer is GNU Make's own — read the makefiles again, which
/// is what a restart is.
pub(crate) fn sources_unmoved(artifact: &Artifact) -> bool {
    artifact
        .units
        .iter()
        .all(|unit| sources_as_they_stand(unit).is_some())
}

/// Whether the ground still answers every one of this unit's recorded
/// questions the way the record holds them.
///
/// `Some(false)` is a unit that has to be read live: something in it is a
/// question this cannot put back to the ground, so the read is what asks it.
/// `None` is a ground that has moved, which is the whole record refused.
///
/// The questions are asked from the unit's own directory, because
/// `$(wildcard *.c)` is a question about a directory and the answer to it
/// elsewhere is an answer to a different question.
///
/// A unit holding ONE unanswerable question has none of its others re-asked.
/// They were asked in an order that ran that `$(shell)` first, and its side
/// effects are part of what they were answered against — a probe's scratch
/// directory, say — so re-asking them before the shell has run would refuse a
/// record that is perfectly good, for ever.
fn ground_agrees(session: &kati::session::Session, unit: &UnitArtifact) -> Option<bool> {
    let recorded = unit.ground.iter().chain(unit.off_journal.iter());
    if recorded
        .clone()
        .any(|answer| answer.question == kati::session::GroundQuestion::Shell)
    {
        return Some(false);
    }
    for answer in recorded {
        match reask(session, answer.question, &answer.asked) {
            Reasked::Answered(now) if now == answer.answer => {}
            Reasked::Answered(_) => return None,
            Reasked::Unanswerable => return Some(false),
        }
    }
    Some(true)
}

/// Whether the read looked up the same environment it looked up then.
///
/// Compared with the job budget's address written as a fixed word, for the
/// reason a compilation key is: the address names a FIFO this process created
/// and carries its process id, so it is different on every run and is a fact
/// about the process rather than about the composition. `MAKEFLAGS` and
/// `MFLAGS` both carry it, and a run that compared them raw would miss every
/// time for ever.
fn environment_matches(
    recorded: &[(kati::bytes::Bytes, Option<kati::bytes::Bytes>)],
    now: &[(kati::bytes::Bytes, Option<kati::bytes::Bytes>)],
) -> bool {
    let settled = |value: &Option<kati::bytes::Bytes>| {
        value.as_ref().map(|value| {
            let mut value = value.to_vec();
            crate::make::cli::settle_job_budget_address(&mut value);
            value
        })
    };
    recorded.len() == now.len()
        && recorded
            .iter()
            .zip(now)
            .all(|(was, now)| was.0 == now.0 && settled(&was.1) == settled(&now.1))
}

/// Whether the read just closed was told what the record says it was told.
///
/// Byte for byte and in order, the nesting counts included. Everything a read
/// depends on that is not its own text is in these two sequences and in the
/// environment beside them, so two reads told the same things over the same
/// text are the same read — which is what lets the graph the record was
/// written beside stand in for the one this read would have emitted.
fn journal_matches(recorded: &[GroundAnswer], closed: &[GroundAnswer]) -> bool {
    recorded.len() == closed.len()
        && recorded.iter().zip(closed).all(|(was, now)| {
            was.question == now.question
                && was.asked == now.asked
                && was.answer == now.answer
                && was.status == now.status
                && was.nested() == now.nested()
        })
}

/// Rebuild one unit's evaluator and take the recipes it leaves for the build.
///
/// `live` says the record holds a question this run could not put back to the
/// ground, so nothing is replayed and the read asks the disk for everything —
/// which is what runs the unit's `$(shell)` calls properly. Either way the
/// journal the read closes is held to the record afterwards.
fn rebuild_unit(
    mut compilation: Compilation,
    unit: &UnitArtifact,
    sources: Vec<(std::ffi::OsString, Vec<u8>)>,
    live: bool,
    evaluation: BuildEvaluation,
) -> Result<RebuiltUnit, Refused> {
    let session = &mut compilation.session;
    // `--shuffle` is Make's own reordering rather than this front end's, and
    // the walk that mints the deferred recipe numbers reads the order it
    // chose. A rebuild told nothing about it walks the makefile's written
    // order and mints a PERMUTATION of the numbers the record holds — every
    // edge then expands somebody else's recipe. Measured on GNU Make's own
    // `options/shuffle`, where the second run of a fixed seed echoed the
    // targets in written order.
    session.flags.shuffle = compilation.shuffle;
    session.flags.default_shell_program =
        crate::subprocess::builtin_shell().map(std::path::Path::to_path_buf);
    session.interrupts = Some(std::sync::Arc::clone(&compilation.context.interrupts));
    for (name, contents) in sources {
        session.supply_makefile(name, contents);
    }
    if !live {
        session.ground_journal.replay(unit.ground.clone());
    }
    let ground = std::sync::Arc::clone(&compilation.context.ground);
    let path_prefix = compilation.context.path_prefix.clone();
    let directory = compilation.context.directory.clone();
    let mut prepared = crate::make::parallel::evaluate_unit(
        compilation.session,
        &directory,
        evaluation,
        false,
        &ground,
    )
    .map_err(|_| Refused::Unit)?;
    if !prepared.refusals.is_empty() {
        return Err(Refused::Unit);
    }
    let mut sink = Discarded {
        evaluation,
        deferred: Vec::new(),
        awaiting: None,
    };
    let deferred =
        kati::ninja::emit_populated(&mut prepared.populated, &mut prepared.ev, &mut sink)
            .map_err(|_| Refused::Unit)?;
    let session = &mut prepared.ev.session;
    let ground = session.ground_journal.close_read();
    let off_journal = session.ground_journal.close_off_journal();
    if !journal_matches(&unit.ground, &ground)
        || !journal_matches(&unit.off_journal, &off_journal)
        || !environment_matches(&unit.environment, &session.environment_dependencies())
    {
        return Err(Refused::Ground);
    }
    Ok(RebuiltUnit {
        key: unit.key.clone(),
        read: std::sync::Arc::new(std::sync::Mutex::new(prepared)),
        deferred,
        deferred_outputs: sink.deferred,
        path_prefix,
    })
}

/// Rebuild every unit the record names, from the root's own compilation and
/// each child's recorded invocation.
///
/// The units are independent: a child is built from what the record says it
/// was invoked with rather than from its parent's read, so none of them waits
/// on another and the whole set is one round of evaluation rather than a
/// chain of passes.
pub(crate) fn rebuild(root: Compilation, artifact: &Artifact) -> Result<Vec<RebuiltUnit>, Refused> {
    // Taken from a sink rather than written down, because
    // [`kati::ninja::emit_populated`] refuses a population emitted under a
    // different policy — so a graph sink that changes its mind about one of
    // these refuses the cache rather than quietly emitting something else.
    let evaluation = BuildEvaluation::of(&crate::make::GraphSink::new_at(
        &root.context.root_directory,
        kati::build_sink::RecipeExpansion::Launch,
    ));
    let shared = root.context.clone();
    let work = Work {
        artifact,
        evaluation,
        shared: &shared,
        root_key: root.cache_key.clone(),
        root: std::sync::Mutex::new(Some(root)),
        next: std::sync::atomic::AtomicUsize::new(0),
        built: std::sync::Mutex::new(Ok(Vec::with_capacity(artifact.units.len()))),
    };
    // One unit at a time where the run was told to run one recipe at a time,
    // for the reason the composition's own pool answers to the same number: a
    // serial run reads every makefile where and when it read it before, and
    // the order two units' `$(shell)` commands run in is part of that.
    let threads = shared.parallel_reads.min(artifact.units.len()).min(
        if crate::make::parallel::threads_own_a_directory() {
            usize::MAX
        } else {
            1
        },
    );
    if threads <= 1 {
        take_units(&work, false);
    } else {
        std::thread::scope(|scope| {
            for _ in 0..threads {
                let work = &work;
                let _ = std::thread::Builder::new()
                    .name("ronin-warm-read".to_owned())
                    .spawn_scoped(scope, move || take_units(work, true));
            }
        });
    }
    work.built.into_inner().unwrap_or(Err(Refused::Unit))
}

/// What every thread rebuilding one composition's units shares.
struct Work<'a> {
    artifact: &'a Artifact,
    evaluation: BuildEvaluation,
    /// The root's context, which every child's is derived from.
    shared: &'a crate::make::CompilationContext,
    root_key: Vec<u8>,
    /// The root unit's own compilation, taken by whichever thread reaches it.
    /// The root is the one unit not built from a recorded invocation.
    root: std::sync::Mutex<Option<Compilation>>,
    next: std::sync::atomic::AtomicUsize,
    /// What has been rebuilt, or the first refusal — which stops the rest,
    /// because a composition is believed whole or not at all.
    built: std::sync::Mutex<Result<Vec<RebuiltUnit>, Refused>>,
}

/// Rebuild units until they run out or one of them refuses.
///
/// `own_directory` says this thread must stand in a directory of its own
/// before it reads anything: kati resolves a relative name against the process
/// working directory, and a thread that shares one with another would read
/// each unit against wherever that other thread had moved to.
fn take_units(work: &Work<'_>, own_directory: bool) {
    if own_directory && !crate::make::parallel::own_a_directory() {
        refuse(work, Refused::Unit);
        return;
    }
    loop {
        let at = work.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some(unit) = work.artifact.units.get(at) else {
            return;
        };
        match rebuild_one(work, unit) {
            Ok(built) => {
                let Ok(mut held) = work.built.lock() else {
                    return;
                };
                match held.as_mut() {
                    Ok(units) => units.push(built),
                    Err(_) => return,
                }
            }
            Err(refused) => {
                refuse(work, refused);
                return;
            }
        }
    }
}

/// Stop every thread: a composition is believed whole or not at all, and the
/// first refusal is the one reported.
fn refuse(work: &Work<'_>, refused: Refused) {
    work.next.store(
        work.artifact.units.len(),
        std::sync::atomic::Ordering::Relaxed,
    );
    if let Ok(mut held) = work.built.lock()
        && held.is_ok()
    {
        *held = Err(refused);
    }
}

/// Check one unit's makefiles and its ground, then rebuild its evaluator.
fn rebuild_one(work: &Work<'_>, unit: &UnitArtifact) -> Result<RebuiltUnit, Refused> {
    let sources = sources_as_they_stand(unit).ok_or(Refused::Sources)?;
    let compilation = if unit.key == work.root_key {
        work.root
            .lock()
            .ok()
            .and_then(|mut root| root.take())
            .ok_or(Refused::Unit)?
    } else {
        let origin = unit.origin.as_ref().ok_or(Refused::Unit)?;
        crate::make::cli::subninja::recompose(origin, unit.key.clone(), work.shared)
            .map_err(|_| Refused::Unit)?
    };
    let live = crate::make::in_directory(&compilation.context.directory, || {
        Ok(ground_agrees(&compilation.session, unit))
    })
    .map_err(|_: MakeError| Refused::Unit)?
    .ok_or(Refused::Ground)?;
    rebuild_unit(compilation, unit, sources, !live, work.evaluation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kati::bytes::Bytes;
    use kati::session::GroundQuestion;

    fn answer(question: GroundQuestion, asked: &str, answer: &str) -> GroundAnswer {
        GroundAnswer::recorded(
            question,
            Bytes::from(asked.as_bytes().to_vec()),
            Bytes::from(answer.as_bytes().to_vec()),
            None,
            0,
        )
    }

    fn named(name: &str, value: Option<&str>) -> (Bytes, Option<Bytes>) {
        (
            Bytes::from(name.as_bytes().to_vec()),
            value.map(|value| Bytes::from(value.as_bytes().to_vec())),
        )
    }

    #[test]
    fn one_read_told_the_same_things_matches() {
        let recorded = vec![answer(GroundQuestion::Wildcard, "*.c", "a.c b.c")];
        assert!(journal_matches(&recorded, &recorded.clone()));
    }

    #[test]
    fn an_answer_that_moved_does_not_match() {
        let recorded = vec![answer(GroundQuestion::Wildcard, "*.c", "a.c b.c")];
        let closed = vec![answer(GroundQuestion::Wildcard, "*.c", "a.c b.c c.c")];
        assert!(!journal_matches(&recorded, &closed));
    }

    #[test]
    fn a_question_nobody_recorded_does_not_match() {
        let recorded = vec![answer(GroundQuestion::Wildcard, "*.c", "a.c")];
        let closed = vec![
            answer(GroundQuestion::Wildcard, "*.c", "a.c"),
            answer(GroundQuestion::Wildcard, "*.h", ""),
        ];
        assert!(!journal_matches(&recorded, &closed));
    }

    /// The count says how many answers after this one were asked while it was
    /// being answered, so a read that asked them differently asked a different
    /// sequence even where every answer agrees.
    #[test]
    fn a_different_nesting_count_does_not_match() {
        let recorded = vec![GroundAnswer::recorded(
            GroundQuestion::Include,
            Bytes::from_static(b"gen.mk"),
            Bytes::from_static(b"gen.mk"),
            None,
            1,
        )];
        let closed = vec![GroundAnswer::recorded(
            GroundQuestion::Include,
            Bytes::from_static(b"gen.mk"),
            Bytes::from_static(b"gen.mk"),
            None,
            0,
        )];
        assert!(!journal_matches(&recorded, &closed));
    }

    #[test]
    fn the_job_budget_address_is_not_the_environment() {
        let recorded = vec![named(
            "MAKEFLAGS",
            Some("-j8 --jobserver-auth=fifo:/tmp/ronin-jobserver-11-0"),
        )];
        let now = vec![named(
            "MAKEFLAGS",
            Some("-j8 --jobserver-auth=fifo:/tmp/ronin-jobserver-22-0"),
        )];
        assert!(environment_matches(&recorded, &now));
    }

    #[test]
    fn a_value_that_moved_is_a_different_environment() {
        let recorded = vec![named("CC", Some("gcc"))];
        let now = vec![named("CC", Some("clang"))];
        assert!(!environment_matches(&recorded, &now));
    }

    #[test]
    fn an_unset_variable_is_not_one_set() {
        let recorded = vec![named("CC", None)];
        let now = vec![named("CC", Some(""))];
        assert!(!environment_matches(&recorded, &now));
    }

    fn unit(directory: &std::path::Path, sources: Vec<(std::ffi::OsString, u64)>) -> UnitArtifact {
        UnitArtifact {
            key: b"unit".to_vec(),
            directory: directory.to_owned(),
            ground: Vec::new(),
            off_journal: Vec::new(),
            environment: Vec::new(),
            sources,
            origin: None,
        }
    }

    #[test]
    fn a_makefile_that_stands_is_handed_back() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        std::fs::write(directory.path().join("Makefile"), b"all:\n").expect("a makefile");
        let unit = unit(
            directory.path(),
            vec![(
                std::ffi::OsString::from("Makefile"),
                rapidhashv1(&b"all:\n"[..]),
            )],
        );
        let supplied = sources_as_they_stand(&unit).expect("the makefile stands");
        assert_eq!(
            supplied,
            vec![(std::ffi::OsString::from("Makefile"), b"all:\n".to_vec())]
        );
    }

    #[test]
    fn a_makefile_rewritten_stands_for_nothing() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        std::fs::write(directory.path().join("Makefile"), b"all:\n\t@:\n").expect("a makefile");
        let unit = unit(
            directory.path(),
            vec![(
                std::ffi::OsString::from("Makefile"),
                rapidhashv1(&b"all:\n"[..]),
            )],
        );
        assert!(sources_as_they_stand(&unit).is_none());
    }

    #[test]
    fn a_makefile_that_is_gone_stands_for_nothing() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        let unit = unit(
            directory.path(),
            vec![(
                std::ffi::OsString::from("Makefile"),
                rapidhashv1(&b"all:\n"[..]),
            )],
        );
        assert!(sources_as_they_stand(&unit).is_none());
    }

    /// A launch under an environment nothing recorded is evidence of nothing,
    /// so the unit is read live and the read is what asks it.
    #[test]
    fn a_unit_asking_a_shell_is_read_live() {
        let session = kati::session::Session::from_args(vec![std::ffi::OsString::from("make")])
            .expect("argv");
        let mut held = unit(std::path::Path::new("/"), Vec::new());
        held.ground = vec![answer(GroundQuestion::Shell, "date", "today")];
        assert_eq!(ground_agrees(&session, &held), Some(false));
    }

    #[test]
    fn a_wildcard_that_moved_refuses_the_record() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        std::fs::write(directory.path().join("a.c"), b"").expect("a source");
        let session = kati::session::Session::from_args(vec![std::ffi::OsString::from("make")])
            .expect("argv");
        let mut held = unit(directory.path(), Vec::new());
        held.ground = vec![answer(GroundQuestion::Wildcard, "*.c", "a.c b.c")];
        let asked =
            crate::make::in_directory(directory.path(), || Ok(ground_agrees(&session, &held)))
                .expect("the directory is enterable");
        assert_eq!(asked, None);
    }
}
