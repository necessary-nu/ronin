//! The one number a wall-time gate compares, and how many samples it needs.
//!
//! Both gates used to take the median of five samples per tool and divide. On
//! a host with anything else running, that statistic straddles its own refusal
//! threshold. Replaying each gate's own protocol over a pool of 151 interleaved
//! repetitions per workload and drawing synthetic runs out of the pool by
//! moving-block bootstrap — blocks rather than individual samples, because a
//! gate run is a contiguous slice of a drifting host and shuffling destroys
//! exactly the drift that makes it flap — a median of five refuses an
//! unmodified tree this often:
//!
//! ```text
//!     dependency-log-load           19.4%      path-canonicalization  15.6%
//!     manifest-command-evaluation   11.4%      deep-graph-evaluation   9.4%
//!     vim-noop                       9.1%      wide-noop-build         7.6%
//! ```
//!
//! Six workloads out of twelve refuse a tree nobody touched more than once in
//! fourteen runs. That is not a gate; it teaches everyone to re-run a red gate
//! until it goes green, which is the same as having none.
//!
//! ## The statistic: the quiet decile
//!
//! A no-op's sample distribution has a hard floor — the work the tree actually
//! costs — and a long right tail that is the rest of the machine. Contention
//! can only ADD time. So the informative part of the sample is the bottom of
//! it, and a central estimator spends its whole life in the contaminated part.
//!
//! The minimum is the extreme of that argument and is the one thing this must
//! not use: a minimum is not a consistent statistic. Its value falls as you
//! take more samples, so a row recorded from five and checked against
//! thirty-one is two different numbers, and the repetition count could never be
//! changed again. **The tenth percentile is the same number at any sample
//! count** and needs only enough samples to locate. Measured over both pools,
//! refusals of an unmodified tree in 20,000 bootstrapped runs:
//!
//! ```text
//!     workload                        median-of-5   p10 @ its count
//!     dependency-log-load                  19.355%   0.000%  (31)
//!     path-canonicalization                15.585%   0.000%  (21)
//!     manifest-command-evaluation          11.400%   0.000%  (21)
//!     deep-graph-evaluation                 9.400%   0.000%  (21)
//!     vim-noop                              9.075%   0.000%  (31)
//!     wide-noop-build                       7.560%   0.000%  (21)
//!     large-manifest-parse                  0.000%   0.000%  (31)
//!     clean-tree-noop                       2.000%   0.000%  (21)
//!     wide-noop / recursive-noop /
//!     zsh-incremental / scheduler-barrier   0.000%   0.000%
//! ```
//!
//! Four other estimators were measured against the same pools and each loses on
//! one side or the other: a 20% trimmed mean is the tightest thing there is on
//! the Make rows and still refuses 0.230% of `manifest-command-evaluation`; the
//! median needs 51+ repetitions to reach what the decile reaches at 21; the
//! median of per-repetition ratios is better than the median and worse than the
//! decile everywhere; the minimum matches the decile and is inconsistent.
//!
//! ## The repetitions, which are per workload
//!
//! `vim-noop` is 34 ms and `zsh-incremental` is 1.8 s, so one count for the
//! catalog is nearly free on one row and a third of the gate's whole runtime on
//! the other — and they need different counts anyway, because the spread that
//! has to fit inside the margin is a different spread. [`REPETITIONS`] carries
//! what each row was measured to need. The whole gated Make catalog is about a
//! hundred seconds, which is what nine repetitions of everything already cost.
//!
//! ## What this is not
//!
//! It is not a wider band. Every threshold is exactly where it was — the
//! recorded-ratio tolerance is still 1.20 on both gates, the Ninja gate's
//! absolute runtime and peak-RSS ceilings are untouched, and the quiet-host
//! guard is still 4.00. Peak RSS keeps the median it always had, deliberately:
//! the decile exists to see under a right tail that is contention, and the
//! failure RSS is gated against IS a high number.

use std::time::Duration;

/// Which quantile the gate judges on, as a percentage.
const QUIET_DECILE: f64 = 0.10;

/// The gate's statistic: the tenth percentile of the samples, linearly
/// interpolated between the two order statistics that bracket it.
///
/// Sorts in place. Defined for any sample count, though a count below about ten
/// puts it between the first and second sample, where it estimates the same
/// thing with much less confidence — which is why [`repetitions_for`] gives
/// every gated row at least fifteen.
pub fn quiet_decile(samples: &mut [Duration]) -> Duration {
    assert!(!samples.is_empty(), "a statistic needs a sample");
    samples.sort_unstable();
    interpolate(samples, QUIET_DECILE)
}

fn interpolate(sorted: &[Duration], quantile: f64) -> Duration {
    if sorted.len() == 1 {
        return sorted[0];
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a repetition count that loses precision as an f64 is a run nobody will finish"
    )]
    let position = quantile * (sorted.len() - 1) as f64;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "position is non-negative and below the length by construction"
    )]
    let low = position as usize;
    let high = (low + 1).min(sorted.len() - 1);
    #[expect(
        clippy::cast_precision_loss,
        reason = "the fractional part of an index, not a measurement"
    )]
    let fraction = position - low as f64;
    sorted[low] + sorted[high].saturating_sub(sorted[low]).mul_f64(fraction)
}

