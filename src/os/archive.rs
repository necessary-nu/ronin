//! Reading a member's date out of an `ar` archive's index.
//!
//! `lib.a(member.o)` is the one target name that is not a filename, so it is
//! the one name whose timestamp cannot come from a `stat`. GNU Make answers it
//! in `f_mtime` (reference/gnumake/src/remake.c) by scanning the archive, and
//! the answer is a rule rather than a lookup:
//!
//! * the archive itself must exist, or the member does not exist either;
//! * the member's date is what the index records for it, and a date of zero or
//!   a member the index does not hold both read as "no such member";
//! * a member file of the same name sitting on disk **newer** than the indexed
//!   date also reads as "no such member" — the file was rebuilt and not yet
//!   filed, so the member is out of date rather than merely old.
//!
//! The middle rule is not a corner case on a modern Linux host: `ar` defaults
//! to deterministic mode, which writes every member's date as zero, so
//! `ar_member_date` answers -1 for every member of an archive built by plain
//! `ar -rv` and the member is always out of date. GNU Make 4.4.1 on this host
//! re-runs `$(AR) $(ARFLAGS)` on every invocation for exactly that reason, and
//! so does Ronin — the agreement is real, and it is agreement about a rule
//! rather than about a shortcut. An archive written with `ar -U` records real
//! dates and both tools then find the member up to date.
//!
//! Only the SysV/GNU archive format is read, which is what `ar` writes here:
//! the `!<arch>\n` magic, fixed 60-byte member headers, the `//` long-name
//! table GNU `ar` uses for names too long for the header, and 4.4BSD's
//! `#1/LEN` extended names. Anything that does not parse is reported as an
//! archive with no such member, which is what GNU Make's `ar_scan` does with
//! an invalid archive it was only asked a date for.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Bytes of the magic that opens an archive.
const MAGIC: &[u8; 8] = b"!<arch>\n";
/// Bytes of one member header.
const HEADER: usize = 60;
/// How much of a member's name the fixed header field keeps, which is all a
/// short name is ever compared over.
const NAME_KEPT: usize = 15;

/// Split a path written as `lib.a(member.o)` into its two halves.
///
/// GNU Make's `ar_name`/`ar_parse_name`: the name must hold a `(`, not begin
/// with one, end with `)`, and not have the two adjacent — so `lib.a()` is an
/// ordinary filename and so is `(x)`.
pub(crate) fn split_member(path: &[u8]) -> Option<(&[u8], &[u8])> {
    let open = path.iter().position(|byte| *byte == b'(')?;
    if open == 0 || path.last() != Some(&b')') || path.len() - 1 <= open + 1 {
        return None;
    }
    Some((&path[..open], &path[open + 1..path.len() - 1]))
}

/// The seconds-since-epoch the index records for `member`, or `None` when the
/// archive has no such member — including a member whose recorded date is zero.
///
/// GNU Make's `ar_member_date`, which folds "not found" and "date zero"
/// together: `ar_scan` returns the first non-zero date a matching member has,
/// and a return of zero or less becomes -1, which `f_mtime` reads as
/// nonexistent.
pub(crate) fn member_date(archive: &Path, member: &[u8]) -> Option<i64> {
    let mut file = std::fs::File::open(archive).ok()?;
    let mut magic = [0u8; MAGIC.len()];
    file.read_exact(&mut magic).ok()?;
    if &magic != MAGIC {
        return None;
    }
    // The member name is matched without its directory: GNU Make's
    // `ar_name_equal` takes the basename before comparing, so
    // `lib.a(d/foo.o)` asks about the entry named `foo.o`.
    let wanted = member
        .iter()
        .rposition(|byte| *byte == b'/')
        .map_or(member, |slash| &member[slash + 1..]);

    let mut names = Vec::new();
    let mut header = [0u8; HEADER];
    loop {
        match file.read_exact(&mut header) {
            Ok(()) => {}
            Err(_) => return None,
        }
        if &header[58..60] != b"`\n" {
            return None;
        }
        let size: i64 = parse_field(&header[48..58], 10)?;
        if size < 0 {
            return None;
        }
        let date: i64 = parse_field(&header[16..28], 10).unwrap_or(0);
        let raw = trim_trailing(&header[..16]);

        // The long-name table is a member like any other and is read for its
        // data rather than compared against.
        if raw == b"//" || raw == b"ARFILENAMES/" {
            names.resize(usize::try_from(size).ok()?, 0);
            file.read_exact(&mut names).ok()?;
            skip_padding(&mut file, size)?;
            continue;
        }

        let mut extended = Vec::new();
        let name: &[u8] = if raw.first() == Some(&b'/') || raw.first() == Some(&b' ') {
            // GNU `ar`: an offset into the long-name table.
            let offset: usize = parse_field(&raw[1..], 10)?;
            let table = names.get(offset..)?;
            let end = table
                .iter()
                .position(|byte| *byte == b'\n' || *byte == b'\0')
                .unwrap_or(table.len());
            trim_trailing_slash(&table[..end])
        } else if raw.starts_with(b"#1/") {
            // 4.4BSD: the real name is the first bytes of the member's data.
            let length: usize = parse_field(&raw[3..], 10)?;
            extended.resize(length, 0);
            file.read_exact(&mut extended).ok()?;
            let end = extended
                .iter()
                .position(|byte| *byte == b'\0')
                .unwrap_or(extended.len());
            extended.truncate(end);
            &extended
        } else {
            trim_trailing_slash(raw)
        };

        if name_matches(wanted, name, extended.is_empty() && !raw.starts_with(b"/")) && date > 0 {
            return Some(date);
        }

        let consumed = i64::try_from(extended.len()).ok()?;
        file.seek(SeekFrom::Current(size - consumed)).ok()?;
        skip_padding(&mut file, size)?;
    }
}

