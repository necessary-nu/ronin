//! Running a graph a front end built.
//!
//! Construction hands back a [`BuildGraph`](super::BuildGraph); this is the
//! other half of what that is for. A front end opens the [`Persistence`] that
//! makes a second build incremental, describes the build it wants through
//! [`Build`], names the targets, and runs the plan it gets back.
//!
//! The engine's own scheduling state stays behind this: a front end says what
//! it wants built and how much of the machine to use, and the decisions that
//! follow from a graph — what is dirty, what order it runs in, what a `restat`
//! rule prunes — are the engine's rather than negotiable options.

use super::{BuildGraph, Node};
use crate::build::{BuildOptions, Builder, JobLimit};
use crate::error::{BuildError, BuildStop, Error, PersistenceError, PersistenceOperation};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::Path;

/// The state that makes a second build incremental.
///
/// Ninja keeps two files beside a build: `.ninja_log` remembers what command
/// produced each output and when, so a changed command line rebuilds what it
/// produced, and `.ninja_deps` remembers the dependencies compilers reported,
/// so a header nothing names in the graph still triggers a rebuild. Both are
/// read once, appended to as the build runs, and flushed by [`Persistence::finish`].
pub struct Persistence {
    pub(crate) build_log: crate::log::BuildLog,
    pub(crate) deps_log: crate::deps::DepsLog,
}

impl Persistence {
    /// Opens both logs for a build in `directory`, creating what is not there.
    ///
    /// `directory` is the build directory. Every front end uses the same two
    /// files there, under Ninja's names and in Ninja's formats.
    ///
    /// The dependency log names paths, so reading it interns them into `graph`;
    /// this is why it takes the graph the builds will run over rather than
    /// standing on its own. Open it once for a graph and reuse it for every
    /// build over that graph, which is what keeps one invocation's appends in
    /// one file.
    ///
    /// The returned warning is the log's own complaint about state it could not
    /// use — a version it does not know, a file a crash truncated. Ninja reports
    /// that and carries on with an empty log, and so does this: it is a warning
    /// rather than a failure because the only cost is a build that rebuilds more
    /// than it had to.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when the directory cannot be created or when either
    /// log exists and cannot be read or reopened for appending.
    // [spec:ronin:req:make.state-outside-the-tree+2]
    pub fn open(graph: &mut BuildGraph, directory: &Path) -> Result<(Self, Option<String>), Error> {
        std::fs::create_dir_all(directory).map_err(|source| {
            PersistenceError::io(
                PersistenceOperation::CreateBuildDirectory,
                directory.to_owned(),
                source,
            )
        })?;
        let build_log = crate::log::BuildLog::open(Some(directory)).map_err(|source| {
            PersistenceError::io(
                PersistenceOperation::OpenBuildLog,
                directory.join(".ninja_log"),
                source,
            )
        })?;
        let deps_path = directory.join(".ninja_deps");
        let (deps_log, warning) = crate::deps::depsloadlog(&deps_path, graph.arenas_mut())
            .map_err(|source| {
                PersistenceError::io(PersistenceOperation::OpenDepsLog, deps_path, source)
            })?;
        Ok((
            Self {
                build_log,
                deps_log,
            },
            warning,
        ))
    }

    /// Flushes both logs.
    ///
    /// Both are flushed whichever fails, because a build log left unwritten
    /// costs the next build a rebuild it did not need.
    ///
    /// # Errors
    ///
    /// Returns the build log's failure if it had one, otherwise the dependency
    /// log's.
    pub fn finish(self) -> Result<(), Error> {
        let build_log_path = self.build_log.path().to_owned();
        let deps_log_path = self.deps_log.path().to_owned();
        let build_log = self.build_log.finish().map_err(|source| {
            PersistenceError::io(PersistenceOperation::FlushBuildLog, build_log_path, source)
        });
        let deps_log = self.deps_log.finish().map_err(|source| {
            PersistenceError::io(PersistenceOperation::FlushDepsLog, deps_log_path, source)
        });
        build_log?;
        deps_log?;
        Ok(())
    }
}

