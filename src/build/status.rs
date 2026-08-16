use super::{BuildOptions, Plan};
use crate::graph::{EdgeId, Graph};
use crate::log::BuildLog;
use std::fmt::Write as _;
use std::fs;
use std::time::{Duration, Instant};

const LOAD_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);

// [spec:ronin:req:runtime.process-supervisor-scalability]
pub(super) struct LoadSampler {
    sampled_at: Option<Instant>,
    value: f64,
}

impl Default for LoadSampler {
    fn default() -> Self {
        Self {
            sampled_at: None,
            value: 0.0,
        }
    }
}

impl LoadSampler {
    pub(super) fn current(&mut self) -> f64 {
        self.sample_with(Instant::now(), queryload)
    }

    fn sample_with(&mut self, now: Instant, read: impl FnOnce() -> f64) -> f64 {
        if self
            .sampled_at
            .is_some_and(|sampled_at| now.duration_since(sampled_at) < LOAD_SAMPLE_INTERVAL)
        {
            return self.value;
        }
        self.value = read();
        self.sampled_at = Some(now);
        self.value
    }
}

pub(crate) struct BuildState {
    pub(crate) started: usize,
    pub(crate) finished: usize,
    pub(crate) total: usize,
    pub(crate) start: Instant,
    /// Time spent by the commands this run has finished.
    pub(crate) spent_millis: i64,
    /// Edges whose previous duration is known, and the sum of those durations.
    ///
    /// "Predictable" is Ninja's word for an edge the build log has timed
    /// before. Progress is weighted by these rather than counted by edge,
    /// because one link step can be worth a hundred compiles and a bar that
    /// says a quarter done when the expensive quarter is behind it is wrong
    /// in the direction people notice.
    pub(crate) predictable_total: usize,
    pub(crate) predictable_remaining: usize,
    pub(crate) predictable_millis_total: i64,
    pub(crate) predictable_millis_remaining: i64,
    pub(crate) unpredictable_remaining: usize,
    /// Share of predicted total work completed, as Ninja's `%P` reports it.
    pub(crate) predicted_fraction: f64,
}

impl BuildState {
    pub(crate) fn new(_options: BuildOptions) -> Self {
        Self {
            started: 0,
            finished: 0,
            total: 0,
            start: Instant::now(),
            spent_millis: 0,
            predictable_total: 0,
            predictable_remaining: 0,
            predictable_millis_total: 0,
            predictable_millis_remaining: 0,
            unpredictable_remaining: 0,
            predicted_fraction: 0.0,
        }
    }

    /// Milliseconds since the build began, as `.ninja_log` records them.
    pub(crate) fn offset_millis(&self) -> i32 {
        i32::try_from(self.start.elapsed().as_millis()).unwrap_or(i32::MAX)
    }

    /// Note that an edge with a known previous duration joined the plan.
    pub(crate) const fn expect_timed_edge(&mut self, previous_millis: i64) {
        self.predictable_total += 1;
        self.predictable_remaining += 1;
        self.predictable_millis_total += previous_millis;
        self.predictable_millis_remaining += previous_millis;
    }

    /// Note that an edge the log has never timed joined the plan.
    pub(crate) const fn expect_untimed_edge(&mut self) {
        self.unpredictable_remaining += 1;
    }

    /// Note that an edge left the plan without running, given what the log says
    /// it cost last time.
    ///
    /// The exact mirror of the two `expect_` calls, because Ninja's
    /// `EdgeRemovedFromPlan` is the exact mirror of `EdgeAddedToPlan`: the
    /// count of work loses the edge and so does the weighting that predicts
    /// how much of the work is left. Leaving the prediction behind would have
    /// `%P` chasing a total that no longer exists.
    pub(crate) const fn forget_edge(&mut self, previous_millis: Option<i64>) {
        self.total = self.total.saturating_sub(1);
        if let Some(previous) = previous_millis {
            self.predictable_total = self.predictable_total.saturating_sub(1);
            self.predictable_remaining = self.predictable_remaining.saturating_sub(1);
            self.predictable_millis_total -= previous;
            self.predictable_millis_remaining -= previous;
        } else {
            self.unpredictable_remaining = self.unpredictable_remaining.saturating_sub(1);
        }
    }

