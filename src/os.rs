//! Rust implementation of the POSIX-facing operating-system adapter.

use crate::util::SamuraiString;
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::UNIX_EPOCH;

pub const MTIME_MISSING: i64 = -1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileReadStatus {
    Okay,
    NotFound,
    OtherError,
}

#[derive(Default)]
pub struct RealDiskInterface;

impl RealDiskInterface {
    /// Return a nanosecond timestamp, zero for a missing path, and an error for
    /// failures other than a missing component.
    pub fn stat(&self, path: &Path) -> io::Result<i64> {
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

    pub fn write_file(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        std::fs::write(path, contents)
    }

    pub fn make_dir(&self, path: &Path) -> io::Result<()> {
        match std::fs::create_dir(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Create every parent directory needed for a file path.
    pub fn make_dirs(&self, path: &Path) -> io::Result<()> {
        let directory = path.parent().unwrap_or_else(|| Path::new(""));
        if directory.as_os_str().is_empty() {
            Ok(())
        } else {
            std::fs::create_dir_all(directory)
        }
    }

    pub fn read_file(&self, path: &Path) -> (FileReadStatus, Vec<u8>, Option<String>) {
        match std::fs::read(path) {
            Ok(contents) => (FileReadStatus::Okay, contents, None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (
                FileReadStatus::NotFound,
                Vec::new(),
                Some(error.to_string()),
            ),
            Err(error) => (
                FileReadStatus::OtherError,
                Vec::new(),
                Some(error.to_string()),
            ),
        }
    }

    /// Remove a file or an empty directory with Ninja's rm -f return values.
    pub fn remove_file(&self, path: &Path) -> i32 {
        let result = match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_dir() => std::fs::remove_dir(path),
            Ok(_) => std::fs::remove_file(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return 1,
            Err(_) => return -1,
        };
        match result {
            Ok(()) => 0,
            Err(error) if error.kind() == io::ErrorKind::NotFound => 1,
            Err(_) => -1,
        }
    }

    pub fn allow_stat_cache(&mut self, _allow: bool) {}
}

// [spec:samurai:def:os.osgetcwd-fn]
// [spec:samurai:sem:os.osgetcwd-fn]
// [spec:samurai:def:os-posix.osgetcwd-fn]
// [spec:samurai:sem:os-posix.osgetcwd-fn]
pub fn osgetcwd() -> io::Result<SamuraiString> {
    let bytes = std::env::current_dir()?
        .into_os_string()
        .into_encoded_bytes();
    let n = bytes.len();
    let mut s = bytes;
    s.push(0);
    Ok(SamuraiString { n, s })
}

// [spec:samurai:def:os.oschdir-fn]
// [spec:samurai:sem:os.oschdir-fn]
// [spec:samurai:def:os-posix.oschdir-fn]
// [spec:samurai:sem:os-posix.oschdir-fn]
pub fn oschdir(dir: &Path) -> io::Result<()> {
    std::env::set_current_dir(dir)
}

// [spec:samurai:def:os.osmkdirs-fn]
// [spec:samurai:sem:os.osmkdirs-fn]
// [spec:samurai:def:os-posix.osmkdirs-fn]
// [spec:samurai:sem:os-posix.osmkdirs-fn]
pub fn osmkdirs(path: &Path, parent: bool) -> io::Result<()> {
    let directory = if parent {
        path.parent().unwrap_or_else(|| Path::new(""))
    } else {
        path
    };
    if directory.as_os_str().is_empty() {
        Ok(())
    } else {
        std::fs::create_dir_all(directory)
    }
}

// [spec:samurai:def:os.osmtime-fn]
// [spec:samurai:sem:os.osmtime-fn]
// [spec:samurai:def:os-posix.osmtime-fn]
// [spec:samurai:sem:os-posix.osmtime-fn]
pub fn osmtime(path: &Path) -> io::Result<i64> {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let duration = metadata
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            Ok(duration.as_nanos().try_into().unwrap_or(i64::MAX))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(MTIME_MISSING),
        Err(error) => Err(error),
    }
}

// [spec:samurai:def:os.osnproc-fn]
// [spec:samurai:sem:os.osnproc-fn]
// [spec:samurai:def:os-posix.osnproc-fn]
// [spec:samurai:sem:os-posix.osnproc-fn]
pub fn osnproc() -> i64 {
    std::thread::available_parallelism()
        .map(|count| count.get() as i64)
        .unwrap_or(1)
}

// [spec:samurai:def:os.osspawn-fn]
// [spec:samurai:sem:os.osspawn-fn]
// [spec:samurai:def:os-posix.osspawn-fn]
// [spec:samurai:sem:os-posix.osspawn-fn]
pub fn osspawn(argv: &[OsString], capture_output: bool) -> io::Result<Child> {
    let (program, arguments) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
    let mut command = Command::new(program);
    command.args(arguments);
    if capture_output {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    }
    command.spawn()
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
                    "samurai-ninja-disk-{}-{name}-{sequence}",
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
    fn ninja_disk_stat_missing_file_with_cache() {
        let directory = TempDirectory::new("stat-cache-missing");
        let mut disk = RealDiskInterface;
        disk.allow_stat_cache(true);
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
        disk.make_dir(&directory.join("subdir")).unwrap();
        disk.make_dir(&directory.join("subdir/subsubdir")).unwrap();
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
    fn ninja_disk_read_file() {
        let directory = TempDirectory::new("read");
        let disk = RealDiskInterface;
        let (status, contents, error) = disk.read_file(&directory.join("foobar"));
        assert_eq!(status, FileReadStatus::NotFound);
        assert!(contents.is_empty());
        assert!(error.is_some());

        let file = directory.join("testfile");
        let expected = b"test content\nok";
        disk.write_file(&file, expected).unwrap();
        let (status, contents, error) = disk.read_file(&file);
        assert_eq!(status, FileReadStatus::Okay);
        assert_eq!(contents, expected);
        assert_eq!(error, None);
    }

    #[test]
    fn ninja_disk_make_directories() {
        let directory = TempDirectory::new("mkdirs");
        let disk = RealDiskInterface;
        let file = directory.join("path/with/double//slash/a_file");
        disk.make_dirs(&file).unwrap();
        disk.write_file(&file, b"").unwrap();
        assert!(file.is_file());
    }

    #[test]
    fn ninja_disk_remove_file() {
        let directory = TempDirectory::new("remove-file");
        let disk = RealDiskInterface;
        let file = directory.join("file-to-remove");
        fs::write(&file, "").unwrap();
        assert_eq!(disk.remove_file(&file), 0);
        assert_eq!(disk.remove_file(&file), 1);
        assert_eq!(disk.remove_file(&directory.join("does not exist")), 1);
    }

    #[test]
    fn ninja_disk_remove_directory() {
        let directory = TempDirectory::new("remove-directory");
        let disk = RealDiskInterface;
        let target = directory.join("directory-to-remove");
        disk.make_dir(&target).unwrap();
        assert_eq!(disk.remove_file(&target), 0);
        assert_eq!(disk.remove_file(&target), 1);
        assert_eq!(disk.remove_file(&directory.join("does not exist")), 1);
    }
}
