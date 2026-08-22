//! What the compiler is told about the user having stopped the read.
//!
//! Ronin's side of `kati::interrupt`: the fork asks a session it was given
//! whether the read has been stopped, and this is what answers. Its own file
//! rather than a corner of `make.rs` because it is the whole of one seam — the
//! signal runtime on this side, an evaluator that runs processes on the other —
//! and because the module it came out of is the compilation entry point rather
//! than a home for host facilities.

/// The answer itself, and the two things it is made of.
///
/// The read phase runs processes — a `$(shell)` call is a command line like any
/// other — and a compiler that waits for one of them is a tool that ignores
/// Ctrl-C for as long as the makefile's slowest shell function takes. The
/// evaluator asks this before launching a command and while waiting for one,
/// and stops when it answers yes.
///
/// The flag is the process-wide one the signal handlers set, because the signal
/// is process-wide; the descriptor beside it is the same wake pipe the process
/// supervisor waits on, polled here and never read from, so an interrupt ends
/// the wait as soon as the handler writes rather than on the next poll interval.
// [spec:ronin:req:make.read-interrupt]
pub(super) struct ReadInterrupts {
    /// Readable once a handled signal has arrived. `None` when no handlers are
    /// installed — a library caller compiling a Makefile — which leaves the
    /// interval as the only wake and is still correct.
    wake: Option<std::os::unix::net::UnixStream>,
}

impl ReadInterrupts {
    pub(super) fn installed() -> std::sync::Arc<dyn kati::interrupt::Interruptible> {
        std::sync::Arc::new(Self {
            wake: crate::signal::wake_reader().ok().flatten(),
        })
    }
}

impl kati::interrupt::Interruptible for ReadInterrupts {
    fn interrupted(&self) -> bool {
        crate::signal::interrupted().is_some()
    }

    fn wake(&self) -> Option<std::os::unix::io::BorrowedFd<'_>> {
        use std::os::unix::io::AsFd as _;
        self.wake
            .as_ref()
            .map(std::os::unix::net::UnixStream::as_fd)
    }
}
