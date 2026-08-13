//! Translation from Make-front-end outcomes into Ronin's ordinary CLI result.

use crate::Error;
use crate::cli::{PRODUCT_NAME, RunResult};
use crate::frontend::Outcome;
use crate::util::terminated;
use std::fmt::Display;
use std::path::Path;

/// What GNU Make exits with when it abandons a build instead of finishing one.
///
/// Every way of not building is this one status: an option it does not know, no
/// makefile to read, a makefile it cannot evaluate, a target with no rule, a
/// recipe that failed. One is not a failure in Make's vocabulary — it is the
/// answer `-q` gives to a question — so a build that gives up must not report
/// it. Scripts branch on the difference.
pub(super) const ABANDONED: i32 = 2;

/// What an invocation with nothing to read reports.
///
/// Absence of a Makefile is a front-end diagnostic, not a reason to recreate
/// GNU Make's `***` and `Stop.` ceremony.
pub(super) fn no_makefile() -> RunResult {
    RunResult {
        stdout: Vec::new(),
        stderr: ordinary_diagnostic("no targets specified and no makefile found"),
        exit_code: ABANDONED,
    }
}

/// Render a compiler rejection through Ronin's ordinary diagnostic shape.
///
/// Kati identifies Makefile locations correctly, but its standalone reporter
/// decorates fatal messages with GNU Make's recursive prefix, stars, and
/// `Stop.` suffix. Those are runner narration, not compiler information.
// [spec:ronin:req:make.narration+1]
pub(super) fn ordinary_diagnostic(failure: impl Display) -> Vec<u8> {
    format!("{PRODUCT_NAME}: {}\n", diagnostic_body(failure)).into_bytes()
}

/// The compiler's own words for a rejection, with the runner's ceremony taken
/// off and no product prefix, for a caller that will add one.
pub(super) fn diagnostic_body(failure: impl Display) -> String {
    let mut diagnostic = failure.to_string();
    if let Some(rest) = diagnostic.strip_prefix(PRODUCT_NAME).and_then(|rest| {
        rest.strip_prefix(": ").or_else(|| {
            rest.strip_prefix('[')
                .and_then(|rest| rest.split_once(": ").map(|(_, message)| message))
        })
    }) {
        diagnostic = rest.to_owned();
    }
    diagnostic = diagnostic.replace(": *** ", ": ");
    if let Some(rest) = diagnostic.strip_prefix("*** ") {
        diagnostic = rest.to_owned();
    }
    if let Some(message) = diagnostic.strip_suffix("  Stop.") {
        diagnostic.truncate(message.len());
    }
    diagnostic
}

/// What an invocation that could not go on reports.
///
/// This exists so the failure does not leave as an error. The process boundary
/// answers an error with Ninja's status, which is one for every failure alike;
/// Make abandons with two whatever the reason was, and in Make mode the status
/// is Make's.
// [spec:ronin:req:make.recursive-invocation+1]
pub(super) fn abandoned(reported: String, failure: Error) -> RunResult {
    RunResult {
        stdout: terminated(reported),
        stderr: terminated(crate::util::diagnostic(PRODUCT_NAME, failure)),
        exit_code: ABANDONED,
    }
}

/// What `-q` reports, which is a status and nothing else.
///
/// GNU Make's question mode runs no recipe and says nothing about the build:
/// zero says the goals are already up to date, one says something would have to
/// run, and two says the question could not be answered at all. That convention
/// is Make's rather than Ninja's, and it governs here because no build ran to
/// have a status of its own.
// [spec:ronin:req:make.question-status]
pub(super) fn answered(reported: String, question: Result<bool, Error>) -> RunResult {
    match question {
        Ok(up_to_date) => RunResult {
            stdout: terminated(reported),
            stderr: Vec::new(),
            exit_code: i32::from(!up_to_date),
        },
        Err(failure) => RunResult {
            stdout: terminated(reported),
            stderr: terminated(crate::util::diagnostic(PRODUCT_NAME, failure)),
            exit_code: 2,
        },
    }
}

/// What the invocation reports about a build that ran.
///
/// A build that stopped is reported after its own output with Ninja's ordinary
/// summary. Exit-status translation remains isolated here until the Make
/// executor boundary is retired.
///
// [spec:ronin:req:make.narration+1]
pub(super) fn finished(
    reported: String,
    up_to_date: bool,
    outcome: &Outcome,
    silent: bool,
) -> RunResult {
    let mut stdout = terminated(reported);
    stdout.extend_from_slice(outcome.output());
    // A recipe rejected as its edge was launched is rejected for the reasons a
    // recipe rejected while compiling is, and reads as the same diagnostic
    // rather than as a build that stopped.
    if let Some(diagnostic) = outcome.front_end_diagnostic() {
        return RunResult {
            stdout,
            stderr: ordinary_diagnostic(diagnostic),
            exit_code: ABANDONED,
        };
    }
    if let Some((reason, _)) = outcome.stopped.as_ref() {
        stdout.extend_from_slice(format!("{PRODUCT_NAME}: build stopped: {reason}.\n").as_bytes());
    } else if up_to_date && stdout.is_empty() && !silent {
        stdout.extend_from_slice(format!("{PRODUCT_NAME}: no work to do.\n").as_bytes());
    }
    RunResult {
        stdout,
        stderr: Vec::new(),
        // Ninja reports the failing command's own status here, which is the
        // right answer for Ninja and the wrong one for Make: GNU Make has two
        // statuses, and every way of not finishing is the second. A recipe that
        // exits 3 makes Make exit 2, not 3.
        exit_code: if outcome.exit_code() == 0 {
            0
        } else {
            ABANDONED
        },
    }
}

/// Throw away the files the build invented to complete a chain of implicit
/// rules without adding Make's `rm ...` narration.
///
/// Last of everything the build does, and it happens whether the build finished
/// or gave up: what was invented on the way is rubbish either way. `-n` ran
/// nothing, so it removes nothing.
pub(super) fn discard_intermediates(disposable: &[Vec<u8>], pretending: bool) {
    use std::os::unix::ffi::OsStrExt;

    if disposable.is_empty() || pretending {
        return;
    }
    for path in disposable {
        let _ = std::fs::remove_file(Path::new(std::ffi::OsStr::from_bytes(path)));
    }
}
