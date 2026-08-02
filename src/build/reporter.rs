//! The seam between what the supervisor knows and what the terminal shows.
//!
//! Every byte a build writes about its own progress is rendered here, so that
//! how a build looks is one decision made in one place rather than a shape
//! spread across the supervisor. Rendering writes into a caller-owned buffer
//! and never touches a sink: the supervisor still owns the write, the flush,
//! and the choice between streaming and buffering.
//!
//! Renderings are an enum rather than a trait object or a type parameter. A
//! trait object would put an indirect call and a heap allocation on the path;
//! a type parameter would monomorphise `Builder` — the whole build loop, dirty
//! evaluation and dependency ingestion — once per rendering, to devirtualise a
//! call that happens once per finished command next to a `write` and a flush.
//! An enum gives the same direct, inlinable call for a branch on a value that
//! is fixed before the first command starts.

use super::command::CommandSpec;
use super::{status, BuildOptions, BuildState};
use crate::graph::{EdgeId, Graph};
use crate::util::ByteSlice;
use std::io::Write as _;

/// Column the subject of a Cargo-style line starts in, counting the space.
const VERB_WIDTH: usize = 12;

/// Indent that lines a continuation up under the subject.
const CONTINUATION: [u8; VERB_WIDTH + 1] = [b' '; VERB_WIDTH + 1];

/// Which rendering a build uses.
///
/// Ninja's is the default and stays the default: it is the output generators
/// and editors parse, so switching on terminal detection alone would change
/// what Ronin emits for consumers that never asked for anything. Selecting
/// another is a Ronin-owned decision made explicitly on the command line.
// [spec:samurai:req:product.output-style]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum OutputStyle {
    #[default]
    Ninja,
    Cargo,
}

/// When to emit terminal colour.
// [spec:samurai:req:product.output-style]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

/// What the process knows about its own output, captured once at startup.
///
/// A build writes through a `dyn Write` that may be a terminal, a pipe, or a
/// buffer the library caller owns, and none of those can be asked which they
/// are. So the answer is taken at the process boundary, where there is a real
/// file descriptor to ask, and carried in rather than inferred.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TerminalContext {
    pub(crate) is_terminal: bool,
    pub(crate) no_color: bool,
}

impl OutputStyle {
    pub(crate) const fn parse(value: &[u8]) -> Option<Self> {
        match value {
            b"ninja" => Some(Self::Ninja),
            b"cargo" => Some(Self::Cargo),
            _ => None,
        }
    }
}

impl ColorChoice {
    pub(crate) const fn parse(value: &[u8]) -> Option<Self> {
        match value {
            b"auto" => Some(Self::Auto),
            b"always" => Some(Self::Always),
            b"never" => Some(Self::Never),
            _ => None,
        }
    }

    /// Decide whether this build actually emits escapes.
    ///
    /// `NO_COLOR` is honoured for `auto` and not for `always`, following the
    /// convention that it overrides a default rather than an instruction.
    pub(crate) const fn resolve(self, terminal: TerminalContext) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => terminal.is_terminal && !terminal.no_color,
        }
    }
}

/// The escapes a rendering writes around the words it emphasises.
///
/// Held as resolved byte strings rather than a flag tested at each write, so
/// that a colourless build costs three empty appends instead of three branches
/// and the rendering code reads the same either way.
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    work: &'static [u8],
    failure: &'static [u8],
    counter: &'static [u8],
    reset: &'static [u8],
}

impl Palette {
    const PLAIN: Self = Self {
        work: b"",
        failure: b"",
        counter: b"",
        reset: b"",
    };
    const COLOURED: Self = Self {
        work: b"\x1b[1;32m",
        failure: b"\x1b[1;31m",
        counter: b"\x1b[2m",
        reset: b"\x1b[0m",
    };

    const fn select(color: bool) -> Self {
        if color {
            Self::COLOURED
        } else {
            Self::PLAIN
        }
    }
}

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
pub(crate) enum Reporter {
    /// Ninja's own output, which is what Ronin emits unless asked otherwise.
    ///
    /// This is the compatibility surface named by `compat.command-runtime`:
    /// the status template, the description-or-command choice, and the
    /// `FAILED:` block are Ninja-owned output that other tools parse, so the
    /// bytes are fixed by the oracle rather than by taste.
    Ninja,
    /// Cargo's shape: a right-aligned verb, then what it acted on.
    Cargo(Palette),
}

