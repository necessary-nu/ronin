//! Build scheduling state translated from `build.c`.

use crate::error::{BuildError, BuildOperation, BuildStop, ProcessError};
use crate::graph::{
    EdgeId, Graph, NodeId, PathStyle, TraversalScratch, edgeadddeps, edgehash, nodestat_with,
    recompute_dirty_with_validations, recompute_edge_dirty_with,
};
use crate::names::Names;
use crate::os::RealDiskInterface;
use crate::runtime::{FileTime, RuntimeState};
use crate::subprocess::{ProcessOutput, ProcessSupervisor, SupervisorWake, status_interrupted};
use crate::util::{BString, ByteSlice};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use self::command::{Advance, CommandSpec, DepsType, PreparedEdge, RunningStep};
use self::reporter::Reporter;
pub(crate) use self::reporter::{ColorChoice, OutputStyle, TerminalContext};
pub(crate) use self::status::BuildState;

type BuildResult<T> = Result<T, BuildError>;

pub(crate) use command::IGNORE_ERRORS;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum JobLimit {
    #[default]
    Auto,
    Unlimited,
    Fixed(NonZeroUsize),
}

impl JobLimit {
    pub(crate) const fn fixed(jobs: usize) -> Option<Self> {
        match NonZeroUsize::new(jobs) {
            Some(jobs) => Some(Self::Fixed(jobs)),
            None => None,
        }
    }
}

