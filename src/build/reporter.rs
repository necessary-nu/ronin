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
use std::time::{Duration, Instant};

/// Column the subject of a Cargo-style line starts in, counting the space.
const VERB_WIDTH: usize = 12;

/// How often the pinned bar may be repainted.
///
/// This is the whole cost control. Clearing the bar is free — it rides along
/// in the same buffer as the line that displaces it — but repainting is a
/// separate write, and a build of trivial commands finishes them faster than
/// any terminal can usefully show. Refusing to repaint more than thirty times
/// a second bounds the bar's cost at thirty writes per second no matter how
/// fast the build runs, rather than at one per command.
const REPAINT_INTERVAL: Duration = Duration::from_millis(33);

/// Terminal width assumed when the real one cannot be read.
const ASSUMED_WIDTH: usize = 80;

/// How wide the drawn portion of the bar is, between its brackets.
const GAUGE_WIDTH: usize = 24;

/// Return to the start of the line and erase to its end.
const ERASE_LINE: &[u8] = b"\r\x1b[K";

/// Indent that lines a continuation up under the subject.
const CONTINUATION: [u8; VERB_WIDTH + 1] = [b' '; VERB_WIDTH + 1];

/// Which rendering a build uses.
///
/// Ninja's is the default and stays the default: it is the output generators
/// and editors parse, so switching on terminal detection alone would change
/// what Ronin emits for consumers that never asked for anything. Selecting
/// another is a Ronin-owned decision made explicitly on the command line.
// [spec:ronin:req:product.output-style]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum OutputStyle {
    #[default]
    Ninja,
    Cargo,
    /// GNU Make's: the recipe itself, echoed line by line before it runs.
    ///
    /// Not offered on the command line, because it is not a preference. Make
    /// mode selects it and nothing else can: a Ninja manifest carries no recipe
    /// to echo, so asking for this over one would ask for silence.
    Make,
}

/// When to emit terminal colour.
// [spec:ronin:req:product.output-style]
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

/// A line held at the bottom of the terminal while the build scrolls above it.
///
/// The bar owns no sink and performs no writes of its own; like every other
/// rendering here it appends bytes and lets the supervisor decide when they
/// go out. What it does own is the knowledge of whether it is currently on
/// screen, because a line printed over a drawn bar has to displace it first.
pub(crate) struct Bar {
    /// Whether a painted bar is currently occupying the cursor's line.
    drawn: bool,
    painted_at: Option<Instant>,
    running: usize,
    /// The most recently started command's subject, reused rather than
    /// reallocated, so tracking what to show costs no allocation per command.
    subject: Vec<u8>,
}

impl Bar {
    const fn new() -> Self {
        Self {
            drawn: false,
            painted_at: None,
            running: 0,
            subject: Vec::new(),
        }
    }

    /// Whether enough time has passed to justify painting again.
    fn may_paint(&self, now: Instant) -> bool {
        self.painted_at
            .is_none_or(|painted_at| now.duration_since(painted_at) >= REPAINT_INTERVAL)
    }
}

/// Everything the Cargo rendering carries between commands.
pub(crate) struct CargoStyle {
    palette: Palette,
    /// Present when the output is being styled for a terminal.
    bar: Option<Bar>,
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
    Cargo(CargoStyle),
    /// Make's shape, which is not a status line at all: Make prints the recipe
    /// it is about to run and then lets the recipe's own output follow.
    ///
    /// There is no counter, no description and no name for the edge. Ninja's
    /// `[N/M] description` is a Ninja product surface, and a Makefile that
    /// never asked for it should not be told how many edges Ronin thinks there
    /// are.
    Make,
}

impl Reporter {
    pub(crate) const fn new(style: OutputStyle, color: bool) -> Self {
        match style {
            OutputStyle::Ninja => Self::Ninja,
            OutputStyle::Make => Self::Make,
            OutputStyle::Cargo => Self::Cargo(CargoStyle {
                palette: Palette::select(color),
                // The bar rides with colour rather than with terminal
                // detection directly: both answer "is this being styled for
                // someone to look at", and tying them together means
                // `--color=always` forces the bar out for a recording or a
                // measurement exactly as it forces escapes out.
                bar: if color { Some(Bar::new()) } else { None },
            }),
        }
    }

    /// Take back the line the bar is holding, if it is holding one.
    ///
    /// Idempotent, and free when no bar is drawn. Every path that writes
    /// anything calls this first, so the erase rides in the same buffer as
    /// whatever displaces it and costs no additional write.
    pub(super) fn clear(&mut self, out: &mut Vec<u8>) {
        if let Self::Cargo(style) = self {
            if let Some(bar) = style.bar.as_mut() {
                if std::mem::replace(&mut bar.drawn, false) {
                    out.extend_from_slice(ERASE_LINE);
                }
            }
        }
    }

