//! Literal translation of the utility data structures and algorithms.

use std::fmt;
use std::io::Write;

// [spec:samurai:def:util.string]
// [spec:samurai:req:compat.byte-inputs]
pub use bstr::{BStr, BString};
pub use bstr::{ByteSlice, ByteVec};

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StringPiece<'a> {
    bytes: &'a [u8],
}

#[cfg(test)]
impl<'a> StringPiece<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub fn from_cstr(value: &'a str) -> Self {
        Self {
            bytes: value
                .as_bytes()
                .split(|byte| *byte == 0)
                .next()
                .unwrap_or_default(),
        }
    }

    pub fn len(self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }

    pub fn as_str(self) -> &'a str {
        std::str::from_utf8(self.bytes).unwrap()
    }

    pub fn substr(self, start: usize, length: Option<usize>) -> Self {
        let start = start.min(self.bytes.len());
        let end = length
            .map(|length| start.saturating_add(length))
            .unwrap_or(self.bytes.len())
            .min(self.bytes.len());
        Self::new(&self.bytes[start..end])
    }
}

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
pub enum EvalPart {
    Literal(BString),
    Variable(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EvalString {
    pub parts: Vec<EvalPart>,
}

impl EvalString {
    #[cfg(test)]
    pub fn literal(value: impl Into<BString>) -> Self {
        Self {
            parts: vec![EvalPart::Literal(value.into())],
        }
    }

    #[cfg(test)]
    pub fn variable(name: impl Into<String>) -> Self {
        Self {
            parts: vec![EvalPart::Variable(name.into())],
        }
    }

    pub fn from_parts(parts: Vec<EvalPart>) -> Self {
        Self { parts }
    }
}

// [spec:samurai:def:util.xasprintf-fn]
// [spec:samurai:sem:util.xasprintf-fn]
// [spec:samurai:def:util.writefile-fn]
// [spec:samurai:sem:util.writefile-fn]
pub fn xasprintf(args: fmt::Arguments<'_>) -> BString {
    let mut output = Vec::new();
    output
        .write_fmt(args)
        .expect("formatting into memory cannot fail");
    BString::from(output)
}

// [spec:samurai:def:util.mkstr-fn]
// [spec:samurai:sem:util.mkstr-fn]
pub fn mkstr(n: usize) -> BString {
    BString::from(vec![0; n])
}

// Formatting diagnostics into owned values replaces global printing and exit
// helpers; the binary decides which stream and exit status to use.
// [spec:samurai:def:util.vwarn-fn]
// [spec:samurai:sem:util.vwarn-fn]
// [spec:samurai:def:util.warn-fn]
// [spec:samurai:sem:util.warn-fn]
// [spec:samurai:def:util.fatal-fn]
// [spec:samurai:sem:util.fatal-fn]
pub fn diagnostic(program: &str, message: impl fmt::Display) -> String {
    format!("{program}: {message}")
}

// [spec:samurai:def:util.canonpath-fn]
// [spec:samurai:sem:util.canonpath-fn]
pub fn canonpath(path: &mut BString) {
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

#[cfg(test)]
pub fn strip_ansi_escape_codes(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        index += 1;
        if bytes.get(index) != Some(&b'[') {
            continue;
        }
        index += 1;
        while index < bytes.len() {
            let byte = bytes[index];
            index += 1;
            if (0x40..=0x7e).contains(&byte) {
                break;
            }
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

pub fn edit_distance(
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
pub fn encode_json_string(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{00}'..='\u{1f}' => {
                use std::fmt::Write as _;
                write!(output, "\\u{:04x}", character as u32).unwrap();
            }
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            _ => output.push(character),
        }
    }
    output
}

#[cfg(test)]
fn ansi_color_sequences(input: &[u8]) -> Vec<(usize, usize)> {
    let mut sequences = Vec::new();
    let mut index = 0;
    while index < input.len() {
        if input[index] != 0x1b || input.get(index + 1) != Some(&b'[') {
            index += 1;
            continue;
        }
        let mut end = index + 2;
        while matches!(input.get(end), Some(b'0'..=b'9' | b';')) {
            end += 1;
        }
        if input.get(end) == Some(&b'm') {
            sequences.push((index, end + 1));
            index = end + 1;
        } else {
            index += 1;
        }
    }
    sequences
}

#[cfg(test)]
pub fn elide_middle(input: &str, width: usize) -> String {
    if input.len() <= width {
        return input.to_owned();
    }
    let bytes = input.as_bytes();
    let sequences = ansi_color_sequences(bytes);
    if sequences.is_empty() {
        if width <= 3 {
            return ".".repeat(width);
        }
        let remaining = width - 3;
        let left = remaining / 2;
        let right = remaining - left;
        return format!("{}...{}", &input[..left], &input[input.len() - right..]);
    }
    let hidden = |index: usize| {
        sequences
            .iter()
            .any(|(start, end)| (*start..*end).contains(&index))
    };
    let visible_width = bytes.len()
        - sequences
            .iter()
            .map(|(start, end)| end - start)
            .sum::<usize>();
    if visible_width <= width {
        return input.to_owned();
    }
    let ellipsis_width = width.min(3);
    let remaining = width - ellipsis_width;
    let visible_left = remaining / 2;
    let visible_right = remaining - visible_left;
    let gap_start = visible_left;
    let gap_end = visible_width - visible_right;

    let raw_index_at = |visible_target: usize| {
        let mut index = 0;
        let mut visible = 0;
        while index < bytes.len() {
            if visible == visible_target {
                return index;
            }
            if !hidden(index) {
                visible += 1;
            }
            index += 1;
        }
        bytes.len()
    };
    let left_end = raw_index_at(gap_start);
    let right_start = raw_index_at(gap_end);
    let mut output = String::from_utf8_lossy(&bytes[..left_end]).into_owned();
    output.push_str(&"...".chars().take(ellipsis_width).collect::<String>());
    for (start, end) in &sequences {
        if *start >= left_end && *end <= right_start {
            output.push_str(&String::from_utf8_lossy(&bytes[*start..*end]));
        }
    }
    output.push_str(&String::from_utf8_lossy(&bytes[right_start..]));
    output
}

#[cfg(test)]
pub fn split_string_piece(input: &str, separator: char) -> Vec<&str> {
    input.split(separator).collect()
}

#[cfg(test)]
pub fn join_string_piece(parts: &[&str], separator: char) -> String {
    parts.join(&separator.to_string())
}

#[cfg(test)]
pub fn to_lower_ascii(character: char) -> char {
    character.to_ascii_lowercase()
}

#[cfg(test)]
pub fn equals_case_insensitive_ascii(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
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
        let mut empty = mkstr(0);
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
    fn ninja_strip_ansi_escape_codes() {
        assert_eq!(strip_ansi_escape_codes("foo\x1b"), "foo");
        assert_eq!(strip_ansi_escape_codes("foo\x1b["), "foo");
        assert_eq!(
            strip_ansi_escape_codes(
                "\x1b[1maffixmgr.cxx:286:15: \x1b[0m\x1b[0;1;35mwarning: \x1b[0m\x1b[1musing the result... [-Wparentheses]\x1b[0m",
            ),
            "affixmgr.cxx:286:15: warning: using the result... [-Wparentheses]"
        );
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

    #[test]
    fn ninja_json_encoding_cases() {
        assert_eq!(encode_json_string("foo bar"), "foo bar");
        assert_eq!(
            encode_json_string("\"\\\u{08}\u{0c}\n\r\t"),
            "\\\"\\\\\\b\\f\\n\\r\\t"
        );
        assert_eq!(encode_json_string("\u{01}\u{1f}"), "\\u0001\\u001f");
        assert_eq!(encode_json_string("你好"), "你好");
    }

    #[test]
    fn ninja_elide_middle_cases() {
        let short = "Nothing to elide in this short string.";
        assert_eq!(elide_middle(short, 80), short);
        assert_eq!(elide_middle(short, 0), "");
        assert_eq!(elide_middle(short, 1), ".");
        assert_eq!(elide_middle(short, 2), "..");
        assert_eq!(elide_middle(short, 3), "...");

        let input = "01234567890123456789";
        assert_eq!(elide_middle(input, 4), "...9");
        assert_eq!(elide_middle(input, 5), "0...9");
        assert_eq!(elide_middle(input, 9), "012...789");
        assert_eq!(elide_middle(input, 10), "012...6789");
        assert_eq!(elide_middle(input, 19), "01234567...23456789");

        assert_eq!(
            elide_middle("012345\x1b[0;35m67890123456789", 10),
            "012...\x1b[0;35m6789"
        );
        assert_eq!(
            elide_middle("abcd\x1b[1;31mefg\x1b[0mhlkmnopqrstuvwxyz", 15),
            "abcd\x1b[1;31mef...\x1b[0muvwxyz"
        );
    }

    #[test]
    fn ninja_string_piece_utility_cases() {
        assert_eq!(split_string_piece("a:b:c", ':'), ["a", "b", "c"]);
        assert_eq!(split_string_piece("", ':'), [""]);
        assert_eq!(split_string_piece(":", ':'), ["", ""]);
        assert_eq!(split_string_piece(":a:b:c:", ':'), ["", "a", "b", "c", ""]);
        assert_eq!(join_string_piece(&["a", "b", "c"], ':'), "a:b:c");
        assert_eq!(join_string_piece(&["a", "b", "c"], '/'), "a/b/c");
        assert_eq!(join_string_piece(&[], ':'), "");
        assert_eq!(to_lower_ascii('A'), 'a');
        assert_eq!(to_lower_ascii('/'), '/');
        assert!(equals_case_insensitive_ascii("AbC", "aBc"));
        assert!(!equals_case_insensitive_ascii("a", "ac"));
        assert!(!equals_case_insensitive_ascii("/", "\\"));
    }

    #[test]
    fn ninja_string_piece_basic_and_substring_cases() {
        let empty = StringPiece::new(b"");
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
        assert_eq!(empty.as_str(), "");

        let source = b"abc";
        let value = StringPiece::new(source);
        assert_eq!(value.len(), 3);
        assert_eq!(value.as_str(), "abc");
        assert_eq!(StringPiece::from_cstr("abcd\0ef").as_str(), "abcd");

        let value = StringPiece::from_cstr("abc");
        assert_eq!(value.substr(0, None).as_str(), "abc");
        assert_eq!(value.substr(0, Some(0)).as_str(), "");
        assert_eq!(value.substr(0, Some(4)).as_str(), "abc");
        assert_eq!(value.substr(1, Some(1)).as_str(), "b");
        assert_eq!(value.substr(2, None).as_str(), "c");
        assert_eq!(value.substr(3, None).as_str(), "");
    }
}
