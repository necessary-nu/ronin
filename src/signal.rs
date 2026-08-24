//! Typed Unix interruption handling and process-signal delivery.

use std::io;

/// An interruption signal handled and forwarded by Ronin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Signal {
    /// Terminal hangup (`SIGHUP`).
    Hangup,
    /// Interactive interrupt (`SIGINT`).
    Interrupt,
    /// Interactive quit (`SIGQUIT`).
    Quit,
    /// Termination request (`SIGTERM`).
    Terminate,
}

/// What a tool that caught `SIGQUIT` leaves with, rather than dying of a signal
/// whose default action writes a core file.
///
/// GNU Make's `MAKE_TROUBLE`, and the same number Ninja spends on
/// `ExitFailure`: the two agree that this is trouble without agreeing on why,
/// which is all a status can say once the signal has been declined.
pub const QUIT_EXIT_CODE: i32 = 1;

impl std::fmt::Display for Signal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Hangup => "SIGHUP",
            Self::Interrupt => "SIGINT",
            Self::Quit => "SIGQUIT",
            Self::Terminate => "SIGTERM",
        })
    }
}

impl Signal {
    #[cfg(unix)]
    const ALL: [Self; 4] = [Self::Hangup, Self::Interrupt, Self::Quit, Self::Terminate];

    #[cfg(unix)]
    const fn os_signal(self) -> rustix::process::Signal {
        match self {
            Self::Hangup => rustix::process::Signal::HUP,
            Self::Interrupt => rustix::process::Signal::INT,
            Self::Quit => rustix::process::Signal::QUIT,
            Self::Terminate => rustix::process::Signal::TERM,
        }
    }

    #[cfg(unix)]
    fn atomic_value(self) -> usize {
        usize::try_from(self.os_signal().as_raw())
            .expect("supported Unix signal numbers are positive")
    }

    #[cfg(unix)]
    pub(crate) fn from_raw(raw: usize) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|signal| signal.atomic_value() == raw)
    }

    /// Leaves the way a build tool that caught this signal leaves.
    ///
    /// The disposition is restored and the signal raised again, so the process
    /// dies of the signal it was sent and a shell reads 128 + the signal
    /// number: 130 for `SIGINT`, 143 for `SIGTERM`, 129 for `SIGHUP`. The
    /// status then says which signal stopped the build rather than only that
    /// something did, and a caller that distinguishes a Ctrl-C from a
    /// supervisor's termination can. The `exit` below is unreachable while the
    /// disposition can be restored, and carries the same number for a platform
    /// where it cannot.
    ///
    /// `SIGQUIT` is the exception, and it is the one GNU Make writes out in as
    /// many words (`commands.c`): "We don't want to send ourselves SIGQUIT,
    /// because it will cause a core dump. Just exit instead." A build tool that
    /// dumped core because the user quit it would write a core file the size of
    /// its address space for something that is not a fault, so the signal is
    /// not re-raised and the status is the plain trouble status GNU Make leaves
    /// there — `MAKE_TROUBLE`, which is 1. Measured against GNU Make 4.4.1 on
    /// 2026-08-24: `SIGQUIT` mid-recipe, during the read, and under `-q` all
    /// leave 1, with no core file and no signal in the wait status.
    // [spec:ronin:req:product.build-outcome+1]
    pub fn die_of(self) -> ! {
        #[cfg(unix)]
        {
            if self == Self::Quit {
                std::process::exit(QUIT_EXIT_CODE);
            }
            let raw = self.os_signal().as_raw();
            let _ = signal_hook::low_level::emulate_default_handler(raw);
            std::process::exit(128 + raw);
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            std::process::exit(1);
        }
    }
}

/// A handle to Ronin's executable-lifetime signal installation.
///
/// The underlying handlers intentionally remain installed until process exit.
/// Keep this guard in the executable scope to make that ownership explicit.
#[must_use = "keep the signal guard alive for the executable's lifetime"]
pub struct SignalHandlers {
    #[cfg(unix)]
    runtime: &'static unix::Runtime,
}

