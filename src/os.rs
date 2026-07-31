//! Rust implementation of the POSIX-facing operating-system adapter.

use std::io;
use std::path::{Path, PathBuf};
#[cfg(not(unix))]
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WorkingDirectory(PathBuf);

impl WorkingDirectory {
    pub(crate) fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        canonical_directory(path.as_ref()).map(Self)
    }

    pub(crate) fn change_to(&mut self, path: &Path) -> io::Result<()> {
        self.0 = canonical_directory(&self.resolve(path))?;
        Ok(())
    }

    pub(crate) fn resolve(&self, path: &Path) -> PathBuf {
        if path.is_absolute() || self.0.as_os_str().is_empty() {
            path.to_owned()
        } else {
            self.0.join(path)
        }
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }
}

/// Convert a stat's modification time to Ninja's nanosecond timestamp.
///
/// Zero is Ninja's missing-file sentinel, so a genuine zero timestamp reports
/// one. Times before the epoch are unrepresentable, matching what
/// `SystemTime::duration_since(UNIX_EPOCH)` rejected.
#[cfg(unix)]
#[allow(
    clippy::useless_conversion,
    reason = "stat time fields are i64 on some targets and C long on others"
)]
fn modification_nanoseconds(stat: rustix::fs::Stat) -> io::Result<i64> {
    let unrepresentable = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "modification time is not representable as nanoseconds since the epoch",
        )
    };
    let seconds = i64::try_from(stat.st_mtime).map_err(|_| unrepresentable())?;
    let nanoseconds = i64::try_from(stat.st_mtime_nsec).map_err(|_| unrepresentable())?;
    if seconds < 0 {
        return Err(unrepresentable());
    }
    let timestamp = seconds
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(nanoseconds))
        .unwrap_or(i64::MAX);
    Ok(if timestamp == 0 { 1 } else { timestamp })
}