    /// Account for an edge that just finished, given what it cost last time.
    pub(crate) const fn retire_edge(&mut self, elapsed: i64, previous_millis: Option<i64>) {
        self.spent_millis += elapsed;
        if let Some(previous) = previous_millis {
            self.predictable_remaining = self.predictable_remaining.saturating_sub(1);
            self.predictable_millis_remaining -= previous;
        } else {
            self.unpredictable_remaining = self.unpredictable_remaining.saturating_sub(1);
        }
    }
}

/// How long the build log says an edge took last time, if it says.
///
/// Ninja looks at the edge's outputs in order and takes the first that has
/// a log entry, so an edge with several outputs is timed by whichever was
/// recorded, not by all of them.
pub(crate) fn previous_duration(
    graph: &Graph,
    log: Option<&BuildLog>,
    edge: EdgeId,
) -> Option<i64> {
    let log = log?;
    graph.edge(edge).out.iter().find_map(|output| {
        let entry = crate::log::logentry(log, graph.node_path(*output))?;
        Some(i64::from(entry.end_time) - i64::from(entry.start_time))
    })
}

/// Take command edges the plan pruned out of the work the progress line
/// counts, so `[N/M]` measures what is going to run rather than what was
/// planned before the build learned better.
// [spec:ronin:sem:build.nodedone-fn]
pub(crate) fn forget_pruned_work(
    state: &mut BuildState,
    graph: &Graph,
    log: Option<&BuildLog>,
    edges: &[EdgeId],
) {
    for &edge in edges {
        state.forget_edge(previous_duration(graph, log, edge));
    }
}

/// Tell the progress state what the plan is expected to cost.
pub(crate) fn seed_prediction(
    state: &mut BuildState,
    plan: &Plan,
    graph: &Graph,
    log: Option<&BuildLog>,
) {
    for edge in plan.command_edges(graph) {
        match previous_duration(graph, log, edge) {
            Some(previous) => state.expect_timed_edge(previous),
            None => state.expect_untimed_edge(),
        }
    }
}

impl Plan {
    pub(crate) fn command_edge_count(&self, graph: &Graph) -> usize {
        self.command_edges(graph).count()
    }

    /// Every planned edge that will actually run a command.
    pub(crate) fn command_edges<'a>(
        &'a self,
        graph: &'a Graph,
    ) -> impl Iterator<Item = EdgeId> + 'a {
        self.wanted
            .iter()
            .zip(graph.edge_ids())
            .filter(|(wanted, edge)| {
                let rule = graph.edge(*edge).rule;
                **wanted && rule.is_some() && !graph.is_phony_rule(rule)
            })
            .map(|(_, edge)| edge)
    }
}

/// Ninja will not trust the previous build's timings until this much of the
/// current one has run, and then only if they broadly agree with it.
const PREDICTION_TRUST_DELAY: Duration = Duration::from_secs(15);
const PREDICTION_TRUST_FRACTION: f64 = 0.05;
/// How far the previous build's average may be from this one's before its
/// timings are discarded — a ccache hit last time and a miss this time can
/// differ by orders of magnitude, and a stale prediction is worse than none.
const PREDICTION_TRUST_RATIO: f64 = 10.0;

