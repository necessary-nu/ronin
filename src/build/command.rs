use super::{legacy, Builder};
use crate::graph::{edgehash, invalidate_edge_hash, EdgeId, Graph};
use crate::util::{BString, ByteSlice};
use std::fs;

pub(super) struct CommandSpec {
    pub(super) command: BString,
    pub(super) description: BString,
    pub(super) rspfile: Option<BString>,
    pub(super) rspfile_content: BString,
    pub(super) deps_type: String,
    pub(super) depfile_path: Option<BString>,
    pub(super) msvc_deps_prefix: String,
    pub(super) restat: bool,
    pub(super) generator: bool,
    pub(super) use_console: bool,
}

impl CommandSpec {
    fn evaluate(graph: &Graph, edge: EdgeId) -> Result<Self, String> {
        let command = crate::env::edgevar(graph, edge, "command", true).unwrap_or_default();
        let description = crate::env::edgevar(graph, edge, "description", true).unwrap_or_default();
        let rspfile =
            crate::env::edgevar(graph, edge, "rspfile", false).filter(|path| !path.is_empty());
        let rspfile_content =
            crate::env::edgevar(graph, edge, "rspfile_content", true).unwrap_or_default();
        let deps_type = crate::env::edgevar(graph, edge, "deps", false)
            .map(Vec::from)
            .map(String::from_utf8)
            .transpose()
            .map_err(|_| "deps binding is not valid UTF-8".to_owned())?
            .unwrap_or_default();
        let depfile_path =
            crate::env::edgevar(graph, edge, "depfile", false).filter(|path| !path.is_empty());
        let msvc_deps_prefix = crate::env::edgevar(graph, edge, "msvc_deps_prefix", false)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
            .unwrap_or_default();
        let restat = crate::env::edgevar(graph, edge, "restat", false)
            .is_some_and(|value| !value.is_empty());
        let generator = crate::env::edgevar(graph, edge, "generator", false)
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

    fn hash_into(&self, graph: &mut Graph, edge: EdgeId) {
        edgehash(
            graph,
            edge,
            self.command.as_bstr(),
            (!self.rspfile_content.is_empty()).then_some(self.rspfile_content.as_bstr()),
        );
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

impl<'a> Builder<'a> {
    pub(super) fn ensure_command(&mut self, edge: EdgeId) -> Result<(), String> {
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
        invalidate_edge_hash(self.graph, edge);
    }

    pub(super) fn take_command(&mut self, edge: EdgeId) -> Result<CommandSpec, String> {
        self.ensure_command(edge)?;
        Ok(self.command_cache[edge.index()]
            .take()
            .expect("command cache was populated"))
    }

    pub(super) fn refresh_command_hash(&mut self, edge: EdgeId) -> Result<(), String> {
        self.ensure_command(edge)?;
        self.command_cache[edge.index()]
            .as_ref()
            .expect("command cache was populated")
            .hash_into(self.graph, edge);
        let hash = self.graph.edge(edge).hash;
        let generator = self.command_cache[edge.index()]
            .as_ref()
            .expect("command cache was populated")
            .generator;
        self.graph.edge_mut(edge).command_dirty = !generator
            && self.graph.edge(edge).out.iter().any(|output| {
                let output = self.graph.node(*output);
                output.hash == 0 || output.hash != hash
            });
        Ok(())
    }

    fn emit(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.output_sink.is_none() {
            self.build_output.extend_from_slice(bytes);
        }
        if let Some(output) = self.output_sink.as_deref_mut() {
            output.write_all(bytes).map_err(|error| error.to_string())?;
            output.flush().map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(super) fn record_child_output(&mut self, bytes: &[u8]) {
        if self.output_sink.is_none() {
            self.command_output.extend_from_slice(bytes);
        }
    }

    fn emit_diagnostic(&mut self, bytes: &[u8]) -> Result<(), String> {
        if let Some(output) = self.diagnostic_sink.as_deref_mut() {
            output.write_all(bytes).map_err(|error| error.to_string())?;
            output.flush().map_err(|error| error.to_string())
        } else {
            self.emit(bytes)
        }
    }

    fn emit_explanations(&mut self, edge: EdgeId) -> Result<(), String> {
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

    fn emit_status(&mut self, edge: EdgeId, command: &CommandSpec) -> Result<(), String> {
        self.emit_explanations(edge)?;
        let description = if self.options.verbose || command.description.is_empty() {
            command.command.as_bytes()
        } else {
            command.description.as_bytes()
        };
        let mut line =
            legacy::format_progress_status(&self.progress, &self.options.statusfmt).into_bytes();
        line.extend_from_slice(description);
        line.push(b'\n');
        self.emit(&line)
    }

    pub(super) fn command_started(
        &mut self,
        edge: EdgeId,
        command: &CommandSpec,
    ) -> Result<(), String> {
        self.progress.started += 1;
        if command.use_console {
            self.emit_status(edge, command)?;
        }
        Ok(())
    }

    pub(super) fn command_finished(
        &mut self,
        edge: EdgeId,
        command: &CommandSpec,
        failure_code: Option<i32>,
        output: &[u8],
    ) -> Result<(), String> {
        self.progress.finished += 1;
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
        self.emit(output)
    }

    pub(super) fn exit_code(status: &std::process::ExitStatus) -> i32 {
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
