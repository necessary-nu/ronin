//! A scratch directory that goes away with the test that made it.
//!
//! Included by `#[path]` rather than shared through a crate, because an
//! integration test is its own crate and `tests/support/` is not a target
//! cargo builds on its own — the same arrangement `support/oracle.rs` uses.
//!
//! A suite that names a directory after its own process and leaves it behind
//! accumulates one per case per run: pids rotate, nothing collects them, and
//! the only symptom is a `/tmp` that reads as a leak in the tool under test.
//! Two suites had that defect independently, so the answer lives in one place.

use std::path::Path;

/// A scratch directory of this test's own.
///
/// Held rather than returned as a path, because the directory lives exactly as
/// long as this value does: a case that took the path and dropped the handle
/// would be working in a directory that had already been removed. It stands in
/// for a `&Path` everywhere a path is wanted, so a case reads the same as it
/// did when the directory was named and left behind.
pub struct Scratch(tempfile::TempDir);

impl Scratch {
    /// A scratch directory under `TMPDIR` whose name STARTS with `prefix`.
    ///
    /// A prefix rather than the whole name is what removes the collision the
    /// per-pid naming had to defend against with an up-front removal: two runs
    /// at once cannot land in one directory, so there is nothing to clear.
    pub fn named(prefix: &str) -> Self {
        Self(
            tempfile::Builder::new()
                .prefix(prefix)
                .tempdir()
                .expect("a scratch directory under TMPDIR"),
        )
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.0.path()
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}

/// The harness's own promise, gated because nothing else would notice it
/// breaking: the removal is a drop rather than a statement at the end of a
/// case, so a case that fails — or returns early — still takes its directory
/// with it, and that is exactly the run whose leavings used to survive.
///
/// Written against BEHAVIOUR rather than against the type, so it compiles
/// against a helper that returns a bare path and fails on what that helper
/// does.
#[test]
fn a_scratch_goes_with_its_test() {
    let path = {
        let directory = Scratch::named("ronin-scratch-self-removing-");
        assert!(directory.is_dir(), "{}", directory.display());
        directory.to_path_buf()
    };
    assert!(
        !path.exists(),
        "the scratch outlived the test that made it: {}",
        path.display()
    );
}