impl Reporter {
    pub(crate) const fn new(style: OutputStyle, color: bool) -> Self {
        match style {
            OutputStyle::Ninja => Self::Ninja,
            OutputStyle::Cargo => Self::Cargo(Palette::select(color)),
        }
    }

    /// Announce that a command has started or finished.
    pub(super) fn status(
        &mut self,
        out: &mut Vec<u8>,
        progress: &BuildState,
        options: &BuildOptions,
        command: &CommandSpec,
    ) {
        match self {
            Self::Ninja => ninja_status(out, progress, options, command),
            Self::Cargo(palette) => cargo_status(out, *palette, progress, options, command),
        }
    }

    /// Announce that a command failed, naming the outputs it did not produce.
    pub(super) fn failure(
        &mut self,
        out: &mut Vec<u8>,
        graph: &Graph,
        edge: EdgeId,
        exit_code: i32,
        command: &CommandSpec,
    ) {
        match self {
            Self::Ninja => ninja_failure(out, graph, edge, exit_code, command),
            Self::Cargo(palette) => cargo_failure(out, *palette, graph, edge, exit_code, command),
        }
    }

    /// Close out a build that ran to completion without failing.
    ///
    /// Ninja says nothing at the end of a build, so nothing is what it says.
    pub(super) fn finish(&mut self, out: &mut Vec<u8>, progress: &BuildState) {
        match self {
            Self::Ninja => {}
            Self::Cargo(palette) => cargo_finish(out, *palette, progress),
        }
    }
}

// [spec:samurai:def:build.printstatus-fn]
// [spec:samurai:sem:build.printstatus-fn]
fn ninja_status(
    out: &mut Vec<u8>,
    progress: &BuildState,
    options: &BuildOptions,
    command: &CommandSpec,
) {
    let description = describe(options, command).text();
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

fn ninja_failure(
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

/// Render `    Building CXX object src/main.cc.o (12/83)`.
///
/// The verb is the description's first word, which costs nothing and works
/// because generators already write descriptions that begin with one: both
/// `CMake` and Meson emit `Building …`, `Linking …`, `Generating …`. A
/// description without a space becomes the whole verb and no subject, and a
/// command shown in place of a description gets `Running`, since splitting
/// `/usr/bin/c++` off the front of a command line would name a path rather
/// than an action.
///
/// The counter closes the line rather than opening it. The verb column is the
/// thing being read down a long build, and a count in front of it is as wide
/// as the numbers currently in it, so the column would shift as the build
/// passed 9, 99 and 999 finished commands. Dimmed and trailing, the count is
/// there when looked for and out of the way when not.
// [spec:samurai:req:product.output-style]
fn cargo_status(
    out: &mut Vec<u8>,
    palette: Palette,
    progress: &BuildState,
    options: &BuildOptions,
    command: &CommandSpec,
) {
    match describe(options, command) {
        Subject::Described(text) => {
            let (verb, rest) = split_verb(text);
            write_verb(out, verb, palette.work, palette.reset);
            if !rest.is_empty() {
                out.push(b' ');
                out.extend_from_slice(rest);
            }
        }
        Subject::Command(text) => {
            write_verb(out, b"Running", palette.work, palette.reset);
            out.push(b' ');
            out.extend_from_slice(text);
        }
    }
    out.extend_from_slice(palette.counter);
    let _ = write!(out, " ({}/{})", progress.finished, progress.total);
    out.extend_from_slice(palette.reset);
    out.push(b'\n');
}

/// Render the failure as a verb line plus the command, indented to line up
/// under the outputs it did not produce.
// [spec:samurai:req:product.output-style]
fn cargo_failure(
    out: &mut Vec<u8>,
    palette: Palette,
    graph: &Graph,
    edge: EdgeId,
    exit_code: i32,
    command: &CommandSpec,
) {
    write_verb(out, b"Failed", palette.failure, palette.reset);
    for output in &graph.edge(edge).out {
        out.push(b' ');
        out.extend_from_slice(graph.node_path(*output).as_bytes());
    }
    let _ = writeln!(out, " (exit {exit_code})");
    out.extend_from_slice(&CONTINUATION);
    out.extend_from_slice(command.command.as_bytes());
    out.push(b'\n');
}

// [spec:samurai:req:product.output-style]
#[allow(
    clippy::cast_precision_loss,
    reason = "an elapsed-time summary is deliberately approximate"
)]
fn cargo_finish(out: &mut Vec<u8>, palette: Palette, progress: &BuildState) {
    if progress.finished == 0 {
        return;
    }
    write_verb(out, b"Finished", palette.work, palette.reset);
    let plural = if progress.finished == 1 { "" } else { "s" };
    let elapsed = progress.start.elapsed().as_secs_f64();
    let _ = writeln!(
        out,
        " {} command{plural} in {elapsed:.2}s",
        progress.finished
    );
}

