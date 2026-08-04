//! Literal translation of the utility data structures and algorithms.

use std::fmt;
use std::io::Write;

// [spec:ronin:def:util.string]
// [spec:ronin:req:compat.byte-inputs]
pub(crate) use bstr::{BStr, BString};
pub(crate) use bstr::{ByteSlice, ByteVec};

/// An adjacency list of arena identifiers, stored inline while it is short.
///
/// Under smallvec's union layout the value is eight bytes plus the larger of
/// the inline array and a pointer/length pair, so four four-byte identifiers
/// occupy exactly the twenty-four bytes a `Vec` already spends on its pointer,
/// length and capacity. Up to four elements therefore cost no allocation and
/// no extra footprint; `id_vec_matches_vec_footprint` holds that guarantee.
pub(crate) type IdVec<T> = smallvec::SmallVec<[T; 4]>;

/// Defines a dense arena identifier backed by a niche-packed `u32`.
///
/// The index is stored as `index + 1` inside a `NonZeroU32`, so `Option<Id>`
/// occupies four bytes rather than sixteen and every side table, adjacency
/// list, and traversal worklist holding identifiers halves in size. The
/// encoding is monotonic, so derived ordering still compares by index.
/// The minting constructor takes an optional visibility. Narrowing it to the
/// module that owns the arena is what makes holding an identifier evidence
/// that its slot exists, rather than merely a claim that it might; the default
/// stays crate-visible for arenas that have not been closed yet.
macro_rules! arena_id {
    ($name:ident) => {
        arena_id!($name, pub(crate));
    };
    ($name:ident, $mint:vis) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub(crate) struct $name(std::num::NonZeroU32);

        impl $name {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "the assertion bounds the index below u32::MAX"
            )]
            $mint const fn from_index(index: usize) -> Self {
                assert!(
                    index < u32::MAX as usize,
                    "arena index exceeds the u32 identifier capacity"
                );
                match std::num::NonZeroU32::new(index as u32 + 1) {
                    Some(raw) => Self(raw),
                    None => panic!("the shifted arena index is nonzero"),
                }
            }

            pub(crate) const fn index(self) -> usize {
                self.0.get() as usize - 1
            }
        }
    };
}

pub(crate) use arena_id;

// Rust containers and ownership replace the source's manual allocation,
// buffer-growth, and destruction helpers.
// [spec:ronin:def:util.buffer]
// [spec:ronin:def:util.evalstring]
// [spec:ronin:def:util.xmalloc-fn]
// [spec:ronin:sem:util.xmalloc-fn]
// [spec:ronin:def:util.reallocarray-fn]
// [spec:ronin:sem:util.reallocarray-fn]
// [spec:ronin:def:util.xreallocarray-fn]
// [spec:ronin:sem:util.xreallocarray-fn]
// [spec:ronin:def:util.xmemdup-fn]
// [spec:ronin:sem:util.xmemdup-fn]
// [spec:ronin:def:util.bufadd-fn]
// [spec:ronin:sem:util.bufadd-fn]
// [spec:ronin:def:util.delevalstr-fn]
// [spec:ronin:sem:util.delevalstr-fn]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EvalPart {
    Literal(BString),
    Variable(crate::names::VarId),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct EvalString {
    pub(crate) parts: Vec<EvalPart>,
}

impl EvalString {
    #[cfg(test)]
    pub(crate) fn literal(value: impl Into<BString>) -> Self {
        Self {
            parts: vec![EvalPart::Literal(value.into())],
        }
    }

    #[cfg(test)]
    pub(crate) fn variable(name: crate::names::VarId) -> Self {
        Self {
            parts: vec![EvalPart::Variable(name)],
        }
    }
}

// [spec:ronin:def:util.xasprintf-fn]
// [spec:ronin:sem:util.xasprintf-fn]
// [spec:ronin:def:util.writefile-fn]
// [spec:ronin:sem:util.writefile-fn]
pub(crate) fn xasprintf(args: fmt::Arguments<'_>) -> BString {
    let mut output = Vec::new();
    output
        .write_fmt(args)
        .expect("formatting into memory cannot fail");
    BString::from(output)
}

// Formatting diagnostics into owned values replaces global printing and exit
// helpers; the binary decides which stream and exit status to use.
// [spec:ronin:def:util.vwarn-fn]
// [spec:ronin:sem:util.vwarn-fn]
// [spec:ronin:def:util.warn-fn]
// [spec:ronin:sem:util.warn-fn]
// [spec:ronin:def:util.fatal-fn]
// [spec:ronin:sem:util.fatal-fn]
pub(crate) fn diagnostic(program: &str, message: impl fmt::Display) -> String {
    format!("{program}: {message}")
}

// [spec:ronin:def:util.canonpath-fn]
// [spec:ronin:sem:util.canonpath-fn]
/// Whether a path already has the form `canonpath` would produce.
///
/// Almost every manifest path is already canonical, and checking costs one
/// pass with no allocation, so the rewrite below runs only when it changes
/// something.
pub(crate) fn is_canonical(path: &[u8]) -> bool {
    let body = path.strip_prefix(b"/").unwrap_or(path);
    !body.is_empty()
        && body
            .split(|byte| *byte == b'/')
            .all(|component| !matches!(component, b"" | b"." | b".."))
}

