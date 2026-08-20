use super::reporter::Rendering;
use super::{Builder, status};
use crate::error::{BuildError, BuildOperation, ProcessError};
use crate::graph::{EdgeId, Graph, PathStyle, edgehash};
use crate::names::Names;
use crate::subprocess::{Launch, ProcessOutput};
use crate::util::{BString, ByteSlice};
use std::fs;

type BuildResult<T> = Result<T, BuildError>;

/// The recipe's errors are Make's to ignore: `-` on every line, `-i`, `.IGNORE`.
///
/// Bound only where a nonzero status can mean nothing else, so the build reads
/// the status the recipe left, says what it was, and carries on.
pub(crate) const IGNORE_ERRORS: &[u8] = b"ignore_errors";

#[allow(
    clippy::struct_excessive_bools,
    reason = "each is one binding an edge either carries or does not, and grouping them would name a state nothing declares"
)]
pub(super) struct CommandSpec {
    pub(super) command: BString,
    pub(super) description: BString,
    pub(super) rspfile: Option<BString>,
    pub(super) rspfile_content: BString,
    pub(super) deps_type: DepsType,
    pub(super) depfile_path: Option<BString>,
    pub(super) msvc_deps_prefix: BString,
    pub(super) restat: bool,
    pub(super) generator: bool,
    pub(super) use_console: bool,
    /// A nonzero status here is an error Make was told to ignore.
    pub(super) ignore_errors: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum DepsType {
    None,
    Gcc,
    Msvc,
    Unsupported(String),
}

impl DepsType {
    /// Classify a `deps` value without taking ownership of it.
    ///
    /// The two supported values and the empty case need no allocation at all;
    /// only an unsupported value is kept, to name it in the error.
    fn from_bytes(value: &[u8]) -> Option<Self> {
        match value {
            b"" => Some(Self::None),
            b"gcc" => Some(Self::Gcc),
            b"msvc" => Some(Self::Msvc),
            _ => std::str::from_utf8(value)
                .ok()
                .map(|name| Self::Unsupported(name.to_owned())),
        }
    }
}

impl CommandSpec {
    /// Evaluate an edge's control bindings in one pass.
    ///
    /// Only four of these are kept; the rest are inspected and discarded, so
    /// they share `scratch` rather than each allocating a result.
    ///
    /// There is no Make exception here and no place for one. A dry run prints
    /// the commands the graph holds and runs none of them, whichever front end
    /// built the graph; recursive Make reached the graph as composed child
    /// edges, so a dry run already has them to print.
    fn evaluate(graph: &Graph, edge: EdgeId, scratch: &mut Vec<u8>) -> BuildResult<Self> {
        let command = crate::env::edgevar(graph, edge, Names::COMMAND, PathStyle::ShellEscaped)
            .unwrap_or_default();
        let description =
            crate::env::edgevar(graph, edge, Names::DESCRIPTION, PathStyle::ShellEscaped)
                .unwrap_or_default();
        let rspfile = crate::env::edgevar(graph, edge, Names::RSPFILE, PathStyle::Raw)
            .filter(|path| !path.is_empty());
        let rspfile_content =
            crate::env::edgevar(graph, edge, Names::RSPFILE_CONTENT, PathStyle::ShellEscaped)
                .unwrap_or_default();
        scratch.clear();
        crate::env::edgevar_into(graph, edge, Names::DEPS, PathStyle::Raw, scratch);
        let deps_type =
            DepsType::from_bytes(scratch).ok_or(BuildError::InvalidDepsEncoding { edge })?;
        let depfile_path = crate::env::edgevar(graph, edge, Names::DEPFILE, PathStyle::Raw)
            .filter(|path| !path.is_empty());
        let msvc_deps_prefix =
            crate::env::edgevar(graph, edge, Names::MSVC_DEPS_PREFIX, PathStyle::Raw)
                .unwrap_or_default();
        scratch.clear();
        crate::env::edgevar_into(graph, edge, Names::RESTAT, PathStyle::Raw, scratch);
        let restat = !scratch.is_empty();
        scratch.clear();
        crate::env::edgevar_into(graph, edge, Names::GENERATOR, PathStyle::Raw, scratch);
        let generator = !scratch.is_empty();
        let use_console = graph.is_console_pool(graph.edge(edge).pool);
        // Bound by the Make front end alone, so a manifest never interned it.
        let ignore_errors = graph
            .names()
            .lookup(bstr::BStr::new(IGNORE_ERRORS))
            .is_some_and(|binding| {
                crate::env::edgevar(graph, edge, binding, PathStyle::Raw)
                    .is_some_and(|value| !value.is_empty())
            });
        Ok(Self {
            command,
            description,
            rspfile,
            rspfile_content,
            deps_type,
            depfile_path,
            msvc_deps_prefix,
            restat,
            generator,
            use_console,
            ignore_errors,
        })
    }
}

/// The command text a front end supplied at the moment the edge was launched.
///
/// Every field replaces the one the graph held, because the graph held a
/// placeholder: an edge whose command is bound this late has no earlier text
/// to merge with.
pub(crate) struct LateCommand {
    /// The whole of what this edge runs, as one command line, for everything
    /// that has to name it: the progress line, the log, and `-n`.
    pub(crate) command: BString,
    /// What to print while it runs. Empty leaves the choice to the reporter,
    /// exactly as an unbound `description` on a rule does.
    pub(crate) description: BString,
    pub(crate) rspfile: Option<BString>,
    pub(crate) rspfile_content: BString,
    pub(crate) ignore_errors: bool,
    /// The processes this edge really is, in order.
    ///
    /// One entry is the ordinary case and is what an edge whose command was
    /// settled in the graph has by construction. Several is GNU Make's recipe:
    /// `start_job_command` runs one command line, and `reap_children` comes
    /// back for the next when that one is done, so a `cd` in one line is not
    /// seen by the next and a line whose shell syntax is left open dies where
    /// it stands instead of being completed by the line after it.
    ///
    /// The build stops at the first step that fails and is not
    /// [`LateStep::ignore_errors`]; the last one's status is the edge's.
    pub(crate) steps: Vec<LateStep>,
}

/// One process an edge is made of.
#[derive(Clone)]
pub(crate) struct LateStep {
    pub(crate) launch: Launch,
    /// A nonzero status from this step is not the edge's answer and does not
    /// stop the steps after it — GNU Make's `-` prefix, read per line.
    pub(crate) ignore_errors: bool,
    /// This step runs even where the build is standing in for its edge rather
    /// than running it.
    ///
    /// A `-t` run gives an edge's outputs a fresh date in place of making
    /// them, and a step that answers `true` here is one the front end says must
    /// happen anyway. GNU Make's `COMMANDS_RECURSE`, in words the engine can
    /// act on without knowing what a recursive Make is: `start_job_command`
    /// steps aside for such a line rather than touching in its place. A Ninja
    /// edge never sets it.
    pub(crate) runs_while_pretending: bool,
}

/// Whether an edge the build reached has a command to run at all.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Runs {
    Command,
    Nothing,
}

