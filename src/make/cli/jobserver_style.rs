//! `--jobserver-style`: the one switch whose value outlives the option stream
//! that named it.
//!
//! GNU Make holds the style in a variable of its own for the whole run. It is
//! never written into `MAKEFLAGS` — a child is told the jobserver's address
//! rather than the style it was built in — and it is not judged where it is
//! read, because the check lives in `jobserver_setup`, which runs after the
//! makefiles have been read and may have changed the job count. Reading it,
//! carrying it, and refusing it are therefore three separate moments, and this
//! is all three of them.

use super::{Action, BString, Error, Invocation, JobLimit, refuse, value};
use crate::util::ByteSlice;

/// The styles GNU Make 4.4.1 knows how to set a jobserver up in.
///
/// Case-sensitive and closed: `jobserver_setup` takes the fifo path for `fifo`
/// (and for no style at all), the pipe path for `pipe`, and refuses the rest.
const JOBSERVER_STYLES: [&[u8]; 2] = [b"fifo", b"pipe"];

/// Record `--jobserver-style`'s value, attached or standing alone.
///
/// Recorded rather than consumed as a no-op: what it names is checked once both
/// option streams have been read, because whether GNU Make looks at the value at
/// all depends on the job count they settle on. An empty value is the one thing
/// refused on sight, which GNU does before the jobserver is ever reached.
pub(super) fn read_jobserver_style(
    invocation: &mut Invocation,
    option: &[u8],
    arguments: &[BString],
    index: &mut usize,
) -> Result<Option<Action>, Error> {
    let style = match option.strip_prefix(b"--jobserver-style=") {
        Some(attached) => BString::from(attached),
        None => value(arguments, index, b"", "--jobserver-style")?,
    };
    if style.is_empty() {
        return Ok(Some(refuse(
            "the '--jobserver-style' option requires a non-empty string argument",
        )));
    }
    invocation.jobserver_style = Some(style);
    Ok(None)
}

/// The refusal a jobserver style this Make cannot provide earns, if it earns
/// one at all.
///
/// GNU Make checks the value inside `jobserver_setup`, which it reaches only
/// with more than one job slot to hand out. So `--jobserver-style=nonsense`
/// alone, or with `-j1`, or with a bare `-j`, is never looked at and never
/// refused — the check is the jobserver's, not the option parser's. That also
/// makes it a question no single option stream can answer, since the style and
/// the job count may arrive from different ones: a makefile's own `MAKEFLAGS`
/// asks this again with whatever the command line already settled.
pub(super) fn unknown_jobserver_style(invocation: &Invocation) -> Option<String> {
    let style = invocation.jobserver_style.as_ref()?;
    if !matches!(invocation.effective_jobs(), Some(JobLimit::Fixed(jobs)) if jobs.get() > 1) {
        return None;
    }
    (!JOBSERVER_STYLES.contains(&style.as_bytes()))
        .then(|| format!("unknown jobserver auth style '{}'", style.to_str_lossy()))
}

/// The switch table's own spelling: what `MAKEFLAGS` publishes, and the
/// switches the table remembers without publishing them.
///
/// `--jobserver-style` is the whole of that set. GNU Make's `define_makeflags`
/// never writes it out, yet the value is still in force while a makefile's own
/// assignment is decoded and when the jobserver is finally started. Carrying it
/// here is what lets `MAKEFLAGS += -j4` on a later line be judged against a
/// style named on an earlier one.
pub(super) fn carried_switches(base: &str, invocation: &Invocation) -> String {
    let mut carried = base.to_owned();
    if let Some(style) = &invocation.jobserver_style {
        carried.push_str(" --jobserver-style=");
        carried.push_str(&style.to_str_lossy());
    }
    carried
}
