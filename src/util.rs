//! Literal translation of the utility data structures and algorithms.

use std::fmt;
use std::io::Write;

// [spec:samurai:def:util.string]
// [spec:samurai:req:compat.byte-inputs]
pub(crate) use bstr::{BStr, BString};
pub(crate) use bstr::{ByteSlice, ByteVec};

/// Defines a dense arena identifier backed by a niche-packed `u32`.
///
/// The index is stored as `index + 1` inside a `NonZeroU32`, so `Option<Id>`
/// occupies four bytes rather than sixteen and every side table, adjacency
/// list, and traversal worklist holding identifiers halves in size. The
/// encoding is monotonic, so derived ordering still compares by index.
macro_rules! arena_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub(crate) struct $name(std::num::NonZeroU32);

        impl $name {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "the assertion bounds the index below u32::MAX"
            )]
            pub(crate) const fn from_index(index: usize) -> Self {
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
// [spec:samurai:def:util.buffer]
// [spec:samurai:def:util.evalstring]
// [spec:samurai:def:util.xmalloc-fn]
// [spec:samurai:sem:util.xmalloc-fn]
// [spec:samurai:def:util.reallocarray-fn]
// [spec:samurai:sem:util.reallocarray-fn]
// [spec:samurai:def:util.xreallocarray-fn]
// [spec:samurai:sem:util.xreallocarray-fn]
// [spec:samurai:def:util.xmemdup-fn]
// [spec:samurai:sem:util.xmemdup-fn]
// [spec:samurai:def:util.bufadd-fn]
// [spec:samurai:sem:util.bufadd-fn]
// [spec:samurai:def:util.delevalstr-fn]
// [spec:samurai:sem:util.delevalstr-fn]
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

    pub(crate) const fn from_parts(parts: Vec<EvalPart>) -> Self {
        Self { parts }
    }
}

// [spec:samurai:def:util.xasprintf-fn]
// [spec:samurai:sem:util.xasprintf-fn]
// [spec:samurai:def:util.writefile-fn]
// [spec:samurai:sem:util.writefile-fn]
pub(crate) fn xasprintf(args: fmt::Arguments<'_>) -> BString {
    let mut output = Vec::new();
    output
        .write_fmt(args)
        .expect("formatting into memory cannot fail");
    BString::from(output)
}

// Formatting diagnostics into owned values replaces global printing and exit
// helpers; the binary decides which stream and exit status to use.
// [spec:samurai:def:util.vwarn-fn]
// [spec:samurai:sem:util.vwarn-fn]
// [spec:samurai:def:util.warn-fn]
// [spec:samurai:sem:util.warn-fn]
// [spec:samurai:def:util.fatal-fn]
// [spec:samurai:sem:util.fatal-fn]
pub(crate) fn diagnostic(program: &str, message: impl fmt::Display) -> String {
    format!("{program}: {message}")
}

// [spec:samurai:def:util.canonpath-fn]
// [spec:samurai:sem:util.canonpath-fn]
pub(crate) fn canonpath(path: &mut BString) {
    if path.is_empty() {
        return;
    }
    let input = path.as_bytes();
    let absolute = input[0] == b'/';
    let mut output = Vec::with_capacity(input.len());
    let mut components = Vec::new();
    if absolute {
        output.push(b'/');
    }

    for component in input.split(|byte| *byte == b'/') {
        if component.is_empty() || component == b"." {
            continue;
        }
        if component == b".." {
            if let Some(start) = components.pop() {
                output.truncate(start);
                continue;
            }
        }

        let truncate_to = output.len();
        if !output.is_empty() && output.last() != Some(&b'/') {
            output.push(b'/');
        }
        output.extend_from_slice(component);
        if component != b".." {
            components.push(truncate_to);
        }
    }

    if output.is_empty() {
        output.push(b'.');
    }
    *path = BString::from(output);
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

#[cfg(test)]
mod tests {
    use super::*;

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
    // [spec:samurai:req:compat.byte-inputs/test]
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