/// How many commands a build runs at once.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Jobs {
    /// One at a time, which is what a build that asks for nothing else gets.
    #[default]
    Serial,
    /// At most this many at once.
    Limit(NonZeroUsize),
    /// Every command whose inputs are ready. A pool's depth, the `console`
    /// pool, and a jobserver still hold back what they hold back.
    Unlimited,
}

impl From<Jobs> for JobLimit {
    fn from(jobs: Jobs) -> Self {
        match jobs {
            Jobs::Serial => Self::Fixed(NonZeroUsize::MIN),
            Jobs::Limit(limit) => Self::Fixed(limit),
            Jobs::Unlimited => Self::Unlimited,
        }
    }
}

/// The build a front end asks for.
///
/// Everything a front end can say about a build it says here, and everything it
/// does not say has a default that does not depend on the front end: one
/// command at a time, stop at the first failure, run the commands rather than
/// print them, and collect the output rather than stream it.
// [spec:ronin:req:frontend.graph-construction]
pub struct Build<'graph, 'sink> {
    graph: &'graph mut BuildGraph,
    persistence: &'graph mut Persistence,
    options: BuildOptions,
    output: Option<&'graph mut (dyn Write + 'sink)>,
    diagnostics: Option<&'graph mut (dyn Write + 'sink)>,
    late_commands: Option<&'graph mut dyn crate::build::LateCommands>,
    /// Whether failing to start reads as an error against the invocation.
    ///
    /// Ninja prefixes what it could not do with the targets it was given that
    /// way and leaves the targets it chose for itself, such as regenerating the
    /// manifest, reported as themselves. A front end phrases its own
    /// diagnostics, so nothing outside this crate sets this.
    pub(crate) invocation_errors: bool,
}

impl<'graph, 'sink> Build<'graph, 'sink> {
    /// A build over `graph`, reading and appending `persistence`.
    #[must_use]
    pub fn new(graph: &'graph mut BuildGraph, persistence: &'graph mut Persistence) -> Self {
        Self {
            graph,
            persistence,
            options: BuildOptions::default(),
            output: None,
            diagnostics: None,
            late_commands: None,
            invocation_errors: false,
        }
    }

    /// A build carrying the Ninja front end's whole command line, which reaches
    /// further than the settings this boundary exposes by name.
    pub(crate) fn with_options(
        graph: &'graph mut BuildGraph,
        persistence: &'graph mut Persistence,
        options: BuildOptions,
    ) -> Self {
        Self {
            options,
            ..Self::new(graph, persistence)
        }
    }

    /// Runs commands `jobs` at a time.
    #[must_use]
    pub fn jobs(mut self, jobs: Jobs) -> Self {
        self.options.jobs = jobs.into();
        self
    }

    /// Stops once `failures` commands have failed, or never when `failures` is
    /// zero.
    ///
    /// The build still stops when everything left to do depends on something
    /// that already failed, since there is nothing left it could run.
    #[must_use]
    pub const fn keep_going(mut self, failures: usize) -> Self {
        self.options.maxfail = if failures == 0 { usize::MAX } else { failures };
        self
    }

    /// Reports the commands a build would run without running any of them.
    #[must_use]
    pub const fn dry_run(mut self, dry_run: bool) -> Self {
        self.options.dryrun = dry_run;
        self
    }

    /// Reports each command in full rather than the description its rule gives.
    #[must_use]
    pub const fn verbose(mut self, verbose: bool) -> Self {
        self.options.verbose = verbose;
        self
    }

