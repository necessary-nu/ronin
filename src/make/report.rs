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

/// What an invocation that names standard input twice reports.
///
/// There is one standard input and a read that may happen more than once, so
/// the second `-f-` is a request that cannot be honoured however the two were
/// spelled. GNU Make refuses it before reading anything, and so does this.
pub(super) fn duplicate_standard_input() -> RunResult {
    RunResult {
        stdout: Vec::new(),
        stderr: ordinary_diagnostic("Makefile from standard input specified twice"),
        exit_code: ABANDONED,
    }
}

/// Render a compiler rejection through Ronin's ordinary diagnostic shape.
///
/// Kati identifies Makefile locations correctly, but its standalone reporter
/// decorates fatal messages with GNU Make's recursive prefix, stars, and
/// `Stop.` suffix. Those are runner narration, not compiler information.
// [spec:ronin:req:make.narration+2]
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
// [spec:ronin:req:make.recursive-invocation+2]
pub(super) fn abandoned(reported: String, failure: Error) -> RunResult {
    RunResult {
        stdout: terminated(reported),
        stderr: terminated(crate::util::diagnostic(PRODUCT_NAME, failure)),
        exit_code: ABANDONED,
    }
}

/// What a read the user stopped reports.
///
/// Not [`abandoned`], although the read did not finish: `ABANDONED` is the
/// status for every way of NOT BEING ABLE to build, and an interrupt is the
/// user stopping a read that was going fine — the same distinction
/// [`stopped_status`] draws for a build, drawn where the compiler is what was
/// cut short. GNU Make 4.4.1 leaves 130 here too, by dying of the signal it
/// caught while a `$(shell)` ran.
///
/// Nothing is written about it. There is no diagnostic to give, because the
/// Makefile said nothing wrong; GNU Make prints nothing either, having already
/// re-raised. Whatever the read narrated before the signal still goes out,
/// which is what a restarted read's remaking narration is.
///
/// What the read does to the command it was waiting for — abandon it rather
/// than signal it, as GNU Make does — is `[spec:ronin:req:make.read-interrupt]`
/// and not this rule's, and it is not `compat.process-integration`'s either:
/// that one says what an interrupt does to a BUILD, and a read has no edges.
// [spec:ronin:req:product.build-outcome+1]
pub(super) fn cut_short(reported: String) -> RunResult {
    RunResult {
        stdout: terminated(reported),
        stderr: Vec::new(),
        exit_code: crate::subprocess::INTERRUPTED_EXIT_CODE,
    }
}