impl SignalHandlers {
    /// Returns the most recently observed interruption signal.
    #[must_use]
    pub fn interrupted(&self) -> Option<Signal> {
        #[cfg(unix)]
        {
            self.runtime.interrupted()
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
}

/// Installs Ronin's executable-lifetime interruption handlers.
///
/// Calling this more than once reuses the one process-global installation.
/// Install it before starting worker threads or invoking [`crate::run_os`].
///
/// # Errors
///
/// Returns an operating-system error if the readiness channel or any signal
/// registration cannot be installed. Known partial registrations are removed
/// before the error is returned.
// [spec:ronin:req:runtime.guarded-signal-boundary]
pub fn install_signal_handlers() -> io::Result<SignalHandlers> {
    #[cfg(unix)]
    {
        let runtime = unix::runtime()?;
        Ok(SignalHandlers { runtime })
    }
    #[cfg(not(unix))]
    {
        Ok(SignalHandlers {})
    }
}

pub(crate) fn interrupted() -> Option<Signal> {
    #[cfg(unix)]
    {
        unix::installed_runtime().and_then(unix::Runtime::interrupted)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
pub(crate) fn wake_reader() -> io::Result<Option<std::os::unix::net::UnixStream>> {
    unix::installed_runtime()
        .map(unix::Runtime::clone_wake_reader)
        .transpose()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Delivery {
    Delivered,
    ProcessGone,
}

pub(crate) fn forward(pid: u32, process_group: bool, signal: Signal) -> io::Result<Delivery> {
    #[cfg(unix)]
    {
        unix::deliver(pid, process_group, signal.os_signal())
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, process_group, signal);
        Ok(Delivery::Delivered)
    }
}

#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) -> io::Result<Delivery> {
    unix::deliver(pid, true, rustix::process::Signal::KILL)
}

#[cfg(unix)]
mod unix {
    use super::{Delivery, Signal};
    use signal_hook::SigId;
    use std::io;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    static INSTALL_LOCK: Mutex<()> = Mutex::new(());

    pub(super) struct Runtime {
        interrupted: Arc<AtomicUsize>,
        wake_reader: UnixStream,
        _registrations: Box<[SigId]>,
    }

    impl Runtime {
        fn install() -> io::Result<Self> {
            let interrupted = Arc::new(AtomicUsize::new(0));
            let (wake_reader, wake_writer) = UnixStream::pair()?;
            wake_reader.set_nonblocking(true)?;
            let mut registrations = PendingRegistrations::default();

            for signal in Signal::ALL {
                let raw = signal.os_signal().as_raw();
                registrations.push(signal_hook::flag::register_usize(
                    raw,
                    Arc::clone(&interrupted),
                    signal.atomic_value(),
                )?);
                registrations.push(signal_hook::low_level::pipe::register(
                    raw,
                    wake_writer.try_clone()?,
                )?);
            }

            Ok(Self {
                interrupted,
                wake_reader,
                _registrations: registrations.commit(),
            })
        }

        pub(super) fn interrupted(&self) -> Option<Signal> {
            Signal::from_raw(self.interrupted.load(Ordering::SeqCst))
        }

        pub(super) fn clone_wake_reader(&self) -> io::Result<UnixStream> {
            self.wake_reader.try_clone()
        }
    }

    #[derive(Default)]
    struct PendingRegistrations {
        registrations: Vec<SigId>,
    }

    impl PendingRegistrations {
        fn push(&mut self, registration: SigId) {
            self.registrations.push(registration);
        }

        fn commit(mut self) -> Box<[SigId]> {
            std::mem::take(&mut self.registrations).into_boxed_slice()
        }
    }

    impl Drop for PendingRegistrations {
        fn drop(&mut self) {
            for registration in self.registrations.drain(..).rev() {
                signal_hook::low_level::unregister(registration);
            }
        }
    }

    pub(super) fn runtime() -> io::Result<&'static Runtime> {
        if let Some(runtime) = RUNTIME.get() {
            return Ok(runtime);
        }
        let _installation = INSTALL_LOCK
            .lock()
            .map_err(|_| io::Error::other("signal installation lock is poisoned"))?;
        if let Some(runtime) = RUNTIME.get() {
            return Ok(runtime);
        }
        let runtime = Runtime::install()?;
        RUNTIME
            .set(runtime)
            .map_err(|_| io::Error::other("signal handlers were installed concurrently"))?;
        Ok(RUNTIME
            .get()
            .expect("successful installation publishes the signal runtime"))
    }

    pub(super) fn installed_runtime() -> Option<&'static Runtime> {
        RUNTIME.get()
    }

    pub(super) fn deliver(
        pid: u32,
        process_group: bool,
        signal: rustix::process::Signal,
    ) -> io::Result<Delivery> {
        let raw_pid = i32::try_from(pid).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "process ID is out of range")
        })?;
        let pid = rustix::process::Pid::from_raw(raw_pid)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "process ID is zero"))?;
        let result = if process_group {
            rustix::process::kill_process_group(pid, signal)
        } else {
            rustix::process::kill_process(pid, signal)
        };
        match result {
            Ok(()) => Ok(Delivery::Delivered),
            Err(rustix::io::Errno::SRCH) => Ok(Delivery::ProcessGone),
            Err(source) => Err(source.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    // [spec:ronin:req:runtime.guarded-signal-boundary/test]
    #[test]
    fn supported_signals_round_trip_through_platform_constants() {
        for signal in Signal::ALL {
            assert_eq!(Signal::from_raw(signal.atomic_value()), Some(signal));
        }
        assert_eq!(Signal::from_raw(0), None);
    }

    #[cfg(unix)]
    // [spec:ronin:req:runtime.guarded-signal-boundary/test]
    #[test]
    fn vanished_process_is_a_benign_delivery_outcome() {
        assert_eq!(
            forward(i32::MAX as u32, false, Signal::Terminate).unwrap(),
            Delivery::ProcessGone
        );
    }
}
