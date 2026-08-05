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
use crate::htab::rapidhashv1;
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// Where a graph's front end keeps the state that makes a build incremental.
///
/// A statement of the front end's own compatibility contract rather than a
/// caller's preference, which is why it travels on the graph: the graph is the
/// one thing both front ends hand to [`Persistence::open`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum StatePlacement {
    /// In the build directory, which is Ninja's contract: `.ninja_log` and
    /// `.ninja_deps` sit where Ninja itself, and anything else reading them,
    /// looks for them.
    #[default]
    BesideTheBuild,
    /// Outside the tree, keyed by the tree the build runs in, which is Make's.
    ///
    /// GNU Make leaves nothing behind, so a Makefile's directory has to look
    /// the same after a build as it did before one: state dropped beside a
    /// Makefile is visible in version control, in packaging, and to anything
    /// that lists the directory. Discarding the state instead would cost the
    /// two things GNU Make cannot do at all — noticing a changed command, and
    /// carrying a compiler's header dependencies to the next build — so the
    /// same two files, in the same formats, move rather than stop existing.
    // [spec:ronin:req:make.state-outside-the-tree]
    OutsideTheTree,
}

/// Where a caller may put Ronin's own state, overriding the platform's answer.
const STATE_HOME_ENV: &str = "RONIN_STATE_HOME";

/// The state root's subdirectory for trees built from a Makefile.
const MAKE_STATE: &str = "make";

/// The file naming the tree an entry belongs to.
const TREE_MARKER: &str = "tree";

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
    /// `directory` is the build directory for a graph whose front end keeps its
    /// state beside the build, and the tree the build runs in for one that
    /// keeps it outside; a Makefile's graph is the second kind and its logs go
    /// to a per-tree entry under Ronin's state home instead. Either way the two
    /// files are the same files, under the same names, in the same formats.
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
    /// Returns an [`Error`] when the directory cannot be created, when a graph
    /// keeping its state outside the tree has nowhere to keep it or finds an
    /// entry belonging to another tree, or when either log exists and cannot be
    /// read or reopened for appending.
    // [spec:ronin:req:make.state-outside-the-tree]
    pub fn open(graph: &mut BuildGraph, directory: &Path) -> Result<(Self, Option<String>), Error> {
        let relocated = match graph.state_placement {
            StatePlacement::BesideTheBuild => None,
            StatePlacement::OutsideTheTree => Some(state_directory(directory)?),
        };
        let directory = relocated.as_deref().unwrap_or(directory);
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

/// The absolute path `name` holds in the environment.
///
/// A relative one is ignored rather than resolved, which is what the XDG base
/// directory specification says to do with it and the only reading that does
/// not make the state's location depend on where a build was started from.
fn absolute_environment_path(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os(name)?);
    path.is_absolute().then_some(path)
}

/// The directory Ronin keeps its own state under.
///
/// `RONIN_STATE_HOME` first, so a caller that needs the state somewhere it
/// controls — a container, a CI job, a `clean` rule that has to remove it —
/// says so once and knows exactly where it went. Otherwise the platform's
/// cache convention, which is what a build's memory of itself is: losing it
/// costs a rebuild and nothing else.
fn state_home() -> Option<PathBuf> {
    if let Some(home) = absolute_environment_path(STATE_HOME_ENV) {
        return Some(home);
    }
    if let Some(cache) = absolute_environment_path("XDG_CACHE_HOME") {
        return Some(cache.join(crate::cli::PRODUCT_NAME));
    }
    let home = absolute_environment_path("HOME")?;
    let cache = if cfg!(target_os = "macos") {
        home.join("Library").join("Caches")
    } else {
        home.join(".cache")
    };
    Some(cache.join(crate::cli::PRODUCT_NAME))
}

