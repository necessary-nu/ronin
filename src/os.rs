//! Rust implementation of the POSIX-facing operating-system adapter.

use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

pub(crate) const MTIME_MISSING: i64 = -1;

#[derive(Default)]
pub(crate) struct RealDiskInterface;

impl RealDiskInterface {
    // [spec:samurai:def:os.osmtime-fn]
    // [spec:samurai:sem:os.osmtime-fn]
    // [spec:samurai:def:os-posix.osmtime-fn]
    // [spec:samurai:sem:os-posix.osmtime-fn]
    /// Return a nanosecond timestamp, zero for a missing path, and an error for
    /// failures other than a missing component.
    pub(crate) fn stat(&self, path: &Path) -> io::Result<i64> {
        match std::fs::metadata(path) {
            Ok(metadata) => {
                let duration = metadata
                    .modified()?
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                let timestamp = duration.as_nanos().try_into().unwrap_or(i64::MAX);
                Ok(if timestamp == 0 { 1 } else { timestamp })
            }
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
        let directory = path.parent().unwrap_or_else(|| Path::new(""));
        if directory.as_os_str().is_empty() {
            Ok(())
        } else {
            std::fs::create_dir_all(directory)
        }
    }
}

// [spec:samurai:def:os.osnproc-fn]
// [spec:samurai:sem:os.osnproc-fn]
// [spec:samurai:def:os-posix.osnproc-fn]
// [spec:samurai:sem:os-posix.osnproc-fn]
pub(crate) fn osnproc() -> i64 {
    std::thread::available_parallelism()
        .map(|count| count.get() as i64)
        .unwrap_or(1)
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
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
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
        let disk = RealDiskInterface;
        assert_eq!(disk.stat(&directory.join("nosuchfile")).unwrap(), 0);
        assert_eq!(
            disk.stat(&directory.join("nosuchdir/nosuchfile")).unwrap(),
            0
        );
        fs::write(directory.join("notadir"), "").unwrap();
        assert_eq!(disk.stat(&directory.join("notadir/nosuchfile")).unwrap(), 0);
    }

    #[test]
    fn ninja_disk_stat_bad_path() {
        let directory = TempDirectory::new("stat-bad");
        let disk = RealDiskInterface;
        assert!(disk.stat(&directory.join("x".repeat(512))).is_err());
    }

    #[test]
    fn ninja_disk_stat_existing_file() {
        let directory = TempDirectory::new("stat-file");
        let disk = RealDiskInterface;
        let file = directory.join("file");
        fs::write(&file, "").unwrap();
        assert!(disk.stat(&file).unwrap() > 1);
    }

    #[cfg(unix)]
    #[test]
    fn ninja_disk_stat_symlink() {
        let directory = TempDirectory::new("stat-symlink");
        let disk = RealDiskInterface;
        let file = directory.join("file");
        let link = directory.join("fileSymlink");
        fs::write(&file, "").unwrap();
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert_eq!(disk.stat(&file).unwrap(), disk.stat(&link).unwrap());
    }

    #[test]
    fn ninja_disk_stat_existing_directory() {
        let directory = TempDirectory::new("stat-directory");
        let disk = RealDiskInterface;
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
        let disk = RealDiskInterface;
        let file = directory.join("path/with/double//slash/a_file");
        disk.make_dirs(&file).unwrap();
        fs::write(&file, b"").unwrap();
        assert!(file.is_file());
    }
}
