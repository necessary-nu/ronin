//! Byte-oriented Ninja manifest lexer.

use crate::util::{BString, EvalPart, EvalString};
use std::fs;
use std::path::{Path, PathBuf};

// [spec:samurai:def:scan.token]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Token {
    Build,
    Default,
    Include,
    Pool,
    Rule,
    Subninja,
    Variable,
}

// [spec:samurai:def:scan.scanner]
pub struct Scanner {
    pub path: PathBuf,
    input: Vec<u8>,
    index: usize,
    pub line: usize,
    pub col: usize,
    pub paths: Vec<EvalString>,
    variable: Option<String>,
    continuation_at_eof: bool,
    pub manifest_version_major: i32,
    pub manifest_version_minor: i32,
}

impl Scanner {
    pub(crate) fn current(&self) -> Option<u8> {
        self.input.get(self.index).copied()
    }

    pub(crate) fn take_variable(&mut self) -> Option<String> {
        self.variable.take()
    }
}

// [spec:samurai:def:scan.scaninit-fn]
// [spec:samurai:sem:scan.scaninit-fn]
pub fn scaninit(path: impl AsRef<Path>) -> Result<Scanner, String> {
    let path = path.as_ref();
    let input = fs::read(path).map_err(|error| error.to_string())?;
    Ok(scanfrombytes(path, input))
}

pub(crate) fn scanfrombytes(path: impl AsRef<Path>, input: Vec<u8>) -> Scanner {
    let path = path.as_ref();
    Scanner {
        path: path.to_owned(),
        input,
        index: 0,
        line: 1,
        col: 1,
        paths: Vec::new(),
        variable: None,
        continuation_at_eof: false,
        manifest_version_major: 1,
        manifest_version_minor: 9,
    }
}

// [spec:samurai:def:scan.scanclose-fn]
// [spec:samurai:sem:scan.scanclose-fn]
pub fn scanclose(_scanner: Scanner) {}

// [spec:samurai:def:scan.scanerror-fn]
// [spec:samurai:sem:scan.scanerror-fn]
pub fn scanerror(scanner: &Scanner, message: &str) -> String {
    format!(
        "{}:{}:{}: {message}",
        scanner.path.display(),
        scanner.line,
        scanner.col
    )
}

// [spec:samurai:def:scan.next-fn]
// [spec:samurai:sem:scan.next-fn]
fn next(scanner: &mut Scanner) -> Option<u8> {
    if scanner.current() == Some(b'\n') {
        scanner.line += 1;
        scanner.col = 1;
    } else {
        scanner.col += 1;
    }
    scanner.index += 1;
    scanner.current()
}

// [spec:samurai:def:scan.issimplevar-fn]
// [spec:samurai:sem:scan.issimplevar-fn]
fn issimplevar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

// [spec:samurai:def:scan.isvar-fn]
// [spec:samurai:sem:scan.isvar-fn]
fn isvar(byte: u8) -> bool {
    issimplevar(byte) || byte == b'.'
}

// [spec:samurai:def:scan.newline-fn]
// [spec:samurai:sem:scan.newline-fn]
fn newline(scanner: &mut Scanner) -> Result<bool, String> {
    match scanner.current() {
        Some(b'\r') => {
            next(scanner);
            if scanner.current() != Some(b'\n') {
                return Err(scanerror(scanner, "expected '\\n' after '\\r'"));
            }
            next(scanner);
            Ok(true)
        }
        Some(b'\n') => {
            next(scanner);
            Ok(true)
        }
        _ => Ok(false),
    }
}

// [spec:samurai:def:scan.singlespace-fn]
// [spec:samurai:sem:scan.singlespace-fn]
fn singlespace(scanner: &mut Scanner) -> Result<bool, String> {
    match scanner.current() {
        Some(b' ') => {
            next(scanner);
            Ok(true)
        }
        Some(b'\t') => Err(scanerror(scanner, "tabs are not allowed, use spaces")),
        Some(b'$') => {
            let index = scanner.index;
            let line = scanner.line;
            let col = scanner.col;
            next(scanner);
            if newline(scanner)? {
                scanner.continuation_at_eof = scanner.current().is_none();
                Ok(true)
            } else {
                scanner.index = index;
                scanner.line = line;
                scanner.col = col;
                Ok(false)
            }
        }
        _ => Ok(false),
    }
}

