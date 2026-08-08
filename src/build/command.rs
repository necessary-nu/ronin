use super::reporter::{make_failure, Rendering};
use super::{status, Builder};
use crate::error::{BuildError, BuildOperation};
use crate::graph::{edgehash, EdgeId, Graph, PathStyle};
use crate::names::Names;
use crate::runtime::FileTime;
use crate::util::{BString, ByteSlice};
use std::fs;

type BuildResult<T> = Result<T, BuildError>;

/// The binding a Makefile's `+`-prefixed recipe lines are assembled into.
pub(crate) const DRY_RUN_COMMAND: &[u8] = b"dryrun_command";

/// Where the recipe an edge runs was written, as `file:line`.
///
/// Only a Makefile has such a place, so only the Make front end binds it and a
/// manifest's edges answer nothing. Make names it in the diagnostics that are
/// about the rule rather than about the file it builds.
pub(crate) const RECIPE_LOCATION: &[u8] = b"recipe_location";

pub(super) struct CommandSpec {
    pub(super) command: BString,
    /// What to run when the run is only pretending. `None` for every Ninja
    /// edge and for a recipe with no `+` line.
    pub(super) dry_run_command: Option<BString>,
    pub(super) description: BString,
    pub(super) rspfile: Option<BString>,
    pub(super) rspfile_content: BString,
    pub(super) deps_type: DepsType,
    pub(super) depfile_path: Option<BString>,
    pub(super) msvc_deps_prefix: BString,
    pub(super) restat: bool,
    pub(super) generator: bool,
    pub(super) use_console: bool,
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
    fn evaluate(graph: &Graph, edge: EdgeId, scratch: &mut Vec<u8>) -> BuildResult<Self> {
        let command = crate::env::edgevar(graph, edge, Names::COMMAND, PathStyle::ShellEscaped)
            .unwrap_or_default();
        // Make's `+` prefix: the part of a recipe that runs even under -n. Only
        // a Makefile ever binds this, so a Ninja manifest never interned the
        // name and the lookup answers None.
        let dry_run_command = graph
            .names()
            .lookup(bstr::BStr::new(DRY_RUN_COMMAND))
            .and_then(|binding| crate::env::edgevar(graph, edge, binding, PathStyle::ShellEscaped))
            .filter(|command| !command.is_empty());
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
        Ok(Self {
            command,
            dry_run_command,
            description,
            rspfile,
            rspfile_content,
            deps_type,
            depfile_path,
            msvc_deps_prefix,
            restat,
            generator,
            use_console,
        })
    }
}

pub(super) struct PreparedEdge {
    pub(super) edge: EdgeId,
    pub(super) old_mtimes: Vec<i64>,
    pub(super) command: CommandSpec,
    pub(super) command_start_mtime: i64,
    /// Milliseconds from the start of the build to this command's launch.
    ///
    /// Ninja records this and the matching end offset in `.ninja_log`, and
    /// reads them back on the next build to weight its progress prediction by
    /// how long each edge actually took. Recording zeroes, as this did before,
    /// costs both tools that prediction: ours has nothing to weight with, and
    /// Ninja reading a log we wrote silently falls back to counting edges.
    pub(super) start_millis: i32,
    pub(super) _response_file: Option<ResponseFile>,
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

    /// Whether `-t` gives this edge's outputs a timestamp rather than leaving
    /// them as the declined recipe left them.
    ///
    /// Not a target with no file behind it, and not a recipe that is nothing
    /// but `+` lines: that one ran rather than being declined.
    pub(super) fn touching(&self, edge: EdgeId, command: &CommandSpec) -> bool {
        self.options.touch
            && !self.graph.edge(edge).untouchable
            && command.dry_run_command.as_ref() != Some(&command.command)
    }

    /// Mark an edge's outputs up to date instead of remaking them, which is
    /// GNU Make's `-t`, and say so in Make's own words.
    ///
    /// Two switches qualify it and each takes back a different half. `-n`
    /// outranks the touch — the line is still said and the file is left alone
    /// — and `-s` withdraws the line while the file is still touched.
    pub(super) fn touch_outputs(&mut self, edge: EdgeId) -> BuildResult<()> {
        for output in self.graph.edge(edge).out.clone() {
            let path = self.graph.node_path(output).to_owned();
            if !self.options.quiet {
                let mut line = BString::from("touch ");
                line.extend_from_slice(path.as_bytes());
                line.push(b'\n');
                self.emit(&line)?;
            }
            if !self.options.dryrun {
                self.disk
                    .touch(path.to_path().expect("byte paths are valid on Unix"))
                    .map_err(|source| {
                        BuildError::io(BuildOperation::TouchOutput, Some(path), Some(edge), source)
                    })?;
            }
        }
        Ok(())
    }

