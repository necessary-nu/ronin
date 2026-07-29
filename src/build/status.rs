use super::BuildState;
use std::fmt::Write as _;
use std::fs;

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

pub(super) fn queryload() -> f64 {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|contents| contents.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0)
}