fn canonical_directory(path: &Path) -> io::Result<PathBuf> {
    let path = std::fs::canonicalize(path)?;
    if std::fs::metadata(&path)?.is_dir() {
        Ok(path)
    } else {
        Err(io::Error::from(io::ErrorKind::NotADirectory))
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RealDiskInterface {
    working_directory: WorkingDirectory,
    /// The working directory held open so relative stats resolve through it.
    ///
    /// Opening once and sharing the descriptor across clones lets every stat
    /// pass the manifest's own relative path to the kernel, which needs
    /// neither a joined path buffer nor the C string `std::fs` allocates per
    /// call. `None` means no working directory was configured, so paths are
    /// already process-relative.
    // [spec:samurai:req:runtime.allocation-free-stat]
    #[cfg(unix)]
    directory: std::sync::Arc<std::sync::OnceLock<Result<rustix::fd::OwnedFd, rustix::io::Errno>>>,
}

impl RealDiskInterface {
    pub(crate) fn new(working_directory: WorkingDirectory) -> Self {
        Self {
            working_directory,
            #[cfg(unix)]
            directory: std::sync::Arc::default(),
        }
    }

    pub(crate) fn resolve(&self, path: &Path) -> PathBuf {
        self.working_directory.resolve(path)
    }

    #[cfg(unix)]
    fn directory_fd(&self) -> io::Result<Option<rustix::fd::BorrowedFd<'_>>> {
        use rustix::fd::AsFd;

        if self.working_directory.as_path().as_os_str().is_empty() {
            return Ok(None);
        }
        let opened = self.directory.get_or_init(|| {
            rustix::fs::open(
                self.working_directory.as_path(),
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
        });
        match opened {
            Ok(directory) => Ok(Some(directory.as_fd())),
            Err(errno) => Err(io::Error::from(*errno)),
        }
    }

    /// Stat one path without allocating for path resolution.
    ///
    /// An absolute path makes the kernel ignore the directory descriptor,
    /// which is exactly [`Self::resolve`]'s absolute-path passthrough.
    #[cfg(unix)]
    fn stat_at(&self, path: &Path) -> io::Result<rustix::fs::Stat> {
        let directory = self.directory_fd()?.unwrap_or(rustix::fs::CWD);
        rustix::fs::statat(directory, path, rustix::fs::AtFlags::empty()).map_err(io::Error::from)
    }

    /// Stat many paths at once, recording only the ones that succeed.
    ///
    /// A dirty scan issues one blocking `stat` per node and does nothing else
    /// while each is in flight, which on an up-to-date tree is the majority of
    /// the run. The syscalls do not depend on each other — only the evaluation
    /// that consumes them is ordered — so they can all go at once.
    ///
    /// Failures are deliberately left as `None` rather than reported. The
    /// caller re-stats those paths on the serial path, which reproduces the
    /// original error at the original point in the traversal. Diagnostics are
    /// part of the Ninja compatibility surface, so preserving *where* an error
    /// surfaces is worth one redundant syscall on a cold path. Note that a
    /// missing file is not a failure: [`Self::stat`] reports it as `Ok(0)`.
    pub(crate) fn stat_many(&self, paths: &[&Path], out: &mut [Option<i64>]) {
        // Spawning a thread costs far more than a cached `stat`, so give each
        // one enough work to be worth starting rather than splitting across
        // every core available. Below one chunk, stay on this thread.
        const PER_THREAD: usize = 512;

        debug_assert_eq!(paths.len(), out.len());
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        let threads = cores.min(paths.len() / PER_THREAD);
        if threads < 2 {
            for (path, slot) in paths.iter().zip(out.iter_mut()) {
                *slot = self.stat(path).ok();
            }
            return;
        }
        // Open the shared directory descriptor before fanning out, so the
        // workers contend on a `OnceLock` that is already initialized.
        let _ = self.directory_fd();
        let chunk = paths.len().div_ceil(threads);
        std::thread::scope(|scope| {
            for (paths, out) in paths.chunks(chunk).zip(out.chunks_mut(chunk)) {
                scope.spawn(move || {
                    for (path, slot) in paths.iter().zip(out.iter_mut()) {
                        *slot = self.stat(path).ok();
                    }
                });
            }
        });
    }

    // [spec:samurai:def:os.osmtime-fn]
    // [spec:samurai:sem:os.osmtime-fn]
    // [spec:samurai:def:os-posix.osmtime-fn]
    // [spec:samurai:sem:os-posix.osmtime-fn]
    /// Return a nanosecond timestamp, zero for a missing path, and an error for
    /// failures other than a missing component.
    pub(crate) fn stat(&self, path: &Path) -> io::Result<i64> {
        #[cfg(unix)]
        let modified = self.stat_at(path).and_then(modification_nanoseconds);
        #[cfg(not(unix))]
        let modified = std::fs::metadata(self.resolve(path)).and_then(|metadata| {
            let duration = metadata
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let timestamp = duration.as_nanos().try_into().unwrap_or(i64::MAX);
            Ok(if timestamp == 0 { 1 } else { timestamp })
        });
        match modified {
            Ok(timestamp) => Ok(timestamp),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(0)
            }
            Err(error) => Err(error),
        }
    }

    /// Create every parent directory needed for a file path.
    // [spec:samurai:def:os.osmkdirs-fn]
    // [spec:samurai:sem:os.osmkdirs-fn]
    // [spec:samurai:def:os-posix.osmkdirs-fn]
    // [spec:samurai:sem:os-posix.osmkdirs-fn]
    pub(crate) fn make_dirs(&self, path: &Path) -> io::Result<()> {
        let path = self.resolve(path);
        let directory = path.parent().unwrap_or_else(|| Path::new(""));
        if directory.as_os_str().is_empty() {
            Ok(())
        } else {
            std::fs::create_dir_all(directory)
        }
    }

    pub(crate) fn exists(&self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            self.stat_at(path).is_ok()
        }
        #[cfg(not(unix))]
        {
            self.resolve(path).exists()
        }
    }

    pub(crate) fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(self.resolve(path))
    }

    pub(crate) fn write(&self, path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
        std::fs::write(self.resolve(path), contents)
    }

    pub(crate) fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(self.resolve(path))
    }

    pub(crate) fn symlink_metadata(&self, path: &Path) -> io::Result<std::fs::Metadata> {
        std::fs::symlink_metadata(self.resolve(path))
    }
}

