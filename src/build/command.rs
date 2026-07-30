use super::{status, Builder};
use crate::error::{BuildError, BuildOperation};
use crate::graph::{edgehash, EdgeId, Graph, PathStyle};
use crate::util::{BString, ByteSlice};
use std::fs;

type BuildResult<T> = Result<T, BuildError>;

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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum DepsType {
    None,
    Gcc,
    Msvc,
    Unsupported(String),
}

impl DepsType {
    fn from_name(name: String) -> Self {
        match name.as_str() {
            "" => Self::None,
            "gcc" => Self::Gcc,
            "msvc" => Self::Msvc,
            _ => Self::Unsupported(name),
        }
    }
}

impl CommandSpec {
    fn evaluate(graph: &Graph, edge: EdgeId) -> BuildResult<Self> {
        let command = crate::env::edgevar(graph, edge, "command", PathStyle::ShellEscaped)
            .unwrap_or_default();
        let description = crate::env::edgevar(graph, edge, "description", PathStyle::ShellEscaped)
            .unwrap_or_default();
        let rspfile = crate::env::edgevar(graph, edge, "rspfile", PathStyle::Raw)
            .filter(|path| !path.is_empty());
        let rspfile_content =
            crate::env::edgevar(graph, edge, "rspfile_content", PathStyle::ShellEscaped)
                .unwrap_or_default();
        let deps_type = crate::env::edgevar(graph, edge, "deps", PathStyle::Raw)
            .map(Vec::from)
            .map(String::from_utf8)
            .transpose()
            .map_err(|_| BuildError::InvalidDepsEncoding { edge })?
            .unwrap_or_default();
        let deps_type = DepsType::from_name(deps_type);
        let depfile_path = crate::env::edgevar(graph, edge, "depfile", PathStyle::Raw)
            .filter(|path| !path.is_empty());
        let msvc_deps_prefix = crate::env::edgevar(graph, edge, "msvc_deps_prefix", PathStyle::Raw)
            .unwrap_or_default();
        let restat = crate::env::edgevar(graph, edge, "restat", PathStyle::Raw)
            .is_some_and(|value| !value.is_empty());
        let generator = crate::env::edgevar(graph, edge, "generator", PathStyle::Raw)
            .is_some_and(|value| !value.is_empty());
        let use_console = graph
            .edge(edge)
            .pool
            .is_some_and(|pool| graph.pool(pool).name == "console");
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
        })
    }
}

pub(super) struct PreparedEdge {
    pub(super) edge: EdgeId,
    pub(super) old_mtimes: Vec<i64>,
    pub(super) command: CommandSpec,
    pub(super) command_start_mtime: i64,
    pub(super) _response_file: Option<ResponseFile>,
}

pub(super) struct ResponseFile {
    pub(super) path: BString,
    pub(super) remove_on_drop: bool,
}

impl Drop for ResponseFile {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_file(self.path.to_path().expect("byte paths are valid on Unix"));
        }
    }
}

impl Builder<'_> {
    pub(super) fn ensure_command(&mut self, edge: EdgeId) -> BuildResult<()> {
        self.command_cache
            .resize_with(self.graph.edge_count(), || None);
        if self.command_cache[edge.index()].is_none() {
            self.command_cache[edge.index()] = Some(CommandSpec::evaluate(self.graph, edge)?);
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

    // [spec:samurai:req:runtime.process-supervisor-scalability]
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

    fn emit_status(&mut self, edge: EdgeId, command: &CommandSpec) -> BuildResult<()> {
        self.emit_explanations(edge)?;
        if self.options.quiet {
            return Ok(());
        }
        let description = if self.options.verbose || command.description.is_empty() {
            command.command.as_bytes()
        } else {
            command.description.as_bytes()
        };
        let mut line =
            status::format_progress_status(&self.progress, &self.options.statusfmt).into_bytes();
        if self.options.status_from_cli {
            let mut rendered = Vec::with_capacity(line.len() + description.len());
            for (index, part) in line.split(|byte| *byte == 0x1f).enumerate() {
                if index != 0 {
                    rendered.extend_from_slice(description);
                }
                rendered.extend_from_slice(part);
            }
            line = rendered;
        } else {
            line.extend_from_slice(description);
        }
        line.push(b'\n');
        self.emit(&line)
    }

    pub(super) fn command_started(
        &mut self,
        edge: EdgeId,
        command: &CommandSpec,
    ) -> BuildResult<()> {
        self.progress.started += 1;
        if command.use_console {
            self.emit_status(edge, command)?;
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
        let wrote_batch = !command.use_console || failure_code.is_some() || !output.is_empty();
        if !command.use_console {
            self.emit_status(edge, command)?;
        }
        if let Some(exit_code) = failure_code {
            let mut failure = format!("FAILED: [code={exit_code}] ").into_bytes();
            for output in &self.graph.edge(edge).out {
                failure.extend_from_slice(self.graph.node(*output).path.as_bytes());
                failure.push(b' ');
            }
            failure.push(b'\n');
            failure.extend_from_slice(command.command.as_bytes());
            failure.push(b'\n');
            self.emit(&failure)?;
        }
        if !output.is_empty() {
            self.emit(output)?;
        }
        if wrote_batch {
            self.flush_sinks()?;
        }
        Ok(())
    }

    pub(super) fn exit_code(status: std::process::ExitStatus) -> i32 {
        if let Some(code) = status.code() {
            return code;
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            status.signal().map_or(1, |signal| 128 + signal)
        }
        #[cfg(not(unix))]
        {
            1
        }
    }
}