/// What a front end answered about an edge it binds this late.
pub(crate) enum LateBinding {
    /// This edge's command was settled when the graph was built.
    Settled,
    /// This edge's command was settled when the graph was built, and these are
    /// the processes it is made of.
    ///
    /// A recipe the front end had to read while compiling still runs the way
    /// GNU Make runs one — a process per command line — even though its text
    /// was fixed long before. The command the graph holds is the whole recipe
    /// assembled into one script and stays the edge's name; these are what
    /// actually start.
    Steps(Vec<LateStep>),
    /// Run this instead of whatever the graph is holding.
    Run(LateCommand),
    /// There is no command: the front end read the recipe and it came to
    /// nothing. The edge is complete the moment it is reached — no process, no
    /// output written, and nothing to report about a command that does not
    /// exist. GNU Make reaches the same state through `cs_not_started`.
    Nothing,
}

/// A front end that binds an edge's command when the build is about to run it.
///
/// The engine asks once per edge, after the edge's prerequisites have been
/// brought up to date and only for an edge it is going to run. What the front
/// end does to answer is its own: this boundary carries command text and a
/// diagnostic, and no front-end vocabulary crosses it in either direction.
// [spec:ronin:req:make.compiler-boundary]
pub(crate) trait LateCommands {
    /// The command for `edge`, whose single output is `output`, or
    /// [`LateBinding::Settled`] when this edge's command was settled when the
    /// graph was built.
    ///
    /// `trigger` names the output the edge is being run on behalf of, which is
    /// the same name for every edge but one writing several the graph reached
    /// separately. A front end whose recipes name the target they are making
    /// binds that name to this one; `output` stays what the edge writes first,
    /// because a response file belongs to the edge rather than to the run.
    ///
    /// # Errors
    ///
    /// A rendered diagnostic, when the front end could not produce the
    /// command. The build stops with it, as it would for a command that could
    /// not be started.
    fn command(
        &mut self,
        edge: EdgeId,
        output: &[u8],
        trigger: &[u8],
    ) -> Result<LateBinding, String>;

