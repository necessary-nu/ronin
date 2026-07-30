//! Retained byte sources and compact source spans.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static NEXT_SOURCE_ID: AtomicU64 = AtomicU64::new(1);

/// Stable identity for one loaded source buffer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SourceId(u64);

/// Immutable source bytes retained by parsers and diagnostics.
// [spec:samurai:req:runtime.borrowed-span-frontend]
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Source {
    id: SourceId,
    path: PathBuf,
    input: Box<[u8]>,
}

impl Source {
    pub(crate) fn from_path(path: impl AsRef<Path>) -> std::io::Result<Arc<Self>> {
        let path = path.as_ref();
        let input = fs::read(path)?;
        Ok(Self::from_bytes(path, input))
    }

    pub(crate) fn from_bytes(path: impl AsRef<Path>, input: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            id: SourceId(NEXT_SOURCE_ID.fetch_add(1, Ordering::Relaxed)),
            path: path.as_ref().to_owned(),
            input: input.into_boxed_slice(),
        })
    }

    pub(crate) const fn id(&self) -> SourceId {
        self.id
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.input
    }
}

/// A retained byte range with line and column coordinates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceSpan {
    source: Arc<Source>,
    pub(crate) byte_start: usize,
    pub(crate) byte_end: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

impl SourceSpan {
    pub(crate) const fn new(
        source: Arc<Source>,
        byte_start: usize,
        byte_end: usize,
        line: usize,
        column: usize,
    ) -> Self {
        Self {
            source,
            byte_start,
            byte_end,
            line,
            column,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        self.source.path()
    }

    #[cfg(test)]
    pub(crate) fn source_id(&self) -> SourceId {
        self.source.id()
    }

    #[cfg(test)]
    pub(crate) fn source_bytes(&self) -> &[u8] {
        self.source.bytes()
    }
}