// [spec:ronin:def:build.buildoptions]
#[derive(Clone)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent Ninja CLI switches are clearer as named options than a synthetic state machine"
)]
pub(crate) struct BuildOptions {
    pub(crate) jobs: JobLimit,
    pub(crate) maxfail: usize,
    pub(crate) verbose: bool,
    pub(crate) explain: bool,
    pub(crate) stats: bool,
    pub(crate) keepdepfile: bool,
    pub(crate) keeprsp: bool,
    pub(crate) dryrun: bool,
    pub(crate) quiet: bool,
    pub(crate) statusfmt: String,
    pub(crate) status_from_cli: bool,
    pub(crate) shell: crate::subprocess::ShellMode,
    pub(crate) style: OutputStyle,
    pub(crate) color: ColorChoice,
    pub(crate) terminal: TerminalContext,
    pub(crate) maxload: f64,
    pub(crate) jobserver: Option<crate::jobserver::Transport>,
    /// Whether this manifest build may publish its fixed limit as a jobserver
    /// for recipe children. Makefile compilation disables this: recursive Make
    /// units are already inside the graph and use this scheduler directly.
    pub(crate) serve_jobserver: bool,
    /// Whether a command's own exit status of 130 says the build was cut short.
    ///
    /// Ninja spends 130 on `ExitInterrupted` and then reads every finished
    /// command's status back through that same number, so a command that exits
    /// 130 is indistinguishable from one killed by `SIGINT`: no `FAILED:` line,
    /// the build stops where it stands, and 130 leaves with it. Make has no such
    /// rule — a recipe exiting 130 is `Error 130`, no different from `Error 5` —
    /// so the Make front end turns this off rather than inheriting a number
    /// Ninja's enum happens to have spent.
    pub(crate) command_status_interrupts: bool,
    /// Whether a recipe killed by a signal is a command that failed rather than
    /// a build that was cut short.
    ///
    /// Ninja reads a child killed by `SIGINT`, `SIGTERM` or `SIGHUP` as the
    /// Ctrl-C it got too — the signal went to the process group, so the child
    /// dying of it says the build is over — and stops where it stands without a
    /// `FAILED:` line. GNU Make separates the two: a signal sent to *make*
    /// runs `fatal_error_signal`, while a child that merely died of one is
    /// reaped by `reap_children`, reported as `*** [target] Terminated`, and
    /// left to the ordinary failure path. So the Make front end turns this on
    /// and the question becomes whether this process was signalled too.
    pub(crate) recipe_signal_fails: bool,
    pub(crate) working_directory: crate::os::WorkingDirectory,
    /// Whether a target written `lib.a(member.o)` names a member of an
    /// archive rather than a file. Make mode only — see [`crate::os`].
    pub(crate) archive_members: bool,
    /// Whether an edge is brought up to date by giving its outputs a fresh date
    /// rather than by running what makes them, which is GNU Make's `-t`.
    ///
    /// Nothing about how the work is narrated changes with it: the edges that
    /// would have run still run through the plan in the same order and are
    /// still reported by the ordinary progress line. What changes is the work —
    /// no process starts, and each output is touched instead.
    // [spec:ronin:req:make.narration+1]
    pub(crate) touch: bool,
    /// Whether every edge that has a command is out of date whatever its
    /// timestamps say, which is GNU Make's `-B` / `--always-make`.
    ///
    /// A scan-level setting rather than a graph one, because it is not a
    /// property of the makefile: one Make run scans the same graph twice —
    /// once to bring the makefiles up to date and once for the goals — and
    /// GNU Make answers the two differently, turning the flag off for the
    /// makefiles after a restart so that an always-remade makefile cannot
    /// send the read around forever (`always_make_flag = always_make_set &&
    /// (restarts == 0)`, main.c). Carrying it per build is what lets the two
    /// scans disagree; a flag written into the edges could not.
    // [spec:ronin:req:make.semantics+1]
    pub(crate) always_make: bool,
    /// The files this run answers about as though each had just been written,
    /// which is GNU Make's `-W` / `--what-if` / `--assume-new` / `--new-file`.
    ///
    /// Names rather than nodes, because the graph a scan reads is not the graph
    /// this was asked of: the makefile pass and the goal pass are two scans over
    /// one graph and a `-W` name is looked up in each. A name the graph does not
    /// hold is nothing, which is GNU Make's answer too — `enter_file` makes a
    /// file nothing depends on.
    ///
    /// A scan-level setting for the same reason `-B` is one, and with the same
    /// restart subtlety behind it: GNU Make stamps these files before the
    /// makefile update on a first read and only after it on a restart
    /// (main.c:2325, main.c:2837), so an assumed-new makefile prerequisite
    /// sends the read around once and not forever.
    // [spec:ronin:req:make.semantics+1]
    pub(crate) assumed_new: Vec<BString>,
    /// The files this run answers about as though each were older than
    /// everything and had already been brought up to date, which is GNU Make's
    /// `-o` / `--old-file` / `--assume-old`.
    ///
    /// Names rather than nodes for the same reason `assumed_new` holds names.
    /// Unlike it, this reaches every pass: GNU Make's `-W` stamp is withheld
    /// from a restarted read because an assumed-new makefile prerequisite would
    /// remake the makefile, move its date and send the read around again, and
    /// an assumed-old one cannot — so `main` stamps the `-o` files with no
    /// `restarts` guard beside them (main.c:2312).
    // [spec:ronin:req:make.semantics+1]
    pub(crate) assumed_old: Vec<BString>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            jobs: JobLimit::Auto,
            maxfail: 1,
            verbose: false,
            explain: false,
            stats: false,
            keepdepfile: false,
            keeprsp: false,
            dryrun: false,
            quiet: false,
            statusfmt: "[%f/%t] ".into(),
            status_from_cli: false,
            shell: crate::subprocess::ShellMode::default(),
            style: OutputStyle::Ninja,
            color: ColorChoice::Auto,
            terminal: TerminalContext::default(),
            maxload: 0.0,
            jobserver: None,
            serve_jobserver: false,
            command_status_interrupts: true,
            recipe_signal_fails: false,
            working_directory: crate::os::WorkingDirectory::default(),
            archive_members: false,
            touch: false,
            always_make: false,
            assumed_new: Vec::new(),
            assumed_old: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EdgeResult {
    Succeeded,
    Failed,
}

/// Why a stopped edge's work is being taken back, which decides how much of it
/// goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Withdraw {
    /// The command never got to finish: the build was cut short under it, or a
    /// signal killed the recipe itself. Nothing it wrote is finished, so all of
    /// it goes — bar the outputs a front end excepted, which is GNU Make's
    /// `.PRECIOUS` and `.PHONY` reaching the same `delete_target` a signal does.
    Stopped,
    /// The command ran to a non-zero exit. The build goes on around it, so
    /// whether what it wrote goes is `.DELETE_ON_ERROR`'s answer and not this
    /// one's.
    DeleteOnError,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct ReadyEdge {
    weight: CriticalPathWeight,
    edge: Reverse<EdgeId>,
}

impl ReadyEdge {
    const fn new(weight: CriticalPathWeight, edge: EdgeId) -> Self {
        Self {
            weight,
            edge: Reverse(edge),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
struct CriticalPathWeight(usize);

impl CriticalPathWeight {
    const ROOT: Self = Self(1);

    const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    const fn max(self, other: Self) -> Self {
        if self.0 >= other.0 { self } else { other }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
struct PoolOccupancy(usize);

impl PoolOccupancy {
    const fn has_capacity(self, depth: usize) -> bool {
        self.0 < depth
    }

    const fn acquire(&mut self) {
        self.0 += 1;
    }

    const fn release(&mut self) {
        debug_assert!(self.0 > 0);
        self.0 -= 1;
    }
}

#[derive(Default)]
pub(crate) struct Plan {
    /// Every edge whose inputs must settle before a consumer may start.
    ///
    /// Ninja keeps clean edges in its plan as `kWantNothing`: they run no
    /// command, but they are still dependency barriers.  `wanted` is the
    /// subset that is dirty and must execute.
    tracked: Vec<bool>,
    tracked_count: usize,
    wanted: Vec<bool>,
    wanted_count: usize,
    weight: Vec<CriticalPathWeight>,
    expanded_weight: Vec<CriticalPathWeight>,
    pending: Vec<usize>,
    dependents: Vec<Vec<EdgeId>>,
    ready: BinaryHeap<ReadyEdge>,
    running: Vec<bool>,
    completed: Vec<bool>,
    pool_occupancy: Vec<PoolOccupancy>,
    /// Which consuming edge last recorded a dependency on each generator.
    ///
    /// Deduplicates the edges pushed onto `dependents` within one rebuild.
    /// A niche-packed identifier makes the empty case free, so this is half
    /// the width of the index-plus-sentinel it replaced.
    dependency_marks: Vec<Option<EdgeId>>,
    /// Edges a failure reached through a wait that was not forgiven, so they
    /// can never run. Empty — and never grown — for a graph with no forgiven
    /// wait in it, which is every Ninja manifest.
    abandoned: Vec<bool>,
    completed_count: usize,
    failures: usize,
}

// [spec:ronin:req:compat.graph-semantics]
impl Plan {
    fn synchronize_arenas(&mut self, graph: &Graph) {
        let edge_count = graph.edge_count();
        self.tracked.resize(edge_count, false);
        self.wanted.resize(edge_count, false);
        self.weight
            .resize(edge_count, CriticalPathWeight::default());
        self.expanded_weight
            .resize(edge_count, CriticalPathWeight::default());
        self.pending.resize(edge_count, 0);
        self.dependents.resize_with(edge_count, Vec::new);
        self.running.resize(edge_count, false);
        self.completed.resize(edge_count, false);
        self.pool_occupancy
            .resize(graph.pool_count(), PoolOccupancy::default());
        self.dependency_marks.resize(edge_count, None);
    }

    // [spec:ronin:def:build.buildreset-fn]
    // [spec:ronin:sem:build.buildreset-fn]
    // [spec:ronin:def:build.isnewer-fn]
    // [spec:ronin:sem:build.isnewer-fn]
    // [spec:ronin:def:build.isdirty-fn]
    // [spec:ronin:sem:build.isdirty-fn]
    // [spec:ronin:def:build.queue-fn]
    // [spec:ronin:sem:build.queue-fn]
    // [spec:ronin:def:build.buildadd-fn]
    // [spec:ronin:sem:build.buildadd-fn]
    pub(crate) fn add_target(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
        node: NodeId,
    ) -> BuildResult<()> {
        self.synchronize_arenas(graph);
        self.add_node(graph, runtime, node, CriticalPathWeight::ROOT)
    }

    fn add_node(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
        node: NodeId,
        weight: CriticalPathWeight,
    ) -> BuildResult<()> {
        let mut work = vec![(node, weight, None)];
        while let Some((node, weight, needed_by)) = work.pop() {
            // A Makefile this read already tried to remake and lost is a target
            // with a rule that has been spent. GNU Make reads `updated` with a
            // failing `update_status` back before it looks at the file or the
            // commands (remake.c: "Recently tried and failed to update file"),
            // so the recipe does not run a second time and the file the losing
            // recipe may have left behind does not count either.
            // A makefile the `-q` pass asked about and was told is not up to
            // date is refused the same way, its recipe being just as spent:
            // `update_goal_chain` leaves it `updated` with `us_question` and
            // the early read makes no distinction between the two statuses.
            // What tells them apart is the exit code the answer carries, and
            // the refusal is stamped with which of the two it is here — where
            // the graph still knows — rather than reconstructed afterwards
            // from a node identity carried out of the failure.
            let unmade = graph.is_unmade_makefile(node) || graph.is_questioned_makefile(node);
            let Some(edge) = graph.node(node).generator.filter(|_| !unmade) else {
                if unmade || runtime.node(node).dirty() {
                    let path = graph.node_path(node).to_owned();
                    let needed_by = needed_by
                        .map(|needed_by| (needed_by, graph.node_path(needed_by).to_owned()));
                    return Err(BuildError::MissingInput {
                        node,
                        path,
                        needed_by,
                        questioned: graph.is_questioned_makefile(node),
                    });
                }
                continue;
            };
            let edge_dirty = graph
                .edge(edge)
                .out
                .iter()
                .any(|output| runtime.node(*output).dirty());
            let phony_with_no_inputs = {
                let edge = graph.edge(edge);
                graph.is_phony_rule(edge.rule) && edge.input.is_empty()
            };
            if edge_dirty && phony_with_no_inputs {
                continue;
            }

            if !self.tracked[edge.index()] {
                self.tracked[edge.index()] = true;
                self.tracked_count += 1;
            }

            if edge_dirty {
                let previous_weight = self.weight[edge.index()];
                let newly_wanted = !self.wanted[edge.index()];
                if !newly_wanted && weight <= previous_weight {
                    continue;
                }
                if newly_wanted {
                    self.wanted[edge.index()] = true;
                    self.wanted_count += 1;
                }
                self.weight[edge.index()] = weight.max(previous_weight);
            } else {
                if weight <= self.expanded_weight[edge.index()] {
                    continue;
                }
                self.expanded_weight[edge.index()] = weight;
            }

            let edge_id = edge;
            let edge = graph.edge(edge_id);
            let needed_by = edge.out.first().copied();
            let depfile_end = edge.non_order_only_input_count();
            let depfile_start =
                depfile_end.saturating_sub(runtime.edge(edge_id).depfile_dependencies());
            let inputs: &[NodeId] = &edge.input;
            for (index, &input) in inputs.iter().enumerate().rev() {
                if !(index >= depfile_start
                    && index < depfile_end
                    && graph.node(input).generator.is_none())
                {
                    work.push((input, weight.next(), needed_by));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_queue(&mut self, graph: &Graph) {
        self.synchronize_arenas(graph);
        self.running.fill(false);
        self.completed.fill(false);
        self.pool_occupancy.fill(PoolOccupancy::default());
        self.completed_count = 0;
        self.failures = 0;
        self.rebuild_frontier(graph);
    }

    fn rebuild_frontier(&mut self, graph: &Graph) {
        self.synchronize_arenas(graph);
        self.pending.fill(0);
        for dependents in &mut self.dependents {
            dependents.clear();
        }
        self.ready.clear();
        // Marks persist across rebuilds, so a stale mark from the previous
        // frontier would wrongly suppress a dependency; reset before reuse.
        self.dependency_marks.fill(None);
        self.abandoned.clear();
        for edge in graph.edge_ids() {
            let index = edge.index();
            if !self.tracked[index] || self.completed[index] {
                continue;
            }
            for input in graph.edge(edge).input.iter().copied() {
                let Some(generator) = graph.node(input).generator else {
                    continue;
                };
                if self.tracked[generator.index()]
                    && !self.completed[generator.index()]
                    && self.dependency_marks[generator.index()] != Some(edge)
                {
                    self.dependency_marks[generator.index()] = Some(edge);
                    self.pending[index] += 1;
                    self.dependents[generator.index()].push(edge);
                }
            }
        }
        let mut clean = Vec::new();
        for edge in graph.edge_ids() {
            let index = edge.index();
            if self.tracked[index]
                && !self.completed[index]
                && !self.running[index]
                && self.pending[index] == 0
            {
                if self.wanted[index] {
                    self.ready.push(ReadyEdge::new(self.weight[index], edge));
                } else {
                    clean.push(edge);
                }
            }
        }
        self.finish_initially_clean(clean);
    }

    /// Settle clean dependency bridges without scheduling them.
    ///
    /// Their only job in the plan is to hold consumers until their own inputs
    /// finish.  This is Ninja's `kWantNothing` state: omitting these edges
    /// entirely lets a consumer cross a clean phony/order-only bridge while a
    /// dirty transitive prerequisite is still running.
    fn finish_initially_clean(&mut self, mut work: Vec<EdgeId>) {
        while let Some(edge) = work.pop() {
            if std::mem::replace(&mut self.completed[edge.index()], true) {
                continue;
            }
            self.completed_count += 1;
            for index in 0..self.dependents[edge.index()].len() {
                let dependent = self.dependents[edge.index()][index];
                self.pending[dependent.index()] -= 1;
                if self.pending[dependent.index()] != 0 {
                    continue;
                }
                if self.wanted[dependent.index()] {
                    self.ready
                        .push(ReadyEdge::new(self.weight[dependent.index()], dependent));
                } else {
                    work.push(dependent);
                }
            }
        }
    }

    pub(crate) fn refresh_dependencies(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
    ) -> BuildResult<()> {
        self.synchronize_arenas(graph);
        for edge in graph.edge_ids() {
            let index = edge.index();
            if !self.tracked[index] {
                continue;
            }
            let weight = self.weight[index];
            let inputs: &[NodeId] = &graph.edge(edge).input;
            for &input in inputs.iter().rev() {
                self.add_node(graph, runtime, input, weight.next())?;
            }
        }
        self.rebuild_frontier(graph);
        Ok(())
    }

    /// The side tables run parallel to the edge arena, so walking the arena
    /// alongside them keeps identifiers coming from the graph that owns them.
    pub(crate) fn wanted_edges(&self, graph: &Graph) -> Vec<EdgeId> {
        self.wanted
            .iter()
            .zip(graph.edge_ids())
            .filter_map(|(wanted, edge)| wanted.then_some(edge))
            .collect()
    }

    pub(crate) fn find_work(&mut self, graph: &Graph) -> Option<EdgeId> {
        let mut blocked = Vec::new();
        let edge = loop {
            let Some(candidate) = self.ready.pop() else {
                self.ready.extend(blocked);
                return None;
            };
            let edge = candidate.edge.0;
            if graph.edge(edge).pool.is_none_or(|pool| {
                let depth = graph
                    .pool(pool)
                    .depth()
                    .expect("validated pools have a depth")
                    .get();
                self.pool_occupancy[pool.index()].has_capacity(depth)
            }) {
                break edge;
            }
            blocked.push(candidate);
        };
        self.ready.extend(blocked);
        if let Some(pool) = graph.edge(edge).pool {
            self.pool_occupancy[pool.index()].acquire();
        }
        self.running[edge.index()] = true;
        Some(edge)
    }

    fn defer_work(&mut self, graph: &Graph, edge: EdgeId) {
        if std::mem::replace(&mut self.running[edge.index()], false) {
            if let Some(pool) = graph.edge(edge).pool {
                self.pool_occupancy[pool.index()].release();
            }
            self.ready
                .push(ReadyEdge::new(self.weight[edge.index()], edge));
        }
    }

    /// Take an edge out of the work this plan still expects to run, and say
    /// whether it was expected. This is Ninja's `kWantNothing`: the edge stays
    /// in the plan as a dependency barrier and stops being work.
    pub(crate) fn unwant(&mut self, edge: EdgeId) -> bool {
        if std::mem::replace(&mut self.wanted[edge.index()], false) {
            self.wanted_count -= 1;
            true
        } else {
            false
        }
    }

    /// Settle a finished edge and answer with the command edges its finishing
    /// took out of the plan, which the caller owes the progress count.
    pub(crate) fn edge_finished(
        &mut self,
        graph: &Graph,
        runtime: &RuntimeState,
        edge: EdgeId,
        result: EdgeResult,
    ) -> BuildResult<Vec<EdgeId>> {
        if !std::mem::replace(&mut self.running[edge.index()], false) {
            return Err(BuildError::EdgeNotRunning { edge });
        }
        if let Some(pool) = graph.edge(edge).pool {
            self.pool_occupancy[pool.index()].release();
        }
        if !std::mem::replace(&mut self.completed[edge.index()], true) {
            self.completed_count += 1;
        }
        if result == EdgeResult::Failed {
            self.failures += 1;
            return Ok(self.abandon_dependents(graph, runtime, edge));
        }
        Ok(self.release_dependents(graph, runtime, edge))
    }

    pub(crate) const fn more_to_do(&self) -> bool {
        self.failures != 0 || self.completed_count < self.tracked_count
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.wanted_count == 0
    }
}

/// What a build did about one target's command.
#[derive(PartialEq, Eq)]
pub(crate) enum Made {
    /// It ran, won, and a `restat` rule did not then find the output unchanged.
    Regenerated,
    /// It ran and lost.
    Failed,
    /// Neither. Nothing ran for this target — either it was already current, or
    /// something it needed failed before its own command was reached — or a
    /// `restat` found the output unchanged. Either way the file on disk is what
    /// it was, which is what a caller asking is about to look at.
    Nothing,
}

// [spec:ronin:def:build.job]
pub(crate) struct Builder<'a> {
    graph: &'a mut Graph,
    runtime: RuntimeState,
    options: BuildOptions,
    disk: RealDiskInterface,
    plan: Plan,
    scratch: TraversalScratch,
    /// Nodes awaiting an mtime, reused across targets by `prefetch_mtimes`.
    stat_targets: Vec<NodeId>,
    visited_edges: crate::graph::MarkSet,
    build_log: Option<&'a mut crate::log::BuildLog>,
    deps_log: Option<&'a mut crate::deps::DepsLog>,
    targets: Vec<NodeId>,
    executed_edges: BTreeSet<EdgeId>,
    /// The edges this build reached and found to have no command at all,
    /// because the front end read the recipe as the edge launched and it came
    /// to nothing. Kept apart from the edges that ran, because an edge that
    /// ran nothing wrote nothing — which is what a dry run has to know before
    /// standing in for the update a prerequisite would have made.
    ran_nothing_edges: BTreeSet<EdgeId>,
    /// The edges this build settled as failed, whether the command said so or
    /// the launch never happened. Read by a caller that has to decide about
    /// each target separately rather than about the build as a whole.
    failed_edges: BTreeSet<EdgeId>,
    command_cache: Vec<Option<CommandSpec>>,
    command_scratch: Vec<u8>,
    progress: BuildState,
    reporter: Reporter,
    /// Buffer every rendered line is built in, reused for the whole build.
    ///
    /// Rendering used to allocate a `String` for the status template and then
    /// a second `Vec` to splice the description into it, once per finished
    /// command. One reused buffer removes the second of those and makes the
    /// first the only allocation left on the path.
    status_scratch: Vec<u8>,
    output_sink: Option<&'a mut dyn Write>,
    diagnostic_sink: Option<&'a mut dyn Write>,
    /// A front end that binds an edge's command as the edge is launched,
    /// rather than having bound it when the graph was built.
    late_commands: Option<&'a mut dyn crate::build::command::LateCommands>,
    explanations: Option<crate::explanations::Explanations>,
    explanations_recorded: Vec<bool>,
    explanations_emitted: Vec<bool>,
    pub(crate) commands_ran: Vec<BString>,
    pub(crate) command_output: Vec<u8>,
    pub(crate) build_output: Vec<u8>,
}

impl<'a> Builder<'a> {
    /// Builds over `graph`, writing through whichever logs and sinks the
    /// invocation has. All four are optional because a test, a library caller
    /// collecting output, and the command line each have a different subset.
    pub(crate) fn from_parts(
        graph: &'a mut Graph,
        options: BuildOptions,
        build_log: Option<&'a mut crate::log::BuildLog>,
        deps_log: Option<&'a mut crate::deps::DepsLog>,
        output_sink: Option<&'a mut dyn Write>,
        diagnostic_sink: Option<&'a mut dyn Write>,
    ) -> Self {
        let progress = BuildState::new(options.clone());
        let options_style = options.style;
        let options_color = options.color.resolve(options.terminal);
        let mut disk = RealDiskInterface::new(options.working_directory.clone());
        if options.archive_members {
            disk = disk.reading_archive_members();
        }
        let mut runtime = RuntimeState::new(graph);
        runtime.always_make = options.always_make;
        // Resolved against the graph this scan reads rather than carried as
        // node ids, because the names were given to the invocation and the same
        // names are looked up again for the goal pass.
        crate::runtime::AssertedDates {
            new: &options.assumed_new,
            old: &options.assumed_old,
        }
        .mark_on(graph, &mut runtime);
        if let Some(log) = build_log.as_deref() {
            log.hydrate_runtime(graph, &mut runtime, graph.node_ids());
        }
        let explanations = options
            .explain
            .then(crate::explanations::Explanations::default);
        Self {
            graph,
            runtime,
            options,
            disk,
            plan: Plan::default(),
            scratch: TraversalScratch::default(),
            stat_targets: Vec::new(),
            visited_edges: crate::graph::MarkSet::default(),
            build_log,
            deps_log,
            targets: Vec::new(),
            executed_edges: BTreeSet::new(),
            ran_nothing_edges: BTreeSet::new(),
            failed_edges: BTreeSet::new(),
            command_cache: Vec::new(),
            command_scratch: Vec::new(),
            progress,
            reporter: Reporter::new(options_style, options_color),
            status_scratch: Vec::new(),
            output_sink,
            diagnostic_sink,
            explanations,
            explanations_recorded: Vec::new(),
            explanations_emitted: Vec::new(),
            commands_ran: Vec::new(),
            command_output: Vec::new(),
            build_output: Vec::new(),
            late_commands: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new(graph: &'a mut Graph, options: BuildOptions) -> Self {
        Self::from_parts(graph, options, None, None, None, None)
    }

    #[cfg(test)]
    pub(crate) fn with_output(
        graph: &'a mut Graph,
        options: BuildOptions,
        output: &'a mut dyn Write,
    ) -> Self {
        Self::from_parts(graph, options, None, None, Some(output), None)
    }

    #[cfg(test)]
    pub(crate) fn with_build_log(
        graph: &'a mut Graph,
        options: BuildOptions,
        build_log: &'a mut crate::log::BuildLog,
    ) -> Self {
        Self::from_parts(graph, options, Some(build_log), None, None, None)
    }

    #[cfg(test)]
    pub(crate) fn with_deps_log(
        graph: &'a mut Graph,
        options: BuildOptions,
        deps_log: &'a mut crate::deps::DepsLog,
    ) -> Self {
        Self::from_parts(graph, options, None, Some(deps_log), None, None)
    }

    fn synchronize_runtime(&mut self) {
        // `synchronize` reports the newly grown span as indices; take the
        // identifiers for it from the arena that just grew.
        let nodes = self.runtime.synchronize(self.graph);
        if let Some(log) = self.build_log.as_deref() {
            let added = self.graph.node_ids().skip(nodes.start);
            log.hydrate_runtime(self.graph, &mut self.runtime, added);
        }
    }

    fn replace_depfile_deps(&mut self, edge: EdgeId, deps: &[NodeId]) {
        self.synchronize_runtime();
        let previous_count = self.runtime.edge(edge).depfile_dependencies();
        self.graph
            .edge_mut(edge)
            .drain_discovered_inputs(previous_count);
        edgeadddeps(self.graph, edge, deps);
        self.runtime
            .edge_mut(edge)
            .set_depfile_dependencies(deps.len());
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the iterative enter/load/refresh traversal is one explicit dependency-loading state machine"
    )]
    fn load_depfiles_for(&mut self, target: NodeId) -> BuildResult<()> {
        enum Work {
            Enter(NodeId),
            Load(EdgeId),
            Refresh(EdgeId),
        }

        let disk = self.disk.clone();
        self.visited_edges.begin(self.graph.edge_count());
        let mut work = vec![Work::Enter(target)];
        while let Some(item) = work.pop() {
            match item {
                Work::Enter(node) => {
                    // Stopping where the dirty scan stops. A name `-o` asserted
                    // a date for is one GNU Make's `update_file_1` returns on
                    // before it reaches a prerequisite, so nothing beneath it
                    // is considered — and an edge this walk called out of date
                    // on the way past would stay called that, because the scan
                    // that stops at the `-o` name never reaches it to say
                    // otherwise.
                    if self.runtime.assumed_old.contains(node) {
                        continue;
                    }
                    let Some(edge) = self.graph.node(node).generator else {
                        if self.runtime.node(node).mtime().is_unobserved() {
                            let mut stat = |path: &Path| disk.stat(path);
                            nodestat_with(self.graph, &mut self.runtime, node, &mut stat)?;
                        }
                        let dirty = self.runtime.node(node).mtime().is_missing();
                        self.runtime.node_mut(node).set_dirty(dirty);
                        continue;
                    };
                    if self.visited_edges.replace(edge.index()) {
                        continue;
                    }
                    work.push(if self.runtime.edge(edge).deps_loaded() {
                        Work::Refresh(edge)
                    } else {
                        Work::Load(edge)
                    });
                    for input in self.graph.edge(edge).input.iter().rev() {
                        work.push(Work::Enter(*input));
                    }
                }
                Work::Load(edge) => {
                    let mut stat = |path: &Path| disk.stat(path);
                    let base_dirty =
                        recompute_edge_dirty_with(self.graph, &mut self.runtime, edge, &mut stat)?;
                    let mut dependencies_changed = false;
                    let uses_deps_log = self.deps_log.is_some()
                        && crate::env::edgevar(self.graph, edge, Names::DEPS, PathStyle::Raw)
                            .is_some_and(|value| !value.is_empty());
                    let depfile = (!uses_deps_log)
                        .then(|| {
                            crate::env::edgevar(self.graph, edge, Names::DEPFILE, PathStyle::Raw)
                        })
                        .flatten()
                        .filter(|path| !path.is_empty());

                    if uses_deps_log {
                        let output = self.graph.edge(edge).out.first().copied();
                        let entry_is_current = output.is_some_and(|output| {
                            self.deps_log
                                .as_deref()
                                .and_then(|log| crate::deps::depsentry(log, output))
                                .is_some_and(|entry| {
                                    self.runtime.node(output).mtime().raw() <= entry.mtime
                                })
                        });
                        if !base_dirty
                            && entry_is_current
                            && let Some(log) = self.deps_log.as_deref()
                        {
                            crate::deps::depsload(self.graph, edge, log);
                            dependencies_changed = true;
                        }
                        let state = self.runtime.edge_mut(edge);
                        state.set_deps_loaded(true);
                        state.set_deps_missing(!entry_is_current);
                    } else if let Some(depfile) = depfile {
                        let path = depfile.to_path().expect("byte paths are valid on Unix");
                        if base_dirty {
                            let state = self.runtime.edge_mut(edge);
                            state.set_deps_loaded(true);
                            state.set_deps_missing(!disk.exists(path));
                        } else if disk.exists(path) {
                            self.runtime.edge_mut(edge).set_deps_loaded(true);
                            match crate::deps::depsparse_for_edge(
                                self.graph,
                                &disk.resolve(path),
                                edge,
                            )? {
                                Some(deps) => {
                                    self.replace_depfile_deps(edge, &deps.nodes);
                                    self.runtime.edge_mut(edge).set_deps_missing(false);
                                    dependencies_changed = true;
                                }
                                None => self.runtime.edge_mut(edge).set_deps_missing(true),
                            }
                        } else {
                            let state = self.runtime.edge_mut(edge);
                            state.set_deps_loaded(true);
                            state.set_deps_missing(true);
                        }
                    } else {
                        self.runtime.edge_mut(edge).set_deps_loaded(true);
                    }

                    if dependencies_changed {
                        work.push(Work::Refresh(edge));
                        for input in self.graph.edge(edge).input.iter().rev() {
                            work.push(Work::Enter(*input));
                        }
                    }
                }
                Work::Refresh(edge) => {
                    let mut stat = |path: &Path| disk.stat(path);
                    recompute_edge_dirty_with(self.graph, &mut self.runtime, edge, &mut stat)?;
                }
            }
        }
        Ok(())
    }

    fn load_ready_dyndeps_for(
        &mut self,
        node: NodeId,
        visited_edges: &mut Vec<bool>,
        loaded_files: &mut Vec<bool>,
    ) -> BuildResult<()> {
        visited_edges.resize(self.graph.edge_count(), false);
        let mut work = vec![node];
        while let Some(node) = work.pop() {
            let Some(edge) = self.graph.node(node).generator else {
                continue;
            };
            if std::mem::replace(&mut visited_edges[edge.index()], true) {
                continue;
            }
            let dyndep = self.graph.edge(edge).dyndep;
            if let Some(dyndep) =
                dyndep.filter(|dyndep| self.runtime.node(*dyndep).dyndep_pending())
            {
                loaded_files.resize(loaded_files.len().max(dyndep.index() + 1), false);
                let path = self.graph.node_path(dyndep).to_owned();
                if self
                    .disk
                    .exists(path.to_path().expect("byte paths are valid on Unix"))
                    && !std::mem::replace(&mut loaded_files[dyndep.index()], true)
                {
                    crate::dyndep::load_dyndep(self.graph, &mut self.runtime, dyndep, &self.disk)?;
                    self.synchronize_runtime();
                }
            }
            for input in self.graph.edge(edge).input.iter().rev() {
                work.push(*input);
            }
        }
        Ok(())
    }

    fn prepare_build_log_for(&mut self, node: NodeId) -> BuildResult<()> {
        self.visited_edges.begin(self.graph.edge_count());
        let mut work = vec![node];
        while let Some(node) = work.pop() {
            let Some(edge) = self.graph.node(node).generator else {
                continue;
            };
            if self.visited_edges.replace(edge.index()) {
                continue;
            }
            // A phony edge has no command to evaluate, hash, or log, and the
            // dirty rule never consults a phony edge's command hash.
            if !self.graph.is_phony_rule(self.graph.edge(edge).rule) {
                self.refresh_command_hash(edge)?;
            }
            for input in self.graph.edge(edge).input.iter().rev() {
                work.push(*input);
            }
        }
        Ok(())
    }

    fn record_dirty_explanations(&mut self) {
        let Some(explanations) = self.explanations.as_mut() else {
            return;
        };
        self.explanations_recorded
            .resize(self.graph.edge_count(), false);
        for edge in self.plan.wanted_edges(self.graph) {
            if std::mem::replace(&mut self.explanations_recorded[edge.index()], true) {
                continue;
            }
            let inputs = self.graph.edge(edge).non_order_only_inputs();
            let newest = inputs
                .iter()
                .filter(|input| !self.runtime.node(**input).mtime().is_missing())
                .max_by_key(|input| self.runtime.node(**input).mtime())
                .copied();
            for output in &self.graph.edge(edge).out {
                let output_state = self.runtime.node(*output);
                if !output_state.dirty() {
                    continue;
                }
                let path = self.graph.node_path(*output).to_str_lossy();
                let message = if output_state.mtime().is_missing() {
                    format!("output {path} doesn't exist")
                } else if self.runtime.edge(edge).command_dirty() {
                    format!("command line changed for {path}")
                } else if self.runtime.edge(edge).deps_missing() {
                    format!("dependency information for {path} is missing")
                } else if let Some(input) =
                    newest.filter(|input| self.runtime.node(*input).mtime() > output_state.mtime())
                {
                    format!(
                        "output {path} older than most recent input {} ({} vs {})",
                        self.graph.node_path(input).to_str_lossy(),
                        output_state.mtime().raw(),
                        self.runtime.node(input).mtime().raw()
                    )
                } else if inputs.iter().any(|input| self.runtime.node(*input).dirty()) {
                    format!("input to {path} is dirty")
                } else {
                    format!("output {path} is dirty")
                };
                explanations.record(output.index(), message);
            }
        }
    }

    /// The target a path names, for the tests that describe one that way.
    #[cfg(test)]
    pub(crate) fn add_target(&mut self, path: impl AsRef<[u8]>) -> BuildResult<()> {
        let path = path.as_ref();
        let node =
            crate::graph::nodeget(self.graph, path).ok_or_else(|| BuildError::UnknownTarget {
                path: BString::from(path),
            })?;
        self.add_target_node(node)
    }

    pub(crate) fn add_target_node(&mut self, node: NodeId) -> BuildResult<()> {
        if !self.targets.contains(&node) {
            self.targets.push(node);
        }
        // Ahead of every traversal below, not just the dirty scan: all three
        // of `load_depfiles_for`, `prepare_build_log_for` and the scan itself
        // walk this graph and stat what they find.
        self.prefetch_mtimes(node);
        self.load_depfiles_for(node)?;
        self.load_ready_dyndeps_for(node, &mut Vec::new(), &mut Vec::new())?;
        if self.build_log.is_some() {
            self.prepare_build_log_for(node)?;
        }
        let disk = self.disk.clone();
        let mut stat = |path: &Path| disk.stat(path);
        let validations = recompute_dirty_with_validations(
            self.graph,
            &mut self.runtime,
            &mut self.scratch,
            node,
            &mut stat,
        )?;
        self.plan
            .add_target(self.graph, &self.runtime, node)
            .map_err(|error| {
                if self.graph.node(node).generator.is_none() {
                    BuildError::MissingRule {
                        node,
                        path: self.graph.node_path(node).to_owned(),
                    }
                } else {
                    error
                }
            })?;
        for validation in validations {
            self.plan
                .add_target(self.graph, &self.runtime, validation)?;
        }
        self.record_dirty_explanations();
        Ok(())
    }

    /// Warm every mtime the coming scan will ask for, in parallel.
    ///
    /// The scan reads mtimes in dependency order but the reads themselves are
    /// independent, so issuing them one at a time leaves the process blocked
    /// in the kernel for most of an up-to-date build. Filling them first turns
    /// `nodestat_with`'s `is_unobserved` guard into a hit and leaves the scan
    /// itself untouched.
    ///
    /// Nodes already observed are skipped, so a second target costs only the
    /// paths the first did not cover, and a failed stat is simply not recorded
    /// — the scan then takes its usual serial path and reports the usual error.
    fn prefetch_mtimes(&mut self, target: NodeId) {
        crate::graph::collect_stat_targets(
            self.graph,
            &mut self.scratch,
            target,
            &mut self.stat_targets,
        );
        self.stat_targets
            .retain(|node| self.runtime.node(*node).mtime().is_unobserved());
        if self.stat_targets.len() < 2 {
            return;
        }

        // These borrow from the graph, so they cannot outlive the call and
        // cannot be reused buffers; two allocations amortize over thousands
        // of syscalls.
        let graph = &*self.graph;
        let paths = self
            .stat_targets
            .iter()
            .map(|node| {
                graph
                    .node_path(*node)
                    .to_path()
                    .expect("byte paths are valid on Unix")
            })
            .collect::<Vec<_>>();
        let mut results = vec![None; paths.len()];
        self.disk.stat_many(&paths, &mut results);

        // A name the front end found somewhere else needs the second look the
        // batch cannot make for it, and there are never many: the batch is
        // issued for every node the scan will reach, and this runs only for the
        // ones that came back absent and carry a second place to look.
        let mut fallback = |path: &Path| self.disk.stat(path);
        for (node, mtime) in self.stat_targets.iter().zip(&mut results) {
            let Some(observed) = *mtime else { continue };
            if !FileTime::observed(observed).is_missing() {
                continue;
            }
            if let Ok(found) = crate::graph::elsewhere_mtime(graph, *node, observed, &mut fallback)
            {
                *mtime = Some(found);
            }
        }
        for (node, mtime) in self.stat_targets.iter().zip(&results) {
            if let Some(mtime) = *mtime {
                // `-W` is asked first because GNU Make stamps it last: `-o`
                // writes `OLD_MTIME` and `-W` writes `NEW_MTIME` over it, so a
                // name given to both is new whichever order it was written in.
                let observed = if self.runtime.assumed_new.contains(*node) {
                    FileTime::NEWEST
                } else if self.runtime.assumed_old.contains(*node) {
                    FileTime::OLDEST
                } else {
                    FileTime::observed(mtime)
                };
                self.runtime.node_mut(*node).observe(observed);
            }
        }
    }

    /// Whether the build has nothing to run, as Ninja judges it.
    ///
    /// Ninja's `more_to_do` requires *both* a wanted edge and a command edge,
    /// so a plan holding only phony work is up to date. Testing the wanted
    /// count alone diverges on any graph whose default target is a phony over
    /// other phonies — abseil's is, so Ronin stayed silent there where Ninja
    /// says `no work to do.`, while the Ninja project's own graph never hits
    /// the shape and looked correct.
    pub(crate) fn already_up_to_date(&self) -> bool {
        self.plan.is_empty() || self.plan.reportable_work_count(self.graph, &self.runtime) == 0
    }

    /// The intermediate files this plan is going to create, which is GNU Make's
    /// own test for which of them it may delete afterwards: one it was never
    /// going to make is not one it put there.
    ///
    /// A peer output is spared however the edge that writes it is classified.
    /// Make enters the other targets of a multi-target pattern rule as targets
    /// in their own right, and being a target is what being intermediate is
    /// the absence of — so a build that sweeps up the invented file leaves the
    /// name written beside it.
    ///
    /// A name `-o` asserted a date for is swept too, and it is the one file the
    /// build was never going to make that GNU Make deletes anyway. The test in
    /// `remove_intermediates` (file.c) is `f->update_status == us_none` — "if
    /// nothing would have created this file yet, don't print an rm command for
    /// it" — and `-o` writes `us_success` over exactly that field
    /// (main.c:2312) at the same moment it writes the date. Which is how the
    /// switch comes to delete a file: the date is asserted rather than stated,
    /// so `f_mtime` never runs and never turns the intermediate bit off, and
    /// the status says something made it. Measured against 4.4.1, where
    /// `-o out.o` over `.INTERMEDIATE: out.o` says `rm out.o` and the same
    /// build without the switch keeps the file.
    pub(crate) fn disposable_outputs(&self) -> Vec<BString> {
        let mut seen = crate::graph::MarkSet::default();
        seen.begin(self.graph.node_ids().len());
        let mut swept = Vec::new();
        for edge in self.plan.wanted_edges(self.graph) {
            let outputs: &[NodeId] = &self.graph.edge(edge).out;
            for output in outputs {
                self.sweep_disposable(*output, &mut seen, &mut swept);
            }
        }
        for node in self.runtime.assumed_old.marked(self.graph) {
            self.sweep_disposable(node, &mut seen, &mut swept);
        }
        swept
            .into_iter()
            .map(|output| self.graph.node_path(output).to_owned())
            .collect()
    }

    /// Record `node` as one to sweep, unless it is not one or already is.
    fn sweep_disposable(
        &self,
        node: NodeId,
        seen: &mut crate::graph::MarkSet,
        swept: &mut Vec<NodeId>,
    ) {
        let Some(edge) = self.graph.node(node).generator else {
            return;
        };
        if !self.graph.edge(edge).disposable || self.graph.peer_outputs(edge).contains(&node) {
            return;
        }
        if !seen.replace(node.index()) {
            swept.push(node);
        }
    }

    /// What this build did about the command that generates `node`.
    ///
    /// One target's own answer. Ninja asks it about the manifest it generated
    /// its own input from, and only of a build that finished; a caller that let
    /// a build carry on past a failure has to ask it of each target separately,
    /// because the build as a whole stopping says nothing about which of them
    /// stopped it.
    pub(crate) fn made(&self, node: NodeId) -> Made {
        let Some(edge) = self.graph.node(node).generator else {
            return Made::Nothing;
        };
        if self.failed_edges.contains(&edge) {
            return Made::Failed;
        }
        if self.executed_edges.contains(&edge) && !self.runtime.edge(edge).restat_clean() {
            return Made::Regenerated;
        }
        Made::Nothing
    }

    /// Get `edge` ready to run, or say that there is nothing to run.
    ///
    /// A front end that binds commands at launch is the only thing that can
    /// answer the second way, and it answers it by having read a recipe that
    /// came to nothing. Nothing below this point happens for such an edge: no
    /// output directory is created, no response file is written, and the edge
    /// is not recorded as executed — a build that ran no command made nothing.
    fn prepare_edge(&mut self, edge: EdgeId) -> BuildResult<Option<PreparedEdge>> {
        let mut command = self.take_command(edge)?;
        let mut bound_steps = Vec::new();
        if self.bind_late_command(edge, &mut command, &mut bound_steps)? == Runs::Nothing {
            return Ok(None);
        }
        let bound = !bound_steps.is_empty();
        let steps = self.prepared_steps(edge, &command, bound_steps);
        let launch_rspfile_content =
            self.deferred_response_file_content(edge, &command.rspfile_content);
        let completion_outputs = self.graph.deferred_freshness(edge).map_or_else(
            || self.graph.edge(edge).out.clone(),
            |freshness| freshness.outputs.clone(),
        );
        let old_mtimes = self.mtimes_the_outputs_hold(edge, &completion_outputs);

        // A name the graph invented to be asked for by has no file behind it,
        // so the directory it appears to sit in is one nothing would ever write
        // into. Making it anyway left an empty `.ronin_recipe_stage/` in the
        // build root of every tree whose recipe composed a `$(MAKE)` — under
        // `-n` as well, where nothing at all may reach the disk.
        for output in completion_outputs
            .iter()
            .filter(|output| !self.graph.is_virtual_output(**output))
        {
            let path = self.graph.node_path(*output).to_owned();
            self.disk
                .make_dirs(path.to_path().expect("byte paths are valid on Unix"))
                .map_err(|source| {
                    BuildError::io(
                        BuildOperation::CreateOutputDirectory,
                        Some(path),
                        Some(edge),
                        source,
                    )
                })?;
        }

        // The depfile's directory is made and a response file the manifest
        // placed is not, which is Ninja's asymmetry rather than an oversight
        // here: a compiler writes its own depfile and cannot be asked to create
        // the directory first, while a response file is written by the build
        // tool, which fails outright if the manifest pointed it somewhere that
        // does not exist. The one exception is a response file named after an
        // invented output, handled in `prepare_response_file`: the compiler
        // chose that path, so the tool that invented it makes its directory.
        if let Some(depfile) = command.depfile_path.as_ref() {
            self.disk
                .make_dirs(depfile.to_path().expect("byte paths are valid on Unix"))
                .map_err(|source| {
                    BuildError::io(
                        BuildOperation::CreateOutputDirectory,
                        Some(depfile.clone()),
                        Some(edge),
                        source,
                    )
                })?;
        }

        // Only an invented output — the staging proxy of a composed recipe's
        // preceding segment — carries a response file with no directory behind
        // it, because the loop above skips exactly those. That is the one output
        // whose response-file directory the tool makes itself.
        let output_is_invented = completion_outputs
            .iter()
            .any(|output| self.graph.is_virtual_output(*output));
        let response_file = self.prepare_response_file(
            edge,
            command.rspfile.as_ref(),
            &launch_rspfile_content,
            output_is_invented,
        )?;

        let command_start_mtime = if self.options.dryrun {
            0
        } else {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|source| BuildError::Clock { source })?
                .as_nanos()
                .try_into()
                .unwrap_or(i64::MAX)
        };
        self.executed_edges.insert(edge);
        crate::graph::mark_written_here(self.graph, &mut self.runtime, edge);
        if self.output_sink.is_none() {
            self.commands_ran.push(command.command.clone());
        }
        // Said here rather than by the caller, so that the one edge with no
        // command has no place to be announced from: an edge is reported as
        // started exactly when this returns something to start.
        self.command_started(edge, &command)?;
        Ok(Some(PreparedEdge {
            edge,
            old_mtimes,
            command,
            steps,
            bound,
            running_step: RunningStep::default(),
            earlier_stdout: Vec::new(),
            earlier_stderr: Vec::new(),
            command_start_mtime,
            start_millis: self.progress.offset_millis(),
            pretended_a_step: false,
            _response_file: response_file,
        }))
    }

    /// How many commands may be running at once, right now.
    ///
    /// A load average over `-l` narrows it to one rather than to none: Ninja
    /// keeps a build moving under load instead of stalling it, so the limit is
    /// a brake and not a gate.
    fn job_limit(&self, load: &mut status::LoadSampler) -> usize {
        if self.options.maxload > 0.0 && load.current() > self.options.maxload {
            return 1;
        }
        match self.options.jobs {
            JobLimit::Auto => 1,
            JobLimit::Unlimited => usize::MAX,
            JobLimit::Fixed(jobs) => jobs.get(),
        }
    }

    /// Whether a finished command says the build was cut short rather than that
    /// it failed.
    ///
    /// Ninja funnels both answers through one number. `ParseExitStatus` turns a
    /// child killed by `SIGINT`, `SIGTERM` or `SIGHUP` into `ExitInterrupted`,
    /// which is 130, and then the build loop asks only whether the status *is*
    /// `ExitInterrupted` — so a command that plainly exited 130 of its own
    /// accord takes the same branch, with no way for Ninja to tell the two
    /// apart and no attempt to. That is the contract, oddity included.
    // [spec:ronin:req:compat.process-integration+2]
    // [spec:ronin:req:product.build-outcome]
    fn command_interrupted(&self, status: std::process::ExitStatus) -> bool {
        if status_interrupted(status) {
            // A recipe of GNU Make's dies of a signal for two quite different
            // reasons, and Make tells them apart by who was signalled. Ctrl-C
            // reaches the whole process group, so this process has the same
            // signal recorded against it and the build really was cut short.
            // A recipe that killed itself, or that something else killed,
            // leaves this process untouched and is simply a command that
            // failed.
            return !self.options.recipe_signal_fails || crate::signal::interrupted().is_some();
        }
        self.options.command_status_interrupts
            && crate::subprocess::exit_status_code(status)
                == crate::subprocess::INTERRUPTED_EXIT_CODE
    }

    /// Remove the outputs a stopped command had already written to.
    ///
    /// The test is GNU Make's own, in `delete_target`: an output goes only when
    /// its timestamp is no longer the one the build read before the recipe ran.
    /// A recipe that failed without ever reaching its target leaves the file
    /// that was already there exactly as it found it, and a recipe that wrote
    /// its target and then put the timestamp back has, as far as either tool
    /// can tell, not written it.
    ///
    /// Nothing is said about it. `[spec:ronin:req:make.narration+1]` puts Make
    /// mode's reporting in the manifest front end's shape, so GNU Make's
    /// `*** Deleting file 'x'` is not owed; withdrawing a half-written output
    /// is the same act on both paths through here, and the interrupt path has
    /// always done it silently.
    fn withdraw_outputs(&self, edge: EdgeId, old_mtimes: &[i64], which: Withdraw) {
        let eligible = match which {
            // No entry is a manifest front end leaving the question alone, and
            // Ninja's answer to it is that everything a cut-short command wrote
            // goes. An entry narrowed to nothing is a Makefile's answer.
            Withdraw::Stopped => self
                .graph
                .withdrawal(edge)
                .map(|withdrawal| withdrawal.outputs.as_slice()),
            Withdraw::DeleteOnError => Some(self.graph.delete_on_error(edge)),
        };
        if eligible.is_some_and(<[NodeId]>::is_empty) {
            return;
        }
        let disk = self.disk.clone();
        let completion_outputs = self.graph.deferred_freshness(edge).map_or_else(
            || self.graph.edge(edge).out.as_slice(),
            |state| &state.outputs,
        );
        for (output, old_mtime) in completion_outputs.iter().zip(old_mtimes) {
            if eligible.is_some_and(|eligible| !eligible.contains(output)) {
                continue;
            }
            let path = self.graph.node_path(*output).to_owned();
            let path = path.to_path().expect("byte paths are valid on Unix");
            if disk.stat(path).ok() != Some(*old_mtime) {
                let _ = disk.remove_file(path);
            }
        }
    }

    // [spec:ronin:def:build.nodedone-fn]
    // [spec:ronin:sem:build.nodedone-fn]
    // [spec:ronin:def:build.shouldprune-fn]
    // [spec:ronin:sem:build.shouldprune-fn]
    // [spec:ronin:def:build.edgedone-fn]
    // [spec:ronin:sem:build.edgedone-fn]
    // [spec:ronin:def:build.jobdone-fn]
    // [spec:ronin:sem:build.jobdone-fn]
    #[allow(
        clippy::too_many_lines,
        reason = "edge completion is one ordered transaction whose cleanup and log updates must stay together"
    )]
    fn finish_edge(
        &mut self,
        prepared: PreparedEdge,
        result: Result<Option<ProcessOutput>, ProcessError>,
    ) -> BuildResult<(bool, Vec<NodeId>)> {
        let PreparedEdge {
            edge,
            old_mtimes,
            command,
            steps: _,
            bound: _,
            running_step: _,
            earlier_stdout,
            earlier_stderr,
            command_start_mtime,
            start_millis,
            pretended_a_step,
            _response_file,
        } = prepared;
        // Account for the edge before anything reports progress: the status
        // line about to be printed is the one that has to show it as done.
        // Reading the previous duration has to happen here too, before this
        // run's own entry replaces last run's in the log.
        let end_millis = self.progress.offset_millis();
        let previous_duration =
            status::previous_duration(self.graph, self.build_log.as_deref(), edge);
        self.progress
            .retire_edge(i64::from(end_millis - start_millis), previous_duration);
        let mut msvc_deps = Vec::new();
        let mut visible_output = Vec::new();
        // What the steps before the last one wrote is this edge's output too,
        // and is reported at the same moment: an edge speaks once, whether the
        // recipe it ran was one process or six.
        if !earlier_stdout.is_empty() || !earlier_stderr.is_empty() {
            self.record_child_output(&earlier_stdout);
            visible_output.extend_from_slice(&earlier_stdout);
            self.record_child_output(&earlier_stderr);
            visible_output.extend_from_slice(&earlier_stderr);
        }
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.command_finished(edge, &command, Some(1), &visible_output)?;
                return Err(error.into());
            }
        };
        if let Some(ProcessOutput {
            status,
            stdout,
            stderr,
        }) = result
        {
            if command.deps_type == DepsType::Msvc {
                let mut parser = crate::msvc::ClParser::default();
                let filtered = parser.parse(&stdout, command.msvc_deps_prefix.as_bytes());
                self.record_child_output(filtered.as_bytes());
                visible_output.extend_from_slice(filtered.as_bytes());
                msvc_deps.extend(
                    parser
                        .includes
                        .into_iter()
                        .map(|include| crate::graph::mknode(self.graph, include)),
                );
            } else {
                self.record_child_output(&stdout);
                visible_output.extend_from_slice(&stdout);
            }
            self.record_child_output(&stderr);
            visible_output.extend_from_slice(&stderr);
            let dependency_result = (|| -> BuildResult<()> {
                if status.success() && !self.options.dryrun {
                    match &command.deps_type {
                        DepsType::None | DepsType::Msvc => Ok(()),
                        DepsType::Gcc => {
                            let path = command.depfile_path.as_ref().ok_or({
                                BuildError::DependencyFileMissing { edge, path: None }
                            })?;
                            // Ninja reads a depfile that is not there as an
                            // empty one — `NotFound` clears the error and the
                            // empty content returns success — so the edge is
                            // recorded with no discovered dependencies and the
                            // command that succeeded stays succeeded. Only a
                            // depfile that exists and will not parse fails the
                            // command. C samurai stopped the build here instead,
                            // which cost a compiler that emits no depfile for a
                            // translation unit with no includes its exit status.
                            let deps = self
                                .disk
                                .exists(path.to_path().expect("byte paths are valid on Unix"))
                                .then(|| {
                                    crate::deps::depsparse(
                                        self.graph,
                                        &self.disk.resolve(
                                            path.to_path().expect("byte paths are valid on Unix"),
                                        ),
                                        false,
                                    )
                                })
                                .transpose()?;
                            self.replace_depfile_deps(
                                edge,
                                deps.as_ref().map_or(&[][..], |deps| &deps.nodes),
                            );
                            let state = self.runtime.edge_mut(edge);
                            state.set_deps_loaded(true);
                            state.set_deps_missing(false);
                            Ok(())
                        }
                        DepsType::Unsupported(deps_type) => Err(BuildError::UnsupportedDepsType {
                            edge,
                            deps_type: deps_type.clone(),
                        }),
                    }
                } else {
                    Ok(())
                }
            })();
            if let Err(error) = dependency_result {
                self.command_finished(edge, &command, Some(1), &visible_output)?;
                return Err(error);
            }
            // An interrupted command is not a failed one, so it is never
            // reported as failed: Ninja tests for the interrupt before it
            // finishes the command, which is why a build cut short by SIGTERM
            // prints no `FAILED:` line. Half-written outputs still go.
            if self.command_interrupted(status) {
                self.withdraw_outputs(edge, &old_mtimes, Withdraw::Stopped);
                return Err(BuildError::Interrupted {
                    status: Some(status),
                });
            }
            let signal = self
                .options
                .recipe_signal_fails
                .then(|| crate::subprocess::signalled_exit_code(status))
                .flatten();
            self.command_finished(
                edge,
                &command,
                (!status.success())
                    .then(|| signal.unwrap_or_else(|| crate::subprocess::exit_status_code(status))),
                &visible_output,
            )?;
            // A recipe whose errors Make was told to ignore leaves its target
            // made: the status has been reported and the build carries on.
            if !status.success() && !command.ignore_errors {
                // Said after the failure is reported and before the build stops,
                // which is where GNU Make says it too: the recipe has had its
                // error printed, and what it half-wrote goes before anything
                // downstream can find a file with that name and believe it.
                //
                // GNU Make asks `exit_sig != 0 || delete_on_error`, two reasons
                // and not one. A recipe that died of a signal never reached the
                // end of its own script, so what it wrote is unfinished whatever
                // the Makefile said; a recipe that chose its exit status is
                // taken at its word, and only `.DELETE_ON_ERROR` overrides it.
                self.withdraw_outputs(
                    edge,
                    &old_mtimes,
                    if signal.is_some() {
                        Withdraw::Stopped
                    } else {
                        Withdraw::DeleteOnError
                    },
                );
                return Err(BuildError::SubcommandFailed {
                    edge,
                    command: command.command,
                    status,
                });
            }
        } else {
            self.command_finished(edge, &command, None, &[])?;
        }

        // Ahead of the stat below, so the date `-t` has just given each output
        // is the date this run records for it.
        self.touch_outputs(edge, pretended_a_step)?;

        let disk = self.disk.clone();
        let mut new_mtimes = Vec::new();
        let deferred_outputs = self
            .graph
            .deferred_freshness(edge)
            .map(|freshness| freshness.outputs.clone());
        let edge_hash = edgehash(
            &mut self.runtime,
            edge,
            command.command.as_bstr(),
            (!command.rspfile_content.is_empty()).then_some(command.rspfile_content.as_bstr()),
        );
        if let Some(outputs) = &deferred_outputs {
            let mut logical_mtime = FileTime::MISSING;
            for output in outputs {
                let path = self.graph.node_path(*output).to_owned();
                let mtime = disk
                    .stat(path.to_path().expect("byte paths are valid on Unix"))
                    .map_err(|source| {
                        BuildError::io(
                            BuildOperation::StatOutput,
                            Some(path.clone()),
                            Some(edge),
                            source,
                        )
                    })?;
                self.runtime
                    .node_mut(*output)
                    .observe(FileTime::observed(mtime));
                logical_mtime = logical_mtime.max(FileTime::observed(mtime));
                new_mtimes.push(mtime);
            }
            for output in &self.graph.edge(edge).out {
                let output = self.runtime.node_mut(*output);
                output.set_mtime(logical_mtime);
                output.set_dirty(false);
                output.set_logged_command_hash(edge_hash);
            }
        } else {
            let output_ids = self.graph.edge(edge).out.clone();
            for output in output_ids {
                let path = self.graph.node_path(output).to_owned();
                let mtime = disk
                    .stat(path.to_path().expect("byte paths are valid on Unix"))
                    .map_err(|source| {
                        BuildError::io(
                            BuildOperation::StatOutput,
                            Some(path.clone()),
                            Some(edge),
                            source,
                        )
                    })?;
                let output = self.runtime.node_mut(output);
                output.observe(FileTime::observed(mtime));
                output.set_dirty(false);
                output.set_logged_command_hash(edge_hash);
                new_mtimes.push(mtime);
            }
        }
        if !self.options.dryrun {
            match &command.deps_type {
                DepsType::Gcc => {
                    if let Some(deps_log) = self.deps_log.as_deref_mut() {
                        crate::deps::depsrecord(
                            deps_log,
                            edge,
                            self.graph,
                            &self.runtime,
                            &self.disk,
                        )?;
                    }
                }
                DepsType::Msvc => {
                    if let Some(deps_log) = self.deps_log.as_deref_mut() {
                        crate::deps::depsrecordnodes(
                            deps_log,
                            self.graph,
                            &self.runtime,
                            edge,
                            &msvc_deps,
                        )?;
                    }
                }
                DepsType::None => {}
                DepsType::Unsupported(_) => {
                    unreachable!("dependency type was validated before status output")
                }
            }
        }
        if command.deps_type == DepsType::Gcc
            && let Some(path) = &command.depfile_path
            && !self.options.keepdepfile
        {
            let _ = self
                .disk
                .remove_file(path.to_path().expect("byte paths are valid on Unix"));
        }
        let mut loaded_dyndeps = Vec::new();
        if !self.options.dryrun {
            let generated_dyndeps = self
                .graph
                .edge(edge)
                .out
                .iter()
                .filter(|output| self.runtime.node(**output).dyndep_pending())
                .copied()
                .collect::<Vec<_>>();
            for dyndep in generated_dyndeps {
                crate::dyndep::load_dyndep(self.graph, &mut self.runtime, dyndep, &self.disk)?;
                self.synchronize_runtime();
                loaded_dyndeps.push(dyndep);
            }
        }
        self.runtime.edge_mut(edge).set_command_dirty(false);
        // An invented file's outputs stood in for the newest thing behind them
        // while the file was not there. The command has now been run and the
        // outputs stat'd, so whatever they hold is theirs and a later scan must
        // not substitute over it. The work the scan was holding for it is done
        // with the same breath, so nothing can come back and ask for it again.
        self.runtime.edge_mut(edge).set_absent_intermediate(false);
        self.runtime.edge_mut(edge).set_intermediate_pending(false);
        let unchanged_outputs = old_mtimes
            .iter()
            .zip(&new_mtimes)
            .map(|(old, new)| old == new)
            .collect::<Vec<_>>();
        let deferred = deferred_outputs.is_some();
        let freshness::Settled { pruned, all_pruned } = self.settled(
            edge,
            &freshness::Outcome {
                deferred,
                restat: command.restat,
                new_mtimes: &new_mtimes,
                unchanged: &unchanged_outputs,
                started: command_start_mtime,
            },
        );
        let mut record_mtime = command_start_mtime;
        if !self.options.dryrun && (command.restat || command.generator || deferred) {
            record_mtime = record_mtime.max(new_mtimes.iter().copied().max().unwrap_or_default());
        }
        if pruned {
            record_mtime = command_start_mtime;
        }
        for output in self.graph.edge(edge).out.clone() {
            self.runtime
                .node_mut(output)
                .set_log_mtime(FileTime::observed(record_mtime));
        }
        // A dry run must leave the log alone. Ninja records nothing for a
        // command it did not run, and recording one entry per planned edge
        // grows the log without bound under any workflow that dry-runs often,
        // which every later invocation of any tool then pays to load. The
        // in-memory mtime above is still set, because the rest of this run's
        // planning depends on it; only the persistent write is skipped.
        if !self.options.dryrun
            && let Some(build_log) = self.build_log.as_deref_mut()
        {
            crate::log::logrecordedge(
                build_log,
                self.graph,
                edge,
                edge_hash,
                start_millis,
                end_millis,
                record_mtime,
            )?;
        }
        self.runtime.edge_mut(edge).set_restat_clean(all_pruned);
        if deferred {
            self.runtime.deferred_mut(edge).settle();
        }
        Ok((pruned, loaded_dyndeps))
    }

    /// Complete an edge with no command, which is the whole of running it.
    ///
    /// An alias is settled by definition: its outputs were never files, so
    /// there is nothing to look at and nothing for a dependent to reconsider.
    ///
    /// A Makefile target whose recipe wrote nothing is settled the same way
    /// every other Make target is — from the disk, afterwards. GNU Make runs
    /// the empty recipe, reads the target again, and lets what it then finds
    /// decide what reads it: a target still on disk where it was leaves its
    /// dependents alone, and one that is not there leaves them out of date. The
    /// second look costs a stat and is the difference between a build that
    /// settles and one that runs the same recipe forever.
    fn finish_phony_edge(&mut self, edge: EdgeId) -> (bool, Vec<NodeId>) {
        let reobserve = self.graph.edge(edge).outputs_unaliased && !self.options.dryrun;
        let outputs: Vec<NodeId> = self.graph.edge(edge).out.to_vec();
        let mut every_output_made = true;
        if reobserve {
            let disk = self.disk.clone();
            for &output in &outputs {
                let path = self.graph.node_path(output);
                let mtime = disk
                    .stat(path.to_path().expect("byte paths are valid on Unix"))
                    .map_or(FileTime::MISSING, FileTime::observed);
                every_output_made &= !mtime.is_missing();
                self.runtime.node_mut(output).observe(mtime);
            }
        }
        for &output in &outputs {
            self.runtime.node_mut(output).set_dirty(false);
        }
        // A target the recipe left absent is what GNU Make reads as infinitely
        // new, so nothing that waited for it is settled by this.
        let pruned = reobserve && every_output_made;
        self.runtime.edge_mut(edge).set_restat_clean(pruned);
        (pruned, Vec::new())
    }

    fn recompute_consumers_after_restat(&mut self, edge: EdgeId) -> BuildResult<()> {
        let mut queue = Vec::new();
        for output in &self.graph.edge(edge).out {
            queue.extend(self.graph.node(*output).uses.iter().copied());
            queue.extend(self.graph.node_validation_uses(*output).iter().copied());
        }
        self.visited_edges.begin(self.graph.edge_count());
        let disk = self.disk.clone();
        while let Some(dependent) = queue.pop() {
            if self.visited_edges.replace(dependent.index()) {
                continue;
            }
            let outputs: &[NodeId] = &self.graph.edge(dependent).out;
            for &output in outputs {
                let mut stat = |path: &Path| disk.stat(path);
                recompute_dirty_with_validations(
                    self.graph,
                    &mut self.runtime,
                    &mut self.scratch,
                    output,
                    &mut stat,
                )?;
            }
            for &output in outputs {
                queue.extend(self.graph.node(output).uses.iter().copied());
                queue.extend(self.graph.node_validation_uses(output).iter().copied());
            }
        }
        Ok(())
    }

    fn recompute_planned_after_dyndep(&mut self, loaded_dyndeps: &[NodeId]) -> BuildResult<()> {
        let disk = self.disk.clone();
        self.plan
            .expanded_weight
            .fill(CriticalPathWeight::default());
        let mut nodes = self.targets.clone();
        nodes.extend(
            self.plan
                .wanted_edges(self.graph)
                .into_iter()
                .filter_map(|edge| self.graph.edge(edge).out.first().copied()),
        );
        let mut loaded_marks = Vec::new();
        for dyndep in loaded_dyndeps {
            loaded_marks.resize(loaded_marks.len().max(dyndep.index() + 1), false);
            loaded_marks[dyndep.index()] = true;
        }
        let mut affected = Vec::new();
        let mut affected_edges = Vec::new();
        for edge in self.graph.edge_ids() {
            if self
                .graph
                .edge(edge)
                .dyndep
                .is_some_and(|dyndep| loaded_marks.get(dyndep.index()).copied().unwrap_or(false))
            {
                affected_edges.push(edge);
                affected.extend(self.graph.edge(edge).out.first().copied());
            }
        }
        for edge in affected_edges.iter().copied() {
            self.invalidate_command(edge);
        }
        nodes.extend(affected.iter().copied());
        let mut visited_edges = Vec::new();
        let mut loaded_files = Vec::new();
        for node in nodes.iter().copied() {
            self.load_ready_dyndeps_for(node, &mut visited_edges, &mut loaded_files)?;
        }
        if self.build_log.is_some() {
            for edge in affected_edges {
                if !self.graph.is_phony_rule(self.graph.edge(edge).rule) {
                    self.refresh_command_hash(edge)?;
                }
            }
        }
        let mut visited = Vec::new();
        let mut validations = Vec::new();
        for node in nodes {
            visited.resize(visited.len().max(node.index() + 1), false);
            if std::mem::replace(&mut visited[node.index()], true) {
                continue;
            }
            let mut stat = |path: &Path| disk.stat(path);
            validations.extend(recompute_dirty_with_validations(
                self.graph,
                &mut self.runtime,
                &mut self.scratch,
                node,
                &mut stat,
            )?);
        }
        for target in self.targets.iter().copied() {
            self.plan.add_target(self.graph, &self.runtime, target)?;
        }
        for output in affected {
            self.plan.add_target(self.graph, &self.runtime, output)?;
        }
        for validation in validations {
            self.plan
                .add_target(self.graph, &self.runtime, validation)?;
        }
        self.record_dirty_explanations();
        Ok(())
    }

    fn settle_edge(
        &mut self,
        edge: EdgeId,
        result: BuildResult<(bool, Vec<NodeId>)>,
    ) -> BuildResult<()> {
        match result {
            Ok((pruned, loaded_dyndeps)) => {
                if pruned {
                    self.recompute_consumers_after_restat(edge)?;
                }
                if !loaded_dyndeps.is_empty() {
                    self.recompute_planned_after_dyndep(&loaded_dyndeps)?;
                    self.plan.refresh_dependencies(self.graph, &self.runtime)?;
                }
                let pruned = self.plan.edge_finished(
                    self.graph,
                    &self.runtime,
                    edge,
                    EdgeResult::Succeeded,
                )?;
                status::forget_pruned_work(
                    &mut self.progress,
                    self.graph,
                    self.build_log.as_deref(),
                    &pruned,
                );
                Ok(())
            }
            Err(error) => {
                self.failed_edges.insert(edge);
                self.plan
                    .edge_finished(self.graph, &self.runtime, edge, EdgeResult::Failed)?;
                Err(error)
            }
        }
    }

    // [spec:ronin:req:compat.scheduling]
    // [spec:ronin:req:compat.process-integration+2]
    // [spec:ronin:def:build.catchsig-fn]
    // [spec:ronin:sem:build.catchsig-fn]
    // [spec:ronin:def:build.build-fn]
    // [spec:ronin:sem:build.build-fn]
    // [spec:ronin:req:compat.command-runtime]
    // [spec:ronin:def:build.formatstatus-fn]
    // [spec:ronin:sem:build.formatstatus-fn]
    // [spec:ronin:def:build.printstatus-fn]
    // [spec:ronin:sem:build.printstatus-fn]
    // [spec:ronin:def:build.jobstart-fn]
    // [spec:ronin:sem:build.jobstart-fn]
    // [spec:ronin:def:build.jobwork-fn]
    // [spec:ronin:sem:build.jobwork-fn]
    // [spec:ronin:def:build.queryload-fn]
    // [spec:ronin:sem:build.queryload-fn]
    #[allow(
        clippy::too_many_lines,
        reason = "the completion-driven scheduler loop is clearer as one explicit state machine"
    )]
    pub(crate) fn build(&mut self) -> BuildResult<()> {
        self.plan.prepare_queue(self.graph);
        self.progress.started = 0;
        self.progress.finished = 0;
        self.progress.total = self.plan.command_edge_count(self.graph);
        self.progress.start = Instant::now();
        status::seed_prediction(
            &mut self.progress,
            &self.plan,
            self.graph,
            self.build_log.as_deref(),
        );
        let mut failures = 0;
        let mut last_error = None;
        let failure_limit = self.options.maxfail.max(1);
        let mut running: Vec<Option<PreparedEdge>> = Vec::new();
        running.resize_with(self.graph.edge_count(), || None);
        let mut running_slots = Vec::new();
        running_slots.resize_with(self.graph.edge_count(), || None);
        let mut console_running = false;
        // A dry run starts no process, so it must claim no slot and publish no
        // budget: a jobserver there would be a budget nothing is spending.
        let transport = if self.options.dryrun {
            None
        } else {
            match self.options.jobserver.clone() {
                Some(inherited) => Some(inherited),
                _ => {
                    if let (true, JobLimit::Fixed(jobs)) =
                        (self.options.serve_jobserver, self.options.jobs)
                    {
                        // A budget of one has nothing to share, and Ninja's `-j0` has no
                        // budget at all. GNU Make publishes no jobserver in either case.
                        // Neither has a build with no command to run, which is most of
                        // them: an up-to-date tree must not pay to create and remove a
                        // fifo nothing was ever going to open.
                        (jobs.get() > 1 && self.progress.total != 0)
                            .then(|| crate::jobserver::Transport::serve(jobs))
                            .transpose()?
                            .flatten()
                    } else {
                        None
                    }
                }
            }
        };
        let mut environment = Vec::new();
        // A generic inherited or manifest-served jobserver remains process
        // environment, as it is for Ninja. Frontend variables are already in
        // graph command text and never reach this executor boundary.
        if let Some(transport) = transport.as_ref() {
            transport.publish_into(&mut environment);
        }
        let mut processes = ProcessSupervisor::<crate::jobserver::Acquisition>::in_directory(
            self.options.working_directory.as_path(),
            self.options.shell.clone(),
            &environment,
        )?;
        let mut jobserver = transport
            .map(|transport| {
                let sender = processes.external_sender();
                crate::jobserver::JobserverClient::new(transport, move |result| {
                    sender.send(result);
                })
            })
            .transpose()?;
        let mut available_slot = None;
        let mut load = status::LoadSampler::default();

        loop {
            // Set when work was deferred for want of a shared slot, which is
            // the one wait a served jobserver cannot wake by itself.
            let mut starved = false;
            if let Some(signal) = crate::signal::interrupted() {
                // The build ends here rather than at the next completion, which
                // is where Ninja ends it: `DoWork` reports the interrupt ahead
                // of the commands it was about to reap, `WaitForCommand` hands
                // the loop back its answer with those commands still running,
                // and `Cleanup` stops them and withdraws what they wrote. None
                // of them is reported finished or recorded, whatever status it
                // would eventually have left with.
                //
                // Waiting for them instead is what let a recipe outlive the
                // signal and be counted as having succeeded. A shell handed
                // several command lines takes an interrupt that arrives between
                // two of them as a flag and runs the next one anyway — measured
                // for Ronin's own shell, for dash and for bash alike — so the
                // command reaching this loop again is one that finished, and
                // the edge was recorded done with its output standing behind a
                // build that had already been cut short.
                processes.interrupt(signal)?;
                // Stopped and reaped before anything is withdrawn: a command
                // still running is a command that can still write the file
                // being taken back.
                processes.stop();
                for prepared in running.iter().flatten() {
                    self.withdraw_outputs(prepared.edge, &prepared.old_mtimes, Withdraw::Stopped);
                }
                last_error = Some(BuildError::Interrupted { status: None });
                break;
            }
            let maxjobs = self.job_limit(&mut load);
            while !console_running && processes.running_len() < maxjobs && failures < failure_limit
            {
                let Some(edge) = self.plan.find_work(self.graph) else {
                    break;
                };
                if !self.advance_deferred(edge, &mut failures, failure_limit, &mut last_error) {
                    if failures >= failure_limit {
                        break;
                    }
                    continue;
                }
                let is_phony = self.graph.is_phony_rule(self.graph.edge(edge).rule);
                if is_phony {
                    let result = Ok(self.finish_phony_edge(edge));
                    if let Err(error) = self.settle_edge(edge, result) {
                        failures += 1;
                        last_error = Some(error);
                    }
                    continue;
                }
                let use_console = self.graph.is_console_pool(self.graph.edge(edge).pool);
                if use_console && processes.running_len() != 0 {
                    self.plan.defer_work(self.graph, edge);
                    break;
                }
                let slot = if let Some(client) = jobserver.as_mut() {
                    // The implicit slot first, because it is the one slot that
                    // costs the shared budget nothing. Only past it does a
                    // command of Ronin's own take capacity a child could have.
                    let held = match available_slot
                        .take()
                        .or_else(|| client.try_acquire_implicit())
                    {
                        Some(slot) => Some(slot),
                        None => client.try_acquire_token()?,
                    };
                    if let Some(slot) = held {
                        Some(slot)
                    } else {
                        self.plan.defer_work(self.graph, edge);
                        client.request_token();
                        starved = true;
                        break;
                    }
                } else {
                    None
                };
                match self.prepare_edge(edge) {
                    // The recipe was read as the edge was launched and held no
                    // command line. Nothing runs, nothing is reported, and the
                    // count of work loses the edge that turned out not to be
                    // any — the same accounting a deferred edge gets when its
                    // freshness test comes out negative.
                    Ok(None) => {
                        if let Some(slot) = slot {
                            slot.release();
                        }
                        if !self.settle_unrun_edge(
                            edge,
                            &mut failures,
                            failure_limit,
                            &mut last_error,
                        ) {
                            break;
                        }
                    }
                    Ok(Some(mut prepared)) => {
                        let (launch, pretended) = Self::take_step(&mut prepared, self.pretending());
                        match processes.spawn(edge, launch, use_console, pretended) {
                            Ok(()) => {
                                running[edge.index()] = Some(prepared);
                                running_slots[edge.index()] = slot;
                                console_running = use_console;
                                if use_console {
                                    break;
                                }
                            }
                            Err(error) => {
                                if let Some(slot) = slot {
                                    slot.release();
                                }
                                let result = self.finish_edge(prepared, Err(error));
                                if let Err(error) = self.settle_edge(edge, result) {
                                    failures += 1;
                                    last_error = Some(error);
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(slot) = slot {
                            slot.release();
                        }
                        self.failed_edges.insert(edge);
                        self.plan.edge_finished(
                            self.graph,
                            &self.runtime,
                            edge,
                            EdgeResult::Failed,
                        )?;
                        // An edge that could not be started ends the build
                        // whatever `-k` still allowed: Ninja leaves its build
                        // loop the moment `StartEdge` fails, without asking
                        // how many failures were permitted. The allowance is
                        // about commands that ran and said no; nothing ran
                        // here, and the manifest asked for something the disk
                        // will refuse just as firmly for every edge after it.
                        failures = failure_limit;
                        last_error = Some(error);
                    }
                }
            }

            if processes.running_len() == 0 {
                break;
            }
            let deadline = starved
                .then(|| {
                    jobserver
                        .as_ref()
                        .and_then(crate::jobserver::JobserverClient::retry_interval)
                })
                .flatten();
            let Some(wake) = processes.wait(deadline)? else {
                continue;
            };
            let completion = match wake {
                SupervisorWake::Process(completion) => completion,
                SupervisorWake::External(result) => {
                    let client = jobserver
                        .as_mut()
                        .expect("jobserver events require an active client");
                    available_slot = Some(client.receive_token(result)?);
                    continue;
                }
            };
            let edge = completion.edge;
            let mut prepared = running[edge.index()]
                .take()
                .expect("completed edges have running preparation state");
            // A recipe is several processes in GNU Make, run one after
            // another, so a completion is the end of the edge only once the
            // last of them has come back or one of them has said stop. The
            // slot and the console stay with the edge while it has more to do.
            let result =
                match self.continue_recipe(&mut processes, &mut prepared, completion.result) {
                    Advance::Relaunched => {
                        running[edge.index()] = Some(prepared);
                        continue;
                    }
                    Advance::Finished(result) => result,
                };
            if let Some(slot) = running_slots[edge.index()].take() {
                slot.release();
            }
            if prepared.command.use_console {
                console_running = false;
            }
            let result = self.finish_edge(prepared, result);
            if let Err(error) = self.settle_edge(edge, result) {
                // Ninja leaves the build loop the moment a command reports an
                // interrupt, whatever allowance `-k` still had: the interrupt
                // is checked before the completion is even counted as a
                // failure. Carrying on would let the next command's status
                // overwrite the 130 that says why the build stopped, which is
                // the whole answer a caller is reading.
                let interrupted = matches!(error, BuildError::Interrupted { .. });
                failures += 1;
                last_error = Some(error);
                if interrupted {
                    // The command that reported has given back what it wrote,
                    // and every other command still running is in exactly the
                    // same position: it was cut short mid-recipe and what it
                    // left is unfinished. GNU Make says so in one loop —
                    // `fatal_error_signal` walks the whole child list calling
                    // `delete_child_targets` — where leaving the loop here
                    // would otherwise let a sibling's half-written output
                    // outlive the build that abandoned it. They have already
                    // been signalled, at the top of this loop.
                    for prepared in running.iter().flatten() {
                        self.withdraw_outputs(
                            prepared.edge,
                            &prepared.old_mtimes,
                            Withdraw::Stopped,
                        );
                    }
                    break;
                }
            }
        }

        // Ninja carries the last failing command's status out of the build, and
        // records the last failure as it goes; those are the same thing, so the
        // status is read back off the error rather than tracked beside it.
        // [spec:ronin:req:product.build-outcome]
        let outcome = if let Some(error) = last_error {
            Err(BuildError::Stopped {
                status: error.exit_code(),
                reason: BuildStop::from_failure(
                    error,
                    failures,
                    failure_limit,
                    self.options.maxfail,
                ),
            })
        } else if self.plan.more_to_do() {
            // Ninja returns success here, having recorded no failure to take a
            // status from. A build that did not finish must not report that it
            // did, so this one deliberately does not.
            Err(BuildError::Stopped {
                status: 1,
                reason: BuildStop::Stuck,
            })
        } else {
            Ok(())
        };
        // The bar has to be given back whatever happened, so this runs on the
        // failure path too — and the build's own error outranks any trouble
        // writing the closing line.
        let closing = self.emit_summary(outcome.is_ok());
        match outcome {
            Err(error) => Err(error),
            Ok(()) => closing,
        }
    }
}

mod command;
mod release;
use command::Runs;
pub(crate) use command::{LateBinding, LateCommand, LateCommands, LateStep};
mod deferred;
mod freshness;
mod reporter;
mod status;
mod touch;
#[cfg(test)]
pub(crate) use status::format_progress_status;

#[cfg(test)]
#[path = "build/tests.rs"]
mod tests;