    /// Whatever binding a command had to say short of failing, rendered and
    /// ready to write, and nothing once it has been taken.
    ///
    /// A front end that reads a recipe as its edge launches can raise a
    /// warning there — the expansion is where GNU Make raises one too — and
    /// the engine owns the descriptor those go to while a build is running.
    /// Asked after every binding, so what a recipe said comes out beside the
    /// edge that said it rather than at the end of the build.
    fn raised(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

/// What the build does next with an edge whose process just finished.
pub(super) enum Advance {
    /// The recipe had another command line and it is running.
    Relaunched,
    /// The edge is over, on this result.
    Finished(Result<Option<ProcessOutput>, ProcessError>),
}

pub(super) struct PreparedEdge {
    pub(super) edge: EdgeId,
    pub(super) old_mtimes: Vec<i64>,
    pub(super) command: CommandSpec,
    /// What is left to run, in order, with scheduling-time values already
    /// substituted. Hashing and narration continue to use `command` itself.
    ///
    /// An edge whose command the graph settled has exactly one, which is what
    /// every Ninja edge is. A recipe a front end bound at launch has one per
    /// command line, because that is how many processes GNU Make gives it.
    pub(super) steps: std::collections::VecDeque<PreparedStep>,
    /// Whether a front end named the steps, as against their having been made
    /// out of the one command the graph held.
    pub(super) bound: bool,
    /// The step now running: what it is called, and whether its failure counts.
    pub(super) running_step: RunningStep,
    /// What the steps before the running one wrote, waiting for the edge to
    /// finish so all of it is reported at once — which is where a single
    /// command's output is reported too.
    pub(super) earlier_stdout: Vec<u8>,
    pub(super) earlier_stderr: Vec<u8>,
    pub(super) command_start_mtime: i64,
    /// Milliseconds from the start of the build to this command's launch.
    ///
    /// Ninja records this and the matching end offset in `.ninja_log`, and
    /// reads them back on the next build to weight its progress prediction by
    /// how long each edge actually took. Recording zeroes, as this did before,
    /// costs both tools that prediction: ours has nothing to weight with, and
    /// Ninja reading a log we wrote silently falls back to counting edges.
    pub(super) start_millis: i32,
    /// Whether the build stood in for any of this edge's steps rather than
    /// running it, which is the question `-t` asks before it touches: GNU Make
    /// touches in place of a line it skipped, so a recipe it skipped nothing of
    /// leaves the target alone.
    pub(super) pretended_a_step: bool,
    pub(super) _response_file: Option<ResponseFile>,
}

/// What a run is standing in for rather than doing.
///
/// Two switches put the build through the motions and they differ in one
/// thing: `-t` steps aside for a step the front end says runs anyway, and `-n`
/// does not, because a dry run over a graph that already holds every edge has
/// nothing to learn by starting one.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Pretending {
    /// Nothing is being stood in for: every step really runs.
    Nothing,
    /// Every step, whatever the front end said about it.
    EveryStep,
    /// Every step but the ones the front end marked as running anyway.
    AllButRunning,
}

impl Pretending {
    pub(super) const fn stands_in_for(self, runs_while_pretending: bool) -> bool {
        match self {
            Self::Nothing => false,
            Self::EveryStep => true,
            Self::AllButRunning => !runs_while_pretending,
        }
    }
}

/// One of an edge's launches, ready to start.
pub(super) struct PreparedStep {
    pub(super) launch: Launch,
    pub(super) ignore_errors: bool,
    /// See [`LateStep::runs_while_pretending`].
    pub(super) runs_while_pretending: bool,
}

/// What the build knows about the step an edge currently has running.
#[derive(Default)]
pub(super) struct RunningStep {
    pub(super) ignore_errors: bool,
}

pub(super) struct ResponseFile {
    pub(super) path: std::path::PathBuf,
    pub(super) remove_on_drop: bool,
}

impl Drop for ResponseFile {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl<'a> Builder<'a> {
    /// Bind this build's commands through `recipes` as each edge is launched.
    pub(crate) fn late_commands(&mut self, recipes: &'a mut dyn LateCommands) {
        self.late_commands = Some(recipes);
    }
}

impl Builder<'_> {
    pub(super) fn ensure_command(&mut self, edge: EdgeId) -> BuildResult<()> {
        self.command_cache
            .resize_with(self.graph.edge_count(), || None);
        if self.command_cache[edge.index()].is_none() {
            self.command_cache[edge.index()] = Some(CommandSpec::evaluate(
                self.graph,
                edge,
                &mut self.command_scratch,
            )?);
        }
        Ok(())
    }

