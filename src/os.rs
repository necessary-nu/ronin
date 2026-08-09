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

    /// Where `path` is, read from this directory.
    ///
    /// An empty path stays empty rather than becoming this directory. Ninja
    /// works from a real process working directory, so a manifest asking for
    /// the empty name asks the kernel for it and is told there is no such file
    /// — not that it found a directory. Joining would answer the wrong
    /// question and report the wrong reason for it.
    pub(crate) fn resolve(&self, path: &Path) -> PathBuf {
        if path.is_absolute() || path.as_os_str().is_empty() || self.0.as_os_str().is_empty() {
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

/// The first index at or after `ideal` where the parent directory changes, or
/// `paths.len()` when it never does.
///
/// Splitting here rather than at `ideal` keeps each directory the concern of a
/// single [`WorkingDirectory::stat_many`] worker; see that function for why
/// two workers in one directory is the case worth avoiding. Directories are
/// compared as raw names rather than as `Path`s, which skips re-parsing the
/// components of two paths that are usually equal.
fn directory_boundary(paths: &[&Path], ideal: usize) -> usize {
    let mut index = ideal.max(1);
    while index < paths.len()
        && paths[index].parent().map(Path::as_os_str)
            == paths[index - 1].parent().map(Path::as_os_str)
    {
        index += 1;
    }
    index
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
    // [spec:ronin:req:runtime.allocation-free-stat]
    #[cfg(unix)]
    directory: std::sync::Arc<std::sync::OnceLock<Result<rustix::fd::OwnedFd, rustix::io::Errno>>>,
    /// Directories [`Self::make_dirs`] has already ensured exist.
    ///
    /// `create_dir_all` on a directory that is already there still costs a
    /// failing `mkdir` and a `statx` to confirm what failed, and a build calls
    /// it once per output — so on a tree whose outputs share a directory,
    /// which is every tree, that is two wasted syscalls on the dispatch loop's
    /// critical path for every job it starts.
    created: std::sync::Arc<std::sync::Mutex<crate::htab::RapidHashSet<PathBuf>>>,
}

impl RealDiskInterface {
    pub(crate) fn new(working_directory: WorkingDirectory) -> Self {
        Self {
            working_directory,
            #[cfg(unix)]
            directory: std::sync::Arc::default(),
            created: std::sync::Arc::default(),
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
    ///
    /// Chunks are split on directory changes, never mid-directory, because a
    /// lookup that misses has to take the parent directory's lock to record
    /// the miss. Threads looking up absent names in the *same* directory
    /// therefore serialize on that lock, and pay contention for the privilege.
    /// Measured on 50,000 paths in one directory: with the files present,
    /// fanning out to four costs 36% more CPU than staying serial, the usual
    /// price of parallelism; with the files absent it costs 135% more and
    /// spends 45% of the run in `__pv_queued_spin_lock_slowpath`. Absent is
    /// not the rare case — it is every output of a build that has not run yet.
    /// Splitting on directory boundaries means one directory is one thread's
    /// business, so a graph spread over many directories still fans out and a
    /// graph confined to one stays serial, which is where it belongs.
    pub(crate) fn stat_many(&self, paths: &[&Path], out: &mut [Option<i64>]) {
        // Spawning a thread costs far more than a cached `stat`, so give each
        // one enough work to be worth starting. Below one chunk, stay here.
        const PER_THREAD: usize = 512;
        /// Past this the kernel serializes on shared directory state faster
        /// than the extra threads help. Measured on a 4,001-path scan against
        /// a 32-core host: wall time bottoms out around four to eight threads
        /// and then *rises* — 32 threads is both slower than 8 and 58% more
        /// CPU — while CPU climbs monotonically throughout. Four takes 95% of
        /// the available wall-clock win for a twentieth of the CPU, which is
        /// the right trade for a tool whose cores belong to the compiler.
        const MAX_THREADS: usize = 4;

        debug_assert_eq!(paths.len(), out.len());
        // Decide from the work before asking about the machine: `cores` is not
        // free, and a scan too small to split does not care how wide the host
        // is.
        let threads = (paths.len() / PER_THREAD).min(MAX_THREADS);
        if threads < 2 {
            self.stat_into(paths, out);
            return;
        }
        let threads = threads.min(cores());
        // Open the shared directory descriptor before fanning out, so the
        // workers contend on a `OnceLock` that is already initialized. Only
        // Unix has one to warm.
        #[cfg(unix)]
        let _ = self.directory_fd();
        let ideal = paths.len().div_ceil(threads);
        std::thread::scope(|scope| {
            let mut paths = paths;
            let mut out = out;
            while paths.len() > ideal {
                let split = directory_boundary(paths, ideal);
                if split >= paths.len() {
                    break;
                }
                let (head, tail) = paths.split_at(split);
                let (head_out, tail_out) = out.split_at_mut(split);
                scope.spawn(move || self.stat_into(head, head_out));
                paths = tail;
                out = tail_out;
            }
            // Whatever is left is this thread's share rather than another
            // spawn, which keeps the single-directory case free of threads.
            self.stat_into(paths, out);
        });
    }

    fn stat_into(&self, paths: &[&Path], out: &mut [Option<i64>]) {
        for (path, slot) in paths.iter().zip(out.iter_mut()) {
            *slot = self.stat(path).ok();
        }
    }

    // [spec:ronin:def:os.osmtime-fn]
    // [spec:ronin:sem:os.osmtime-fn]
    // [spec:ronin:def:os-posix.osmtime-fn]
    // [spec:ronin:sem:os-posix.osmtime-fn]
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
    // [spec:ronin:def:os.osmkdirs-fn]
    // [spec:ronin:sem:os.osmkdirs-fn]
    // [spec:ronin:def:os-posix.osmkdirs-fn]
    // [spec:ronin:sem:os-posix.osmkdirs-fn]
    pub(crate) fn make_dirs(&self, path: &Path) -> io::Result<()> {
        let path = self.resolve(path);
        let directory = path.parent().unwrap_or_else(|| Path::new(""));
        if directory.as_os_str().is_empty() {
            return Ok(());
        }
        if self
            .created
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(directory)
        {
            return Ok(());
        }
        std::fs::create_dir_all(directory)?;
        self.created
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(directory.to_owned());
        Ok(())
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

/// How many threads this process may usefully run, resolved once.
///
/// The standard library does not cache this. On Linux each call re-reads
/// `/proc/self/cgroup` and then walks `cpu.max` up the cgroup hierarchy —
/// five file opens here, 51 to 101 microseconds measured — which is real money
/// against a no-op build that finishes in under three milliseconds. The value
/// cannot change usefully within one run, so read it once and keep it.
fn cores() -> usize {
    static CORES: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CORES
        .get_or_init(|| std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get))
}

// [spec:ronin:def:os.osnproc-fn]
// [spec:ronin:sem:os.osnproc-fn]
// [spec:ronin:def:os-posix.osnproc-fn]
// [spec:ronin:sem:os-posix.osnproc-fn]
pub(crate) fn osnproc() -> i64 {
    i64::try_from(cores()).unwrap_or(i64::MAX)
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

    // [spec:ronin:req:runtime.allocation-free-stat/test]
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
    // [spec:ronin:req:runtime.allocation-free-stat/test]
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

    #[test]
    fn a_stat_chunk_never_splits_one_directory_across_two_workers() {
        let owned: Vec<PathBuf> = ["a/0", "a/1", "a/2", "b/0", "b/1", "c/0"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let paths: Vec<&Path> = owned.iter().map(PathBuf::as_path).collect();

        // An ideal split inside "a" slides forward to where "b" begins.
        assert_eq!(super::directory_boundary(&paths, 1), 3);
        assert_eq!(super::directory_boundary(&paths, 2), 3);
        // One that already falls on a change stays put.
        assert_eq!(super::directory_boundary(&paths, 3), 3);
        assert_eq!(super::directory_boundary(&paths, 5), 5);

        // A graph confined to one directory yields no split at all, which is
        // what keeps the whole scan on one thread.
        let owned: Vec<PathBuf> = (0..8)
            .map(|index| PathBuf::from(format!("d/{index}")))
            .collect();
        let paths: Vec<&Path> = owned.iter().map(PathBuf::as_path).collect();
        assert_eq!(super::directory_boundary(&paths, 2), paths.len());
    }
}