/// An archive member header field, which is left-aligned ASCII padded with
/// spaces and may be entirely blank.
fn parse_field<T: TryFrom<i64>>(field: &[u8], radix: u32) -> Option<T> {
    let text = std::str::from_utf8(trim_trailing(field)).ok()?;
    if text.is_empty() {
        return T::try_from(0).ok();
    }
    T::try_from(i64::from_str_radix(text, radix).ok()?).ok()
}

fn trim_trailing(field: &[u8]) -> &[u8] {
    let end = field
        .iter()
        .rposition(|byte| *byte != b' ' && *byte != 0)
        .map_or(0, |last| last + 1);
    &field[..end]
}

const fn trim_trailing_slash(name: &[u8]) -> &[u8] {
    match name.split_last() {
        Some((b'/', rest)) => rest,
        _ => name,
    }
}

/// Every member's data is padded to an even offset.
fn skip_padding(file: &mut std::fs::File, size: i64) -> Option<()> {
    if size % 2 == 1 {
        file.seek(SeekFrom::Current(1)).ok()?;
    }
    Some(())
}

/// GNU Make's `ar_name_equal`. A name that came out of the fixed header field
/// is compared over that field's width only, because that is all of it the
/// archive kept.
fn name_matches(wanted: &[u8], entry: &[u8], truncated: bool) -> bool {
    if !truncated {
        return wanted == entry;
    }
    wanted.get(..NAME_KEPT).unwrap_or(wanted) == entry.get(..NAME_KEPT).unwrap_or(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_name_comes_apart_like_gnu() {
        assert_eq!(
            split_member(b"lib.a(foo.o)"),
            Some((&b"lib.a"[..], &b"foo.o"[..]))
        );
        assert_eq!(split_member(b"lib.a()"), None);
        assert_eq!(split_member(b"(foo.o)"), None);
        assert_eq!(split_member(b"plain.o"), None);
    }

    /// Built by `ar` on this host rather than by hand, so the reader is
    /// measured against the format that actually reaches it.
    fn archive(directory: &Path, flags: &str) -> std::path::PathBuf {
        std::fs::write(directory.join("member.o"), b"body\n").unwrap();
        std::fs::write(directory.join("a-very-long-member-name.o"), b"body\n").unwrap();
        let path = directory.join("lib.a");
        let ok = std::process::Command::new("ar")
            .arg(flags)
            .arg(&path)
            .arg("member.o")
            .arg("a-very-long-member-name.o")
            .current_dir(directory)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        path
    }

    #[test]
    fn deterministic_archive_records_no_date() {
        let directory = tempfile::tempdir().unwrap();
        let path = archive(directory.path(), "-rc");
        // `ar` defaults to deterministic mode here, which writes zero, and
        // GNU Make reads a zero date as no member at all.
        assert_eq!(member_date(&path, b"member.o"), None);
    }

    #[test]
    fn dated_archive_answers_both_name_lengths() {
        let directory = tempfile::tempdir().unwrap();
        let path = archive(directory.path(), "-rcU");
        assert!(member_date(&path, b"member.o").is_some_and(|date| date > 0));
        assert!(
            member_date(&path, b"a-very-long-member-name.o").is_some_and(|date| date > 0),
            "a name too long for the header comes out of the long-name table"
        );
        assert_eq!(member_date(&path, b"absent.o"), None);
        assert!(
            member_date(&path, b"sub/member.o").is_some(),
            "the directory is dropped before the comparison, as ar_name_equal does"
        );
    }

    #[test]
    fn a_non_archive_holds_no_members() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("not-an-archive");
        std::fs::write(&path, b"just bytes\n").unwrap();
        assert_eq!(member_date(&path, b"member.o"), None);
        assert_eq!(
            member_date(&directory.path().join("absent.a"), b"m.o"),
            None
        );
    }
}
