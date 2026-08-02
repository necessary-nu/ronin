use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RewriteStage {
    Create,
    Write,
    Flush,
    Sync,
    Reopen,
    Replace,
}

#[cfg(test)]
impl RewriteStage {
    pub(crate) const ALL: [Self; 6] = [
        Self::Create,
        Self::Write,
        Self::Flush,
        Self::Sync,
        Self::Reopen,
        Self::Replace,
    ];
}

#[cfg(test)]
fn inject_failure(fault: Option<RewriteStage>, stage: RewriteStage) -> io::Result<()> {
    if fault == Some(stage) {
        Err(io::Error::other(format!(
            "injected atomic rewrite failure at {stage:?}"
        )))
    } else {
        Ok(())
    }
}

fn atomic_rewrite_inner(
    path: &Path,
    #[cfg(test)] fault: Option<RewriteStage>,
    write_contents: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<File> {
    #[cfg(test)]
    inject_failure(fault, RewriteStage::Create)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    match fs::metadata(path) {
        Ok(metadata) => temporary
            .as_file()
            .set_permissions(metadata.permissions())?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    #[cfg(test)]
    inject_failure(fault, RewriteStage::Write)?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        write_contents(&mut writer)?;
        #[cfg(test)]
        inject_failure(fault, RewriteStage::Flush)?;
        writer.flush()?;
    }
    #[cfg(test)]
    inject_failure(fault, RewriteStage::Sync)?;
    temporary.as_file().sync_all()?;

    #[cfg(test)]
    inject_failure(fault, RewriteStage::Reopen)?;
    let replacement = OpenOptions::new()
        .read(true)
        .append(true)
        .open(temporary.path())?;

    #[cfg(test)]
    inject_failure(fault, RewriteStage::Replace)?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(replacement)
}

// [spec:ronin:req:runtime.persistence-transactions]
pub(crate) fn atomic_rewrite(
    path: &Path,
    write_contents: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<File> {
    #[cfg(test)]
    {
        atomic_rewrite_inner(path, None, write_contents)
    }
    #[cfg(not(test))]
    {
        atomic_rewrite_inner(path, write_contents)
    }
}

#[cfg(test)]
pub(crate) fn atomic_rewrite_with_fault(
    path: &Path,
    stage: RewriteStage,
    write_contents: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<File> {
    atomic_rewrite_inner(path, Some(stage), write_contents)
}
