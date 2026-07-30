//! MSVC show-includes parsing compatible with Ninja's clparser source.

#[cfg(test)]
use crate::error::ToolError;
use crate::util::{canonpath, BString};
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct ClParser {
    pub(crate) includes: BTreeSet<BString>,
}

pub(crate) fn filter_show_includes<'a>(line: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    let prefix = if prefix.is_empty() {
        b"Note: including file:".as_slice()
    } else {
        prefix
    };
    let suffix = line.strip_prefix(prefix)?.trim_ascii_start();
    (!suffix.is_empty()).then_some(suffix)
}

fn ends_with_ascii_case_insensitive(bytes: &[u8], suffix: &[u8]) -> bool {
    bytes
        .get(bytes.len().saturating_sub(suffix.len())..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
}

pub(crate) fn filter_input_filename(line: &[u8]) -> bool {
    [".c", ".cc", ".cpp", ".cxx", ".c++"]
        .iter()
        .any(|extension| ends_with_ascii_case_insensitive(line, extension.as_bytes()))
}

fn normalize_include(path: &[u8]) -> BString {
    let mut path = BString::from(
        path.iter()
            .map(|byte| if *byte == b'\\' { b'/' } else { *byte })
            .collect::<Vec<_>>(),
    );
    canonpath(&mut path);
    path
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

fn system_include(path: &[u8]) -> bool {
    contains_ascii_case_insensitive(path, b"program files")
        || contains_ascii_case_insensitive(path, b"microsoft visual studio")
}

#[cfg(test)]
fn path_parts(path: &str) -> (Option<String>, bool, Vec<String>) {
    let path = path.replace('\\', "/");
    let (drive, path) = if path.as_bytes().get(1) == Some(&b':') {
        (Some(path[..2].to_owned()), &path[2..])
    } else {
        (None, path.as_str())
    };
    let absolute = path.starts_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            ".." if parts.last().is_some_and(|last: &String| last != "..") => {
                parts.pop();
            }
            ".." if !absolute => parts.push(part.to_owned()),
            "" | "." | ".." => {}
            _ => parts.push(part.to_owned()),
        }
    }
    (drive, absolute, parts)
}

#[cfg(test)]
fn relative_parts(base: &[String], target: &[String]) -> String {
    let common = base
        .iter()
        .zip(target)
        .take_while(|(left, right)| left.eq_ignore_ascii_case(right))
        .count();
    let mut result = Vec::new();
    result.extend(std::iter::repeat_n("..".to_owned(), base.len() - common));
    result.extend(target[common..].iter().cloned());
    if result.is_empty() {
        ".".into()
    } else {
        result.join("/")
    }
}

#[cfg(test)]
pub(crate) fn normalize_include_path(
    path: &str,
    relative_to: &str,
    current_directory: &str,
) -> Result<String, ToolError> {
    if path.len() > 260 || relative_to.len() > 260 {
        return Err(ToolError::PathTooLong);
    }
    let (path_drive, path_absolute, path_components) = path_parts(path);
    let (relative_drive, relative_absolute, relative_components) = path_parts(relative_to);
    if path_drive.is_some() || relative_drive.is_some() {
        if path_drive
            .as_deref()
            .zip(relative_drive.as_deref())
            .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
        {
            return Ok(relative_parts(&relative_components, &path_components));
        }
        let mut result = path_drive.unwrap_or_default();
        if path_absolute {
            result.push('/');
        }
        result.push_str(&path_components.join("/"));
        return Ok(result);
    }

    let (_, _, mut cwd) = path_parts(current_directory);
    if relative_absolute {
        cwd = relative_components;
    } else {
        for part in relative_components {
            if part == ".." {
                cwd.pop();
            } else {
                cwd.push(part);
            }
        }
    }
    let base = cwd;

    let (_, _, mut target) = path_parts(current_directory);
    if path_absolute {
        target = path_components;
    } else {
        for part in path_components {
            if part == ".." {
                target.pop();
            } else {
                target.push(part);
            }
        }
    }
    Ok(relative_parts(&base, &target))
}

#[cfg(test)]
pub(crate) fn escape_for_depfile(path: &str) -> String {
    path.replace(' ', "\\ ")
}