    /// Note that a command has begun, so the bar can name it.
    pub(super) fn started(&mut self, options: &BuildOptions, command: &CommandSpec) {
        if let Self::Cargo(style) = self {
            if let Some(bar) = style.bar.as_mut() {
                bar.running += 1;
                bar.subject.clear();
                bar.subject
                    .extend_from_slice(describe(options, command).text());
            }
        }
    }

    /// Note that a command has ended.
    pub(super) const fn ended(&mut self) {
        if let Self::Cargo(style) = self {
            if let Some(bar) = style.bar.as_mut() {
                bar.running = bar.running.saturating_sub(1);
            }
        }
    }

    /// Paint the bar again, unless it was painted too recently.
    ///
    /// Appends nothing when the repaint is skipped, which lets the caller
    /// avoid the write entirely rather than write zero bytes.
    pub(super) fn paint(&mut self, out: &mut Vec<u8>, progress: &BuildState) {
        let Self::Cargo(style) = self else {
            return;
        };
        let palette = style.palette;
        let Some(bar) = style.bar.as_mut() else {
            return;
        };
        let now = Instant::now();
        if !bar.may_paint(now) {
            return;
        }
        bar.painted_at = Some(now);
        bar.drawn = true;
        paint_bar(out, palette, bar, progress);
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
            Self::Cargo(style) => cargo_status(out, style.palette, progress, options, command),
            Self::Make => make_status(out, command),
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
            // Make's failure line is its own shape and is not this; until
            // make-recipe-failure-report gives it one, Ninja's is what there is.
            Self::Ninja | Self::Make => ninja_failure(out, graph, edge, exit_code, command),
            Self::Cargo(style) => {
                cargo_failure(out, style.palette, graph, edge, exit_code, command);
            }
        }
    }

    /// Close out the build, whatever became of it.
    ///
    /// Always called, including when the build failed or was interrupted,
    /// because the bar must not be left holding a line the shell is about to
    /// print a prompt on. Ninja says nothing at the end of a build either way.
    pub(super) fn finish(&mut self, out: &mut Vec<u8>, progress: &BuildState, succeeded: bool) {
        self.clear(out);
        if let Self::Cargo(style) = self {
            if succeeded {
                cargo_finish(out, style.palette, progress);
            }
        }
    }
}

/// Render `    Building [=========>       ] 24/83: Building src/graph.cc.o`.
///
/// Everything is cut to the terminal's width, because a bar that wraps stops
/// being one: the erase sequence takes back one line, so a wrapped bar leaves
/// its own first half on screen for the rest of the build.
// [spec:ronin:req:product.output-style]
fn paint_bar(out: &mut Vec<u8>, palette: Palette, bar: &Bar, progress: &BuildState) {
    let width = terminal_width();
    write_verb(out, b"Building", palette.work, palette.reset);
    out.extend_from_slice(b" [");
    let filled = (GAUGE_WIDTH * progress.finished)
        .checked_div(progress.total)
        .unwrap_or(0);
    for column in 0..GAUGE_WIDTH {
        out.push(match column.cmp(&filled) {
            std::cmp::Ordering::Less => b'=',
            std::cmp::Ordering::Equal => b'>',
            std::cmp::Ordering::Greater => b' ',
        });
    }
    let _ = write!(out, "] {}/{}", progress.finished, progress.total);
    // Whatever the fixed part came to, the subject gets the rest of the line.
    let fixed =
        VERB_WIDTH + 2 + GAUGE_WIDTH + 2 + digits(progress.finished) + digits(progress.total);
    let room = width.saturating_sub(fixed + 2);
    if room > 0 && !bar.subject.is_empty() {
        out.extend_from_slice(b": ");
        out.extend_from_slice(truncated(&bar.subject, room));
    }
}

const fn digits(value: usize) -> usize {
    let mut digits = 1;
    let mut value = value;
    while value >= 10 {
        digits += 1;
        value /= 10;
    }
    digits
}

/// Cut `text` to at most `columns`, without splitting a character.
fn truncated(text: &[u8], columns: usize) -> &[u8] {
    if text.len() <= columns {
        return text;
    }
    std::str::from_utf8(text).map_or(&text[..columns], |text| {
        let end = text
            .char_indices()
            .take(columns)
            .last()
            .map_or(0, |(index, character)| index + character.len_utf8());
        &text.as_bytes()[..end]
    })
}

/// How wide the terminal is, or a conventional guess.
///
/// Read afresh on each repaint rather than captured at startup, because it is
/// the one process fact here that changes while a build runs — a window is
/// resized mid-build often enough to matter, and a stale width is exactly the
/// wrapped bar the truncation above exists to prevent. Repaints are already
/// capped at thirty a second, which caps this too.
fn terminal_width() -> usize {
    #[cfg(unix)]
    {
        rustix::termios::tcgetwinsize(std::io::stdout())
            .ok()
            .map(|size| size.ws_col as usize)
            .filter(|columns| *columns > 0)
            .unwrap_or(ASSUMED_WIDTH)
    }
    #[cfg(not(unix))]
    {
        ASSUMED_WIDTH
    }
}

