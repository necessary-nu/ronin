use super::BuildState;
use std::fmt::Write as _;
use std::fs;
use std::time::{Duration, Instant};

const LOAD_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);

// [spec:samurai:req:runtime.process-supervisor-scalability]
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

#[allow(
    clippy::cast_precision_loss,
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
            'p' | 'P' => {
                let percentage = if state.total == 0 {
                    0
                } else {
                    100 * state.finished / state.total
                };
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
            'E' | 'W' => {
                if state.finished == 0 {
                    output.push('?');
                } else {
                    let remaining = state.total.saturating_sub(state.finished);
                    let estimate = elapsed * remaining as f64 / state.finished as f64;
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
    // [spec:samurai:req:runtime.process-supervisor-scalability/test]
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