impl ClParser {
    // [spec:samurai:req:runtime.msvc-byte-parsing]
    pub(crate) fn parse(&mut self, input: &[u8], prefix: &[u8]) -> BString {
        let mut output = Vec::new();
        let mut saw_include = false;
        let mut start = 0;
        while start < input.len() {
            let end = input[start..]
                .iter()
                .position(|byte| matches!(byte, b'\r' | b'\n'))
                .map_or(input.len(), |offset| start + offset);
            let line = &input[start..end];
            if let Some(path) = filter_show_includes(line, prefix) {
                saw_include = true;
                if !system_include(path) {
                    self.includes.insert(normalize_include(path));
                }
            } else if saw_include || !filter_input_filename(line) {
                output.extend_from_slice(line);
                output.push(b'\n');
            }
            start = end;
            if input.get(start) == Some(&b'\r') {
                start += 1;
            }
            if input.get(start) == Some(&b'\n') {
                start += 1;
            }
        }
        BString::from(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cases adapted from Ninja's src/clparser_test.cc.
    #[test]
    fn ninja_clparser_show_includes_and_filename_filter() {
        assert_eq!(filter_show_includes(b"Sample compiler output", b""), None);
        assert_eq!(
            filter_show_includes(b"Note: including file:    c:\\initspaces.h", b""),
            Some(b"c:\\initspaces.h".as_slice())
        );
        assert_eq!(
            filter_show_includes(
                b"Non-default prefix: inc file:    c:\\initspaces.h",
                b"Non-default prefix: inc file:"
            ),
            Some(b"c:\\initspaces.h".as_slice())
        );
        assert!(filter_input_filename(b"foobar.cc"));
        assert!(filter_input_filename(b"foo bar.cc"));
        assert!(filter_input_filename(b"baz.c"));
        assert!(filter_input_filename(b"FOOBAR.CC"));
        assert!(filter_input_filename(b"foobar.c++"));
        assert!(!filter_input_filename(
            b"src\\cl_helper.cc(166) : fatal error C1075: end of file"
        ));
    }

    // [spec:samurai:req:runtime.msvc-byte-parsing/test]
    #[test]
    fn ninja_clparser_parse_and_deduplicate_includes() {
        let mut parser = ClParser::default();
        assert_eq!(
            parser.parse(
                b"foo\r\nNote: inc file prefix:  foo.h\r\nbar\r\n",
                b"Note: inc file prefix:"
            ),
            b"foo\nbar\n"
        );
        assert_eq!(parser.includes, BTreeSet::from(["foo.h".into()]));

        let mut parser = ClParser::default();
        assert_eq!(
            parser.parse(b"foo.cc\r\ncl: warning\r\n", b""),
            b"cl: warning\n"
        );
        assert_eq!(
            parser.parse(b"foo.cc\rcl: warning\r", b""),
            b"cl: warning\n"
        );
        assert_eq!(
            parser.parse(
                b"foo.cc\r\nNote: including file: foo.h\r\nsomething something foo.cc\r\n",
                b""
            ),
            b"something something foo.cc\n"
        );

        let mut parser = ClParser::default();
        assert_eq!(
            parser.parse(
                b"Note: including file: c:\\PrOgRaM FiLeS\\foo.h\r\n\
                  Note: including file: d:\\mIcRoSoFt ViSuAl StUdIo\\bar.h\r\n\
                  Note: including file: path.h\r\n",
                b""
            ),
            b""
        );
        assert_eq!(parser.includes, BTreeSet::from(["path.h".into()]));

        let mut parser = ClParser::default();
        parser.parse(
            b"Note: including file: sub/./foo.h\r\n\
              Note: including file: bar.h\r\n\
              Note: including file: sub/foo.h\r\n",
            b"",
        );
        assert_eq!(
            parser.includes,
            BTreeSet::from(["bar.h".into(), "sub/foo.h".into()])
        );

        let mut parser = ClParser::default();
        assert_eq!(
            parser.parse(
                b"source.cc\r\nNote: including file: inc-\xff.h\r\nwarning-\xfe\r\n",
                b""
            ),
            b"warning-\xfe\n"
        );
        assert_eq!(
            parser.includes,
            BTreeSet::from([BString::from(b"inc-\xff.h")])
        );
    }

    // Cases adapted from Ninja's src/includes_normalize_test.cc.
    #[test]
    fn ninja_include_normalization_cases() {
        let current_directory = std::env::current_dir().unwrap();
        let current_directory = current_directory.to_string_lossy();
        assert_eq!(
            normalize_include_path("a\\..\\b", ".", &current_directory).unwrap(),
            "b"
        );
        assert_eq!(
            normalize_include_path("a\\./b", ".", &current_directory).unwrap(),
            "a/b"
        );
        assert_eq!(
            normalize_include_path("a/b/c", "a/b", &current_directory).unwrap(),
            "c"
        );
        assert_eq!(
            normalize_include_path("a", "b/c", &current_directory).unwrap(),
            "../../a"
        );
        assert_eq!(
            normalize_include_path("a", "a", &current_directory).unwrap(),
            "."
        );
        assert_eq!(
            normalize_include_path("p:\\vs08\\stuff.h", "p:\\vs08", &current_directory).unwrap(),
            "stuff.h"
        );
        assert_eq!(
            normalize_include_path("P:\\Vs08\\stuff.h", "p:\\vs08", &current_directory).unwrap(),
            "stuff.h"
        );
        assert_eq!(
            normalize_include_path("P:/vs08\\stufF.h", "D:\\stuff/things", &current_directory,)
                .unwrap(),
            "P:/vs08/stufF.h"
        );
        assert_eq!(
            normalize_include_path(
                "P:/vs08\\../wee\\stuff.h",
                "D:\\stuff/things",
                &current_directory,
            )
            .unwrap(),
            "P:/wee/stuff.h"
        );
        assert_eq!(
            normalize_include_path(&"a".repeat(261), ".", &current_directory)
                .unwrap_err()
                .to_string(),
            "path too long"
        );
    }

    #[test]
    fn ninja_escape_for_depfile_spaces() {
        assert_eq!(
            escape_for_depfile("sub\\some sdk\\foo.h"),
            "sub\\some\\ sdk\\foo.h"
        );
    }
}