    /// Streams progress and command output to `sink` as the build runs.
    ///
    /// Without one the same bytes are collected and handed back by
    /// [`Outcome::output`], which is a whole build's output at once rather than
    /// a running account of it.
    #[must_use]
    pub fn output(mut self, sink: &'graph mut (dyn Write + 'sink)) -> Self {
        self.output = Some(sink);
        self
    }

    /// Streams the build's diagnostics to `sink` as the build runs.
    #[must_use]
    pub fn diagnostics(mut self, sink: &'graph mut (dyn Write + 'sink)) -> Self {
        self.diagnostics = Some(sink);
        self
    }

    /// Binds an edge's command through `recipes` as the edge is launched,
    /// for a front end that did not settle every command when it built the
    /// graph.
    #[must_use]
    pub(crate) fn late_commands(
        mut self,
        recipes: &'graph mut dyn crate::build::LateCommands,
    ) -> Self {
        self.late_commands = Some(recipes);
        self
    }

    /// Works out what has to run for `targets`.
    ///
    /// This is where a build reads the disk: the mtime of everything the
    /// targets reach, the dependencies the last build recorded for them, and
    /// any dyndep file that is ready. Nothing runs yet, so a front end that
    /// finds [`Planned::already_up_to_date`] can stop without having started a
    /// build at all.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when a target needs a file that is missing and
    /// nothing builds it, when the graph reaching a target has a cycle, or when
    /// recorded dependencies cannot be read.
    // [spec:ronin:req:frontend.graph-construction]
    pub fn plan(self, targets: &[Node]) -> Result<Planned<'graph>, Error> {
        let Self {
            graph,
            persistence,
            options,
            output,
            diagnostics,
            late_commands,
            invocation_errors,
        } = self;
        // Every frontend reaches the same Ninja dirtiness and persistence
        // semantics once its graph crosses this boundary.
        let mut builder = Builder::from_parts(
            graph.arenas_mut(),
            options,
            Some(&mut persistence.build_log),
            Some(&mut persistence.deps_log),
            output.map(|sink| sink as &mut dyn Write),
            diagnostics.map(|sink| sink as &mut dyn Write),
        );
        if let Some(recipes) = late_commands {
            builder.late_commands(recipes);
        }
        for target in targets {
            builder.add_target_node(target.0).map_err(|error| {
                if invocation_errors {
                    BuildError::target_context(error)
                } else {
                    error
                }
            })?;
        }
        Ok(Planned {
            builder,
            targets: targets.to_vec(),
        })
    }
}

/// A build that knows what it would run.
pub struct Planned<'graph> {
    builder: Builder<'graph>,
    targets: Vec<Node>,
}

impl Planned<'_> {
    /// Whether the build has no command to run.
    ///
    /// A plan holding only phony edges counts as nothing to do, because a phony
    /// edge produces nothing: Ninja reports such a build as up to date and this
    /// agrees with it.
    #[must_use]
    pub fn already_up_to_date(&self) -> bool {
        self.builder.already_up_to_date()
    }

    /// Whether the plan would really run anything, asking the front end about
    /// every command it binds as an edge launches.
    ///
    /// This is what a Make `-q` answers. It differs from
    /// [`Planned::already_up_to_date`] for exactly the edges whose command is
    /// not known until it is asked for: a recipe that expands to no command
    /// line is planned work that is not work.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when a front end could not produce a command it
    /// was asked for.
    pub(crate) fn interrogate(&mut self) -> Result<bool, Error> {
        Ok(self.builder.interrogate()?)
    }

    /// The files this plan will create only to complete a chain of implicit
    /// rules, which GNU Make deletes once it has finished with them.
    ///
    /// Asked before the build rather than after it: what makes a file eligible
    /// is that the build set out to create it, and once it exists there is
    /// nothing left to tell it from a file that was already there.
    #[must_use]
    pub fn disposable(&self) -> Vec<Vec<u8>> {
        self.builder
            .disposable_outputs()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    /// Runs the plan to completion.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when the build could not be carried out: a command
    /// that could not be started, an output directory that could not be made, a
    /// dependency file a command promised and did not write. A command that
    /// runs and fails is not one of these — that build stops, and
    /// [`Outcome::stopped`] says so.
    // [spec:ronin:req:frontend.graph-construction]
    pub fn run(mut self) -> Result<Outcome, Error> {
        let result = self.builder.build();
        let mut regenerated = Vec::new();
        let mut unmade = Vec::new();
        for target in &self.targets {
            match self.builder.made(target.0) {
                crate::build::Made::Regenerated => regenerated.push(*target),
                crate::build::Made::Failed => unmade.push(*target),
                crate::build::Made::Nothing => {}
            }
        }
        let output = std::mem::take(&mut self.builder.build_output);
        let stopped = match result {
            Err(BuildError::Stopped { reason, status }) => Some((reason, status)),
            other => {
                other?;
                None
            }
        };
        let ran_a_command = self.builder.ran_a_command();
        Ok(Outcome {
            stopped,
            regenerated,
            unmade,
            output,
            ran_a_command,
        })
    }
}