/// Everything that distinguishes one tree from another, as bytes.
///
/// The resolved path separates two checkouts of one project, and separates a
/// tree from the one it was moved away from. The inode separates a tree from
/// whatever previously stood where it stands. Both together mean the two
/// failures left are a moved tree and a recreated one, and both of those lose
/// good state rather than inherit stale state: a build that rebuilds more than
/// it had to is slow, and one that rebuilds less is wrong.
// [spec:ronin:req:make.state-outside-the-tree]
fn tree_identity(tree: &Path) -> io::Result<Vec<u8>> {
    let metadata = std::fs::metadata(tree)?;
    // Relocated state outlives the tree it belongs to, which is the one hazard
    // moving it introduces: a directory deleted and recreated at the same path
    // is a different tree and must not inherit what the old one recorded. An
    // inode says that where a path cannot. Platforms without one contribute
    // nothing and fall back to the path alone.
    #[cfg(unix)]
    let distinct = std::os::unix::fs::MetadataExt::ino(&metadata);
    #[cfg(not(unix))]
    let distinct = 0_u64;
    let _ = &metadata;

    let mut identity = tree.as_os_str().as_encoded_bytes().to_vec();
    identity.push(b'\n');
    identity.extend_from_slice(distinct.to_string().as_bytes());
    identity.push(b'\n');
    Ok(identity)
}

/// The name of the entry a tree's state lives in.
///
/// A hash of the tree's identity, so no two trees share one, behind the tree's
/// own last component, so that a person reading the cache can tell the entries
/// apart without opening them.
fn entry_name(tree: &Path, identity: &[u8]) -> String {
    let mut label = tree
        .file_name()
        .unwrap_or_default()
        .as_encoded_bytes()
        .iter()
        .take(40)
        .map(|byte| match byte {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'.' | b'-' => char::from(*byte),
            _ => '_',
        })
        .skip_while(|character| *character == '.')
        .collect::<String>();
    if label.is_empty() {
        label.push_str("tree");
    }
    format!("{label}-{:016x}", rapidhashv1(identity))
}

/// Records which tree an entry belongs to, and refuses one that is another's.
///
/// An entry is named by a hash, and a hash can collide. The identity it was
/// made from is written beside the logs so that a collision is a refusal
/// somebody can act on rather than a build quietly reading another tree's
/// history. It is also what makes the state legible from outside: the entry to
/// remove for a given tree is the one whose `tree` file names it.
fn claim_entry(entry: &Path, identity: &[u8]) -> Result<(), PersistenceError> {
    let marker = entry.join(TREE_MARKER);
    let unusable = |path: PathBuf, source| {
        PersistenceError::io(PersistenceOperation::OpenStateDirectory, path, source)
    };
    match std::fs::read(&marker) {
        Ok(recorded) if recorded == identity => Ok(()),
        Ok(recorded) => {
            let claimed = recorded.split(|byte| *byte == b'\n').next().unwrap_or(&[]);
            Err(unusable(
                entry.to_owned(),
                io::Error::other(format!(
                    "already holds the state of {}; remove it to start over",
                    String::from_utf8_lossy(claimed)
                )),
            ))
        }
        // Written the way the logs beside it are rewritten, so that a build
        // starting in this tree while another is claiming it reads either
        // nothing or the whole claim, never half of one.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            crate::persistence::atomic_rewrite(&marker, |writer| writer.write_all(identity))
                .map(drop)
                .map_err(|source| unusable(marker, source))
        }
        Err(error) => Err(unusable(marker, error)),
    }
}

/// The entry under `home` that holds the state of builds run in `tree`.
// [spec:ronin:req:make.state-outside-the-tree]
fn entry_in(home: &Path, tree: &Path) -> Result<PathBuf, PersistenceError> {
    let identity = tree_identity(tree).map_err(|source| {
        PersistenceError::io(
            PersistenceOperation::OpenStateDirectory,
            tree.to_owned(),
            source,
        )
    })?;
    let entry = home.join(MAKE_STATE).join(entry_name(tree, &identity));
    std::fs::create_dir_all(&entry).map_err(|source| {
        PersistenceError::io(
            PersistenceOperation::CreateBuildDirectory,
            entry.clone(),
            source,
        )
    })?;
    claim_entry(&entry, &identity)?;
    Ok(entry)
}