/// The median, which is what peak RSS is still summarised by.
///
/// Kept as its own named function rather than left inline in the Ninja gate so
/// that the two summaries beside each other say which is which, and so the
/// reason the RSS column did not move to the decile is written where somebody
/// changing one of them will read it.
#[allow(
    dead_code,
    reason = "the Make gate includes this module and measures no memory"
)]
pub fn median_u64(samples: &mut [u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    Some(samples[samples.len() / 2])
}

/// How many repetitions one workload needs, by name.
///
/// Measured rather than chosen: each is the smallest count on the grid at which
/// no run out of 20,000 bootstrapped from a 151-repetition pool refused, and at
/// which the one-in-a-thousand draw stays inside 1.15 of the row's own centre —
/// so at least a quarter of the 1.20 margin is left for a regression to occupy
/// rather than spent on the host.
pub fn repetitions_for(workload: &str) -> usize {
    REPETITIONS
        .iter()
        .find(|(name, _)| *name == workload)
        .map_or(DEFAULT_REPETITIONS, |(_, count)| *count)
}

/// The count for a workload nothing has measured the spread of. The noisiest
/// measured row's count rather than the cheapest, because the failure that
/// matters is a gate that flaps and an unmeasured row is one nobody can say
/// won't.
pub const DEFAULT_REPETITIONS: usize = 31;

/// Per workload: the count, and the one-in-a-thousand bootstrapped draw as a
/// fraction of that row's own centre. The gate refuses at 1.20.
///
/// ```text
///     vim-noop                      31  1.113    clean-tree-noop      21  1.086
///     recursive-noop                31  1.054    deep-graph-evaluation 21  1.113
///     wide-noop                     21  1.133    dependency-log-load  31  1.117
///     zsh-incremental               15  1.084    large-manifest-parse 31  1.131
///     manifest-command-evaluation   21  1.085    path-canonicalization 21  1.118
///     scheduler-barrier             21  1.097    wide-noop-build      21  1.041
/// ```
const REPETITIONS: &[(&str, usize)] = &[
    // The Make gate. `vim-noop` and `recursive-noop` are tens of milliseconds,
    // so thirty-one of each costs seconds; `wide-noop` and `zsh-incremental`
    // are one and two seconds a sample and pay for their own counts.
    ("vim-noop", 31),
    ("recursive-noop", 31),
    ("wide-noop", 21),
    ("zsh-incremental", 15),
    // Recorded rather than gated, and twenty-one seconds a side: this one is
    // reached only under `--clean-build`, where five is already three and a
    // half minutes.
    ("vim-clean-build", 5),
    // The Ninja gate. Every row but one is between 3 and 45 ms across three
    // tools, so these counts are set by the spread and not by the clock.
    ("manifest-command-evaluation", 21),
    ("deep-graph-evaluation", 21),
    ("wide-noop-build", 21),
    ("path-canonicalization", 21),
    ("dependency-log-load", 31),
    ("scheduler-barrier", 21),
    ("clean-tree-noop", 21),
    ("large-manifest-parse", 31),
];

#[cfg(test)]
mod statistic_tests {
    use super::*;

    fn milliseconds(values: &[u64]) -> Vec<Duration> {
        values.iter().copied().map(Duration::from_millis).collect()
    }

    /// The whole point: the tail a busy host adds does not reach the answer.
    /// Half the run can be somebody else's build and the verdict does not move.
    // [spec:ronin:req:performance.reproducible-baseline/test]
    #[test]
    fn a_contended_upper_half_never_reaches_the_answer() {
        let mut quiet = milliseconds(&[10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]);
        let answer = quiet_decile(&mut quiet);

        let mut contended = milliseconds(&[10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]);
        for sample in contended.iter_mut().skip(6) {
            *sample = Duration::from_millis(900);
        }
        assert_eq!(quiet_decile(&mut contended), answer);
    }

    /// And the statistic is CONSISTENT, which is why it is a decile and not a
    /// minimum: the same distribution answers the same number whether the run
    /// took eleven samples or a hundred and one. A minimum would fall with
    /// every extra sample and make the recorded row depend on the count.
    // [spec:ronin:req:performance.reproducible-baseline/test]
    #[test]
    fn the_answer_holds_at_any_sample_count() {
        // The same distribution, 100 ms to 200 ms, sampled eleven times and a
        // hundred and one times. Its tenth percentile is 110 ms either way.
        let mut short = milliseconds(&(0..11).map(|index| 100 + index * 10).collect::<Vec<_>>());
        let mut long = milliseconds(&(0..101).map(|index| 100 + index).collect::<Vec<_>>());
        assert_eq!(quiet_decile(&mut short), Duration::from_millis(110));
        assert_eq!(quiet_decile(&mut long), Duration::from_millis(110));
    }

    /// A single sample is still an answer rather than a panic on an index.
    // [spec:ronin:req:performance.reproducible-baseline/test]
    #[test]
    fn one_sample_answers_itself() {
        let mut one = vec![Duration::from_millis(7)];
        assert_eq!(quiet_decile(&mut one), Duration::from_millis(7));
    }

    /// An unmeasured workload gets the careful count, not the cheap one.
    // [spec:ronin:req:performance.reproducible-baseline/test]
    #[test]
    fn an_unlisted_workload_takes_the_careful_count() {
        assert_eq!(repetitions_for("vim-noop"), 31);
        assert_eq!(repetitions_for("zsh-incremental"), 15);
        assert_eq!(
            repetitions_for("something-nobody-measured"),
            DEFAULT_REPETITIONS
        );
    }
}
