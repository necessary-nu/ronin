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

use super::{BuildOptions, BuildState, status};
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

/// Erase from the cursor to the end of the line, which is what Ninja writes
/// after an overprinted status line so a shorter line leaves no tail of the
/// longer one it replaced.
const ERASE_TO_END: &[u8] = b"\x1b[K";

/// What an overprinted line is shortened with.
const ELLIPSIS: &[u8] = b"...";

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
    /// Whether the terminal can be driven with cursor motion: it is a
    /// terminal, `TERM` is set, and `TERM` is not `dumb`. Ninja's
    /// `smart_terminal_`, decided by the same three facts.
    // [spec:ronin:req:compat.terminal-status]
    pub(crate) smart: bool,
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
        if color { Self::COLOURED } else { Self::PLAIN }
    }
}

/// What the supervisor has asked to have rendered.
///
/// Both cases end in the same buffer through the same emit path, so they are
/// one value rather than two nearly identical call sites.
#[derive(Clone, Copy)]
pub(super) enum Rendering<'a> {
    /// A command has started (console pool) or finished (everything else).
    Status(Narrated<'a>),
    /// A command exited non-zero.
    Failure {
        edge: EdgeId,
        exit_code: i32,
        command: Narrated<'a>,
    },
}

/// The two texts a command can be narrated by.
///
/// Held apart from the [`super::command::CommandSpec`] they are usually taken
/// from, because they are not always what it holds: a reference the front end
/// left for the build to fill in as the command launches is filled into these
/// as well, so that a reader is shown the command a run would execute rather
/// than the name that carried the value. See [`super::command::Narration`].
#[derive(Clone, Copy)]
pub(super) struct Narrated<'a> {
    pub(super) command: &'a [u8],
    pub(super) description: &'a [u8],
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

/// Whether an overprinted line is cut to the terminal's width.
///
/// Ninja's `LinePrinter::LineType`. `Full` is what `-v` asks for: a command
/// line shown whole, however long, because it was asked to be seen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LineType {
    Full,
    Elide,
}

/// Where the width an overprinted line is cut to comes from.
enum Columns {
    /// The terminal, asked on every print as Ninja asks it, so a window
    /// resized during the build cuts to its new width from the next line.
    Terminal,
    /// A width a test fixed, so the bytes it asserts do not depend on the
    /// terminal the suite happens to be run in.
    #[cfg(test)]
    Fixed(Option<usize>),
}

/// Ninja's `LinePrinter`, which decides how a line reaches the screen.
///
/// On a smart terminal a status line overprints the one before it and leaves
/// no trace once the next arrives; everything that is not a status line is
/// first moved below it, and the build's end takes the new line. Anywhere else
/// every line is written whole. While a `console` command holds the terminal,
/// every line is held back and released when it lets go.
///
/// A port rather than an approximation, because the bytes are the contract:
/// stock Ninja under a pty is the oracle a test diffs against, and every
/// branch here answers to one of Ninja's. Like everything else in this file it
/// appends to a caller-owned buffer and never writes; the supervisor still
/// owns the write and the flush.
// [spec:ronin:req:compat.terminal-status]
pub(super) struct LinePrinter {
    /// Whether the terminal can be driven with cursor motion at all.
    smart: bool,
    /// Whether the cursor is at the start of a blank line.
    have_blank_line: bool,
    /// Whether a `console` command holds the terminal.
    console_locked: bool,
    /// The status line printed while the console was held, if any. Only the
    /// most recent is kept: on a smart terminal each would have overprinted
    /// the last, so the last is the one that would be on screen.
    line_buffer: Vec<u8>,
    line_type: LineType,
    /// Everything else printed while the console was held, in order.
    output_buffer: Vec<u8>,
    columns: Columns,
}

impl LinePrinter {
    const fn new(smart: bool) -> Self {
        Self {
            smart,
            have_blank_line: true,
            console_locked: false,
            line_buffer: Vec::new(),
            line_type: LineType::Elide,
            output_buffer: Vec::new(),
            columns: Columns::Terminal,
        }
    }

    const fn is_smart(&self) -> bool {
        self.smart
    }

    fn columns(&self) -> Option<usize> {
        match self.columns {
            Columns::Terminal => terminal_columns(),
            #[cfg(test)]
            Columns::Fixed(columns) => columns,
        }
    }

