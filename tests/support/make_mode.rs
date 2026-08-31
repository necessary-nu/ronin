//! Reaching Make mode from an integration test, and measuring what it ran.
//!
//! Included by `#[path]` rather than shared through a crate, because an
//! integration test is its own crate and `tests/support/` is not a target
//! cargo builds on its own — the same arrangement `support/scratch.rs` and
//! `support/oracle.rs` use. `Scratch` is re-exported from here so a suite that
//! wants both includes one file and gets one type: two `#[path]` copies of the
//! same module in one crate are two different types, and a scratch directory
//! from one would not stand in for a scratch directory from the other.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "scratch.rs"]
mod scratch_directory;

pub use scratch_directory::Scratch;

/// A scratch directory of this test's own, which goes away with the test.
pub fn test_directory(label: &str) -> Scratch {
    Scratch::named(&format!("ronin-{label}-"))
}

/// A `make`-named link to this build's binary, inside `directory`.
///
/// Make mode is selected by the invoked name and by nothing else, so a test
/// that wants it has to reach the binary under that name.
#[cfg(all(unix, feature = "make"))]
pub fn invoked_as(directory: &Path, name: &str) -> PathBuf {
    let link = directory.join(name);
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ronin"), &link).unwrap();
    link
}

/// A Make invocation with the environment a parent Make would have left.
///
/// All four names cleared, not just `MAKEFLAGS`: a suite run from a `make`
/// or a `cargo` would otherwise hand every case an inherited budget and an
/// inherited switch table, and a test measuring either would be measuring the
/// harness.
#[cfg(all(unix, feature = "make"))]
pub fn make_command(program: &Path, directory: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(directory)
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKELEVEL");
    command
}

/// The peak number of work units alive at once, from a log of start and end
/// stamps, and how many units the log recorded.
#[cfg(unix)]
pub fn peak_concurrency(log: &str) -> (usize, usize) {
    let mut events = log
        .lines()
        .filter_map(|line| {
            let (kind, stamp) = line.split_once(' ')?;
            let start = kind == "S";
            // Ends sort before starts at an identical stamp, so a unit that
            // finishes exactly as another begins is not counted as an overlap.
            Some((stamp.parse::<f64>().ok()?, i32::from(start), start))
        })
        .collect::<Vec<_>>();
    events.sort_by(|left, right| left.partial_cmp(right).expect("stamps are finite"));
    let (mut live, mut peak, mut started) = (0, 0, 0);
    for (_, _, start) in events {
        if start {
            live += 1;
            started += 1;
            peak = peak.max(live);
        } else {
            live -= 1;
        }
    }
    (peak, started)
}