/// Canonicalize in place.
///
/// Canonicalization only ever removes bytes, so the write cursor never passes
/// the read cursor and the result can be built over the input.
pub(crate) fn canonpath(path: &mut Vec<u8>) {
    if path.is_empty() || is_canonical(path) {
        return;
    }
    let absolute = path[0] == b'/';
    // Start offsets of the components written so far, so `..` can pop one.
    let mut components = Vec::new();
    let mut write = usize::from(absolute);
    let mut read = 0;

    while read < path.len() {
        let start = read;
        while read < path.len() && path[read] != b'/' {
            read += 1;
        }
        let length = read - start;
        read += 1;
        if length == 0 || (length == 1 && path[start] == b'.') {
            continue;
        }
        let parent = length == 2 && path[start] == b'.' && path[start + 1] == b'.';
        if parent {
            if let Some(previous) = components.pop() {
                write = previous;
                continue;
            }
        }

        let component_start = write;
        if write != 0 && path[write - 1] != b'/' {
            path[write] = b'/';
            write += 1;
        }
        debug_assert!(write <= start, "canonicalization never grows the path");
        path.copy_within(start..start + length, write);
        write += length;
        if !parent {
            components.push(component_start);
        }
    }

    path.truncate(write);
    if path.is_empty() {
        path.push(b'.');
    }
}

pub(crate) fn edit_distance(
    left: &str,
    right: &str,
    allow_replacements: bool,
    max_edit_distance: Option<usize>,
) -> usize {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut row = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_byte) in left.iter().enumerate() {
        let mut previous = row[0];
        row[0] = left_index + 1;
        let mut best = row[0];
        for (right_index, right_byte) in right.iter().enumerate() {
            let old = row[right_index + 1];
            row[right_index + 1] = if allow_replacements {
                (previous + usize::from(left_byte != right_byte))
                    .min(row[right_index] + 1)
                    .min(old + 1)
            } else if left_byte == right_byte {
                previous
            } else {
                row[right_index].min(old) + 1
            };
            previous = old;
            best = best.min(row[right_index + 1]);
        }
        if let Some(limit) = max_edit_distance {
            if best > limit {
                return limit + 1;
            }
        }
    }
    row[right.len()]
}

/// Ends output with a newline, as every Ninja run does that produced any.
pub(crate) fn terminated(output: impl AsRef<[u8]>) -> Vec<u8> {
    let mut output = output.as_ref().to_vec();
    if !output.is_empty() && !matches!(output.last(), Some(b'\n' | b'\0')) {
        output.push(b'\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inline capacity for arena identifiers must stay free.
    ///
    /// The whole reason adjacency lists carry four inline slots is that the
    /// value is no larger than the `Vec` it replaced. Raising the inline count
    /// or widening an identifier would silently start charging every node and
    /// edge for the privilege, so pin the guarantee rather than trusting it.
    #[test]
    fn id_vec_matches_vec_footprint() {
        arena_id!(ProbeId);
        assert_eq!(std::mem::size_of::<ProbeId>(), 4);
        assert_eq!(
            std::mem::size_of::<IdVec<ProbeId>>(),
            std::mem::size_of::<Vec<ProbeId>>()
        );

        let mut probe = IdVec::new();
        for index in 0..4 {
            probe.push(ProbeId::from_index(index));
        }
        assert!(!probe.spilled(), "four identifiers must stay inline");
        assert_eq!(probe[0].index(), 0);
        probe.push(ProbeId::from_index(4));
        assert!(probe.spilled(), "the fifth identifier reaches the heap");
    }

    #[test]
    fn canonicalizes_relative_paths() {
        let mut path = xasprintf(format_args!("a//b/../c/."));
        canonpath(&mut path);
        assert_eq!(path.as_bytes(), b"a/c");
    }

    #[test]
    fn ninja_canonicalize_path_empty_and_many_components() {
        let mut empty = BString::default();
        canonpath(&mut empty);
        assert!(empty.is_empty());

        let source = std::iter::repeat_n("a", 220).collect::<Vec<_>>().join("/");
        let mut path = xasprintf(format_args!("{source}"));
        canonpath(&mut path);
        assert_eq!(path.len(), source.len());
        assert_eq!(path.as_bytes(), source.as_bytes());
    }

    #[cfg(unix)]
    #[test]
    // [spec:ronin:req:compat.byte-inputs/test]
    fn byte_strings_round_trip_non_utf8_unix_paths() {
        let mut name = format!("ronin-bstr-{}-", std::process::id()).into_bytes();
        name.push(0xff);
        let name = BString::from(name);
        let distinct = BString::from(b"ronin-bstr-\xfe");
        assert_ne!(name, distinct);

        let path = std::env::temp_dir().join(
            name.to_os_str()
                .expect("all byte strings are valid Unix OS strings"),
        );
        std::fs::write(&path, b"exact bytes").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"exact bytes");

        let round_trip = BString::from(
            Vec::from_path_buf(path.clone()).expect("all Unix paths have a byte representation"),
        );
        assert!(round_trip.ends_with(name.as_bytes()));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn ninja_edit_distance_cases() {
        assert_eq!(edit_distance("", "ninja", true, None), 5);
        assert_eq!(edit_distance("ninja", "", true, None), 5);
        assert_eq!(edit_distance("", "", true, None), 0);
        for limit in 1..7 {
            assert_eq!(
                edit_distance("abcdefghijklmnop", "ponmlkjihgfedcba", true, Some(limit)),
                limit + 1
            );
        }
        assert_eq!(edit_distance("ninja", "njnja", true, None), 1);
        assert_eq!(edit_distance("ninja", "njnja", false, None), 2);
        assert_eq!(
            edit_distance("browser_tests", "browser_tests", true, None),
            0
        );
        assert_eq!(
            edit_distance("browser_test", "browser_tests", true, None),
            1
        );
    }
}