/// How a build ended.
pub struct Outcome {
    pub(crate) stopped: Option<(BuildStop, i32)>,
    regenerated: Vec<Node>,
    unmade: Vec<Node>,
    output: Vec<u8>,
    ran_a_command: bool,
}

impl Outcome {
    /// Why the build stopped short of building everything asked for, absent
    /// when it built all of it.
    ///
    /// The text is the engine's own account, in the words Ninja uses for the
    /// same situations, and is meant to be reported rather than matched on.
    #[must_use]
    pub fn stopped(&self) -> Option<String> {
        self.stopped.as_ref().map(|(reason, _)| reason.to_string())
    }

    /// Whether the build ever had a command to run.
    ///
    /// A plan can hold an edge whose command is only read as the edge is
    /// launched, and reading it is what discovers that there is no command —
    /// so how much work a build did is a fact about the build rather than
    /// about the plan, and only the finished build can be asked.
    pub(crate) const fn ran_a_command(&self) -> bool {
        self.ran_a_command
    }

    /// The front end's own diagnostic, when what stopped the build was a
    /// command the front end was asked for as an edge launched and could not
    /// produce.
    pub(crate) fn front_end_diagnostic(&self) -> Option<&str> {
        match self.stopped.as_ref() {
            Some((crate::error::BuildStop::Failed(error), _)) => error.front_end_diagnostic(),
            _ => None,
        }
    }