    /// Put a status line on screen, over the previous one where that is
    /// possible.
    ///
    /// Ninja's `LinePrinter::Print`. A narration that spans several lines is
    /// not one line and cannot be overprinted as one: it is written whole, each
    /// of its lines erased to the end so no tail of an earlier status line is
    /// left beside it, and it stays in the scrollback as it would on a pipe.
    /// That is the recipe GNU Make echoes over several lines — narration a
    /// manifest cannot hold and Ninja never sees — and being loud, it is shown.
    fn print(&mut self, out: &mut Vec<u8>, text: &[u8], kind: LineType) {
        if self.console_locked {
            self.line_buffer.clear();
            self.line_buffer.extend_from_slice(text);
            self.line_type = kind;
            return;
        }
        if self.smart {
            // Over the previous line, if any.
            out.push(b'\r');
        }
        if self.smart && kind == LineType::Elide {
            if text.contains(&b'\n') {
                for (index, line) in text.split(|byte| *byte == b'\n').enumerate() {
                    if index != 0 {
                        out.push(b'\n');
                    }
                    out.extend_from_slice(line);
                    out.extend_from_slice(ERASE_TO_END);
                }
                out.push(b'\n');
                self.have_blank_line = true;
                return;
            }
            // Cut to the terminal's width so the line does not wrap: a
            // wrapped line cannot be taken back with one carriage return.
            match self.columns() {
                Some(columns) => elide_middle(out, text, columns),
                None => out.extend_from_slice(text),
            }
            out.extend_from_slice(ERASE_TO_END);
            self.have_blank_line = false;
        } else {
            out.extend_from_slice(text);
            out.push(b'\n');
        }
    }

    fn print_or_buffer(&mut self, out: &mut Vec<u8>, data: &[u8]) {
        if self.console_locked {
            self.output_buffer.extend_from_slice(data);
        } else {
            out.extend_from_slice(data);
        }
    }

    /// Write something that is not a status line, below the status line.
    ///
    /// Ninja's `LinePrinter::PrintOnNewLine`.
    fn print_on_new_line(&mut self, out: &mut Vec<u8>, text: &[u8]) {
        if self.console_locked && !self.line_buffer.is_empty() {
            self.output_buffer.extend_from_slice(&self.line_buffer);
            self.output_buffer.push(b'\n');
            self.line_buffer.clear();
        }
        if !self.have_blank_line {
            self.print_or_buffer(out, b"\n");
        }
        if !text.is_empty() {
            self.print_or_buffer(out, text);
        }
        self.have_blank_line = text.is_empty() || text.last() == Some(&b'\n');
    }

    /// Hand the terminal to a `console` command, or take it back.
    ///
    /// Ninja's `LinePrinter::SetConsoleLocked`. Taking it back releases what
    /// was held: the output first, on its own line, then the last status line
    /// printed in the meantime, over it.
    fn set_console_locked(&mut self, out: &mut Vec<u8>, locked: bool) {
        if locked == self.console_locked {
            return;
        }
        if locked {
            self.print_on_new_line(out, b"");
        }
        self.console_locked = locked;
        if !locked {
            let output = std::mem::take(&mut self.output_buffer);
            self.print_on_new_line(out, &output);
            if !self.line_buffer.is_empty() {
                let line = std::mem::take(&mut self.line_buffer);
                self.print(out, &line, self.line_type);
                self.line_buffer = line;
            }
            self.output_buffer = output;
            self.output_buffer.clear();
            self.line_buffer.clear();
        }
    }
}

/// Everything the Ninja rendering carries between commands.
pub(crate) struct NinjaStyle {
    line: LinePrinter,
    /// Whether the `FAILED:` prefix is coloured, which Ninja does when its
    /// output supports colour.
    color: bool,
    /// Where a line is rendered before the printer decides how to show it,
    /// reused so the decision costs no allocation per command.
    scratch: Vec<u8>,
}

/// How a build narrates itself.
pub(crate) enum Reporter {
    /// Ninja's own output, which is what Ronin emits unless asked otherwise.
    ///
    /// This is the compatibility surface named by `compat.command-runtime`:
    /// the status template, the description-or-command choice, and the
    /// `FAILED:` block are Ninja-owned output that other tools parse, so the
    /// bytes are fixed by the oracle rather than by taste.
    Ninja(NinjaStyle),
    /// Cargo's shape: a right-aligned verb, then what it acted on.
    Cargo(CargoStyle),
}

