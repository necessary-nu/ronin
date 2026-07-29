use super::BuildState;
use std::fs;

pub(crate) fn format_progress_status(state: &BuildState, template: &str) -> String {
    let mut output = String::new();
    let mut characters = template.chars();
    let elapsed = state.start.elapsed().as_secs_f64();
    let format_duration = |seconds: f64| {
        let seconds = seconds.max(0.0) as u64;
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
            'p' => output.push_str(&format!(
                "{:3}%",
                if state.total == 0 {
                    0
                } else {
                    100 * state.finished / state.total
                }
            )),
            'o' | 'c' => output.push_str(&format!(
                "{:.1}",
                if state.finished == 0 {
                    0.0
                } else {
                    state.finished as f64 / elapsed.max(f64::EPSILON)
                }
            )),
            'e' => output.push_str(&format!("{elapsed:.3}")),
            'E' | 'W' => {
                if state.finished == 0 {
                    output.push('?');
                } else {
                    let remaining = state.total.saturating_sub(state.finished);
                    let estimate = elapsed * remaining as f64 / state.finished as f64;
                    if code == 'E' {
                        output.push_str(&format!("{estimate:.3}"));
                    } else {
                        output.push_str(&format_duration(estimate));
                    }
                }
            }
            'w' => output.push_str(&format_duration(elapsed)),
            'P' => output.push_str(&format!(
                "{:3}%",
                if state.total == 0 {
                    0
                } else {
                    100 * state.finished / state.total
                }
            )),
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