/// Right-align `verb` in its column, then colour it.
///
/// Padding is written before the escape so that the escape bytes, which
/// occupy no columns, cannot shift the alignment.
fn write_verb(out: &mut Vec<u8>, verb: &[u8], colour: &[u8], reset: &[u8]) {
    for _ in display_width(verb)..VERB_WIDTH {
        out.push(b' ');
    }
    out.extend_from_slice(colour);
    out.extend_from_slice(verb);
    out.extend_from_slice(reset);
}

/// How many columns a verb occupies, near enough to align a column.
///
/// Descriptions are bytes and need not be UTF-8, so this counts characters
/// when it can and bytes when it cannot. Neither is a true display width —
/// a wide or combining character would still sit a column out — but verbs are
/// short ASCII words in every generator's output, and being wrong about an
/// unusual one costs alignment rather than correctness.
fn display_width(text: &[u8]) -> usize {
    std::str::from_utf8(text).map_or_else(|_| text.len(), |text| text.chars().count())
}

/// Split a description into its first word and everything after it.
fn split_verb(text: &[u8]) -> (&[u8], &[u8]) {
    text.iter()
        .position(|byte| *byte == b' ')
        .map_or((text, &[][..]), |index| {
            (&text[..index], &text[index + 1..])
        })
}

/// What stands for a command in the output, and which of the two it is.
#[derive(Clone, Copy)]
enum Subject<'a> {
    Described(&'a [u8]),
    Command(&'a [u8]),
}

impl<'a> Subject<'a> {
    const fn text(self) -> &'a [u8] {
        match self {
            Self::Described(text) | Self::Command(text) => text,
        }
    }
}