impl Reporter {
    /// `smart` is whether status lines may be overprinted: the output is a
    /// terminal that can be driven, and the run is neither verbose nor quiet,
    /// which is Ninja's `verbosity == NORMAL` guard on the same decision.
    // [spec:ronin:req:compat.terminal-status]
    pub(crate) const fn new(style: OutputStyle, color: bool, smart: bool) -> Self {
        match style {
            OutputStyle::Ninja => Self::Ninja(NinjaStyle {
                line: LinePrinter::new(smart),
                color,
                scratch: Vec::new(),
            }),
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

    /// Whether a command's status is announced as it starts, and not only as
    /// it finishes.
    ///
    /// Ninja does so on a smart terminal, where the line shows what is running
    /// and is overprinted by the finish; anywhere else a line per start would
    /// double the output. A narration of several lines is the one thing a
    /// smart terminal cannot overprint, so it is written whole and once, when
    /// the command finishes, as it would be on a pipe.
    // [spec:ronin:req:compat.terminal-status]
    pub(super) fn prints_at_start(&self, options: &BuildOptions, command: Narrated<'_>) -> bool {
        match self {
            Self::Ninja(style) => {
                style.line.is_smart() && !describe(options, command).text().contains(&b'\n')
            }
            Self::Cargo(_) => false,
        }
    }

    /// Hand the terminal to a `console` command, or take it back.
    // [spec:ronin:req:compat.terminal-status]
    pub(super) fn set_console_locked(&mut self, out: &mut Vec<u8>, locked: bool) {
        if let Self::Ninja(style) = self {
            style.line.set_console_locked(out, locked);
        }
    }

    /// Write a command's own output, displacing whatever the rendering had on
    /// the cursor's line first.
    // [spec:ronin:req:compat.terminal-status]
    pub(super) fn output(&mut self, out: &mut Vec<u8>, bytes: &[u8]) {
        match self {
            Self::Ninja(style) => style.line.print_on_new_line(out, bytes),
            Self::Cargo(_) => {
                self.clear(out);
                out.extend_from_slice(bytes);
            }
        }
    }

    /// Take back the line the bar is holding, if it is holding one.
    ///
    /// Idempotent, and free when no bar is drawn. Every path that writes
    /// anything calls this first, so the erase rides in the same buffer as
    /// whatever displaces it and costs no additional write.
    pub(super) fn clear(&mut self, out: &mut Vec<u8>) {
        if let Self::Cargo(style) = self
            && let Some(bar) = style.bar.as_mut()
            && std::mem::replace(&mut bar.drawn, false)
        {
            out.extend_from_slice(ERASE_LINE);
        }
    }

    /// Note that a command has begun, so the bar can name it.
    pub(super) fn started(&mut self, options: &BuildOptions, command: Narrated<'_>) {
        if let Self::Cargo(style) = self
            && let Some(bar) = style.bar.as_mut()
        {
            bar.running += 1;
            bar.subject.clear();
            bar.subject
                .extend_from_slice(describe(options, command).text());
        }
    }

    /// Note that a command has ended.
    pub(super) const fn ended(&mut self) {
        if let Self::Cargo(style) = self
            && let Some(bar) = style.bar.as_mut()
        {
            bar.running = bar.running.saturating_sub(1);
        }
    }

    /// Paint the bar again, unless it was painted too recently.
    ///
    /// Appends nothing when the repaint is skipped, which lets the caller
    /// avoid the write entirely rather than write zero bytes.
    pub(super) fn paint(&mut self, out: &mut Vec<u8>, progress: &BuildState) {
        self.paint_as_of(out, progress, Instant::now());
    }

    /// Paint as of `now` rather than as of whenever this was reached.
    ///
    /// The budget is a claim about two instants, so the moment it is judged
    /// against is an input like any other. Reading the clock here would make a
    /// test of the budget a test of how fast the host ran it — which is what
    /// this seam exists to keep out of the suite; the one caller in the product
    /// passes the clock.
    fn paint_as_of(&mut self, out: &mut Vec<u8>, progress: &BuildState, now: Instant) {
        let Self::Cargo(style) = self else {
            return;
        };
        let palette = style.palette;
        let Some(bar) = style.bar.as_mut() else {
            return;
        };
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
        command: Narrated<'_>,
    ) {
        match self {
            Self::Ninja(style) => {
                style.scratch.clear();
                ninja_status(&mut style.scratch, progress, options, command);
                // `-v` asked for the command line whole; anything else is one
                // status line, cut to fit where the terminal allows.
                let kind = if options.verbose {
                    LineType::Full
                } else {
                    LineType::Elide
                };
                style.line.print(out, &style.scratch, kind);
            }
            Self::Cargo(style) => cargo_status(out, style.palette, progress, options, command),
        }
    }

    /// Announce that a command failed, naming the outputs it did not produce.
    pub(super) fn failure(
        &mut self,
        out: &mut Vec<u8>,
        graph: &Graph,
        edge: EdgeId,
        exit_code: i32,
        command: Narrated<'_>,
    ) {
        match self {
            Self::Ninja(style) => {
                style.scratch.clear();
                ninja_failure(
                    &mut style.scratch,
                    graph,
                    edge,
                    exit_code,
                    command,
                    style.color,
                );
                style.line.print_on_new_line(out, &style.scratch);
            }
            Self::Cargo(style) => {
                cargo_failure(out, style.palette, graph, edge, exit_code, command);
            }
        }
    }

    /// Close out the build, whatever became of it.
    ///
    /// Always called, including when the build failed or was interrupted,
    /// because neither rendering may leave the cursor on a line it was still
    /// drawing on: the bar's line goes back, and an overprinted status line
    /// gets the newline Ninja's `BuildFinished` gives it, so the shell's
    /// prompt lands on a line of its own. Ninja says nothing else at the end
    /// of a build either way.
    pub(super) fn finish(&mut self, out: &mut Vec<u8>, progress: &BuildState, succeeded: bool) {
        self.clear(out);
        match self {
            Self::Ninja(style) => {
                style.line.set_console_locked(out, false);
                style.line.print_on_new_line(out, b"");
            }
            Self::Cargo(style) => {
                if succeeded {
                    cargo_finish(out, style.palette, progress);
                }
            }
        }
    }
}

/// Cut `text` to at most `max_width` columns with `...` in its middle, and
/// append the result to `out`.
///
/// Ninja's `ElideMiddleInPlace`, including its reading of ANSI colour
/// sequences: those occupy no columns, so they are kept whole wherever they
/// fall, and one inside the cut is still written so the text after the
/// ellipsis keeps the colour it would have had. Sequences that are not colour
/// are counted as text, as Ninja counts them.
// [spec:ronin:req:compat.terminal-status]
fn elide_middle(out: &mut Vec<u8>, text: &[u8], max_width: usize) {
    if text.len() <= max_width {
        out.extend_from_slice(text);
        return;
    }
    let sequences = color_sequences(text);
    if sequences.is_empty() {
        if max_width <= ELLIPSIS.len() {
            out.extend_from_slice(&ELLIPSIS[..max_width]);
            return;
        }
        let remaining = max_width - ELLIPSIS.len();
        let left = remaining / 2;
        let right = remaining - left;
        out.extend_from_slice(&text[..left]);
        out.extend_from_slice(ELLIPSIS);
        out.extend_from_slice(&text[text.len() - right..]);
        return;
    }
    let visible_width = text.len()
        - sequences
            .iter()
            .map(|(start, end)| end - start)
            .sum::<usize>();
    if visible_width <= max_width {
        out.extend_from_slice(text);
        return;
    }
    let ellipsis_width = max_width.min(ELLIPSIS.len());
    let visible_left = (max_width - ellipsis_width) / 2;
    let visible_right = (max_width - ellipsis_width) - visible_left;
    let gap_start = visible_left;
    let gap_end = visible_width - visible_right;

    // Walk the text once, tracking each byte's column and whether it has one.
    let mut visible_position = 0;
    let mut sequence = sequences.iter().peekable();
    let mut in_sequence = |index: usize| -> bool {
        while let Some((_, end)) = sequence.peek()
            && *end <= index
        {
            sequence.next();
        }
        sequence
            .peek()
            .is_some_and(|(start, end)| (*start..*end).contains(&index))
    };
    let mut index = 0;
    // The left span: every byte, visible or not, before the cut begins.
    while index < text.len() && visible_position != gap_start {
        if !in_sequence(index) {
            visible_position += 1;
        }
        index += 1;
    }
    out.extend_from_slice(&text[..index]);
    out.extend_from_slice(&ELLIPSIS[..ellipsis_width]);
    // Inside the cut only the colour sequences survive.
    while index < text.len() && visible_position != gap_end {
        let visible = !in_sequence(index);
        if !visible {
            out.push(text[index]);
        }
        visible_position += usize::from(visible);
        index += 1;
    }
    out.extend_from_slice(&text[index..]);
}

/// The byte ranges of every ANSI colour sequence in `text`: an escape, `[`,
/// digits and semicolons, and `m`. Anything else that begins with an escape is
/// text, as it is to Ninja's `AnsiColorSequenceIterator`.
fn color_sequences(text: &[u8]) -> Vec<(usize, usize)> {
    let mut sequences = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].iter().position(|byte| *byte == 0x1b) {
        let start = from + offset;
        // The shortest colour sequence is four bytes.
        if start + 4 > text.len() {
            break;
        }
        if text[start + 1] != b'[' {
            from = start + 1;
            continue;
        }
        let mut end = start + 2;
        while text
            .get(end)
            .is_some_and(|byte| byte.is_ascii_digit() || *byte == b';')
        {
            end += 1;
            if end == text.len() {
                return sequences;
            }
        }
        if text[end] != b'm' {
            // Not a colour sequence; a three-byte sequence may have ended
            // here, so the search restarts after its last byte.
            from = start + 3;
            continue;
        }
        sequences.push((start, end + 1));
        from = end + 1;
    }
    sequences
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
    terminal_columns().unwrap_or(ASSUMED_WIDTH)
}

/// How wide the terminal says it is, when it says.
///
/// Ninja reads this with `TIOCGWINSZ` before each overprinted line and elides
/// only when the answer is a positive width; a terminal that reports none gets
/// the line whole. The same three outcomes here, so a guess never cuts a line.
fn terminal_columns() -> Option<usize> {
    #[cfg(unix)]
    {
        rustix::termios::tcgetwinsize(std::io::stdout())
            .ok()
            .map(|size| size.ws_col as usize)
            .filter(|columns| *columns > 0)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

// [spec:ronin:def:build.printstatus-fn]
// [spec:ronin:sem:build.printstatus-fn]
fn ninja_status(
    out: &mut Vec<u8>,
    progress: &BuildState,
    options: &BuildOptions,
    command: Narrated<'_>,
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
}

/// Render Ninja's `FAILED:` block: the prefix, red where the output takes
/// colour, the outputs the command did not produce, and the command itself.
fn ninja_failure(
    out: &mut Vec<u8>,
    graph: &Graph,
    edge: EdgeId,
    exit_code: i32,
    command: Narrated<'_>,
    color: bool,
) {
    if color {
        out.extend_from_slice(b"\x1b[31m");
    }
    let _ = write!(out, "FAILED: [code={exit_code}] ");
    if color {
        out.extend_from_slice(b"\x1b[0m");
    }
    for output in &graph.edge(edge).out {
        out.extend_from_slice(graph.node_path(*output).as_bytes());
        out.push(b' ');
    }
    out.push(b'\n');
    out.extend_from_slice(command.command);
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
    command: Narrated<'_>,
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
    command: Narrated<'_>,
) {
    write_verb(out, b"Failed", palette.failure, palette.reset);
    for output in &graph.edge(edge).out {
        out.push(b' ');
        out.extend_from_slice(graph.node_path(*output).as_bytes());
    }
    let _ = writeln!(out, " (exit {exit_code})");
    out.extend_from_slice(&CONTINUATION);
    out.extend_from_slice(command.command);
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
///
/// The middle case is Ninja's repair for a manifest with a gap in it, and
/// `descriptions_are_complete` is how a front end says its graph has no gaps:
/// there the empty description is the answer rather than the absence of one,
/// and the subject is empty too. `-v` is asked for and so outranks both.
///
/// An empty subject is not a line to withhold. The status format is written
/// as it stands — which is what stock Ninja writes for a `--status` format
/// with no `$description` in it — and what becomes of the line is the
/// printer's decision, the same one it makes for every other line: overprinted
/// on a terminal, whole on a pipe. Withholding it would put a hole in the
/// counter that nothing could tell from a lost edge.
const fn describe<'a>(options: &BuildOptions, command: Narrated<'a>) -> Subject<'a> {
    if options.verbose || (command.description.is_empty() && !options.descriptions_are_complete) {
        Subject::Command(command.command)
    } else {
        Subject::Described(command.description)
    }
}

#[cfg(test)]
mod tests {
    use super::super::command::CommandSpec;
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
            ignore_errors: false,
        }
    }