/// Print the recipe, which is the whole of what Make says before running one.
///
/// The lines arrive already joined by newlines and already filtered: a line
/// prefixed `@` never reached the binding, so there is nothing to decide here.
/// An empty recipe is a recipe Make would print nothing for, and prints
/// nothing — not a blank line.
// [spec:ronin:req:make.recipe-echo]
fn make_status(out: &mut Vec<u8>, command: &CommandSpec) {
    if command.recipe.is_empty() {
        return;
    }
    out.extend_from_slice(&command.recipe);
    out.push(b'\n');
}

// [spec:ronin:def:build.printstatus-fn]
// [spec:ronin:sem:build.printstatus-fn]
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
// [spec:ronin:req:product.output-style]
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
// [spec:ronin:req:product.output-style]
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

// [spec:ronin:req:product.output-style]
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
        recipe_spec(command, description, "")
    }

    fn recipe_spec(command: &str, description: &str, recipe: &str) -> CommandSpec {
        CommandSpec {
            command: command.into(),
            description: description.into(),
            recipe: recipe.into(),
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

    /// Make says the recipe and nothing else — no counter, no description, and
    /// nothing at all for a recipe it would not have echoed.
    // [spec:ronin:req:make.recipe-echo/test]
    #[test]
    fn make_mode_prints_the_recipe_and_no_progress_of_its_own() {
        let options = BuildOptions::default();

        let echoed = recipe_spec("cc -c main.c", "build main.o", "cc -c main.c");
        assert_eq!(
            render(OutputStyle::Make, &options, &echoed),
            "cc -c main.c\n"
        );

        // Every line of the recipe, in order, because Make echoes each one.
        let several = recipe_spec("a && b", "build out", "a\nb");
        assert_eq!(render(OutputStyle::Make, &options, &several), "a\nb\n");

        // A recipe whose lines were all `@` arrives empty and says nothing —
        // not a blank line, which is what printing an empty string would give.
        let silent = recipe_spec("echo hi", "build out", "");
        assert_eq!(render(OutputStyle::Make, &options, &silent), "");

        // The description is Ninja's answer to a question Make never asks, and
        // it is not consulted even when there is one.
        assert!(!render(OutputStyle::Make, &options, &echoed).contains("build main.o"));
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

    // [spec:ronin:req:compat.command-runtime/test]
    #[test]
    fn the_default_reporter_emits_ninjas_counter_and_description() {
        let options = BuildOptions::default();
        assert_eq!(
            ninja(&options, &spec("cc -c a.c", "CC a.o")),
            "[3/7] CC a.o\n"
        );
    }

    // [spec:ronin:req:compat.command-runtime/test]
    #[test]
    fn a_command_without_a_description_stands_for_itself() {
        let options = BuildOptions::default();
        assert_eq!(ninja(&options, &spec("cc -c a.c", "")), "[3/7] cc -c a.c\n");
    }

    // [spec:ronin:req:compat.command-runtime/test]
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

    // [spec:ronin:req:compat.command-runtime/test]
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

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn cargo_style_right_aligns_the_descriptions_first_word() {
        let options = BuildOptions::default();
        assert_eq!(
            cargo(&options, &spec("c++ -c a.cc", "Building CXX object a.cc.o")),
            "    Building CXX object a.cc.o (3/7)\n"
        );
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_verb_at_or_past_the_column_is_not_truncated() {
        let options = BuildOptions::default();
        assert_eq!(
            cargo(&options, &spec("x", "Regenerating build.ninja")),
            "Regenerating build.ninja (3/7)\n"
        );
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_description_of_one_word_is_all_verb() {
        let options = BuildOptions::default();
        assert_eq!(
            cargo(&options, &spec("x", "Linking")),
            "     Linking (3/7)\n"
        );
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_command_shown_in_place_of_a_description_is_running() {
        let options = BuildOptions::default();
        assert_eq!(
            cargo(&options, &spec("/usr/bin/c++ -c a.cc", "")),
            "     Running /usr/bin/c++ -c a.cc (3/7)\n"
        );
    }

    // [spec:ronin:req:product.output-style/test]
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

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_summary_closes_a_cargo_style_build_and_nothing_closes_ninjas() {
        let mut progress = BuildState::new(BuildOptions::default());
        progress.finished = 1;
        let mut out = Vec::new();
        Reporter::new(OutputStyle::Cargo, false).finish(&mut out, &progress, true);
        let summary = String::from_utf8(out).expect("the summary renders as text");
        assert!(
            summary.starts_with("    Finished 1 command in "),
            "unexpected summary: {summary:?}"
        );
        let mut out = Vec::new();
        Reporter::new(OutputStyle::Ninja, false).finish(&mut out, &progress, true);
        assert!(out.is_empty());
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_build_that_ran_nothing_is_not_summarised() {
        let options = BuildOptions::default();
        let progress = BuildState::new(options);
        let mut out = Vec::new();
        Reporter::new(OutputStyle::Cargo, false).finish(&mut out, &progress, true);
        assert!(out.is_empty());
    }

    // [spec:ronin:req:product.output-style/test]
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

    fn bar_state(reporter: &mut Reporter) -> &mut Bar {
        match reporter {
            Reporter::Cargo(style) => style.bar.as_mut().expect("this rendering has a bar"),
            Reporter::Ninja | Reporter::Make => panic!("only Cargo's rendering has a bar"),
        }
    }

    fn painted(reporter: &mut Reporter, finished: usize, total: usize) -> String {
        let mut progress = BuildState::new(BuildOptions::default());
        progress.finished = finished;
        progress.total = total;
        let mut out = Vec::new();
        reporter.paint(&mut out, &progress);
        String::from_utf8(out).expect("the bar renders as text")
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_rendering_that_is_not_being_styled_paints_no_bar() {
        let mut plain = Reporter::new(OutputStyle::Cargo, false);
        assert_eq!(painted(&mut plain, 1, 4), "");
        let mut ninja = Reporter::new(OutputStyle::Ninja, true);
        assert_eq!(painted(&mut ninja, 1, 4), "");
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn the_gauge_fills_as_the_build_advances() {
        let mut reporter = Reporter::new(OutputStyle::Cargo, true);
        let bar = bar_state(&mut reporter);
        bar.subject.extend_from_slice(b"Building src/graph.cc.o");
        let mut show = |finished: usize| {
            bar_state(&mut reporter).painted_at = None;
            let text = painted(&mut reporter, finished, 24);
            text.replace('\u{1b}', "").replace("[0m", "")
        };
        assert!(
            show(0).contains("[>                       ] 0/24"),
            "{:?}",
            show(0)
        );
        assert!(
            show(12).contains("[============>           ] 12/24"),
            "{:?}",
            show(12)
        );
        assert!(
            show(24).contains("[========================] 24/24"),
            "{:?}",
            show(24)
        );
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_painted_bar_is_taken_back_before_anything_displaces_it() {
        let mut reporter = Reporter::new(OutputStyle::Cargo, true);
        assert!(!painted(&mut reporter, 1, 4).is_empty());
        let mut out = Vec::new();
        reporter.clear(&mut out);
        assert_eq!(out, b"\r\x1b[K");
        // Clearing twice must not emit a second erase: the line is already back.
        let mut again = Vec::new();
        reporter.clear(&mut again);
        assert!(again.is_empty());
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn repainting_inside_the_budget_is_refused() {
        let mut reporter = Reporter::new(OutputStyle::Cargo, true);
        assert!(!painted(&mut reporter, 1, 4).is_empty());
        assert_eq!(
            painted(&mut reporter, 2, 4),
            "",
            "a repaint this soon is skipped"
        );
        bar_state(&mut reporter).painted_at =
            Instant::now().checked_sub(REPAINT_INTERVAL + Duration::from_millis(1));
        assert!(
            !painted(&mut reporter, 3, 4).is_empty(),
            "the budget refills"
        );
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn finishing_gives_back_the_bars_line_whatever_the_outcome() {
        let mut progress = BuildState::new(BuildOptions::default());
        progress.finished = 2;
        progress.total = 4;
        for succeeded in [true, false] {
            let mut reporter = Reporter::new(OutputStyle::Cargo, true);
            assert!(!painted(&mut reporter, 2, 4).is_empty());
            let mut out = Vec::new();
            reporter.finish(&mut out, &progress, succeeded);
            let text = String::from_utf8(out).expect("the closing bytes render as text");
            assert!(text.starts_with("\r\u{1b}[K"), "{text:?}");
            assert_eq!(text.contains("Finished"), succeeded, "{text:?}");
        }
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_subject_is_cut_without_splitting_a_character() {
        assert_eq!(truncated(b"short", 40), b"short");
        assert_eq!(truncated(b"abcdef", 3), b"abc");
        // Three two-byte characters: two columns must yield four bytes, not two.
        assert_eq!(
            truncated("\u{e9}\u{e9}\u{e9}".as_bytes(), 2),
            "\u{e9}\u{e9}".as_bytes()
        );
        assert_eq!(truncated(&[0xff, 0xfe, 0xfd], 2), &[0xff, 0xfe]);
    }

    // [spec:ronin:req:product.output-style/test]
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