/// Recompute the share of predicted work that is done.
///
/// This is Ninja's `RecalculateProgressPrediction`. Weighting by recorded
/// duration rather than by edge count is what makes `%P`, `%E` and `%W` mean
/// what Ninja documents them to mean: a build of one long link and three quick
/// compiles is nearly finished once the link lands, and counting edges would
/// call it a quarter done.
// [spec:ronin:req:compat.command-runtime]
#[allow(
    clippy::cast_precision_loss,
    reason = "prediction is a deliberately approximate floating-point metric"
)]
pub(crate) fn recalculate_prediction(state: &mut BuildState) {
    state.predicted_fraction = 0.0;
    let elapsed = state.start.elapsed();
    let mut use_previous =
        state.predictable_remaining != 0 && state.predictable_millis_remaining != 0;
    // Only second-guess the previous build once this one has enough of its own
    // evidence to be worth comparing against.
    if use_previous
        && state.total != 0
        && state.finished != 0
        && elapsed >= PREDICTION_TRUST_DELAY
        && (state.finished as f64 / state.total as f64) >= PREDICTION_TRUST_FRACTION
    {
        let actual_average = state.spent_millis as f64 / state.finished as f64;
        let previous_average =
            state.predictable_millis_total as f64 / state.predictable_total as f64;
        let ratio = actual_average.max(previous_average)
            / actual_average.min(previous_average).max(f64::MIN_POSITIVE);
        use_previous = ratio < PREDICTION_TRUST_RATIO;
    }

    let known_edges = state.finished
        + if use_previous {
            state.predictable_remaining
        } else {
            0
        };
    if known_edges == 0 {
        return;
    }
    let unknown_edges = if use_previous {
        state.unpredictable_remaining
    } else {
        state.total.saturating_sub(state.finished)
    };
    let known_millis = state.spent_millis
        + if use_previous {
            state.predictable_millis_remaining
        } else {
            0
        };
    let average = known_millis as f64 / known_edges as f64;
    let mut remaining = average * unknown_edges as f64;
    if use_previous {
        remaining += state.predictable_millis_remaining as f64;
    }
    let predicted_total = state.spent_millis as f64 + remaining;
    if predicted_total == 0.0 {
        return;
    }
    state.predicted_fraction = state.spent_millis as f64 / predicted_total;
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "Ninja status rates are deliberately approximate floating-point metrics"
)]
pub(crate) fn format_progress_status(state: &BuildState, template: &str) -> String {
    let mut output = String::new();
    let mut characters = template.chars();
    let elapsed = state.start.elapsed().as_secs_f64();
    let format_duration = |seconds: f64| {
        let seconds = std::time::Duration::try_from_secs_f64(seconds.max(0.0))
            .map_or(u64::MAX, |duration| duration.as_secs());
        let hours = seconds / 3600;
        let minutes = seconds % 3600 / 60;
        let seconds = seconds % 60;
        if hours == 0 {
            format!("{minutes:02}:{seconds:02}")
        } else {
            format!("{hours}:{minutes:02}:{seconds:02}")
        }
    };
    while let Some(character) = characters.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        let Some(code) = characters.next() else {
            output.push('%');
            break;
        };
        match code {
            '%' => output.push('%'),
            's' => output.push_str(&state.started.to_string()),
            'f' => output.push_str(&state.finished.to_string()),
            't' => output.push_str(&state.total.to_string()),
            'r' => output.push_str(&(state.started - state.finished).to_string()),
            'u' => output.push_str(&(state.total - state.started).to_string()),
            'p' => {
                let percentage = (100 * state.finished).checked_div(state.total).unwrap_or(0);
                let _ = write!(output, "{percentage:3}%");
            }
            // `%p` counts edges; `%P` weighs them by how long they take.
            'P' => {
                let percentage = (100.0 * state.predicted_fraction) as i64;
                let _ = write!(output, "{percentage:3}%");
            }
            'o' | 'c' => {
                let rate = if state.finished == 0 {
                    0.0
                } else {
                    state.finished as f64 / elapsed.max(f64::EPSILON)
                };
                let _ = write!(output, "{rate:.1}");
            }
            'e' => {
                let _ = write!(output, "{elapsed:.3}");
            }
            // The estimate is the elapsed time scaled by how much of the
            // predicted work it bought, so an unpredictable build says so with
            // a question mark rather than inventing a number.
            'E' | 'W' => {
                if state.predicted_fraction == 0.0 {
                    output.push('?');
                } else {
                    let predicted_total = elapsed / state.predicted_fraction;
                    let estimate = predicted_total - elapsed;
                    if code == 'E' {
                        let _ = write!(output, "{estimate:.3}");
                    } else {
                        output.push_str(&format_duration(estimate));
                    }
                }
            }
            'w' => output.push_str(&format_duration(elapsed)),
            _ => {
                output.push('%');
                output.push(code);
            }
        }
    }
    output
}

fn queryload() -> f64 {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|contents| contents.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    // [spec:ronin:req:runtime.process-supervisor-scalability/test]
    fn load_sampler_reuses_recent_observations() {
        let mut sampler = LoadSampler::default();
        let reads = Cell::new(0);
        let start = Instant::now();
        let read = || {
            reads.set(reads.get() + 1);
            f64::from(reads.get())
        };

        assert!((sampler.sample_with(start, read) - 1.0).abs() < f64::EPSILON);
        assert!(
            (sampler.sample_with(start + LOAD_SAMPLE_INTERVAL / 2, read) - 1.0).abs()
                < f64::EPSILON
        );
        assert!(
            (sampler.sample_with(start + LOAD_SAMPLE_INTERVAL, read) - 2.0).abs() < f64::EPSILON
        );
        assert_eq!(reads.get(), 2);
    }
}