    pub(super) fn record_child_output(&mut self, bytes: &[u8]) {
        if self.output_sink.is_none() {
            self.command_output.extend_from_slice(bytes);
        }
    }

    fn emit_diagnostic(&mut self, bytes: &[u8]) -> BuildResult<()> {
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
            } => match (
                self.options.recipe_failure.as_deref(),
                self.graph.edge(edge).out.first().copied(),
            ) {
                (Some(name), Some(target)) => make_failure(
                    &mut line,
                    name,
                    &self.recipe_location(edge),
                    self.graph.node_path(target).as_bytes(),
                    exit_code,
                ),
                _ => self
                    .reporter
                    .failure(&mut line, self.graph, edge, exit_code, command),
            },
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

    /// GNU Make's `--trace`: name the rule about to run and say what for.
    ///
    /// Make's own line, word for word, because everything that reads `--trace`
    /// reads it as Make's — the place the recipe was written, the target, and
    /// one of Make's four reasons in Make's order: a target with no file behind
    /// it, a target that is not there, the prerequisites newer than it, and the
    /// prerequisites still missing.
    fn trace_update(&mut self, edge: EdgeId) -> BuildResult<()> {
        if !self.options.trace {
            return Ok(());
        }
        let Some(output) = self.graph.edge(edge).out.first().copied() else {
            return Ok(());
        };
        let line = format!(
            "{}: update target '{}' due to: {}\n",
            self.recipe_location(edge).to_str_lossy(),
            self.graph.node_path(output).to_str_lossy(),
            self.update_reason(edge, output),
        );
        self.emit(line.as_bytes())
    }

    /// Where the recipe this edge runs was written, or Make's word for a rule
    /// nobody wrote down.
    fn recipe_location(&self, edge: EdgeId) -> BString {
        self.graph
            .names()
            .lookup(bstr::BStr::new(RECIPE_LOCATION))
            .and_then(|binding| crate::env::edgevar(self.graph, edge, binding, PathStyle::Raw))
            .filter(|written| !written.is_empty())
            .unwrap_or_else(|| BString::from(&b"<builtin>"[..]))
    }

    /// Which of Make's reasons this edge is running for.
    fn update_reason(&self, edge: EdgeId, output: crate::graph::NodeId) -> String {
        if self.graph.edge(edge).always_dirty {
            return "target is .PHONY".to_owned();
        }
        let target = self.runtime.node(output).mtime();
        if target.is_missing() {
            return "target does not exist".to_owned();
        }
        let named = |inputs: &[crate::graph::NodeId], keep: &dyn Fn(FileTime) -> bool| {
            inputs
                .iter()
                .filter(|input| keep(self.runtime.node(**input).mtime()))
                .map(|input| self.graph.node_path(*input).to_str_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        // `$?`, which is what Make itself prints here.
        let newer = named(self.graph.edge(edge).non_order_only_inputs(), &|mtime| {
            mtime > target
        });
        if !newer.is_empty() {
            return newer;
        }
        let absent = named(&self.graph.edge(edge).input, &FileTime::is_missing);
        if absent.is_empty() {
            "unknown reasons".to_owned()
        } else {
            absent
        }
    }

    pub(super) fn command_started(
        &mut self,
        edge: EdgeId,
        command: &CommandSpec,
    ) -> BuildResult<()> {
        self.trace_update(edge)?;
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
        // GNU Make releases a block only when the command left something in it,
        // so a silent recipe that succeeded is bracketed by nothing.
        let held = failure_code.is_some() || !output.is_empty();
        if held {
            self.emit_boundary(true)?;
        }
        // Ninja opens the block with its banner and shows the output under it;
        // Make's account of the failure closes the block, after the output the
        // recipe left.
        let closing = self.options.recipe_failure.is_some();
        if let Some(exit_code) = failure_code.filter(|_| !closing) {
            self.emit_failure(edge, exit_code, command)?;
        }
        if !output.is_empty() {
            self.emit_below_bar(output)?;
        }
        if let Some(exit_code) = failure_code.filter(|_| closing) {
            self.emit_failure(edge, exit_code, command)?;
        }
        if held {
            self.emit_boundary(false)?;
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

    /// Open or close the directory bracket around one held block.
    ///
    /// Nothing at all unless `-O` asked for it, which is why the pair is an
    /// option rather than a flag: the lines are the front end's own wording.
    fn emit_boundary(&mut self, opening: bool) -> BuildResult<()> {
        if self.options.output_group.is_none() {
            return Ok(());
        }
        let mut line = std::mem::take(&mut self.status_scratch);
        line.clear();
        self.reporter.clear(&mut line);
        let group = self
            .options
            .output_group
            .as_ref()
            .expect("the group was checked above");
        line.extend_from_slice(if opening {
            &group.entering
        } else {
            &group.leaving
        });
        let result = self.emit(&line);
        self.status_scratch = line;
        result
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