/// What a run that ended over a required Makefile nothing can make reports.
///
/// The same ending as [`abandoned`], but reached after work rather than instead
/// of it: GNU Make brings the Makefiles it reached before that one up to date
/// and refuses from inside that update, so whatever the remaking narrated is
/// already in `reported` and goes out in front of the refusal.
///
/// Each complaint is the located `No such file or directory` for the file that
/// would not open, which GNU Make holds back from the read and prints here, one
/// line ahead of what it dies on. It says why the file could not be read, which
/// the refusal beside it does not, so it is reporting a failure rather than
/// narrating one.
///
/// More than one refusal only under `-k`, where `complain()` reports instead of
/// dying and the update walks on to the next makefile.
// [spec:ronin:req:make.narration+2]
pub(super) fn refused_makefile(
    reported: String,
    refusals: Vec<(Option<String>, impl Display)>,
) -> RunResult {
    let mut stderr = Vec::new();
    for (complaint, failure) in refusals {
        if let Some(complaint) = complaint {
            stderr.extend_from_slice(format!("{complaint}\n").as_bytes());
        }
        stderr.extend(ordinary_diagnostic(failure));
    }
    RunResult {
        stdout: terminated(reported),
        stderr,
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
///
/// A refusal is ordinarily the third of those, and there is one refusal that is
/// the second instead. GNU Make has two failing statuses for a makefile: one the
/// `-q` pass merely ASKED about is left `us_question`, which `main.c` turns into
/// `MAKE_TROUBLE` — 1, the same answer as "something would have to run" — where
/// one whose recipe ran and lost is left `us_failed` and `MAKE_FAILURE`. A
/// makefile in that second state is refused over too and still answers 2, and
/// still outranks a question the same run holds, which is GNU Make taking the
/// worse of the two statuses.
///
/// `keep_going` is what makes the distinction visible and so what gates it:
/// `complain()` chooses `error` over `fatal` on `keep_going_flag`
/// (remake.c:422), and without the switch the complaint is fatal and 2 wins
/// whatever status the file was left in.
///
/// The refusal is reported either way, on stderr with every other diagnostic.
/// It is the reason the question could not be answered any other way, so it is
/// a failure being reported rather than a run being narrated.
///
/// `cut_short` outranks all three. A question the user stopped was not answered
/// and must not report that it was — least of all with the affirmative zero,
/// which tells a script branching on `-q` that there is nothing to do. The
/// interrupt is READ rather than inferred from the walk: `interrogate` answers
/// `Ok(true)` for a `+` line that DECLINES the signal and reaches the end of its
/// own script, so the walk's own result cannot tell an interrupted run from an
/// uneventful one, and a `+` line killed by a signal nobody sent this process is
/// an ordinary failure that keeps its 2. Measured across the whole `-q` matrix:
/// GNU Make 4.4.1 leaves 130 for an interrupt during a `+` line trapping or not,
/// under `-k`, with more work behind it, and through a recursive child, and
/// leaves 2 only for a question the makefile cannot answer with no signal in
/// sight.
// [spec:ronin:req:make.question-status+1]
// [spec:ronin:req:make.narration+2]
pub(super) fn answered(
    reported: String,
    question: Result<bool, Error>,
    keep_going: bool,
    cut_short: bool,
) -> RunResult {
    if cut_short {
        return self::cut_short(reported);
    }
    match question {
        Ok(up_to_date) => RunResult {
            stdout: terminated(reported),
            stderr: Vec::new(),
            exit_code: i32::from(!up_to_date),
        },
        Err(failure) => {
            let questioned = keep_going && failure.refused_a_questioned_makefile();
            RunResult {
                stdout: terminated(reported),
                stderr: terminated(crate::util::diagnostic(PRODUCT_NAME, failure)),
                exit_code: if questioned { 1 } else { 2 },
            }
        }
    }
}

/// What the invocation reports about a build that ran.
///
/// A build that stopped is reported after its own output with Ninja's ordinary
/// summary. Exit-status translation remains isolated here until the Make
/// executor boundary is retired.
///
// [spec:ronin:req:make.narration+2]
pub(super) fn finished(
    reported: String,
    up_to_date: bool,
    outcome: &Outcome,
    silent: bool,
) -> RunResult {
    complained_of(reported, up_to_date, outcome, silent, &[])
}

/// The same, with the complaints a lost remake released.
///
/// GNU Make's second `show_goal_error` caller is `child_error` (job.c:581),
/// which prints the held complaint one line before the line that names the
/// failure — so a required `include` whose own rule ran and lost says both why
/// the file mattered and why it is not there.
///
/// The complaint is a diagnostic and goes where Ronin's diagnostics go, which
/// is stderr, as it does for the other `show_goal_error` caller in
/// [`refused_makefile`]. GNU Make puts it on the stream carrying the line it
/// precedes; matching that interleaving would be choosing a stream to reproduce
/// GNU Make's output order rather than to say where a diagnostic belongs.
// [spec:ronin:req:make.narration+2]
pub(super) fn complained_of(
    reported: String,
    up_to_date: bool,
    outcome: &Outcome,
    silent: bool,
    complaints: &[String],
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
    let mut stderr = Vec::new();
    for complaint in complaints {
        stderr.extend_from_slice(complaint.as_bytes());
        stderr.push(b'\n');
    }
    if let Some((reason, _)) = outcome.stopped.as_ref() {
        stdout.extend_from_slice(format!("{PRODUCT_NAME}: build stopped: {reason}.\n").as_bytes());
    } else if (up_to_date || !outcome.ran_a_command()) && stdout.is_empty() && !silent {
        stdout.extend_from_slice(format!("{PRODUCT_NAME}: no work to do.\n").as_bytes());
    }
    RunResult {
        stdout,
        stderr,
        exit_code: stopped_status(outcome),
    }
}

/// The status a Make-mode build leaves with once it has run.
///
/// Ninja reports the failing command's own status, which is the right answer
/// for Ninja and the wrong one for Make: GNU Make has two statuses, and every
/// way of not finishing is the second. A recipe that exits 3 makes Make exit 2,
/// not 3.
///
/// A build cut short is not one of those ways. `ABANDONED` is what the refusals
/// enumerated above this file leave with — an option Make does not know, no
/// makefile to read, a makefile that will not evaluate, a target with no rule,
/// a recipe that failed — and each of them is a build that would not or could
/// not go on. An interrupt is the user stopping a build that was going fine,
/// and it is answered with 130 — which
/// `[spec:ronin:req:make.question-status+1]` names as governing every Make
/// invocation's status but `-q`'s THREE ANSWERS. An interrupted `-q` is not one
/// of those three, and leaves the same 130 as this.
///
/// This number is not what the process leaves with, and cannot be: it says the
/// run was cut short, and `main` reads the signal that cut it short and dies of
/// it, so a shell sees 143 for `SIGTERM` and 129 for `SIGHUP`. That is GNU
/// Make's own disposition and it governs the manifest front end equally. What a
/// status value can carry and what an ending can carry are different widths,
/// and this is the narrower one.
///
/// The reason is read rather than the number: a recipe that exits 130 of its
/// own accord is a failed recipe, and Make mode reports it as the 2 that every
/// other failed recipe gets. Nothing turns that 2 into a signal afterwards,
/// because no signal reached this process to be raised again.
// [spec:ronin:req:product.build-outcome+1]
fn stopped_status(outcome: &Outcome) -> i32 {
    if outcome.exit_code() == 0 {
        return 0;
    }
    if outcome.interrupted() {
        return crate::subprocess::INTERRUPTED_EXIT_CODE;
    }
    ABANDONED
}

/// Throw away the files the build invented to complete a chain of implicit
/// rules without adding Make's `rm ...` narration.
///
/// Last of everything the build does, and it happens whether the build finished
/// or gave up: what was invented on the way is rubbish either way.
///
/// `swept_by_nothing` is `remove_intermediates`' own early return (`file.c`),
/// which gives up on the whole run rather than on one file. `-q` and `-t` are
/// two of the four flags it reads, and it reads them where it runs, which is
/// once, at the end — so a file `-t` brought into existence by touching it, and
/// one a Makefile pass made in earnest while `-q` was set aside for it, are both
/// files no sweep reaches. `-n` is the caller's other term: GNU Make still walks
/// the list under it and only declines the `unlink`, which comes to the same
/// thing here because nothing was made to remove. The remaining two — a bare
/// `.SECONDARY:` and `.NOTINTERMEDIATE:` — are answered where the manifest is
/// compiled, because they are what the makefile said rather than what the
/// invocation asked.
pub(super) fn discard_intermediates(disposable: &[Vec<u8>], swept_by_nothing: bool) {
    use std::os::unix::ffi::OsStrExt;

    if disposable.is_empty() || swept_by_nothing {
        return;
    }
    for path in disposable {
        let _ = std::fs::remove_file(Path::new(std::ffi::OsStr::from_bytes(path)));
    }
}
