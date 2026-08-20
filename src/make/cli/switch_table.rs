//! What the switch table stores when a switch names a file, and the three
//! rules GNU Make applies on the way in.
//!
//! `decode_switches` (main.c) refuses an empty argument and filters a duplicate
//! out of every list switch but `-f`; `expand_command_line_file`, one line
//! later, canonicalises what is left. All three are here rather than at the
//! spellings that reach them, because a switch is decoded from four places —
//! a short cluster, a long option standing alone, a long option carrying its
//! value after an `=`, and the table of switches this Make accepts and ignores
//! — and a rule written at one of them is a rule the other three do not have.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use super::{ArgumentSource, BString, Bytes, Invocation};
use crate::Error;
use crate::error::CliError;
use crate::util::ByteSlice;

/// A file name a switch gave, in the spelling GNU Make's switch table stores.
///
/// `expand_command_line_file` (main.c) is the gate every switch that names a
/// file goes through — `-C`, `-f`, `-I`, `-o` and `-W` — and it rewrites the
/// word twice. A leading `~` is expanded, by the same rule and the same
/// function the include search path expands one with. Then leading `./`
/// sequences are stripped repeatedly, following slashes and all, so `.//foo` is
/// `foo` rather than `/foo` and `././inc` is `inc`; a name that was nothing but
/// `./` and slashes becomes `./` again, because there has to be something left.
///
/// A stripped name searches and opens exactly what the written one did, so what
/// this is FOR is the two places the name is read back. `$(MAKEFILE_LIST)` and
/// `$(.INCLUDE_DIRS)` publish it, and makefiles branch on those; and the switch
/// table's duplicate filter compares the next word as written against the names
/// already stored in this form.
pub(super) fn command_line_file(named: &[u8]) -> Vec<u8> {
    let home = std::env::var_os("HOME").map(OsString::into_vec);
    let expanded =
        kati::evaluate::tilde_expand(home.as_deref(), Path::new(OsStr::from_bytes(named)));
    let expanded = expanded.into_os_string().into_vec();
    let mut name = expanded.as_slice();
    while let Some(rest) = name.strip_prefix(b"./") {
        name = rest;
        while let Some(rest) = name.strip_prefix(b"/") {
            name = rest;
        }
    }
    if name.is_empty() { b"./" } else { name }.to_vec()
}

/// Whether one of the switch table's lists already holds this argument.
///
/// GNU Make filters a duplicate out of every list switch but `-f`, and it
/// compares the word AS WRITTEN against the names already stored — which have
/// been through [`command_line_file`]. The asymmetry is not an accident of the
/// reading, it is observable: `-I ./inc -I inc` searches one directory, because
/// the second word matches what the first was stored as, while `-I inc -I ./inc`
/// searches two, because `./inc` matches nothing that is there.
fn already_listed(list: &[PathBuf], written: &[u8]) -> bool {
    list.iter()
        .any(|listed| listed.as_os_str().as_bytes() == written)
}

pub(super) fn path_of(value: &[u8]) -> Result<PathBuf, Error> {
    value
        .to_os_str()
        .map(|value| PathBuf::from(value.to_owned()))
        .map_err(|_| {
            CliError::InvalidEncoding {
                context: crate::error::EncodingContext::Argument,
            }
            .into()
        })
}

impl Invocation {
    /// GNU Make's empty-argument gate, and whether there is anything to store.
    ///
    /// `decode_switches` (main.c) tests the argument of every switch that takes
    /// a string — a file name, a one-per-invocation value, or a list entry —
    /// before it stores anything, and an empty one sets the same `bad` flag a
    /// switch it cannot read sets. The word is consumed either way and nothing
    /// is recorded, so the only stream that can tell the difference is the one
    /// that dies of it.
    ///
    /// `option` is the switch rather than the spelling that reached it: GNU
    /// names the short form whenever the switch has one, so `--include-dir=`
    /// complains about `-I`.
    pub(super) fn non_empty(&mut self, source: ArgumentSource, option: &str, named: &[u8]) -> bool {
        if !named.is_empty() {
            return true;
        }
        self.complain(
            source,
            format!("the '{option}' option requires a non-empty string argument"),
        );
        false
    }

    /// A word this stream could not read, which GNU Make says nothing about.
    ///
    /// The `bad` flag getopt raises for a switch it does not know and for one
    /// whose argument is not there. `opterr = origin == o_command` silences
    /// getopt's own message for every other stream, and `if (bad && origin ==
    /// o_command) print_usage (bad)` is the only thing that then acts on the
    /// flag — so a makefile's own `MAKEFLAGS` loses the switch and says nothing
    /// about having lost it.
    ///
    /// Recorded rather than acted on where it is noticed, because GNU Make
    /// consumes the whole word before it gives up on it.
    pub(super) fn unreadable(&mut self, source: ArgumentSource, message: String) {
        if source.refuses_a_bad_switch() && self.bad.is_none() {
            self.bad = Some(message);
        }
    }

