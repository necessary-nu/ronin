//! Byte-oriented Ninja manifest lexer.

use crate::error::{ScanError, ScanErrorKind, SeparatorKind, SourceSpan};
use crate::util::{BString, EvalPart, EvalString};
use std::fs;
use std::path::{Path, PathBuf};

type ScanResult<T> = Result<T, ScanError>;

// [spec:samurai:def:scan.token]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Token {
    Build,
    Default,
    Include,
    Pool,
    Rule,
    Subninja,
    Variable,
}

// [spec:samurai:def:scan.scanner]
pub(crate) struct Scanner {
    pub(crate) path: PathBuf,
    input: Vec<u8>,
    index: usize,
    pub(crate) line: usize,
    pub(crate) col: usize,
    pub(crate) paths: Vec<EvalString>,
    variable: Option<String>,
    continuation_at_eof: bool,
    pub(crate) manifest_version_major: i32,
    pub(crate) manifest_version_minor: i32,
}

impl Scanner {
    // [spec:samurai:def:scan.scaninit-fn]
    // [spec:samurai:sem:scan.scaninit-fn]
    // [spec:samurai:def:scan.scanclose-fn]
    // [spec:samurai:sem:scan.scanclose-fn]
    pub(crate) fn from_path(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let input = fs::read(path)?;
        Ok(Self::from_bytes(path, input))
    }

