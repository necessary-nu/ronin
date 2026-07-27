//! MSVC show-includes parsing compatible with Ninja's clparser source.

use crate::util::{canonpath, xasprintf};
use std::collections::BTreeSet;

#[derive(Default)]
pub struct ClParser {
    pub includes: BTreeSet<String>,
}

pub fn filter_show_includes<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let prefix = if prefix.is_empty() {
        "Note: including file:"
    } else {
        prefix
    };
    line.strip_prefix(prefix).map(str::trim_start)
}

pub fn filter_input_filename(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    !line.contains('(')
        && [".c", ".cc", ".cpp", ".cxx"]
            .iter()
            .any(|extension| lower.ends_with(extension))
}

fn normalize_include(path: &str) -> String {
    let path = path.replace('\\', "/");
    let mut path = xasprintf(format_args!("{path}"));
    canonpath(&mut path);
    String::from_utf8_lossy(&path.s[..path.n]).into_owned()
}

fn system_include(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.contains("program files") || path.contains("microsoft visual studio")
}

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
            "" | "." => {}
            ".." if parts.last().is_some_and(|last: &String| last != "..") => {
                parts.pop();
            }
            ".." if !absolute => parts.push(part.to_owned()),
            ".." => {}
            _ => parts.push(part.to_owned()),
        }
    }
    (drive, absolute, parts)
}

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

pub fn normalize_include_path(path: &str, relative_to: &str) -> Result<String, String> {
    if path.len() > 260 || relative_to.len() > 260 {
        return Err("path too long".into());
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

    let (_, _, mut cwd) = path_parts(
        &std::env::current_dir()
            .map_err(|error| error.to_string())?
            .to_string_lossy(),
    );
    if !relative_absolute {
        for part in relative_components {
            if part == ".." {
                cwd.pop();
            } else {
                cwd.push(part);
            }
        }
    } else {
        cwd = relative_components;
    }
    let base = cwd;

    let (_, _, mut target) = path_parts(
        &std::env::current_dir()
            .map_err(|error| error.to_string())?
            .to_string_lossy(),
    );
    if !path_absolute {
        for part in path_components {
            if part == ".." {
                target.pop();
            } else {
                target.push(part);
            }
        }
    } else {
        target = path_components;
    }
    Ok(relative_parts(&base, &target))
}

pub fn escape_for_depfile(path: &str) -> String {
    path.replace(' ', "\\ ")
}

impl ClParser {
    pub fn parse(&mut self, input: &str, prefix: &str) -> String {
        let mut output = String::new();
        let mut saw_include = false;
        for line in input.lines() {
            if let Some(path) = filter_show_includes(line, prefix) {
                saw_include = true;
                if !system_include(path) {
                    self.includes.insert(normalize_include(path));
                }
            } else if saw_include || !filter_input_filename(line) {
                output.push_str(line);
                output.push('\n');
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cases adapted from Ninja's src/clparser_test.cc.
    #[test]
    fn ninja_clparser_show_includes_and_filename_filter() {
        assert_eq!(filter_show_includes("Sample compiler output", ""), None);
        assert_eq!(
            filter_show_includes("Note: including file:    c:\\initspaces.h", ""),
            Some("c:\\initspaces.h")
        );
        assert_eq!(
            filter_show_includes(
                "Non-default prefix: inc file:    c:\\initspaces.h",
                "Non-default prefix: inc file:"
            ),
            Some("c:\\initspaces.h")
        );
        assert!(filter_input_filename("foobar.cc"));
        assert!(filter_input_filename("foo bar.cc"));
        assert!(filter_input_filename("baz.c"));
        assert!(filter_input_filename("FOOBAR.CC"));
        assert!(!filter_input_filename(
            "src\\cl_helper.cc(166) : fatal error C1075: end of file"
        ));
    }

    #[test]
    fn ninja_clparser_parse_and_deduplicate_includes() {
        let mut parser = ClParser::default();
        assert_eq!(
            parser.parse(
                "foo\r\nNote: inc file prefix:  foo.h\r\nbar\r\n",
                "Note: inc file prefix:"
            ),
            "foo\nbar\n"
        );
        assert_eq!(parser.includes, BTreeSet::from(["foo.h".into()]));

        let mut parser = ClParser::default();
        assert_eq!(
            parser.parse("foo.cc\r\ncl: warning\r\n", ""),
            "cl: warning\n"
        );
        assert_eq!(
            parser.parse(
                "foo.cc\r\nNote: including file: foo.h\r\nsomething something foo.cc\r\n",
                ""
            ),
            "something something foo.cc\n"
        );

        let mut parser = ClParser::default();
        assert_eq!(
            parser.parse(
                "Note: including file: c:\\Program Files\\foo.h\r\n\
                 Note: including file: d:\\Microsoft Visual Studio\\bar.h\r\n\
                 Note: including file: path.h\r\n",
                ""
            ),
            ""
        );
        assert_eq!(parser.includes, BTreeSet::from(["path.h".into()]));

        let mut parser = ClParser::default();
        parser.parse(
            "Note: including file: sub/./foo.h\r\n\
             Note: including file: bar.h\r\n\
             Note: including file: sub/foo.h\r\n",
            "",
        );
        assert_eq!(
            parser.includes,
            BTreeSet::from(["bar.h".into(), "sub/foo.h".into()])
        );
    }

    // Cases adapted from Ninja's src/includes_normalize_test.cc.
    #[test]
    fn ninja_include_normalization_cases() {
        assert_eq!(normalize_include_path("a\\..\\b", ".").unwrap(), "b");
        assert_eq!(normalize_include_path("a\\./b", ".").unwrap(), "a/b");
        assert_eq!(normalize_include_path("a/b/c", "a/b").unwrap(), "c");
        assert_eq!(normalize_include_path("a", "b/c").unwrap(), "../../a");
        assert_eq!(normalize_include_path("a", "a").unwrap(), ".");
        assert_eq!(
            normalize_include_path("p:\\vs08\\stuff.h", "p:\\vs08").unwrap(),
            "stuff.h"
        );
        assert_eq!(
            normalize_include_path("P:\\Vs08\\stuff.h", "p:\\vs08").unwrap(),
            "stuff.h"
        );
        assert_eq!(
            normalize_include_path("P:/vs08\\stufF.h", "D:\\stuff/things").unwrap(),
            "P:/vs08/stufF.h"
        );
        assert_eq!(
            normalize_include_path("P:/vs08\\../wee\\stuff.h", "D:\\stuff/things").unwrap(),
            "P:/wee/stuff.h"
        );
        assert_eq!(
            normalize_include_path(&"a".repeat(261), ".").unwrap_err(),
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