    pub(super) fn invalidate_command(&mut self, edge: EdgeId) {
        if let Some(command) = self.command_cache.get_mut(edge.index()) {
            *command = None;
        }
        self.runtime.edge_mut(edge).invalidate_command_hash();
    }

    pub(super) fn take_command(&mut self, edge: EdgeId) -> BuildResult<CommandSpec> {
        self.ensure_command(edge)?;
        Ok(self.command_cache[edge.index()]
            .take()
            .expect("command cache was populated"))
    }

    /// Let the front end bind this edge's command now that it is about to run.
    ///
    /// Asked here rather than where commands are evaluated and hashed, because
    /// those run over every edge the targets reach: asking there would bind
    /// every command in the graph, which is the thing this exists not to do.
    pub(super) fn bind_late_command(
        &mut self,
        edge: EdgeId,
        command: &mut CommandSpec,
        steps: &mut Vec<LateStep>,
    ) -> BuildResult<Runs> {
        match self.late_binding(edge)? {
            LateBinding::Settled => Ok(Runs::Command),
            LateBinding::Steps(bound) => {
                *steps = bound;
                Ok(Runs::Command)
            }
            LateBinding::Nothing => Ok(Runs::Nothing),
            LateBinding::Run(bound) => {
                command.description = bound.description;
                command.command = bound.command;
                command.rspfile = bound.rspfile;
                command.rspfile_content = bound.rspfile_content;
                command.ignore_errors = bound.ignore_errors;
                *steps = bound.steps;
                Ok(Runs::Command)
            }
        }
    }

    /// Whether any edge of this build turned out to have a command.
    ///
    /// Asked once the build is over, because an edge whose recipe is read as
    /// it launches can only be found to have no command by launching it.
    pub(crate) fn ran_a_command(&self) -> bool {
        !self.executed_edges.is_empty()
    }

