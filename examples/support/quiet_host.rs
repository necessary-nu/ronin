//! The quiet-host guard both wall-time gates run behind.
//!
//! An absolute wall time measured on a busy machine is not a measurement, and
//! a gate that records one anyway is worse than no gate: it teaches everyone to
//! ignore it. A RATIO measured on a busy machine still is one, because both
//! tools meet that machine interleaved. The guard is therefore two numbers:
//! [`DEFAULT_MAX_LOAD`], which it refuses above, and [`QUIET_LOAD`], below
//! which the milliseconds may be quoted too. Every record carries
//! [`provenance`] saying which of the two that run earned.
//!
//! This lived in `examples/make_baseline.rs` alone, which is how the Ninja gate
//! came to have no guard at all — it measured whatever the host was doing and
//! reported the result as a verdict. One implementation, both gates, so the
//! next guard either gate grows is a guard both of them have.

use std::cmp::Ordering;
use std::fs;
use std::thread;
use std::time::{Duration, Instant};

/// The load below which a wall time is quotable on its own.
///
/// An absolute figure rather than a fraction of the core count, and that is
/// deliberate on a machine with many cores: what a short workload competes for
/// is not only CPU but the last-level cache and the memory bus, which do not
/// scale with the core count the way a load average does. A host with 4.0 of
/// somebody else's work on it is already a host whose milliseconds are not
/// about the tree.
pub const QUIET_LOAD: f64 = 4.0;

/// Default ceiling on the one-minute load average.
///
/// Well above [`QUIET_LOAD`], because a ceiling at the quiet threshold refuses
/// on every host that has anything else to do. This one runs on a 32-core
/// machine sharing its cores with another tenant between 5 and 78, typically 8
/// to 20; 20.00 measures in that ordinary condition and still refuses a swamped
/// machine. Waiting is not an alternative — [`QUIET_HOST_PATIENCE`] waits out a
/// load the gate's own dependencies raised, not one belonging to somebody else.
///
/// Measuring here is sound because every threshold either gate validates is a
/// RATIO — Ronin against its recorded ratio, against the pinned reference, and
/// its peak RSS against that reference — and both tools are sampled interleaved
/// with a rotating offset, so they meet the same busy machine. The millisecond
/// printed beside the ratio is not sound here, which is what [`provenance`]
/// exists to say.
pub const DEFAULT_MAX_LOAD: f64 = 20.0;

/// Whether a run measured at this load may be quoted only as ratios.
///
/// `NaN` — a kernel that publishes no average — counts as not quiet, because a
/// run that could not look is not a run that looked and found the host idle.
pub fn ratio_only(load: f64) -> bool {
    // Three answers rather than two, which is why this is not a `<=`: a load
    // that orders against neither side is `NaN`, and it belongs on the
    // ratio-only branch with the loads that are simply too high.
    !matches!(
        load.partial_cmp(&QUIET_LOAD),
        Some(Ordering::Less | Ordering::Equal)
    )
}

/// What a reader of the record is allowed to quote from this run.
///
/// Both gates already print the load they measured at and the ceiling they
/// allowed. Neither says what the two together mean, and a reader who has to
/// derive it will quote the milliseconds.
pub fn provenance(load: f64) -> String {
    if ratio_only(load) {
        format!(
            "ratios only; the one-minute load average was {load:.2}, above the {QUIET_LOAD:.2} \
             at which a wall time is about the tree rather than about the host. Both tools met \
             this machine interleaved, so the RATIOS below are the verdict and the milliseconds \
             beside them must not be quoted as absolute performance or recorded as a baseline"
        )
    } else {
        format!(
            "quotable; the one-minute load average was {load:.2}, at or below the \
             {QUIET_LOAD:.2} this gate calls quiet, so the milliseconds below are about the tree"
        )
    }
}

/// How long to wait for the machine to go quiet before giving up on it.
///
/// Generous, because of the load these gates usually inherit:
/// `scripts/check-release.sh` runs a `-j8` build of vim and zsh a few lines
/// above them, and the one-minute average takes about this long to decay back
/// through [`DEFAULT_MAX_LOAD`]. A gate that refused the moment it inherited the
/// load average of the work it depends on would fail every release run, and a
/// gate that fails every run gets deleted. It buys nothing against a load that
/// is somebody else's and does not decay, which is why the ceiling sits where
/// it does rather than at [`QUIET_LOAD`].
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_load_earns_its_milliseconds() {
        assert!(!ratio_only(0.0));
        assert!(!ratio_only(QUIET_LOAD));
        assert!(provenance(1.5).starts_with("quotable;"));
    }

    #[test]
    fn a_busy_load_earns_only_its_ratios() {
        assert!(ratio_only(QUIET_LOAD + 0.01));
        assert!(ratio_only(DEFAULT_MAX_LOAD));
        let line = provenance(19.28);
        assert!(line.starts_with("ratios only;"));
        assert!(line.contains("19.28"));
        assert!(line.contains("must not be quoted"));
    }

    /// A host whose kernel publishes no load average is not a quiet host. The
    /// guard hands `NaN` to the record for exactly this case, and `NaN`
    /// compares to neither side of the threshold, so the answer has to come
    /// from the ordering being absent rather than from an inequality.
    #[test]
    fn an_unreadable_load_is_not_a_quiet_one() {
        assert!(ratio_only(f64::NAN));
        assert!(provenance(f64::NAN).starts_with("ratios only;"));
    }

    /// The ceiling sits above the threshold at which a wall time stops being
    /// about the tree, which is the whole of what this change is. A future
    /// edit that lowers the ceiling back through it would make the ratio-only
    /// branch unreachable and the record's provenance line a constant.
    #[test]
    fn the_ceiling_leaves_room_above_the_quiet_threshold() {
        assert!(DEFAULT_MAX_LOAD > QUIET_LOAD);
    }
}