    /// The status to leave with: the failing command's own, or zero for a build
    /// that finished.
    ///
    /// Ninja carries a command's status out of the build so a caller can tell a
    /// compiler that rejected the source from one the kernel killed.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        self.stopped.as_ref().map_or(0, |(_, status)| *status)
    }

    /// Which of the targets asked for the build actually regenerated.
    ///
    /// A target is here when its command ran and a `restat` rule did not then
    /// find the output unchanged. This is the question a front end asks about a
    /// build it generated its own input from: Ninja rebuilds its manifest and
    /// reads it again only when this says the manifest changed.
    #[must_use]
    pub fn regenerated(&self) -> &[Node] {
        &self.regenerated
    }

    /// Which of the targets asked for the build tried to make and did not.
    ///
    /// The other half of the same question, for a build allowed to carry on
    /// past a failure: `stopped` says the build as a whole did not finish,
    /// while this says which targets it is true of. A target whose command was
    /// never reached is not here — nothing was tried, and nothing changed.
    pub(crate) fn unmade(&self) -> &[Node] {
        &self.unmade
    }

    /// The build's output, when no sink was there to stream it.
    ///
    /// Empty for a build given a sink through [`Build::output`], which received
    /// the same bytes as they were produced.
    #[must_use]
    pub fn output(&self) -> &[u8] {
        &self.output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{EdgeSpec, Template};

    struct Fixture {
        directory: tempfile::TempDir,
        graph: BuildGraph,
        persistence: Persistence,
        targets: Vec<Node>,
    }

    /// `out` is copied from `mid`, which is copied from the source file `in`.
    ///
    /// Every path is absolute, so the build runs the same wherever the process
    /// happens to be: setting a working directory is a command-line option
    /// rather than something this boundary exposes.
    fn fixture(suffix: &[u8]) -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let path = |name: &str| {
            let mut bytes = directory.path().as_os_str().as_encoded_bytes().to_vec();
            bytes.push(b'/');
            bytes.extend_from_slice(name.as_bytes());
            bytes
        };
        std::fs::write(directory.path().join("in"), b"source\n").unwrap();

        let mut graph = BuildGraph::new();
        let root = graph.root();
        let command = graph.binding(b"command");
        let mut recipe = Template::literal(b"cp ");
        let inputs = graph.binding(b"in");
        recipe.push_variable(inputs);
        recipe.push_literal(b" ");
        let outputs = graph.binding(b"out");
        recipe.push_variable(outputs);
        recipe.push_literal(suffix);
        let copy = graph
            .define_rule(root, b"copy", vec![(command, recipe)])
            .unwrap();

        let source = graph.node(&path("in")).unwrap();
        let middle = graph.node(&path("mid")).unwrap();
        let final_output = graph.node(&path("out")).unwrap();
        for (output, input) in [(middle, source), (final_output, middle)] {
            graph
                .add_edge(EdgeSpec {
                    scope: root,
                    rule: copy,
                    explicit_outputs: &[output],
                    implicit_outputs: &[],
                    explicit_inputs: &[input],
                    implicit_inputs: &[],
                    order_only_inputs: &[],
                    validations: &[],
                    always_dirty: false,
                    intermediate: false,
                    disposable: false,
                    outputs_unaliased: false,
                    outputs_low_resolution: false,
                    bindings: Vec::new(),
                })
                .unwrap();
        }
        graph.add_default(final_output);

        let (persistence, warning) = Persistence::open(&mut graph, directory.path()).unwrap();
        assert!(warning.is_none());
        let targets = graph.default_targets();
        Fixture {
            directory,
            graph,
            persistence,
            targets,
        }
    }

    // [spec:ronin:req:frontend.graph-construction/test]
    #[test]
    fn a_graph_built_without_a_manifest_builds_and_is_then_up_to_date() {
        let Fixture {
            directory,
            mut graph,
            mut persistence,
            targets,
        } = fixture(b"");

        let planned = Build::new(&mut graph, &mut persistence)
            .jobs(Jobs::Limit(NonZeroUsize::new(2).unwrap()))
            .plan(&targets)
            .unwrap();
        assert!(!planned.already_up_to_date());
        let outcome = planned.run().unwrap();

        assert_eq!(outcome.stopped(), None);
        assert_eq!(outcome.exit_code(), 0);
        assert_eq!(outcome.regenerated(), targets.as_slice());
        assert!(String::from_utf8_lossy(outcome.output()).contains("cp "));
        assert_eq!(
            std::fs::read(directory.path().join("out")).unwrap(),
            b"source\n"
        );

        // The second build reads what the first one recorded, which is the
        // whole point of opening the persistent state before either of them.
        let planned = Build::new(&mut graph, &mut persistence)
            .plan(&targets)
            .unwrap();
        assert!(planned.already_up_to_date());
        let outcome = planned.run().unwrap();
        assert!(outcome.regenerated().is_empty());
        persistence.finish().unwrap();
    }

    // [spec:ronin:req:frontend.graph-construction/test]
    #[test]
    fn a_failing_command_stops_the_build_and_carries_its_status_out() {
        let Fixture {
            directory,
            mut graph,
            mut persistence,
            targets,
        } = fixture(b" && exit 3");

        let outcome = Build::new(&mut graph, &mut persistence)
            .plan(&targets)
            .unwrap()
            .run()
            .unwrap();

        assert_eq!(outcome.stopped().as_deref(), Some("subcommand failed"));
        assert_eq!(outcome.exit_code(), 3);
        assert!(outcome.regenerated().is_empty());
        // The first edge ran and failed, so the second never started.
        assert!(!directory.path().join("out").exists());
        persistence.finish().unwrap();
    }

    // [spec:ronin:req:frontend.graph-construction/test]
    #[test]
    fn a_dry_run_streams_the_commands_it_did_not_run() {
        let Fixture {
            directory,
            mut graph,
            mut persistence,
            targets,
        } = fixture(b"");
        let mut streamed = Vec::new();

        let outcome = Build::new(&mut graph, &mut persistence)
            .dry_run(true)
            .verbose(true)
            .output(&mut streamed)
            .plan(&targets)
            .unwrap()
            .run()
            .unwrap();

        assert!(outcome.output().is_empty());
        assert!(String::from_utf8_lossy(&streamed).contains("cp "));
        assert!(!directory.path().join("out").exists());
        persistence.finish().unwrap();
    }
}