    /// Whether the plan holds anything that would really run.
    ///
    /// GNU Make's `-q` is not a question about the plan: `start_job_command`
    /// answers "something to do" only once a recipe line has been expanded and
    /// come out as text, so a recipe that expands to nothing answers zero, and
    /// the expansion happens — a `$(shell)` in such a recipe runs under `-q`
    /// exactly as it does under a build. The walk stops at the first edge that
    /// would run, which is where GNU Make stops asking too.
    ///
    /// # Errors
    ///
    /// Whatever the front end says about a recipe it could not expand.
    pub(crate) fn interrogate(&mut self) -> BuildResult<bool> {
        if self.plan.is_empty() {
            return Ok(true);
        }
        for edge in self.plan.reportable_work_edges(self.graph, &self.runtime) {
            if !matches!(self.late_binding(edge)?, LateBinding::Nothing) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Ask the front end for this edge's command, if there is a front end to
    /// ask.
    pub(super) fn late_binding(&mut self, edge: EdgeId) -> BuildResult<LateBinding> {
        let Some(recipes) = self.late_commands.as_deref_mut() else {
            return Ok(LateBinding::Settled);
        };
        let output = self
            .graph
            .edge(edge)
            .out
            .first()
            .map(|output| self.graph.node_path(*output).to_vec())
            .unwrap_or_default();
        let trigger = crate::graph::trigger_output(self.graph, &self.runtime, edge).map_or_else(
            || output.clone(),
            |output| self.graph.node_path(output).to_vec(),
        );
        let bound = recipes.command(edge, &output, &trigger);
        // Taken whether the binding worked or not: an expansion that ended in a
        // refusal may have warned on its way there.
        let raised = recipes.raised();
        if !raised.is_empty() {
            self.emit_diagnostic(&raised)?;
        }
        bound.map_err(|diagnostic| BuildError::LateCommand { diagnostic })
    }

    pub(super) fn refresh_command_hash(&mut self, edge: EdgeId) -> BuildResult<()> {
        self.ensure_command(edge)?;
        let command = self.command_cache[edge.index()]
            .as_ref()
            .expect("command cache was populated");
        let hash = edgehash(
            &mut self.runtime,
            edge,
            command.command.as_bstr(),
            (!command.rspfile_content.is_empty()).then_some(command.rspfile_content.as_bstr()),
        );
        let generator = command.generator;
        let command_dirty = !generator
            && self.graph.edge(edge).out.iter().any(|output| {
                let logged = self.runtime.node(*output).logged_command_hash();
                logged.is_missing() || logged != hash
            });
        self.runtime.edge_mut(edge).set_command_dirty(command_dirty);
        Ok(())
    }

    fn emit(&mut self, bytes: &[u8]) -> BuildResult<()> {
        if self.output_sink.is_none() {
            self.build_output.extend_from_slice(bytes);
        }
        if let Some(output) = self.output_sink.as_deref_mut() {
            output.write_all(bytes).map_err(|source| {
                BuildError::io(BuildOperation::WriteOutput, None, None, source)
            })?;
        }
        Ok(())
    }

    pub(super) fn record_child_output(&mut self, bytes: &[u8]) {
        if self.output_sink.is_none() {
            self.command_output.extend_from_slice(bytes);
        }
    }

    pub(super) fn emit_diagnostic(&mut self, bytes: &[u8]) -> BuildResult<()> {
        let Some(output) = self.diagnostic_sink.as_deref_mut() else {
            return self.emit(bytes);
        };
        output
            .write_all(bytes)
            .map_err(|source| BuildError::io(BuildOperation::WriteDiagnostic, None, None, source))
    }

    // [spec:ronin:req:runtime.process-supervisor-scalability]
    fn flush_sinks(&mut self) -> BuildResult<()> {
        if let Some(output) = self.diagnostic_sink.as_deref_mut() {
            output.flush().map_err(|source| {
                BuildError::io(BuildOperation::WriteDiagnostic, None, None, source)
            })?;
        }
        if let Some(output) = self.output_sink.as_deref_mut() {
            output.flush().map_err(|source| {
                BuildError::io(BuildOperation::WriteOutput, None, None, source)
            })?;
        }
        Ok(())
    }

    fn emit_explanations(&mut self, edge: EdgeId) -> BuildResult<()> {
        if self.explanations.is_none() {
            return Ok(());
        }
        self.explanations_emitted
            .resize(self.graph.edge_count(), false);
        if std::mem::replace(&mut self.explanations_emitted[edge.index()], true) {
            return Ok(());
        }
        let mut messages = Vec::new();
        let explanations = self
            .explanations
            .as_ref()
            .expect("explanations were checked above");
        for output in &self.graph.edge(edge).out {
            explanations.lookup_and_append(output.index(), &mut messages);
        }
        for message in messages {
            self.emit_diagnostic(format!("ronin explain: {message}\n").as_bytes())?;
        }
        Ok(())
    }

    /// Render one thing the reporter knows how to say, and write it.
    ///
    /// The buffer is moved out of `self` for the duration so that rendering
    /// can borrow the graph, the progress counters and the options while the
    /// bytes accumulate, then put back for the next command to reuse.
    fn emit_rendered(&mut self, rendering: Rendering<'_>) -> BuildResult<()> {
        let mut line = std::mem::take(&mut self.status_scratch);
        line.clear();
        self.reporter.clear(&mut line);
        match rendering {
            Rendering::Status(command) => {
                self.reporter
                    .status(&mut line, &self.progress, &self.options, command);
            }
            Rendering::Failure {
                edge,
                exit_code,
                command,
            } => self
                .reporter
                .failure(&mut line, self.graph, edge, exit_code, command),
        }
        let result = self.emit(&line);
        self.status_scratch = line;
        result
    }

    /// Let the reporter close out the build and give back the bar's line.
    ///
    /// Runs whatever the outcome, so an interrupted or failed build does not
    /// leave a painted bar for the shell prompt to land on.
    pub(super) fn emit_summary(&mut self, succeeded: bool) -> BuildResult<()> {
        let mut line = std::mem::take(&mut self.status_scratch);
        line.clear();
        self.reporter
            .finish(&mut line, &self.progress, succeeded && !self.options.quiet);
        let result = if line.is_empty() {
            Ok(())
        } else {
            self.emit(&line).and_then(|()| self.flush_sinks())
        };
        self.status_scratch = line;
        result
    }

    /// Put the bar back after a command's output has been written.
    ///
    /// Skipped entirely when the repaint budget says no, so this is a write
    /// at most thirty times a second rather than once per command.
    ///
    /// Reports whether anything was written, because the flush that follows a
    /// batch is counted: a rendering that paints nothing must not turn into an
    /// extra flush per command.
    fn repaint(&mut self) -> BuildResult<bool> {
        if self.options.quiet {
            return Ok(false);
        }
        let mut line = std::mem::take(&mut self.status_scratch);
        line.clear();
        self.reporter.paint(&mut line, &self.progress);
        let painted = !line.is_empty();
        let result = if painted { self.emit(&line) } else { Ok(()) };
        self.status_scratch = line;
        result.map(|()| painted)
    }

    fn emit_status(&mut self, edge: EdgeId, command: &CommandSpec) -> BuildResult<()> {
        self.emit_explanations(edge)?;
        if self.options.quiet {
            return Ok(());
        }
        self.emit_rendered(Rendering::Status(command))
    }

    pub(super) fn command_started(
        &mut self,
        edge: EdgeId,
        command: &CommandSpec,
    ) -> BuildResult<()> {
        self.progress.started += 1;
        self.reporter.started(&self.options, command);
        if command.use_console {
            self.emit_status(edge, command)?;
            self.flush_sinks()?;
            return Ok(());
        }
        if self.repaint()? {
            self.flush_sinks()?;
        }
        Ok(())
    }

    pub(super) fn command_finished(
        &mut self,
        edge: EdgeId,
        command: &CommandSpec,
        failure_code: Option<i32>,
        output: &[u8],
    ) -> BuildResult<()> {
        self.progress.finished += 1;
        // The prediction has to be refreshed before the line that reports it.
        status::recalculate_prediction(&mut self.progress);
        self.reporter.ended();
        let wrote_batch = !command.use_console || failure_code.is_some() || !output.is_empty();
        if !command.use_console {
            self.emit_status(edge, command)?;
        }
        if let Some(exit_code) = failure_code {
            self.emit_failure(edge, exit_code, command)?;
        }
        if !output.is_empty() {
            self.emit_below_bar(output)?;
        }
        let repainted = self.repaint()?;
        if wrote_batch || repainted {
            self.flush_sinks()?;
        }
        Ok(())
    }

    fn emit_failure(
        &mut self,
        edge: EdgeId,
        exit_code: i32,
        command: &CommandSpec,
    ) -> BuildResult<()> {
        self.emit_rendered(Rendering::Failure {
            edge,
            exit_code,
            command,
        })
    }

    /// Write bytes the reporter did not render, displacing the bar first.
    ///
    /// A command's own output goes out verbatim, but it still has to take the
    /// bar's line before it does, or the first line of a compiler diagnostic
    /// lands on top of the bar.
    fn emit_below_bar(&mut self, bytes: &[u8]) -> BuildResult<()> {
        let mut line = std::mem::take(&mut self.status_scratch);
        line.clear();
        self.reporter.clear(&mut line);
        let result = if line.is_empty() {
            Ok(())
        } else {
            self.emit(&line)
        };
        self.status_scratch = line;
        result.and_then(|()| self.emit(bytes))
    }
}

impl Builder<'_> {
    /// The processes this edge is, with the scheduling-time values the front
    /// end left for the build to fill in already substituted.
    ///
    /// An edge whose command the graph settled is one process, which is every
    /// Ninja edge and every recipe a front end read before the build began; a
    /// recipe bound at launch is as many as it has command lines.
    pub(super) fn prepared_steps(
        &self,
        edge: EdgeId,
        command: &CommandSpec,
        bound: Vec<crate::build::LateStep>,
    ) -> std::collections::VecDeque<PreparedStep> {
        use crate::subprocess::Launch;
        if bound.is_empty() {
            return std::collections::VecDeque::from(vec![PreparedStep {
                launch: Launch::Shell(self.deferred_launch_command(edge, &command.command)),
                ignore_errors: command.ignore_errors,
                // A command the graph settled is one process and nothing said
                // it runs anyway, which is every Ninja edge.
                runs_while_pretending: false,
            }]);
        }
        bound
            .into_iter()
            .map(|step| PreparedStep {
                launch: match step.launch {
                    Launch::Shell(command) => {
                        Launch::Shell(self.deferred_launch_command(edge, &command))
                    }
                    // Nothing to substitute into: the front end that reads a
                    // recipe at launch never leaves one of these unfinished.
                    direct @ Launch::Direct(_) => direct,
                },
                ignore_errors: step.ignore_errors,
                runs_while_pretending: step.runs_while_pretending,
            })
            .collect()
    }

    /// Start the next process this edge is made of, or say that it is over.
    ///
    /// GNU Make's `reap_children` asks the same question of a finished recipe
    /// line: the recipe carries on into its next line when this one succeeded,
    /// or failed and was written with a `-`, and stops where it stands
    /// otherwise. The status the edge is finally judged on is the last line's,
    /// and a failure the makefile said to ignore leaves the target made.
    ///
    /// An edge whose command the graph settled has one step, so it always ends
    /// here on the first completion and nothing about it changes.
    pub(super) fn continue_recipe<External: Send + 'static>(
        &self,
        processes: &mut crate::subprocess::ProcessSupervisor<External>,
        prepared: &mut PreparedEdge,
        result: Result<Option<ProcessOutput>, ProcessError>,
    ) -> Advance {
        match self.next_step(prepared, result) {
            Advance::Finished(result) => return Advance::Finished(result),
            Advance::Relaunched => {}
        }
        let (launch, pretended) = Self::take_step(prepared, self.pretending());
        let use_console = prepared.command.use_console;
        match processes.spawn(prepared.edge, launch, use_console, pretended) {
            Ok(()) => Advance::Relaunched,
            Err(error) => Advance::Finished(Err(error)),
        }
    }

