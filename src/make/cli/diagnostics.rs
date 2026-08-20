//! Where the compiler's own diagnostics go once a run has raised them.
//!
//! The compiler writes what it has to say to a descriptor the invocation
//! owns rather than to the process's standard error, so that a caller holding
//! a run as a value holds its warnings too. This is the other end of that:
//! the moments a run drains the descriptor, and what it does with what comes
//! out.

use super::PreparedGraph;
use crate::Error;
use crate::cli::RunResult;
use crate::error::CliError;
use std::io::Write;

/// Write out whatever the compiler has raised since the last drain.
///
/// The same arrangement Ninja's own warnings have: streamed to the
/// invocation's diagnostic sink where there is one, and retained for the
/// result's standard error where there is not. The words are the compiler's
/// own and already rendered — a compile-time diagnostic keeps the Makefile
/// location it points at — so nothing is added to them here.
// [spec:ronin:req:make.narration+1]
pub(super) fn emit_raised(
    raised: &kati::diagnostics::Diagnostics,
    diagnostics: &mut Option<&mut dyn Write>,
    held: &mut Vec<u8>,
) -> Result<(), Error> {
    let rendered = raised.take();
    if rendered.is_empty() {
        return Ok(());
    }
    match diagnostics {
        Some(sink) => sink
            .write_all(&rendered)
            .and_then(|()| sink.flush())
            .map_err(CliError::write_output)?,
        None => held.extend_from_slice(&rendered),
    }
    Ok(())
}

/// A refusal with whatever the read raised on its way to it in front of it.
///
/// GNU Make writes a warning where it is raised, so one raised by a line the
/// read got past belongs ahead of the refusal that ended the read rather than
/// after it. With a sink to stream to that ordering is the sink's already; with
/// none, the two share one buffer and the order has to be built here.
// [spec:ronin:req:make.narration+1]
pub(super) fn led_by_raised(
    mut refusal: RunResult,
    raised: &kati::diagnostics::Diagnostics,
    diagnostics: &mut Option<&mut dyn Write>,
    held: &mut Vec<u8>,
) -> Result<PreparedGraph, Error> {
    emit_raised(raised, diagnostics, held)?;
    if diagnostics.is_none() {
        let mut leading = std::mem::take(held);
        leading.append(&mut refusal.stderr);
        refusal.stderr = leading;
    }
    Ok(PreparedGraph::Finished(refusal))
}

#[cfg(test)]
mod tests {
    /// A compiler diagnostic goes where the invocation's own warnings go: out
    /// through the diagnostic sink when there is one, and into the result's
    /// standard error when there is not. Neither path adds anything to the
    /// words — a compile-time diagnostic already points at its Makefile.
    // [spec:ronin:req:make.narration+1/test]
    #[test]
    fn a_diagnostic_is_streamed_or_retained() {
        let raised = kati::diagnostics::Diagnostics::collected();
        raised.write_line("Makefile:1: careful");
        let mut held = Vec::new();
        super::emit_raised(&raised, &mut None, &mut held).unwrap();
        assert_eq!(held, b"Makefile:1: careful\n");

        raised.write_line("Makefile:2: careful again");
        let mut streamed = Vec::new();
        let mut sink: Option<&mut dyn std::io::Write> = Some(&mut streamed);
        super::emit_raised(&raised, &mut sink, &mut held).unwrap();
        assert_eq!(streamed, b"Makefile:2: careful again\n");
        assert_eq!(
            held, b"Makefile:1: careful\n",
            "a streamed diagnostic is not retained as well"
        );
    }
}
