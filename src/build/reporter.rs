//! The seam between what the supervisor knows and what the terminal shows.
//!
//! Every byte a build writes about its own progress is rendered here, so that
//! how a build looks is one decision made in one place rather than a shape
//! spread across the supervisor. Rendering writes into a caller-owned buffer
//! and never touches a sink: the supervisor still owns the write, the flush,
//! and the choice between streaming and buffering.

use super::command::CommandSpec;
use super::{status, BuildOptions, BuildState};
use crate::graph::{EdgeId, Graph};
use crate::util::ByteSlice;
use std::io::Write as _;

/// What the supervisor has asked to have rendered.
///
/// Both cases end in the same buffer through the same emit path, so they are
/// one value rather than two nearly identical call sites.
#[derive(Clone, Copy)]
pub(super) enum Rendering<'a> {
    /// A command has started (console pool) or finished (everything else).
    Status(&'a CommandSpec),
    /// A command exited non-zero.
    Failure {
        edge: EdgeId,
        exit_code: i32,
        command: &'a CommandSpec,
    },
}

/// How a build narrates itself.
///
/// Implementations append to `out` and return nothing: a renderer that cannot
/// fail keeps error handling where the writing happens.
pub(crate) trait Reporter {
    /// Announce that a command has started or finished.
    fn status(
        &mut self,
        out: &mut Vec<u8>,
        progress: &BuildState,
        options: &BuildOptions,
        command: &CommandSpec,
    );

    /// Announce that a command failed, naming the outputs it did not produce.
    fn failure(
        &mut self,
        out: &mut Vec<u8>,
        graph: &Graph,
        edge: EdgeId,
        exit_code: i32,
        command: &CommandSpec,
    );
}

/// Ninja's own rendering, which is what Ronin emits unless asked otherwise.
///
/// This is the compatibility surface named by
/// [`compat.command-runtime`](../../../docs/spec/ronin/compatibility.md): the
/// status template, the description-or-command choice, and the `FAILED:` block
/// are Ninja-owned output that generators and editors parse, so the bytes here
/// are fixed by the oracle rather than by taste.
pub(crate) struct NinjaReporter;

impl Reporter for NinjaReporter {
    // [spec:samurai:def:build.printstatus-fn]
    // [spec:samurai:sem:build.printstatus-fn]
    fn status(
        &mut self,
        out: &mut Vec<u8>,
        progress: &BuildState,
        options: &BuildOptions,
        command: &CommandSpec,
    ) {
        let description = describe(options, command);
        let line = status::format_progress_status(progress, &options.statusfmt);
        if options.status_from_cli {
            // A CLI-supplied format may place the description anywhere, so the
            // expansion left a marker byte at each position it should occupy.
            for (index, part) in line.as_bytes().split(|byte| *byte == 0x1f).enumerate() {
                if index != 0 {
                    out.extend_from_slice(description);
                }
                out.extend_from_slice(part);
            }
        } else {
            out.extend_from_slice(line.as_bytes());
            out.extend_from_slice(description);
        }
        out.push(b'\n');
    }

    fn failure(
        &mut self,
        out: &mut Vec<u8>,
        graph: &Graph,
        edge: EdgeId,
        exit_code: i32,
        command: &CommandSpec,
    ) {
        let _ = write!(out, "FAILED: [code={exit_code}] ");
        for output in &graph.edge(edge).out {
            out.extend_from_slice(graph.node_path(*output).as_bytes());
            out.push(b' ');
        }
        out.push(b'\n');
        out.extend_from_slice(command.command.as_bytes());
        out.push(b'\n');
    }
}

/// The text that stands for a command: its description, or the command itself
/// when there is no description or the operator asked to see command lines.
fn describe<'a>(options: &BuildOptions, command: &'a CommandSpec) -> &'a [u8] {
    if options.verbose || command.description.is_empty() {
        command.command.as_bytes()
    } else {
        command.description.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(command: &str, description: &str) -> CommandSpec {
        CommandSpec {
            command: command.into(),
            description: description.into(),
            rspfile: None,
            rspfile_content: crate::util::BString::default(),
            deps_type: super::super::command::DepsType::None,
            depfile_path: None,
            msvc_deps_prefix: crate::util::BString::default(),
            restat: false,
            generator: false,
            use_console: false,
        }
    }

    fn render(options: &BuildOptions, command: &CommandSpec) -> String {
        let mut progress = BuildState::new(options.clone());
        progress.finished = 3;
        progress.total = 7;
        let mut out = Vec::new();
        NinjaReporter.status(&mut out, &progress, options, command);
        String::from_utf8(out).expect("the fixture renders as text")
    }

    // [spec:samurai:req:compat.command-runtime/test]
    #[test]
    fn the_default_reporter_emits_ninjas_counter_and_description() {
        let options = BuildOptions::default();
        assert_eq!(
            render(&options, &spec("cc -c a.c", "CC a.o")),
            "[3/7] CC a.o\n"
        );
    }

    // [spec:samurai:req:compat.command-runtime/test]
    #[test]
    fn a_command_without_a_description_stands_for_itself() {
        let options = BuildOptions::default();
        assert_eq!(
            render(&options, &spec("cc -c a.c", "")),
            "[3/7] cc -c a.c\n"
        );
    }

    // [spec:samurai:req:compat.command-runtime/test]
    #[test]
    fn verbose_prefers_the_command_over_the_description() {
        let options = BuildOptions {
            verbose: true,
            ..BuildOptions::default()
        };
        assert_eq!(
            render(&options, &spec("cc -c a.c", "CC a.o")),
            "[3/7] cc -c a.c\n"
        );
    }

    // [spec:samurai:req:compat.command-runtime/test]
    #[test]
    fn a_cli_format_places_the_description_at_every_marker() {
        let options = BuildOptions {
            statusfmt: "\u{1f} (\u{1f}) ".into(),
            status_from_cli: true,
            ..BuildOptions::default()
        };
        assert_eq!(
            render(&options, &spec("cc -c a.c", "CC a.o")),
            "CC a.o (CC a.o) \n"
        );
    }
}