    fn render(style: OutputStyle, options: &BuildOptions, command: &CommandSpec) -> String {
        let mut progress = BuildState::new(options.clone());
        progress.finished = 3;
        progress.total = 7;
        let mut out = Vec::new();
        Reporter::new(style, false, false).status(&mut out, &progress, options, command.narrated());
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
        Reporter::new(OutputStyle::Cargo, true, false).status(
            &mut out,
            &progress,
            &options,
            command.narrated(),
        );
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
        Reporter::new(OutputStyle::Cargo, false, false).finish(&mut out, &progress, true);
        let summary = String::from_utf8(out).expect("the summary renders as text");
        assert!(
            summary.starts_with("    Finished 1 command in "),
            "unexpected summary: {summary:?}"
        );
        let mut out = Vec::new();
        Reporter::new(OutputStyle::Ninja, false, false).finish(&mut out, &progress, true);
        assert!(out.is_empty());
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_build_that_ran_nothing_is_not_summarised() {
        let options = BuildOptions::default();
        let progress = BuildState::new(options);
        let mut out = Vec::new();
        Reporter::new(OutputStyle::Cargo, false, false).finish(&mut out, &progress, true);
        assert!(out.is_empty());
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn no_color_suppresses_auto_colour_but_not_a_request_for_it() {
        let piped = TerminalContext::default();
        let terminal = TerminalContext {
            is_terminal: true,
            no_color: false,
            smart: true,
        };
        let suppressed = TerminalContext {
            is_terminal: true,
            no_color: true,
            smart: true,
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
            Reporter::Ninja(_) => panic!("Ninja has no bar"),
        }
    }

    fn painted(reporter: &mut Reporter, finished: usize, total: usize) -> String {
        painted_as_of(reporter, finished, total, Instant::now())
    }

    /// What the bar paints when it is told the time, so a test of the repaint
    /// budget states both instants instead of reading one off the host.
    fn painted_as_of(
        reporter: &mut Reporter,
        finished: usize,
        total: usize,
        now: Instant,
    ) -> String {
        let mut progress = BuildState::new(BuildOptions::default());
        progress.finished = finished;
        progress.total = total;
        let mut out = Vec::new();
        reporter.paint_as_of(&mut out, &progress, now);
        String::from_utf8(out).expect("the bar renders as text")
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn a_rendering_that_is_not_being_styled_paints_no_bar() {
        let mut plain = Reporter::new(OutputStyle::Cargo, false, false);
        assert_eq!(painted(&mut plain, 1, 4), "");
        let mut ninja = Reporter::new(OutputStyle::Ninja, true, false);
        assert_eq!(painted(&mut ninja, 1, 4), "");
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn the_gauge_fills_as_the_build_advances() {
        let mut reporter = Reporter::new(OutputStyle::Cargo, true, false);
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
        let mut reporter = Reporter::new(OutputStyle::Cargo, true, false);
        assert!(!painted(&mut reporter, 1, 4).is_empty());
        let mut out = Vec::new();
        reporter.clear(&mut out);
        assert_eq!(out, b"\r\x1b[K");
        // Clearing twice must not emit a second erase: the line is already back.
        let mut again = Vec::new();
        reporter.clear(&mut again);
        assert!(again.is_empty());
    }

    /// Three paints at instants this test names: the first, one a tick inside
    /// the interval, and one exactly at it.
    ///
    /// Every instant is stated rather than sampled. Asking the clock how long
    /// the host took to reach the second statement — which is what this test
    /// did — asserts that the machine is fast rather than that the budget is
    /// honoured, and a loaded host that spends more than the interval between
    /// two lines of test code fails a bar that behaved perfectly.
    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn repainting_inside_the_budget_is_refused() {
        let mut reporter = Reporter::new(OutputStyle::Cargo, true, false);
        let first = Instant::now();
        assert!(
            !painted_as_of(&mut reporter, 1, 4, first).is_empty(),
            "a bar that has never been painted has no budget to spend"
        );
        assert_eq!(
            painted_as_of(
                &mut reporter,
                2,
                4,
                first + REPAINT_INTERVAL.saturating_sub(Duration::from_millis(1))
            ),
            "",
            "a repaint this soon is skipped"
        );
        assert!(
            !painted_as_of(&mut reporter, 3, 4, first + REPAINT_INTERVAL).is_empty(),
            "the budget refills once the interval has passed"
        );
        // The refused paint must also leave the budget where it found it, or
        // two skipped repaints in a row would spend the interval between them.
        assert_eq!(
            bar_state(&mut reporter).painted_at,
            Some(first + REPAINT_INTERVAL),
            "a paint records the instant it was judged against"
        );
    }

    // [spec:ronin:req:product.output-style/test]
    #[test]
    fn finishing_gives_back_the_bars_line_whatever_the_outcome() {
        let mut progress = BuildState::new(BuildOptions::default());
        progress.finished = 2;
        progress.total = 4;
        for succeeded in [true, false] {
            let mut reporter = Reporter::new(OutputStyle::Cargo, true, false);
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

    /// A Ninja reporter on a terminal of `columns` width, or one that reports
    /// no width, so the bytes asserted below do not depend on the terminal
    /// the suite runs in.
    fn smart_ninja(columns: Option<usize>) -> Reporter {
        let mut reporter = Reporter::new(OutputStyle::Ninja, false, true);
        if let Reporter::Ninja(style) = &mut reporter {
            style.line.columns = Columns::Fixed(columns);
        }
        reporter
    }

    fn status_line(reporter: &mut Reporter, finished: usize, description: &str) -> String {
        let options = BuildOptions::default();
        let mut progress = BuildState::new(options.clone());
        progress.finished = finished;
        progress.total = 3;
        let command = spec("true", description);
        let mut out = Vec::new();
        reporter.status(&mut out, &progress, &options, command.narrated());
        String::from_utf8(out).expect("the line renders as text")
    }

    fn written_output(reporter: &mut Reporter, bytes: &str) -> String {
        let mut out = Vec::new();
        reporter.output(&mut out, bytes.as_bytes());
        String::from_utf8(out).expect("the output renders as text")
    }

    fn closed(reporter: &mut Reporter) -> String {
        let mut out = Vec::new();
        reporter.finish(&mut out, &BuildState::new(BuildOptions::default()), true);
        String::from_utf8(out).expect("the closing bytes render as text")
    }

    /// The bytes stock Ninja writes under a pty: a carriage return, the line,
    /// an erase to the end of it, and no newline — so the next status line
    /// lands on top of this one and the build's end supplies the newline.
    // [spec:ronin:req:compat.terminal-status/test]
    #[test]
    fn a_smart_terminal_overprints_each_status_line() {
        let mut reporter = smart_ninja(None);
        assert_eq!(
            status_line(&mut reporter, 0, "said a"),
            "\r[0/3] said a\x1b[K"
        );
        assert_eq!(
            status_line(&mut reporter, 1, "said a"),
            "\r[1/3] said a\x1b[K"
        );
        assert_eq!(closed(&mut reporter), "\n");
        assert_eq!(closed(&mut reporter), "", "the newline is owed once");
    }

    /// Nothing is withheld and nothing is padded: an edge with nothing to say
    /// writes the format's output, which on a terminal is a counter advancing
    /// in place, and on a pipe the line stock Ninja writes for a `--status`
    /// format without `$description`.
    // [spec:ronin:req:compat.terminal-status/test]
    #[test]
    fn an_empty_narration_is_the_counter_alone() {
        let options = BuildOptions {
            descriptions_are_complete: true,
            ..BuildOptions::default()
        };
        assert_eq!(ninja(&options, &spec("@true", "")), "[3/7] \n");
        let mut reporter = smart_ninja(None);
        let mut progress = BuildState::new(options.clone());
        progress.finished = 3;
        progress.total = 7;
        let mut out = Vec::new();
        reporter.status(&mut out, &progress, &options, spec("@true", "").narrated());
        assert_eq!(out, b"\r[3/7] \x1b[K");
    }

    /// A command's output is not a status line, so it goes below the one on
    /// screen; the next status line then starts over on a line of its own.
    // [spec:ronin:req:compat.terminal-status/test]
    #[test]
    fn output_moves_below_the_status_line() {
        let mut reporter = smart_ninja(None);
        assert_eq!(
            status_line(&mut reporter, 1, "echo x"),
            "\r[1/3] echo x\x1b[K"
        );
        assert_eq!(written_output(&mut reporter, "x\n"), "\nx\n");
        assert_eq!(
            status_line(&mut reporter, 2, "said c"),
            "\r[2/3] said c\x1b[K"
        );
        // Output that does not end its line leaves the cursor owing one.
        assert_eq!(written_output(&mut reporter, "partial"), "\npartial");
        assert_eq!(closed(&mut reporter), "\n");
    }

    /// Off a terminal every line is whole, and the printer adds nothing.
    // [spec:ronin:req:compat.terminal-status/test]
    #[test]
    fn a_pipe_gets_every_line_whole() {
        let mut reporter = Reporter::new(OutputStyle::Ninja, false, false);
        assert_eq!(status_line(&mut reporter, 1, "said a"), "[1/3] said a\n");
        assert_eq!(written_output(&mut reporter, "x\n"), "x\n");
        assert_eq!(closed(&mut reporter), "");
    }

    /// A line is cut to the terminal's width with the ellipsis in its middle,
    /// as Ninja cuts it, and a terminal that reports no width cuts nothing.
    // [spec:ronin:req:compat.terminal-status/test]
    #[test]
    fn a_long_line_is_cut_to_terminal_width() {
        let mut reporter = smart_ninja(Some(12));
        // Twenty-two columns into twelve: nine remain around the ellipsis,
        // four on the left and five on the right, as Ninja splits them.
        assert_eq!(
            status_line(&mut reporter, 1, "0123456789abcdef"),
            "\r[1/3...bcdef\x1b[K"
        );
        let mut reporter = smart_ninja(None);
        assert_eq!(
            status_line(&mut reporter, 1, "0123456789abcdef"),
            "\r[1/3] 0123456789abcdef\x1b[K"
        );
    }

    /// A narration of several lines cannot be overprinted as one, so it is
    /// written whole with every line erased to its end, and it does not leave
    /// a newline owed; it is not announced as the command starts either.
    // [spec:ronin:req:compat.terminal-status/test]
    #[test]
    fn a_multi_line_narration_is_written_whole_once() {
        let mut reporter = smart_ninja(Some(12));
        let options = BuildOptions::default();
        assert!(reporter.prints_at_start(&options, spec("x", "one line").narrated()));
        assert!(!reporter.prints_at_start(&options, spec("x", "echo a\necho b").narrated()));
        assert_eq!(
            status_line(&mut reporter, 1, "echo a\necho b"),
            "\r[1/3] echo a\x1b[K\necho b\x1b[K\n"
        );
        assert_eq!(written_output(&mut reporter, "a\n"), "a\n");
    }

    /// While a `console` command has the terminal every other line waits, and
    /// when it lets go the output comes first on its own line and the last
    /// status line printed in the meantime goes over it — Ninja's
    /// `SetConsoleLocked`, on a terminal and on a pipe.
    // [spec:ronin:req:compat.terminal-status/test]
    #[test]
    fn a_console_command_holds_every_other_line() {
        let mut reporter = smart_ninja(None);
        assert_eq!(
            status_line(&mut reporter, 0, "console"),
            "\r[0/3] console\x1b[K"
        );
        let mut out = Vec::new();
        reporter.set_console_locked(&mut out, true);
        assert_eq!(out, b"\n", "the console starts on a line of its own");
        assert_eq!(status_line(&mut reporter, 1, "said b"), "");
        assert_eq!(written_output(&mut reporter, "b\n"), "");
        assert_eq!(status_line(&mut reporter, 2, "said c"), "");
        let mut out = Vec::new();
        reporter.set_console_locked(&mut out, false);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "[1/3] said b\nb\n\r[2/3] said c\x1b[K"
        );

        let mut reporter = Reporter::new(OutputStyle::Ninja, false, false);
        assert_eq!(status_line(&mut reporter, 0, "console"), "[0/3] console\n");
        let mut out = Vec::new();
        reporter.set_console_locked(&mut out, true);
        assert!(out.is_empty());
        assert_eq!(status_line(&mut reporter, 1, "said b"), "");
        assert_eq!(written_output(&mut reporter, "b\n"), "");
        let mut out = Vec::new();
        reporter.set_console_locked(&mut out, false);
        assert_eq!(String::from_utf8(out).unwrap(), "[1/3] said b\nb\n");
    }

    fn elided(text: &str, width: usize) -> String {
        let mut out = Vec::new();
        elide_middle(&mut out, text.as_bytes(), width);
        String::from_utf8(out).unwrap()
    }

    /// Ninja's own `ElideMiddle` cases, plain text.
    // [spec:ronin:req:compat.terminal-status/test]
    #[test]
    fn elision_matches_ninjas_cases() {
        let input = "Nothing to elide in this short string.";
        assert_eq!(elided(input, 80), input);
        assert_eq!(elided(input, 38), input);
        assert_eq!(elided(input, 0), "");
        assert_eq!(elided(input, 1), ".");
        assert_eq!(elided(input, 2), "..");
        assert_eq!(elided(input, 3), "...");
        let input = "01234567890123456789";
        assert_eq!(elided(input, 4), "...9");
        assert_eq!(elided(input, 5), "0...9");
        assert_eq!(elided(input, 9), "012...789");
        assert_eq!(elided(input, 10), "012...6789");
        assert_eq!(elided(input, 11), "0123...6789");
        assert_eq!(elided(input, 19), "01234567...23456789");
        assert_eq!(elided(input, 20), "01234567890123456789");
    }

    /// Ninja's own `ElideMiddle` cases with colour sequences, which occupy no
    /// columns and are kept wherever they fall.
    // [spec:ronin:req:compat.terminal-status/test]
    #[test]
    fn elision_keeps_colour_sequences_where_they_fall() {
        const MAGENTA: &str = "\x1b[0;35m";
        const NOTHING: &str = "\x1b[m";
        const RED: &str = "\x1b[1;31m";
        const RESET: &str = "\x1b[0m";
        let input = format!("012345{MAGENTA}67890123456789");
        assert_eq!(elided(&input, 10), format!("012...{MAGENTA}6789"));
        assert_eq!(elided(&input, 19), format!("012345{MAGENTA}67...23456789"));
        let input = format!("Nothing {NOTHING} string.");
        assert_eq!(elided(&input, 18), input);
        let input = format!("0{NOTHING}1234567890123456789");
        assert_eq!(elided(&input, 10), format!("0{NOTHING}12...6789"));

        let input = format!("abcd{RED}efg{RESET}hlkmnopqrstuvwxyz");
        assert_eq!(elided(&input, 0), format!("{RED}{RESET}"));
        assert_eq!(elided(&input, 1), format!(".{RED}{RESET}"));
        assert_eq!(elided(&input, 2), format!("..{RED}{RESET}"));
        assert_eq!(elided(&input, 3), format!("...{RED}{RESET}"));
        assert_eq!(elided(&input, 4), format!("...{RED}{RESET}z"));
        assert_eq!(elided(&input, 5), format!("a...{RED}{RESET}z"));
        assert_eq!(elided(&input, 6), format!("a...{RED}{RESET}yz"));
        assert_eq!(elided(&input, 7), format!("ab...{RED}{RESET}yz"));
        assert_eq!(elided(&input, 8), format!("ab...{RED}{RESET}xyz"));
        assert_eq!(elided(&input, 9), format!("abc...{RED}{RESET}xyz"));
        assert_eq!(elided(&input, 10), format!("abc...{RED}{RESET}wxyz"));
        assert_eq!(elided(&input, 11), format!("abcd...{RED}{RESET}wxyz"));
        assert_eq!(elided(&input, 12), format!("abcd...{RED}{RESET}vwxyz"));
        assert_eq!(elided(&input, 15), format!("abcd{RED}ef...{RESET}uvwxyz"));
        assert_eq!(elided(&input, 16), format!("abcd{RED}ef...{RESET}tuvwxyz"));
        assert_eq!(elided(&input, 17), format!("abcd{RED}efg...{RESET}tuvwxyz"));
        assert_eq!(
            elided(&input, 18),
            format!("abcd{RED}efg...{RESET}stuvwxyz")
        );
        assert_eq!(
            elided(&input, 19),
            format!("abcd{RED}efg{RESET}h...stuvwxyz")
        );

        let input = format!("abcdef{RED}A{RESET}BC");
        assert_eq!(elided(&input, 4), format!("...{RED}{RESET}C"));
        assert_eq!(elided(&input, 5), format!("a...{RED}{RESET}C"));
        assert_eq!(elided(&input, 6), format!("a...{RED}{RESET}BC"));
        assert_eq!(elided(&input, 7), format!("ab...{RED}{RESET}BC"));
        assert_eq!(elided(&input, 8), format!("ab...{RED}A{RESET}BC"));
        assert_eq!(elided(&input, 9), format!("abcdef{RED}A{RESET}BC"));
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