// [spec:samurai:def:scan.space-fn]
// [spec:samurai:sem:scan.space-fn]
fn space(scanner: &mut Scanner) -> Result<bool, String> {
    let mut found = false;
    while singlespace(scanner)? {
        found = true;
    }
    Ok(found)
}

// [spec:samurai:def:scan.comment-fn]
// [spec:samurai:sem:scan.comment-fn]
fn comment(scanner: &mut Scanner) -> Result<bool, String> {
    if scanner.current() != Some(b'#') {
        return Ok(false);
    }
    while scanner.current().is_some() && !newline(scanner)? {
        next(scanner);
    }
    Ok(true)
}

// [spec:samurai:def:scan.name-fn]
// [spec:samurai:sem:scan.name-fn]
fn name(scanner: &mut Scanner) -> Result<String, String> {
    let start = scanner.index;
    while scanner.current().is_some_and(isvar) {
        next(scanner);
    }
    if scanner.index == start {
        return Err(scanerror(scanner, "expected name"));
    }
    let name = std::str::from_utf8(&scanner.input[start..scanner.index])
        .expect("variable names are ASCII")
        .to_owned();
    space(scanner)?;
    Ok(name)
}

// [spec:samurai:def:scan.scankeyword-fn]
// [spec:samurai:sem:scan.scankeyword-fn]
pub fn scankeyword(scanner: &mut Scanner) -> Result<Option<Token>, String> {
    loop {
        match scanner.current() {
            None => return Ok(None),
            Some(b' ' | b'\t') => {
                space(scanner)?;
                if !comment(scanner)? && !newline(scanner)? {
                    return Err(scanerror(scanner, "unexpected indent"));
                }
            }
            Some(b'#') => {
                comment(scanner)?;
            }
            Some(b'\r' | b'\n') => {
                newline(scanner)?;
            }
            _ => {
                let name = name(scanner)?;
                let token = match name.as_str() {
                    "build" => Token::Build,
                    "default" => Token::Default,
                    "include" => Token::Include,
                    "pool" => Token::Pool,
                    "rule" => Token::Rule,
                    "subninja" => Token::Subninja,
                    _ => {
                        scanner.variable = Some(name);
                        Token::Variable
                    }
                };
                return Ok(Some(token));
            }
        }
    }
}

// [spec:samurai:def:scan.scanname-fn]
// [spec:samurai:sem:scan.scanname-fn]
pub fn scanname(scanner: &mut Scanner) -> Result<String, String> {
    name(scanner)
}

// [spec:samurai:def:scan.addstringpart-fn]
// [spec:samurai:sem:scan.addstringpart-fn]
fn addstringpart(parts: &mut Vec<EvalPart>, bytes: Vec<u8>, variable: bool) {
    if variable {
        parts.push(EvalPart::Variable(
            String::from_utf8(bytes).expect("variable names are ASCII"),
        ));
    } else if !bytes.is_empty() {
        parts.push(EvalPart::Literal(BString::from(bytes)));
    }
}

fn flush_literal(parts: &mut Vec<EvalPart>, literal: &mut Vec<u8>) {
    if !literal.is_empty() {
        addstringpart(parts, std::mem::take(literal), false);
    }
}

// [spec:samurai:def:scan.escape-fn]
// [spec:samurai:sem:scan.escape-fn]
fn escape(
    scanner: &mut Scanner,
    parts: &mut Vec<EvalPart>,
    literal: &mut Vec<u8>,
) -> Result<(), String> {
    match scanner.current() {
        Some(byte @ (b'$' | b' ' | b':')) => {
            literal.push(byte);
            next(scanner);
        }
        Some(b'{') => {
            flush_literal(parts, literal);
            next(scanner);
            let start = scanner.index;
            while scanner.current().is_some_and(isvar) {
                next(scanner);
            }
            if scanner.current() != Some(b'}') {
                return Err(scanerror(scanner, "invalid variable name"));
            }
            let variable = scanner.input[start..scanner.index].to_vec();
            next(scanner);
            addstringpart(parts, variable, true);
        }
        Some(b'\r' | b'\n') => {
            newline(scanner)?;
            space(scanner)?;
            scanner.continuation_at_eof = scanner.current().is_none();
        }
        Some(b'^') => {
            if scanner.manifest_version_major < 1
                || scanner.manifest_version_major == 1 && scanner.manifest_version_minor < 14
            {
                return Err(scanerror(
                    scanner,
                    "using $^ escape requires specifying 'ninja_required_version' with version greater or equal 1.14",
                ));
            }
            next(scanner);
            literal.push(b'\n');
        }
        _ => {
            flush_literal(parts, literal);
            let start = scanner.index;
            while scanner.current().is_some_and(issimplevar) {
                next(scanner);
            }
            if scanner.index == start {
                return Err(scanerror(scanner, "invalid $ escape"));
            }
            addstringpart(parts, scanner.input[start..scanner.index].to_vec(), true);
        }
    }
    Ok(())
}