    pub(crate) fn from_bytes(path: impl AsRef<Path>, input: Vec<u8>) -> Self {
        let path = path.as_ref();
        Self {
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

    pub(crate) fn current(&self) -> Option<u8> {
        self.input.get(self.index).copied()
    }

    pub(crate) const fn take_variable(&mut self) -> Option<String> {
        self.variable.take()
    }
}

// [spec:samurai:def:scan.scanerror-fn]
// [spec:samurai:sem:scan.scanerror-fn]
pub(crate) fn scanerror(scanner: &Scanner, kind: ScanErrorKind) -> ScanError {
    ScanError {
        span: SourceSpan::new(&scanner.path, scanner.line, scanner.col),
        kind,
    }
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
const fn issimplevar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

// [spec:samurai:def:scan.isvar-fn]
// [spec:samurai:sem:scan.isvar-fn]
const fn isvar(byte: u8) -> bool {
    issimplevar(byte) || byte == b'.'
}

// [spec:samurai:def:scan.newline-fn]
// [spec:samurai:sem:scan.newline-fn]
fn newline(scanner: &mut Scanner) -> ScanResult<bool> {
    match scanner.current() {
        Some(b'\r') => {
            next(scanner);
            if scanner.current() != Some(b'\n') {
                return Err(scanerror(
                    scanner,
                    ScanErrorKind::ExpectedNewlineAfterCarriageReturn,
                ));
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
fn singlespace(scanner: &mut Scanner) -> ScanResult<bool> {
    match scanner.current() {
        Some(b' ') => {
            next(scanner);
            Ok(true)
        }
        Some(b'\t') => Err(scanerror(scanner, ScanErrorKind::TabsNotAllowed)),
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
fn space(scanner: &mut Scanner) -> ScanResult<bool> {
    let mut found = false;
    while singlespace(scanner)? {
        found = true;
    }
    Ok(found)
}

// [spec:samurai:def:scan.comment-fn]
// [spec:samurai:sem:scan.comment-fn]
fn comment(scanner: &mut Scanner) -> ScanResult<bool> {
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
fn name(scanner: &mut Scanner) -> ScanResult<String> {
    let start = scanner.index;
    while scanner.current().is_some_and(isvar) {
        next(scanner);
    }
    if scanner.index == start {
        return Err(scanerror(scanner, ScanErrorKind::ExpectedName));
    }
    let name = std::str::from_utf8(&scanner.input[start..scanner.index])
        .expect("variable names are ASCII")
        .to_owned();
    space(scanner)?;
    Ok(name)
}

// [spec:samurai:def:scan.scankeyword-fn]
// [spec:samurai:sem:scan.scankeyword-fn]
pub(crate) fn scankeyword(scanner: &mut Scanner) -> ScanResult<Option<Token>> {
    loop {
        match scanner.current() {
            None => return Ok(None),
            Some(b' ' | b'\t') => {
                space(scanner)?;
                if !comment(scanner)? && !newline(scanner)? {
                    return Err(scanerror(scanner, ScanErrorKind::UnexpectedIndent));
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
pub(crate) fn scanname(scanner: &mut Scanner) -> ScanResult<String> {
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
) -> ScanResult<()> {
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
                return Err(scanerror(scanner, ScanErrorKind::InvalidVariableName));
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
                    ScanErrorKind::CaretEscapeRequiresVersion,
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
                return Err(scanerror(scanner, ScanErrorKind::InvalidDollarEscape));
            }
            addstringpart(parts, scanner.input[start..scanner.index].to_vec(), true);
        }
    }
    Ok(())
}

// [spec:samurai:def:scan.scanstring-fn]
// [spec:samurai:sem:scan.scanstring-fn]
pub(crate) fn scanstring(scanner: &mut Scanner, path: bool) -> ScanResult<Option<EvalString>> {
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
                return Err(scanerror(scanner, ScanErrorKind::TabsNotAllowed));
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
pub(crate) fn scanpaths(scanner: &mut Scanner) -> ScanResult<()> {
    while let Some(path) = scanstring(scanner, true)? {
        scanner.paths.push(path);
    }
    Ok(())
}

// [spec:samurai:def:scan.scanchar-fn]
// [spec:samurai:sem:scan.scanchar-fn]
pub(crate) fn scanchar(scanner: &mut Scanner, expected: char) -> ScanResult<()> {
    let expected = u8::try_from(expected)
        .map_err(|_| scanerror(scanner, ScanErrorKind::ExpectedAsciiToken))?;
    if scanner.current() != Some(expected) {
        return Err(scanerror(
            scanner,
            ScanErrorKind::ExpectedCharacter(char::from(expected)),
        ));
    }
    next(scanner);
    space(scanner)?;
    Ok(())
}

// [spec:samurai:def:scan.scanpipe-fn]
// [spec:samurai:sem:scan.scanpipe-fn]
pub(crate) fn scanpipe(scanner: &mut Scanner, allowed: i32) -> ScanResult<i32> {
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
        let separator = match kind {
            1 => SeparatorKind::Implicit,
            2 => SeparatorKind::OrderOnly,
            _ => SeparatorKind::Validation,
        };
        return Err(scanerror(
            scanner,
            ScanErrorKind::UnexpectedSeparator(separator),
        ));
    }
    space(scanner)?;
    Ok(kind)
}

// [spec:samurai:def:scan.scanindent-fn]
// [spec:samurai:sem:scan.scanindent-fn]
pub(crate) fn scanindent(scanner: &mut Scanner) -> ScanResult<bool> {
    loop {
        let indent = space(scanner)?;
        if !comment(scanner)? {
            return Ok(indent && !newline(scanner)?);
        }
    }
}

// [spec:samurai:def:scan.scannewline-fn]
// [spec:samurai:sem:scan.scannewline-fn]
pub(crate) fn scannewline(scanner: &mut Scanner) -> ScanResult<()> {
    if newline(scanner)? {
        scanner.continuation_at_eof = false;
        Ok(())
    } else if scanner.current().is_none() {
        if scanner.continuation_at_eof {
            Err(scanerror(
                scanner,
                ScanErrorKind::UnexpectedEof {
                    after_continuation: true,
                },
            ))
        } else {
            Err(scanerror(
                scanner,
                ScanErrorKind::UnexpectedEof {
                    after_continuation: false,
                },
            ))
        }
    } else {
        Err(scanerror(scanner, ScanErrorKind::ExpectedNewline))
    }
}
