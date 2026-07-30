//! Byte-exact GNU-make depfile parsing.
//!
//! Kept separate from `.ninja_deps` persistence: this module turns depfile
//! bytes into ordered, deduplicated path sets, and its caller turns those into
//! graph nodes.

use crate::error::{DepfileProblem, PersistenceError};
use crate::htab::RapidHashMap;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ParsedDepfile {
    pub(super) outputs: Vec<Vec<u8>>,
    pub(super) inputs: Vec<Vec<u8>>,
}

/// Rule-local tokens in source order, holding their buffers across lines.
///
/// Rule-local duplicates need no removal because the accumulating
/// [`OrderedPaths`] deduplicate, and neither the nested-input check nor the
/// merge depends on within-rule uniqueness.
#[derive(Default)]
struct TokenList {
    buffers: Vec<Vec<u8>>,
    len: usize,
}

impl TokenList {
    const fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, token: &[u8]) {
        if self.len == self.buffers.len() {
            self.buffers.push(Vec::new());
        }
        let buffer = &mut self.buffers[self.len];
        buffer.clear();
        buffer.extend_from_slice(token);
        self.len += 1;
    }

    const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn iter(&self) -> impl Iterator<Item = &[u8]> {
        self.buffers[..self.len].iter().map(Vec::as_slice)
    }
}

/// Buffers reused for every rule in one depfile.
#[derive(Default)]
struct DepfileScratch {
    token: Vec<u8>,
    outputs: TokenList,
    inputs: TokenList,
}

fn finish_token(scratch: &mut DepfileScratch, inputs_started: bool) {
    if scratch.token.is_empty() {
        return;
    }
    if inputs_started {
        scratch.inputs.push(&scratch.token);
    } else {
        scratch.outputs.push(&scratch.token);
    }
    scratch.token.clear();
}

#[derive(Default)]
struct OrderedPaths {
    indices: RapidHashMap<Vec<u8>, usize>,
}

impl OrderedPaths {
    /// Record a path, allocating only the first time it is seen.
    ///
    /// Depfiles repeat the same headers across every rule, so taking a slice
    /// and copying on first sight keeps repeats allocation-free.
    fn insert(&mut self, path: &[u8]) {
        if self.indices.contains_key(path) {
            return;
        }
        let next = self.indices.len();
        self.indices.insert(path.to_vec(), next);
    }

    fn contains(&self, path: &[u8]) -> bool {
        self.indices.contains_key(path)
    }

    fn into_vec(self) -> Vec<Vec<u8>> {
        let mut paths = vec![Vec::new(); self.indices.len()];
        for (path, index) in self.indices {
            paths[index] = path;
        }
        paths
    }
}

/// Tokenize one logical rule line into the scratch token lists.
///
/// Returns whether the line held a rule at all.
fn parse_depfile_rule(line: &[u8], scratch: &mut DepfileScratch) -> Result<bool, PersistenceError> {
    scratch.token.clear();
    scratch.outputs.clear();
    scratch.inputs.clear();
    let mut inputs_started = false;
    let mut index = 0;
    let mut saw_non_whitespace = false;

    while index < line.len() {
        match line[index] {
            b' ' | b'\t' | b'\r' => {
                finish_token(scratch, inputs_started);
                index += 1;
            }
            b'$' => {
                saw_non_whitespace = true;
                if line.get(index + 1) != Some(&b'$') {
                    return Err(PersistenceError::depfile(DepfileProblem::VariableReference));
                }
                scratch.token.push(b'$');
                index += 2;
            }
            b'\\' => {
                saw_non_whitespace = true;
                let start = index;
                while index < line.len() && line[index] == b'\\' {
                    index += 1;
                }
                let slashes = index - start;
                match line.get(index).copied() {
                    Some(b' ' | b'\t') if slashes % 2 == 1 => {
                        scratch
                            .token
                            .extend(std::iter::repeat_n(b'\\', slashes / 2));
                        scratch.token.push(line[index]);
                        index += 1;
                    }
                    Some(b'#') => {
                        scratch
                            .token
                            .extend(std::iter::repeat_n(b'\\', slashes / 2));
                        if slashes % 2 == 1 {
                            scratch.token.push(b'#');
                            index += 1;
                        }
                    }
                    _ => scratch.token.extend(std::iter::repeat_n(b'\\', slashes)),
                }
            }
            b':' if !inputs_started => {
                saw_non_whitespace = true;
                if scratch.token.len() == 1
                    && scratch.token[0].is_ascii_alphabetic()
                    && matches!(line.get(index + 1), Some(b'/' | b'\\'))
                {
                    scratch.token.push(b':');
                    index += 1;
                    continue;
                }
                if scratch.token.ends_with(b"\\")
                    && scratch.token.len() == 2
                    && line.get(index + 1) == Some(&b'\\')
                {
                    scratch.token.pop();
                    scratch.token.push(b':');
                    index += 1;
                    continue;
                }
                finish_token(scratch, false);
                inputs_started = true;
                index += 1;
            }
            character => {
                saw_non_whitespace = true;
                scratch.token.push(character);
                index += 1;
            }
        }
    }
    finish_token(scratch, inputs_started);
    if !saw_non_whitespace {
        return Ok(false);
    }
    if !inputs_started {
        return Err(PersistenceError::depfile(DepfileProblem::MissingColon));
    }
    Ok(true)
}