/// A command's description, or the command itself when there is no
/// description or the operator asked to see command lines.
fn describe<'a>(options: &BuildOptions, command: &'a CommandSpec) -> Subject<'a> {
    if options.verbose || command.description.is_empty() {
        Subject::Command(command.command.as_bytes())
    } else {
        Subject::Described(command.description.as_bytes())
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

    fn render(style: OutputStyle, options: &BuildOptions, command: &CommandSpec) -> String {
        let mut progress = BuildState::new(options.clone());
        progress.finished = 3;
        progress.total = 7;
        let mut out = Vec::new();
        Reporter::new(style, false).status(&mut out, &progress, options, command);
        String::from_utf8(out).expect("the fixture renders as text")
    }

    fn ninja(options: &BuildOptions, command: &CommandSpec) -> String {
        render(OutputStyle::Ninja, options, command)
    }

    fn cargo(options: &BuildOptions, command: &CommandSpec) -> String {
        render(OutputStyle::Cargo, options, command)
    }

    // [spec:samurai:req:compat.command-runtime/test]
    #[test]
    fn the_default_reporter_emits_ninjas_counter_and_description() {
        let options = BuildOptions::default();
        assert_eq!(
            ninja(&options, &spec("cc -c a.c", "CC a.o")),
            "[3/7] CC a.o\n"
        );
    }

    // [spec:samurai:req:compat.command-runtime/test]
    #[test]
    fn a_command_without_a_description_stands_for_itself() {
        let options = BuildOptions::default();
        assert_eq!(ninja(&options, &spec("cc -c a.c", "")), "[3/7] cc -c a.c\n");
    }

    // [spec:samurai:req:compat.command-runtime/test]
    #[test]
    fn verbose_prefers_the_command_over_the_description() {
        let options = BuildOptions {
            verbose: true,
            ..BuildOptions::default()
        };
        assert_eq!(
            ninja(&options, &spec("cc -c a.c", "CC a.o")),
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
            ninja(&options, &spec("cc -c a.c", "CC a.o")),
            "CC a.o (CC a.o) \n"
        );
    }

    // [spec:samurai:req:product.output-style/test]
    #[test]
    fn cargo_style_right_aligns_the_descriptions_first_word() {
        let options = BuildOptions::default();
        assert_eq!(
            cargo(&options, &spec("c++ -c a.cc", "Building CXX object a.cc.o")),
            "    Building CXX object a.cc.o (3/7)\n"
        );
    }

    // [spec:samurai:req:product.output-style/test]
    #[test]
    fn a_verb_at_or_past_the_column_is_not_truncated() {
        let options = BuildOptions::default();
        assert_eq!(
            cargo(&options, &spec("x", "Regenerating build.ninja")),
            "Regenerating build.ninja (3/7)\n"
        );
    }

    // [spec:samurai:req:product.output-style/test]
    #[test]
    fn a_description_of_one_word_is_all_verb() {
        let options = BuildOptions::default();
        assert_eq!(
            cargo(&options, &spec("x", "Linking")),
            "     Linking (3/7)\n"
        );
    }

    // [spec:samurai:req:product.output-style/test]
    #[test]
    fn a_command_shown_in_place_of_a_description_is_running() {
        let options = BuildOptions::default();
        assert_eq!(
            cargo(&options, &spec("/usr/bin/c++ -c a.cc", "")),
            "     Running /usr/bin/c++ -c a.cc (3/7)\n"
        );
    }

    // [spec:samurai:req:product.output-style/test]
    #[test]
    fn colour_surrounds_the_verb_without_shifting_the_column() {
        let options = BuildOptions::default();
        let command = spec("c++ -c a.cc", "Building a.cc.o");
        let progress = BuildState::new(options.clone());
        let mut out = Vec::new();
        Reporter::new(OutputStyle::Cargo, true).status(&mut out, &progress, &options, &command);
        assert_eq!(
            String::from_utf8(out).expect("the fixture renders as text"),
            "    \u{1b}[1;32mBuilding\u{1b}[0m a.cc.o\u{1b}[2m (0/0)\u{1b}[0m\n"
        );
    }

    // [spec:samurai:req:product.output-style/test]
    #[test]
    fn a_summary_closes_a_cargo_style_build_and_nothing_closes_ninjas() {
        let mut progress = BuildState::new(BuildOptions::default());
        progress.finished = 1;
        let mut out = Vec::new();
        Reporter::new(OutputStyle::Cargo, false).finish(&mut out, &progress);
        let summary = String::from_utf8(out).expect("the summary renders as text");
        assert!(
            summary.starts_with("    Finished 1 command in "),
            "unexpected summary: {summary:?}"
        );
        let mut out = Vec::new();
        Reporter::new(OutputStyle::Ninja, false).finish(&mut out, &progress);
        assert!(out.is_empty());
    }

    // [spec:samurai:req:product.output-style/test]
    #[test]
    fn a_build_that_ran_nothing_is_not_summarised() {
        let options = BuildOptions::default();
        let progress = BuildState::new(options);
        let mut out = Vec::new();
        Reporter::new(OutputStyle::Cargo, false).finish(&mut out, &progress);
        assert!(out.is_empty());
    }

    // [spec:samurai:req:product.output-style/test]
    #[test]
    fn no_color_suppresses_auto_colour_but_not_a_request_for_it() {
        let piped = TerminalContext::default();
        let terminal = TerminalContext {
            is_terminal: true,
            no_color: false,
        };
        let suppressed = TerminalContext {
            is_terminal: true,
            no_color: true,
        };
        assert!(!ColorChoice::Auto.resolve(piped));
        assert!(ColorChoice::Auto.resolve(terminal));
        assert!(!ColorChoice::Auto.resolve(suppressed));
        assert!(ColorChoice::Always.resolve(suppressed));
        assert!(!ColorChoice::Never.resolve(terminal));
    }

    // [spec:samurai:req:product.output-style/test]
    #[test]
    fn styles_and_colour_choices_are_named_on_the_command_line() {
        assert_eq!(OutputStyle::parse(b"ninja"), Some(OutputStyle::Ninja));
        assert_eq!(OutputStyle::parse(b"cargo"), Some(OutputStyle::Cargo));
        assert_eq!(OutputStyle::parse(b"fancy"), None);
        assert_eq!(ColorChoice::parse(b"auto"), Some(ColorChoice::Auto));
        assert_eq!(ColorChoice::parse(b"always"), Some(ColorChoice::Always));
        assert_eq!(ColorChoice::parse(b"never"), Some(ColorChoice::Never));
        assert_eq!(ColorChoice::parse(b"maybe"), None);
    }
}
