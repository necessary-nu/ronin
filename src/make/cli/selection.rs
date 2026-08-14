//! Which Makefiles one invocation reads, and in what order.
//!
//! GNU Make reads every `-f` argument as though the named files had been
//! concatenated, so the answer is a list rather than a file, and the order it
//! comes back in is the semantics: an earlier file's assignments are in scope
//! while a later one is read, a target declared twice is the ordinary
//! re-declaration, and the default goal falls to the first file that declares
//! an eligible target.

use super::Invocation;
use std::path::{Path, PathBuf};

/// The makefiles GNU Make reads when no `-f` names one, in its own order.
pub(super) const DEFAULT_MAKEFILES: [&str; 3] = ["GNUmakefile", "makefile", "Makefile"];

/// The name `-f` gives standard input, which is a source rather than a path.
pub(super) const STANDARD_INPUT: &str = "-";

/// The first of GNU Make's default makefiles that exists in `directory`.
fn default_makefile(directory: &Path) -> Option<PathBuf> {
    DEFAULT_MAKEFILES
        .iter()
        .map(PathBuf::from)
        .find(|candidate| directory.join(candidate).is_file())
}

/// Whether this name is GNU Make's spelling for standard input.
pub(super) fn is_standard_input(makefile: &Path) -> bool {
    makefile == Path::new(STANDARD_INPUT)
}

/// Every Makefile this invocation reads, in the order it reads them.
///
/// The `-f` arguments when there are any, and otherwise the one default name
/// the directory offers — GNU Make looks for a default only when the command
/// line named nothing at all, not once per name it could not find.
pub(super) fn named_makefiles(invocation: &Invocation, directory: &Path) -> Vec<PathBuf> {
    if invocation.makefiles.is_empty() {
        return default_makefile(directory).into_iter().collect();
    }
    invocation.makefiles.clone()
}