fn merge_depfile_rule(
    line: &[u8],
    scratch: &mut DepfileScratch,
    outputs: &mut OrderedPaths,
    inputs: &mut OrderedPaths,
) -> Result<(), PersistenceError> {
    if !parse_depfile_rule(line, scratch)? {
        return Ok(());
    }
    let output_is_input = scratch.outputs.iter().any(|output| inputs.contains(output));
    if output_is_input && !scratch.inputs.is_empty() {
        return Err(PersistenceError::depfile(DepfileProblem::NestedInputs));
    }
    if !output_is_input {
        for output in scratch.outputs.iter() {
            outputs.insert(output);
        }
        for input in scratch.inputs.iter() {
            inputs.insert(input);
        }
    }
    Ok(())
}

pub(super) fn parse_depfile(text: &[u8]) -> Result<ParsedDepfile, PersistenceError> {
    let mut outputs = OrderedPaths::default();
    let mut inputs = OrderedPaths::default();
    let mut scratch = DepfileScratch::default();
    let mut line = Vec::new();
    let mut index = 0;
    while index < text.len() {
        if text[index] == b'\\' {
            let start = index;
            while index < text.len() && text[index] == b'\\' {
                index += 1;
            }
            let slashes = index - start;
            let newline = match text.get(index..) {
                Some([b'\r', b'\n', ..]) => Some(2),
                Some([b'\n', ..]) => Some(1),
                _ => None,
            };
            if let Some(newline) = newline.filter(|_| slashes % 2 == 1) {
                line.extend(std::iter::repeat_n(b'\\', slashes / 2));
                line.push(b' ');
                index += newline;
            } else {
                line.extend(std::iter::repeat_n(b'\\', slashes));
            }
            continue;
        }
        if text[index] == b'\n' {
            merge_depfile_rule(&line, &mut scratch, &mut outputs, &mut inputs)?;
            line.clear();
            index += 1;
            continue;
        }
        line.push(text[index]);
        index += 1;
    }
    merge_depfile_rule(&line, &mut scratch, &mut outputs, &mut inputs)?;
    Ok(ParsedDepfile {
        outputs: outputs.into_vec(),
        inputs: inputs.into_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_depfile(input: &str, outputs: &[&str], inputs: &[&str]) {
        let parsed = parse_depfile(input.as_bytes()).unwrap();
        assert_eq!(
            parsed.outputs,
            outputs
                .iter()
                .map(|path| path.as_bytes().to_vec())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            parsed.inputs,
            inputs
                .iter()
                .map(|path| path.as_bytes().to_vec())
                .collect::<Vec<_>>()
        );
    }

    // Cases adapted from Ninja's src/depfile_parser_test.cc.
    #[test]
    fn ninja_depfile_parser_core_cases() {
        assert_depfile(
            "build/ninja.o: ninja.cc ninja.h eval_env.h manifest_parser.h\n",
            &["build/ninja.o"],
            &["ninja.cc", "ninja.h", "eval_env.h", "manifest_parser.h"],
        );
        assert_depfile(" \\\n  out: in\n", &["out"], &["in"]);
        assert_depfile(
            "foo.o: \\\n  bar.h baz.h\n",
            &["foo.o"],
            &["bar.h", "baz.h"],
        );
        assert_depfile("foo.o: //?/c:/bar.h\n", &["foo.o"], &["//?/c:/bar.h"]);
        assert_depfile(
            "foo&bar.o foo'bar.o foo\"bar.o: foo&bar.h foo'bar.h foo\"bar.h\n",
            &["foo&bar.o", "foo'bar.o", "foo\"bar.o"],
            &["foo&bar.h", "foo'bar.h", "foo\"bar.h"],
        );
        assert_depfile(
            "foo.o: \\\r\n  bar.h baz.h\r\n",
            &["foo.o"],
            &["bar.h", "baz.h"],
        );
        assert_depfile(
            "Project\\Dir\\Build\\Release8\\Foo\\Foo.res : \\\n  Dir\\Library\\Foo.rc \\\n  Dir\\Library\\Version\\Bar.h \\\n  Dir\\Library\\Foo.ico \\\n  Project\\Thing\\Bar.tlb \\\n",
            &["Project\\Dir\\Build\\Release8\\Foo\\Foo.res"],
            &[
                "Dir\\Library\\Foo.rc",
                "Dir\\Library\\Version\\Bar.h",
                "Dir\\Library\\Foo.ico",
                "Project\\Thing\\Bar.tlb",
            ],
        );
        assert_depfile(
            "a\\ bc\\ def:   a\\ b c d",
            &["a bc def"],
            &["a b", "c", "d"],
        );
        assert_depfile(
            "a\\ b\\#c.h: \\\\\\\\\\  \\\\\\\\ \\\\share\\info\\\\#1",
            &["a b#c.h"],
            &["\\\\ ", "\\\\\\\\", "\\\\share\\info\\#1"],
        );
        assert_depfile(
            "\\!\\@\\#$$\\%\\^\\&\\[\\]\\\\:",
            &["\\!\\@#$\\%\\^\\&\\[\\]\\\\"],
            &[],
        );
        assert_depfile(
            "c\\:\\gcc\\x86_64-w64-mingw32\\include\\stddef.o: \\\n c:\\gcc\\x86_64-w64-mingw32\\include\\stddef.h \n",
            &["c:\\gcc\\x86_64-w64-mingw32\\include\\stddef.o"],
            &["c:\\gcc\\x86_64-w64-mingw32\\include\\stddef.h"],
        );
        assert_depfile(
            "foo1\\: x\nfoo1\\:\nfoo1\\:\r\nfoo1\\:\t\nfoo1\\:",
            &["foo1\\"],
            &["x"],
        );
        assert_depfile(
            "C:/Program\\ Files\\ (x86)/Microsoft\\ crtdefs.h: \\\n en@quot.header~ t+t-x!=1 \\\n openldap/slapd.d/cn=config/cn=schema/cn={0}core.ldif\\\n Fußball\\\n a[1]b@2%c",
            &["C:/Program Files (x86)/Microsoft crtdefs.h"],
            &[
                "en@quot.header~",
                "t+t-x!=1",
                "openldap/slapd.d/cn=config/cn=schema/cn={0}core.ldif",
                "Fußball",
                "a[1]b@2%c",
            ],
        );
    }

    #[test]
    fn ninja_depfile_parser_multi_rule_cases() {
        assert_depfile("foo foo: x y z", &["foo"], &["x", "y", "z"]);
        assert_depfile("foo bar: x y z", &["foo", "bar"], &["x", "y", "z"]);
        assert_depfile("foo: x\nfoo: \nfoo:\n", &["foo"], &["x"]);
        assert_depfile(
            "foo: x\nfoo: y\nfoo \\\nfoo: z\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile(
            "foo: x\r\nfoo: y\r\nfoo \\\r\nfoo: z\r\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile(
            "foo: x\\\n     y\nfoo \\\nfoo: z\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile(
            "foo: x\\\r\n     y\r\nfoo \\\r\nfoo: z\r\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile(" foo: x\n foo: y\n foo: z\n", &["foo"], &["x", "y", "z"]);
        assert_depfile(
            " foo: x\r\n foo: y\r\n foo: z\r\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile("foo: x y z\nx:\ny:\nz:\n", &["foo"], &["x", "y", "z"]);
        assert_depfile(
            "foo: x\nx:\nfoo: y\ny:\nfoo: z\nz:\n",
            &["foo"],
            &["x", "y", "z"],
        );
        assert_depfile("foo: x y\nbar: y z\n", &["foo", "bar"], &["x", "y", "z"]);
        assert_depfile("", &[], &[]);
        assert_depfile("\n\n", &[], &[]);
    }

    #[test]
    fn ninja_depfile_parser_rejects_invalid_rules() {
        assert_eq!(
            parse_depfile(b"foo: x y z\nx: alsoin\ny:\nz:\n")
                .unwrap_err()
                .to_string(),
            "inputs may not also have inputs"
        );
        assert_eq!(
            parse_depfile(b"foo.o foo.c\n").unwrap_err().to_string(),
            "expected ':' in depfile"
        );
    }

    #[test]
    fn ninja_depfile_parser_preserves_non_utf8_paths() {
        let parsed = parse_depfile(b"out: in-\xff.h in-\xff.h\n").unwrap();
        assert_eq!(parsed.outputs, [b"out".to_vec()]);
        assert_eq!(parsed.inputs, [b"in-\xff.h".to_vec()]);
    }
}