    /// A word GNU Make complains about whichever stream it came from.
    ///
    /// The other half of the same `bad` flag: `decode_switches` reaches its own
    /// `error (NILF, ...)` for an argument that is empty where a string was
    /// wanted, and for one that is not a positive integer where a count was,
    /// and that call is not guarded by the origin at all. Only the dying is.
    /// So the stream that forgives the switch still says why it dropped it.
    pub(super) fn complain(&mut self, source: ArgumentSource, message: String) {
        if source.refuses_a_bad_switch() {
            if self.bad.is_none() {
                self.bad = Some(message);
            }
            return;
        }
        self.complaints.push(message);
    }

    /// A switch value no stream forgives.
    ///
    /// `decode_debug_flags` and `decode_output_sync_flags` run after
    /// `decode_switches` has finished with every stream and call `fatal` rather
    /// than raising the `bad` flag, so the origin that decides whether an
    /// unreadable switch ends the run does not reach these. A makefile writing
    /// `MAKEFLAGS += -Obogus` dies of it exactly as the command line does.
    pub(super) fn undecodable(&mut self, message: String) {
        if self.bad.is_none() {
            self.bad = Some(message);
        }
    }

    /// Add a directory `-I` named, keeping a bare `-` as the entry it is.
    ///
    /// GNU Make stores `-` in the list beside the directories rather than
    /// acting on it here, and `construct_include_path` reaches it where it was
    /// written. Acting on it here instead would be a different program: the
    /// duplicate filter below sees the list as it stands, so a `-I inc` on both
    /// sides of a `-I -` is filtered out by the copy the reset was supposed to
    /// have thrown away, and a second `-I -` is filtered out by the first and
    /// resets nothing.
    pub(super) fn include_dir(
        &mut self,
        source: ArgumentSource,
        named: &[u8],
    ) -> Result<(), Error> {
        if !self.non_empty(source, "-I", named) {
            return Ok(());
        }
        if named == b"-" {
            if !already_listed(&self.include_dirs, named) {
                self.include_dirs.push(path_of(named)?);
            }
            return Ok(());
        }
        if already_listed(&self.include_dirs, named) {
            return Ok(());
        }
        self.include_dirs.push(path_of(&command_line_file(named))?);
        Ok(())
    }

    /// Add a Makefile `-f` named, duplicate or not.
    ///
    /// The one list switch GNU Make does not filter, under a comment reading
    /// `Allow duplicate makefiles for backward compatibility.` — so `-f x -f x`
    /// reads the file twice, warns about overriding its own recipes, and enters
    /// it into `MAKEFILE_LIST` twice.
    pub(super) fn makefile(&mut self, source: ArgumentSource, named: &[u8]) -> Result<(), Error> {
        if !self.non_empty(source, "-f", named) {
            return Ok(());
        }
        self.makefiles.push(path_of(&command_line_file(named))?);
        Ok(())
    }

    /// Add a directory `-C` named.
    pub(super) fn directory(&mut self, source: ArgumentSource, named: &[u8]) -> Result<(), Error> {
        if !self.non_empty(source, "-C", named) || already_listed(&self.directories, named) {
            return Ok(());
        }
        self.directories.push(path_of(&command_line_file(named))?);
        Ok(())
    }

    /// Add a statement `-E` supplied.
    ///
    /// A list switch like the rest, so the same text twice is read once — but
    /// not a file name, so nothing about the text is rewritten on the way in.
    pub(super) fn eval_statement(&mut self, source: ArgumentSource, named: &[u8]) {
        if !self.non_empty(source, "-E", named)
            || self.evals.iter().any(|listed| listed.as_ref() == named)
        {
            return;
        }
        self.evals.push(Bytes::from(named.to_vec()));
    }

    /// Add a `--debug` word.
    pub(super) fn debug_spec(&mut self, source: ArgumentSource, named: &[u8]) {
        if !self.non_empty(source, "--debug", named)
            || self.debug.iter().any(|listed| listed.as_slice() == named)
        {
            return;
        }
        self.debug.push(BString::from(named));
    }

    /// Consume the argument of a switch this Make accepts and does nothing
    /// with — `-W` and `-o` among them.
    ///
    /// The gate is not nothing even where the value is: GNU Make refuses an
    /// empty argument to these exactly as it refuses one to the switches whose
    /// value it keeps, and a build that would have run does not.
    pub(super) fn discarded_argument(
        &mut self,
        source: ArgumentSource,
        option: &str,
        named: &[u8],
    ) {
        let _ = self.non_empty(source, option, named);
    }
}