// [spec:samurai:def:os.osnproc-fn]
// [spec:samurai:sem:os.osnproc-fn]
// [spec:samurai:def:os-posix.osnproc-fn]
// [spec:samurai:sem:os-posix.osnproc-fn]
pub(crate) fn osnproc() -> i64 {
    std::thread::available_parallelism()
        .map_or(1, |count| i64::try_from(count.get()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DISK_TEST: AtomicUsize = AtomicUsize::new(0);

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new(name: &str) -> Self {
            for _ in 0..1024 {
                let sequence = NEXT_DISK_TEST.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "ronin-ninja-disk-{}-{name}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("could not create test directory: {error}"),
                }
            }
            panic!("could not allocate a unique disk test directory")
        }

        fn join(&self, path: impl AsRef<Path>) -> PathBuf {
            self.0.join(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn returns_a_positive_processor_fallback() {
        assert!(osnproc() >= 1);
    }

    #[test]
    fn ninja_disk_stat_missing_file() {
        let directory = TempDirectory::new("stat-missing");
        let disk = RealDiskInterface::default();
        assert_eq!(disk.stat(&directory.join("nosuchfile")).unwrap(), 0);
        assert_eq!(
            disk.stat(&directory.join("nosuchdir/nosuchfile")).unwrap(),
            0
        );
        fs::write(directory.join("notadir"), "").unwrap();
        assert_eq!(disk.stat(&directory.join("notadir/nosuchfile")).unwrap(), 0);
    }

    // [spec:samurai:req:runtime.allocation-free-stat/test]
    #[test]
    fn working_directory_relative_stats_match_resolved_paths() {
        let directory = TempDirectory::new("relative-stat");
        let nested = directory.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("file"), b"contents").unwrap();

        // A disk interface rooted at the temporary directory must agree with
        // one that resolves the same file through an absolute path.
        let rooted =
            RealDiskInterface::new(WorkingDirectory::new(directory.join("nested")).unwrap());
        let process = RealDiskInterface::default();
        let relative = Path::new("file");
        assert_eq!(
            rooted.stat(relative).unwrap(),
            process.stat(&nested.join("file")).unwrap()
        );
        assert!(rooted.exists(relative));
        assert!(!rooted.exists(Path::new("missing")));
        assert_eq!(rooted.stat(Path::new("missing")).unwrap(), 0);

        // An absolute path ignores the configured directory, matching resolve.
        assert_eq!(
            rooted.stat(&nested.join("file")).unwrap(),
            process.stat(&nested.join("file")).unwrap()
        );
    }

    #[cfg(unix)]
    // [spec:samurai:req:runtime.allocation-free-stat/test]
    #[test]
    fn relative_stats_preserve_non_utf8_paths() {
        use crate::util::ByteSlice;

        let directory = TempDirectory::new("relative-stat-bytes");
        let mut name = b"file-".to_vec();
        name.push(0xff);
        let name = name.to_os_str().unwrap().to_owned();
        let path = directory.join(&name);
        fs::write(&path, b"contents").unwrap();

        let rooted = RealDiskInterface::new(WorkingDirectory::new(directory.0.clone()).unwrap());
        let relative = Path::new(&name);
        assert!(rooted.exists(relative));
        assert_eq!(
            rooted.stat(relative).unwrap(),
            RealDiskInterface::default().stat(&path).unwrap()
        );
        assert_eq!(rooted.stat(Path::new("file-\u{fffd}")).unwrap(), 0);
    }

    #[test]
    fn ninja_disk_stat_bad_path() {
        let directory = TempDirectory::new("stat-bad");
        let disk = RealDiskInterface::default();
        assert!(disk.stat(&directory.join("x".repeat(512))).is_err());
    }

    #[test]
    fn ninja_disk_stat_existing_file() {
        let directory = TempDirectory::new("stat-file");
        let disk = RealDiskInterface::default();
        let file = directory.join("file");
        fs::write(&file, "").unwrap();
        assert!(disk.stat(&file).unwrap() > 1);
    }

    #[cfg(unix)]
    #[test]
    fn ninja_disk_stat_symlink() {
        let directory = TempDirectory::new("stat-symlink");
        let disk = RealDiskInterface::default();
        let file = directory.join("file");
        let link = directory.join("fileSymlink");
        fs::write(&file, "").unwrap();
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert_eq!(disk.stat(&file).unwrap(), disk.stat(&link).unwrap());
    }

    #[test]
    fn ninja_disk_stat_existing_directory() {
        let directory = TempDirectory::new("stat-directory");
        let disk = RealDiskInterface::default();
        fs::create_dir(directory.join("subdir")).unwrap();
        fs::create_dir(directory.join("subdir/subsubdir")).unwrap();
        for path in [".", "subdir", "subdir/subsubdir"] {
            assert!(disk.stat(&directory.join(path)).unwrap() > 1);
        }
        assert_eq!(
            disk.stat(&directory.join("subdir")).unwrap(),
            disk.stat(&directory.join("subdir/.")).unwrap()
        );
        assert_eq!(
            disk.stat(&directory.join("subdir")).unwrap(),
            disk.stat(&directory.join("subdir/subsubdir/..")).unwrap()
        );
        assert_eq!(
            disk.stat(&directory.join("subdir/subsubdir")).unwrap(),
            disk.stat(&directory.join("subdir/subsubdir/.")).unwrap()
        );
    }

    #[test]
    fn ninja_disk_make_directories() {
        let directory = TempDirectory::new("mkdirs");
        let disk = RealDiskInterface::default();
        let file = directory.join("path/with/double//slash/a_file");
        disk.make_dirs(&file).unwrap();
        fs::write(&file, b"").unwrap();
        assert!(file.is_file());
    }
}