    /// Whether the recipe has another command line to run, given how the last
    /// one came out and what it wrote.
    fn next_step(
        &self,
        prepared: &mut PreparedEdge,
        result: Result<Option<ProcessOutput>, ProcessError>,
    ) -> Advance {
        let Ok(finished) = result else {
            return Advance::Finished(result);
        };
        let Some(output) = finished else {
            // A dry run: nothing ran, so every step of it "succeeded" and the
            // recipe is walked to its end exactly as a real one would be.
            if prepared.steps.is_empty() {
                return Advance::Finished(Ok(None));
            }
            return Advance::Relaunched;
        };
        let carry_on = output.status.success() && !prepared.steps.is_empty();
        let ignored_and_more = !output.status.success()
            && prepared.running_step.ignore_errors
            && !prepared.steps.is_empty()
            && !self.command_interrupted(output.status);
        if carry_on || ignored_and_more {
            // The failure of an ignored step is not the edge's answer and is
            // not reported as one: the `-` prefix is what the makefile said
            // about it, and what it wrote still belongs to the edge.
            prepared.earlier_stdout.extend_from_slice(&output.stdout);
            prepared.earlier_stderr.extend_from_slice(&output.stderr);
            return Advance::Relaunched;
        }
        if !output.status.success() && prepared.bound {
            // Read per line rather than per recipe: only the step the edge
            // stopped at decides whether its status is one to ignore, where
            // the assembled script could only be believed when every line of
            // it said so.
            prepared.command.ignore_errors = prepared.running_step.ignore_errors;
        }
        Advance::Finished(Ok(Some(output)))
    }

    /// Take the next process this edge is made of, and remember what its
    /// status will mean.
    /// The next step's launch, and whether the build stands in for it.
    ///
    /// An edge the run is pretending about still pretends, step by step,
    /// except where the front end marked the step as one that runs anyway —
    /// and a step the build stood in for is what makes the edge's outputs worth
    /// touching, so the edge remembers that it did.
    pub(super) fn take_step(
        prepared: &mut PreparedEdge,
        pretending: Pretending,
    ) -> (crate::subprocess::Launch, bool) {
        let step = prepared
            .steps
            .pop_front()
            .expect("an edge is prepared with at least one step");
        prepared.running_step = RunningStep {
            ignore_errors: step.ignore_errors,
        };
        let pretended = pretending.stands_in_for(step.runs_while_pretending);
        prepared.pretended_a_step |= pretended;
        (step.launch, pretended)
    }
}
