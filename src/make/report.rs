//! What a Make invocation says about how it went.
//!
//! Separated from the command line because the two answer different questions:
//! [`super::cli`] reads what was asked for, and this turns what happened into
//! the status and the words GNU Make would have used.

use crate::cli::{RunResult, PRODUCT_NAME};
use crate::error::CliError;
use crate::frontend::Outcome;
use crate::util::terminated;
use crate::Error;
use std::io::Write;
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
/// The announcement is a pair or it is nothing: an Entering with no Leaving
/// leaves every parser reading them resolving paths against a directory the
/// build has already left.
pub(super) fn no_makefile(reported: String, announcing: bool, directory: &Path) -> RunResult {
    departed(
        RunResult {
            stdout: terminated(reported),
            stderr: format!(
                "{PRODUCT_NAME}: *** No targets specified and no makefile found.  Stop.\n"
            )
            .into_bytes(),
            exit_code: ABANDONED,
        },
        announcing,
        directory,
    )
}

/// What an invocation that could not go on reports.
///
/// This exists so the failure does not leave as an error. The process boundary
/// answers an error with Ninja's status, which is one for every failure alike;
/// Make abandons with two whatever the reason was, and in Make mode the status
/// is Make's.
// [spec:ronin:req:make.recursive-invocation]
pub(super) fn abandoned(reported: String, failure: Error) -> RunResult {
    RunResult {
        stdout: terminated(reported),
        stderr: terminated(crate::util::diagnostic(PRODUCT_NAME, failure)),
        exit_code: ABANDONED,
    }
}

/// One half of GNU Make's directory announcement, in GNU Make's own words.
///
/// Every error parser that inherited the convention reads this pair to resolve
/// the relative paths a compiler then prints, so the wording and the quoting
/// are Make 4.4's rather than Ninja's; only the name in front is Ronin's.
// [spec:ronin:req:product.make-identity]
pub(super) fn announcement(verb: &str, directory: &Path) -> String {
    format!("{PRODUCT_NAME}: {verb} directory '{}'", directory.display())
}

/// Put a line where the caller will see it, in the order the build saw it.
///
/// A caller that gave a sink is watching the build happen and gets the line as
/// it is said; a caller that did not is handed it back with the result.
pub(super) fn say(
    output: &mut Option<&mut dyn Write>,
    reported: &mut String,
    line: &str,
) -> Result<(), Error> {
    if let Some(sink) = output.as_deref_mut() {
        writeln!(sink, "{line}").map_err(CliError::write_output)?;
        sink.flush().map_err(CliError::write_output)?;
    } else {
        reported.push_str(line);
        reported.push('\n');
    }
    Ok(())
}

/// Close the directory announcement this invocation opened.
///
/// Last of everything the invocation says, which is where GNU Make puts it and
/// where a parser reading the pair stops resolving paths against the directory.
pub(super) fn departed(mut result: RunResult, announcing: bool, directory: &Path) -> RunResult {
    if announcing {
        result
            .stdout
            .extend_from_slice(&terminated(announcement("Leaving", directory)));
    }
    result
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
/// A build that stopped is a result rather than a diagnostic: it is said on
/// stdout after the build's own output, and the status left with is the
/// failing recipe's own. That is Ninja's contract rather than Make's exit 2,
/// and where the two contracts meet the Ninja one governs.
pub(super) fn finished(
    reported: String,
    up_to_date: bool,
    outcome: &Outcome,
    silent: bool,
    removed: &[u8],
) -> RunResult {
    let mut stdout = terminated(reported);
    stdout.extend_from_slice(outcome.output());
    if let Some(reason) = outcome.stopped() {
        stdout.extend_from_slice(format!("{PRODUCT_NAME}: build stopped: {reason}.\n").as_bytes());
    } else if up_to_date && stdout.is_empty() && !silent {
        stdout.extend_from_slice(format!("{PRODUCT_NAME}: no work to do.\n").as_bytes());
    }
    // Last of everything the build itself said, which is where GNU Make says
    // what it threw away.
    stdout.extend_from_slice(removed);
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
