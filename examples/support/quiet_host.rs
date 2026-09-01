//! The quiet-host guard both wall-time gates run behind.
//!
//! Wall time measured on a busy machine is not a measurement, and a gate that
//! records one anyway is worse than no gate: it teaches everyone to ignore it.
//!
//! This lived in `examples/make_baseline.rs` alone, which is how the Ninja gate
//! came to have no guard at all — it measured whatever the host was doing and
//! reported the result as a verdict. One implementation, both gates, so the
//! next guard either gate grows is a guard both of them have.

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

/// Default ceiling on the one-minute load average.
///
/// An absolute figure rather than a fraction of the core count, and that is
/// deliberate on a machine with many cores: what a short workload competes for
/// is not only CPU but the last-level cache and the memory bus, which do not
/// scale with the core count the way a load average does. A host with 4.0 of
/// somebody else's work on it is already a host whose milliseconds are not
/// about the tree.
///
/// A run that genuinely has to measure above it raises `--max-load` and says
/// so in the record; the refusal message asks for exactly that. The ratios
/// survive a raise — both tools are sampled interleaved into the same
/// competition — but the milliseconds beside them do not, and only the record
/// can say which of the two a reader is allowed to quote.
pub const DEFAULT_MAX_LOAD: f64 = 4.0;

/// How long to wait for the machine to go quiet before giving up on it.
///
/// Generous, because of the load these gates usually inherit:
/// `scripts/check-release.sh` runs a `-j8` build of vim and zsh a few lines
/// above them, and the one-minute average takes about this long to decay back
/// through 4.0. A gate that refused the moment it inherited the load average of
/// the work it depends on would fail every release run, and a gate that fails
/// every run gets deleted.
pub const QUIET_HOST_PATIENCE: Duration = Duration::from_mins(5);

/// How often to look while waiting. Long enough that the watching costs
/// nothing, short enough that a run does not sit idle after the machine has
/// already gone quiet.
pub const QUIET_HOST_POLL: Duration = Duration::from_secs(5);

/// The one-minute load average, or `None` where the kernel does not publish
/// one.
pub fn load_average() -> Option<f64> {
    let loadavg = fs::read_to_string("/proc/loadavg").ok()?;
    loadavg.split_whitespace().next()?.parse().ok()
}

/// Wait for a quiet machine, and refuse if one does not arrive.
///
/// Checked only before sampling, and that is not an oversight. Once sampling
/// has begun the one-minute average includes the harness's own workloads — a
/// `-j8` clean build of vim drives it past any threshold worth setting by
/// itself — so the reading afterwards measures the gate rather than the
/// competition for the machine. It is recorded for the reader and not gated on.
///
/// Returns the load the run went on to measure at, or `NAN` where the kernel
/// publishes no average, so the caller can put it in the record.
pub fn require_quiet_host(gate: &str, max_load: f64) -> Result<f64, String> {
    let Some(mut load) = load_average() else {
        return Ok(f64::NAN);
    };
    let deadline = Instant::now() + QUIET_HOST_PATIENCE;
    let mut waited = false;
    // `while load > max` reads as a float comparison in a loop condition,
    // which the lint is right to question in general and which is exactly
    // what is wanted here: the average is a float and the ceiling is a float,
    // and the loop ends when one falls below the other.
    loop {
        if load <= max_load {
            return Ok(load);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "one-minute load average is still {load:.2} after waiting {} s, above the \
                 {max_load:.2} this gate will measure at. Wall time from a busy machine is not \
                 a measurement. Wait for the host to go quiet, or raise --max-load deliberately \
                 and say so in the record.",
                QUIET_HOST_PATIENCE.as_secs(),
            ));
        }
        if !waited {
            eprintln!(
                "{gate}: load average is {load:.2}, waiting for it to fall below {max_load:.2}"
            );
            waited = true;
        }
        thread::sleep(QUIET_HOST_POLL);
        load = load_average().unwrap_or(load);
    }
}