/// The entry outside `tree` that holds the state of builds run in it.
// [spec:ronin:req:make.state-outside-the-tree]
fn state_directory(tree: &Path) -> Result<PathBuf, Error> {
    // A path the process was handed rather than one it read from the kernel
    // may still contain a symlink, and two names for one tree are one tree.
    let tree = std::fs::canonicalize(tree).unwrap_or_else(|_| tree.to_owned());
    let home = state_home().ok_or_else(|| {
        PersistenceError::io(
            PersistenceOperation::OpenStateDirectory,
            tree.clone(),
            io::Error::other(format!(
                "nowhere to keep build state outside the tree; set {STATE_HOME_ENV}, \
                 XDG_CACHE_HOME, or HOME"
            )),
        )
    })?;
    Ok(entry_in(&home, &tree)?)
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
            invocation_errors,
        } = self;
        let mut builder = Builder::from_parts(
            graph.arenas_mut(),
            options,
            Some(&mut persistence.build_log),
            Some(&mut persistence.deps_log),
            output.map(|sink| sink as &mut dyn Write),
            diagnostics.map(|sink| sink as &mut dyn Write),
        );
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
        let regenerated = self
            .targets
            .iter()
            .copied()
            .filter(|target| self.builder.regenerated(target.0))
            .collect();
        let output = std::mem::take(&mut self.builder.build_output);
        let stopped = match result {
            Err(BuildError::Stopped { reason, status }) => Some((reason, status)),
            other => {
                other?;
                None
            }
        };
        Ok(Outcome {
            stopped,
            regenerated,
            output,
        })
    }
}

/// How a build ended.
pub struct Outcome {
    pub(crate) stopped: Option<(BuildStop, i32)>,
    regenerated: Vec<Node>,
    output: Vec<u8>,
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

    // [spec:ronin:req:make.state-outside-the-tree/test]
    #[test]
    fn a_tree_is_told_apart_from_its_copies_from_its_replacement_and_from_itself_moved() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("state");
        let tree = root.path().join("checkout");
        let sibling = root.path().join("other-checkout");
        std::fs::create_dir(&tree).unwrap();
        std::fs::create_dir(&sibling).unwrap();

        let first = entry_in(&home, &tree).unwrap();
        // Nothing was written where the build runs; the entry is elsewhere and
        // says which tree it holds.
        assert_eq!(std::fs::read_dir(&tree).unwrap().count(), 0);
        assert!(first.starts_with(home.join(MAKE_STATE)));
        let claimed = std::fs::read(first.join(TREE_MARKER)).unwrap();
        assert!(claimed.starts_with(tree.as_os_str().as_encoded_bytes()));
        // Asking again for the same tree is the same entry, which is the whole
        // reason a second build has anything to read.
        assert_eq!(entry_in(&home, &tree).unwrap(), first);
        // A second checkout of the same project is a different tree.
        assert_ne!(entry_in(&home, &sibling).unwrap(), first);

        // A tree replaced at the path it stood at is a different tree, and
        // starts from nothing rather than reading what its predecessor left.
        std::fs::remove_dir(&tree).unwrap();
        std::fs::create_dir(&tree).unwrap();
        assert_ne!(entry_in(&home, &tree).unwrap(), first);
        // So is one that moved away from where it was built.
        let moved = root.path().join("moved");
        std::fs::rename(&tree, &moved).unwrap();
        assert_ne!(entry_in(&home, &moved).unwrap(), first);
    }

    // [spec:ronin:req:make.state-outside-the-tree/test]
    #[test]
    fn an_entry_another_tree_claimed_is_refused_rather_than_read() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("state");
        let tree = root.path().join("checkout");
        std::fs::create_dir(&tree).unwrap();
        let entry = entry_in(&home, &tree).unwrap();

        // Only a hash collision produces this, so it is provoked rather than
        // waited for. A build reading another tree's history would be wrong in
        // the one way persistence must never be wrong.
        std::fs::write(entry.join(TREE_MARKER), b"/somewhere/else\n1\n").unwrap();
        let refused = entry_in(&home, &tree).unwrap_err().to_string();
        assert!(
            refused.contains("already holds the state of /somewhere/else"),
            "{refused}"
        );
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
