//! Reading the argument an option carries, and saying what GNU Make says when
//! it cannot be read.
//!
//! Three of these answer `Option` rather than `Result`, which is the whole
//! point: `decode_switches` (main.c) does not die where it notices a word it
//! cannot read. It records the failure against the invocation, consumes the
//! word, and leaves the origin to decide whether the run ends. See
//! [`Invocation::unreadable`](super::Invocation) and its two neighbours for
//! the three ways GNU Make raises that one flag.

use super::{ArgumentSource, BString, BuildOptions, Invocation, JobLimit, LoadLimit};
use crate::util::ByteSlice;

/// The value an option takes, whether it was attached or stands alone.
///
/// `None` when the stream ended with the switch and nothing after it, which is
/// getopt's missing-argument case: recorded against the invocation and left to
/// the origin to decide about, rather than raised. The word is over either
/// way — GNU Make's getopt has consumed it — so the caller stops reading this
/// one and goes on to the next.
///
/// `spelling` is the switch as this word wrote it rather than the switch it
/// resolves to, because getopt says it back that way: a short one is named
/// after the sentence and a long one inside it, and `--makefile` is not
/// reported as `--file`.
pub(super) fn value(
    invocation: &mut Invocation,
    source: ArgumentSource,
    arguments: &[BString],
    index: &mut usize,
    attached: &[u8],
    spelling: &str,
) -> Option<BString> {
    if !attached.is_empty() {
        return Some(BString::from(attached));
    }
    *index += 1;
    let Some(named) = arguments.get(*index).cloned() else {
        invocation.unreadable(source, missing_argument(spelling));
        return None;
    };
    Some(named)
}

/// What getopt says about a switch whose argument is not there.
///
/// Two sentences, and which one is used is decided by the spelling that
/// reached it rather than by the switch: `getopt_long` reports a short option
/// after the sentence and a long one inside it.
fn missing_argument(spelling: &str) -> String {
    spelling
        .strip_prefix('-')
        .filter(|rest| !rest.starts_with('-'))
        .map_or_else(
            || format!("option '{spelling}' requires an argument"),
            |letter| format!("option requires an argument -- '{letter}'"),
        )
}

/// `-j`'s argument, which GNU Make lets stand alone only when it is a number.
///
/// `make -j all` is unlimited jobs and one goal; `make -j 8 all` is eight jobs
/// and one goal. Reading the next word unconditionally would swallow the goal.
pub(super) fn jobs_value(
    invocation: &mut Invocation,
    source: ArgumentSource,
    arguments: &[BString],
    index: &mut usize,
    attached: &[u8],
) -> Option<JobLimit> {
    let digits = if attached.is_empty() {
        let Some(next) = arguments
            .get(*index + 1)
            .filter(|argument| argument.iter().all(u8::is_ascii_digit) && !argument.is_empty())
        else {
            return Some(JobLimit::Unlimited);
        };
        *index += 1;
        next.clone()
    } else {
        BString::from(attached)
    };
    // GNU Make rejects `-j0`; every other count is a limit. A word that is not
    // a count at all is `make_toui`'s failure, which `decode_switches` states
    // for every origin and dies of for one — so the complaint is made here and
    // the job count is left as it stands.
    let limit = digits
        .to_str()
        .ok()
        .and_then(|digits| digits.parse::<usize>().ok())
        .and_then(JobLimit::fixed);
    if limit.is_none() {
        invocation.complain(
            source,
            "the '-j' option requires a positive integer argument".to_owned(),
        );
    }
    limit
}

/// `-l`'s argument, which stands alone only when it is a number.
///
/// The same shape as `-j`'s, for the same reason: `make -l all` is one goal and
/// no load limit at all, so reading the next word unconditionally would swallow
/// it. A bare `-l` lifts the limit, which is the zero the scheduler reads as
/// "do not consult the load average".
pub(super) fn load_value(arguments: &[BString], index: &mut usize, attached: &[u8]) -> LoadLimit {
    let numeric = |argument: &&BString| {
        !argument.is_empty()
            && argument
                .iter()
                .all(|byte| byte.is_ascii_digit() || *byte == b'.')
    };
    let digits = if attached.is_empty() {
        let Some(next) = arguments.get(*index + 1).filter(numeric) else {
            return LoadLimit {
                ceiling: 0.0,
                propagated: false,
            };
        };
        *index += 1;
        next.clone()
    } else {
        BString::from(attached)
    };
    // Whatever `atof` would answer, which is GNU Make's `case floating:` and
    // its whole validation: a word it cannot read is zero, said about by
    // nobody. `-l` is the one switch with an argument that has no way of being
    // wrong.
    let ceiling = digits
        .to_str()
        .ok()
        .and_then(|digits| digits.parse::<f64>().ok())
        .unwrap_or(0.0);
    LoadLimit {
        ceiling,
        propagated: true,
    }
}

/// The two job counts one invocation carries, which are not the same number.
///
/// They differ for exactly one invocation shape — no `-j` at all — and that
/// difference matters, so they travel together rather than one of them being
/// derived from the other at each use.
#[derive(Clone, Copy)]
pub(super) struct JobCounts {
    /// What `MAKEFLAGS` carries, and what the session is told.
    pub(super) carried: usize,
    /// How many of this invocation's Makefile reads may overlap.
    pub(super) parallel_reads: usize,
}

impl JobCounts {
    pub(super) const fn of(options: &BuildOptions) -> Self {
        Self {
            carried: job_count(options.jobs),
            parallel_reads: read_job_count(options),
        }
    }
}

/// How many Makefile reads this invocation may have in flight at once.
///
/// [`job_count`] is what `MAKEFLAGS` carries, and it answers "as many as it
/// takes" for both `-j` with no number and no `-j` at all, because the switch
/// table makes no distinction between them. Reading does: with no `-j`, GNU
/// Make runs one recipe at a time and reads one recursive child at a time, and
/// a compilation that read several would be doing something a serial run of
/// GNU Make never does. This mirrors `Build::job_limit`, which is the number
/// the build itself runs commands against.
const fn read_job_count(options: &BuildOptions) -> usize {
    match options.jobs {
        JobLimit::Auto => 1,
        JobLimit::Unlimited => usize::MAX,
        JobLimit::Fixed(jobs) => jobs.get(),
    }
}

/// The budget a `-j` stands for, as a count of recipes at once.
///
/// A `-j` with no number is every recipe at once, and so is no `-j` at all, for
/// the reason [`read_job_count`] gives: the switch table makes no distinction
/// between them, and the largest count there is says "no ceiling".
pub(super) const fn job_count(jobs: JobLimit) -> usize {
    match jobs {
        JobLimit::Fixed(jobs) => jobs.get(),
        JobLimit::Auto | JobLimit::Unlimited => usize::MAX,
    }
}

/// The budget a settled `MAKEFLAGS` names, for the unit that settled it.
///
/// A value the argument reader will not take is not a budget; the read that
/// owns that `MAKEFLAGS` refuses over it in its own right — see
/// [`super::interface::evaluated_invocation`], which is where the complaint
/// belongs — and a unit asking about its job count is not the place to raise it
/// a second time.
// [spec:ronin:req:make.jobserver+3]
pub(in crate::make) fn makeflags_job_budget(makeflags: &str) -> Option<usize> {
    super::interface::evaluated_invocation(makeflags)
        .ok()?
        .effective_jobs()
        .map(job_count)
}