// [spec:samurai:def:scan.scanstring-fn]
// [spec:samurai:sem:scan.scanstring-fn]
pub fn scanstring(scanner: &mut Scanner, path: bool) -> Result<Option<EvalString>, String> {
    let mut parts = Vec::new();
    let mut literal = Vec::new();
    loop {
        match scanner.current() {
            Some(b'$') => {
                next(scanner);
                escape(scanner, &mut parts, &mut literal)?;
            }
            Some(b':' | b'|' | b' ') if path => break,
            Some(b'\t') if path => {
                return Err(scanerror(scanner, "tabs are not allowed, use spaces"));
            }
            Some(b'\r' | b'\n') | None => break,
            Some(byte) => {
                scanner.continuation_at_eof = false;
                literal.push(byte);
                next(scanner);
            }
        }
    }
    flush_literal(&mut parts, &mut literal);
    if path {
        space(scanner)?;
    }
    Ok((!parts.is_empty()).then(|| EvalString::from_parts(parts)))
}

// [spec:samurai:def:scan.scanpaths-fn]
// [spec:samurai:sem:scan.scanpaths-fn]
pub fn scanpaths(scanner: &mut Scanner) -> Result<(), String> {
    while let Some(path) = scanstring(scanner, true)? {
        scanner.paths.push(path);
    }
    Ok(())
}

// [spec:samurai:def:scan.scanchar-fn]
// [spec:samurai:sem:scan.scanchar-fn]
pub fn scanchar(scanner: &mut Scanner, expected: char) -> Result<(), String> {
    let expected =
        u8::try_from(expected).map_err(|_| scanerror(scanner, "expected ASCII token"))?;
    if scanner.current() != Some(expected) {
        return Err(scanerror(
            scanner,
            &format!("expected '{}'", char::from(expected)),
        ));
    }
    next(scanner);
    space(scanner)?;
    Ok(())
}

// [spec:samurai:def:scan.scanpipe-fn]
// [spec:samurai:sem:scan.scanpipe-fn]
pub fn scanpipe(scanner: &mut Scanner, allowed: i32) -> Result<i32, String> {
    if scanner.current() != Some(b'|') {
        return Ok(0);
    }
    next(scanner);
    let kind = match scanner.current() {
        Some(b'|') => {
            next(scanner);
            2
        }
        Some(b'@') => {
            next(scanner);
            4
        }
        _ => 1,
    };
    if allowed & kind == 0 {
        return Err(scanerror(
            scanner,
            match kind {
                1 => "unexpected '|'",
                2 => "unexpected '||'",
                _ => "unexpected '|@'",
            },
        ));
    }
    space(scanner)?;
    Ok(kind)
}

// [spec:samurai:def:scan.scanindent-fn]
// [spec:samurai:sem:scan.scanindent-fn]
pub fn scanindent(scanner: &mut Scanner) -> Result<bool, String> {
    loop {
        let indent = space(scanner)?;
        if !comment(scanner)? {
            return Ok(indent && !newline(scanner)?);
        }
    }
}

// [spec:samurai:def:scan.scannewline-fn]
// [spec:samurai:sem:scan.scannewline-fn]
pub fn scannewline(scanner: &mut Scanner) -> Result<(), String> {
    if newline(scanner)? {
        scanner.continuation_at_eof = false;
        Ok(())
    } else if scanner.current().is_none() {
        if scanner.continuation_at_eof {
            Err("unexpected EOF after continuation".into())
        } else {
            Err("unexpected EOF".into())
        }
    } else {
        Err(scanerror(scanner, "expected newline"))
    }
}
