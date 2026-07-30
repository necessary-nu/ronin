//! Byte-oriented Ninja manifest lexer.

use crate::error::{ScanError, ScanErrorKind, SeparatorKind};
use crate::names::Names;
pub(crate) use crate::source::Source;
use crate::source::{SourceId, SourceSpan};
use crate::util::{BString, EvalPart, EvalString};
use std::sync::Arc;

type ScanResult<T> = Result<T, ScanError>;

/// A byte range within one immutable source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ByteSpan {
    pub(crate) source_id: SourceId,
    pub(crate) byte_start: usize,
    pub(crate) byte_end: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

/// An ASCII manifest name borrowed from its source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Lexeme<'source> {
    pub(crate) text: &'source str,
    pub(crate) span: ByteSpan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenKind {
    Build,
    Default,
    Include,
    Pool,
    Rule,
    Subninja,
    Variable,
}

// [spec:samurai:def:scan.token]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Token<'source> {
    pub(crate) kind: TokenKind,
    pub(crate) lexeme: Lexeme<'source>,
}

/// One evaluation fragment borrowed from manifest bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScannedEvalPart<'source> {
    Literal(&'source [u8]),
    EscapedByte(u8),
    Variable(&'source str),
}

/// An evaluation string whose source-backed fragments have not been interned.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ScannedEvalString<'source> {
    pub(crate) parts: Vec<ScannedEvalPart<'source>>,
}

fn append_scanned_literal(part: ScannedEvalPart<'_>, literal: &mut Vec<u8>) {
    match part {
        ScannedEvalPart::Literal(bytes) => literal.extend_from_slice(bytes),
        ScannedEvalPart::EscapedByte(byte) => literal.push(byte),
        ScannedEvalPart::Variable(_) => unreachable!("literal group contained a variable"),
    }
}

const fn scanned_literal_len(part: ScannedEvalPart<'_>) -> usize {
    match part {
        ScannedEvalPart::Literal(bytes) => bytes.len(),
        ScannedEvalPart::EscapedByte(_) => 1,
        ScannedEvalPart::Variable(_) => 0,
    }
}

impl ScannedEvalString<'_> {
    /// Interns a parsed value at the graph-ownership boundary.
    pub(crate) fn into_owned(self, names: &mut Names) -> EvalString {
        let mut scanned = self.parts.into_iter().peekable();
        let mut parts = Vec::new();
        while let Some(part) = scanned.next() {
            match part {
                ScannedEvalPart::Variable(name) => {
                    parts.push(EvalPart::Variable(names.intern(name)));
                }
                first @ (ScannedEvalPart::Literal(_) | ScannedEvalPart::EscapedByte(_)) => {
                    let mut literal = Vec::with_capacity(scanned_literal_len(first));
                    append_scanned_literal(first, &mut literal);
                    while matches!(
                        scanned.peek(),
                        Some(ScannedEvalPart::Literal(_) | ScannedEvalPart::EscapedByte(_))
                    ) {
                        append_scanned_literal(
                            scanned.next().expect("peeked evaluation part"),
                            &mut literal,
                        );
                    }
                    parts.push(EvalPart::Literal(BString::from(literal)));
                }
            }
        }
        EvalString::from_parts(parts)
    }
}

// [spec:samurai:def:scan.scanner]
pub(crate) struct Scanner<'source> {
    source: &'source Arc<Source>,
    index: usize,
    line: usize,
    column: usize,
    continuation_at_eof: bool,
    manifest_version_major: i32,
    manifest_version_minor: i32,
}

impl<'source> Scanner<'source> {
    // [spec:samurai:def:scan.scaninit-fn]
    // [spec:samurai:sem:scan.scaninit-fn]
    // [spec:samurai:def:scan.scanclose-fn]
    // [spec:samurai:sem:scan.scanclose-fn]
    pub(crate) const fn new(source: &'source Arc<Source>) -> Self {
        Self {
            source,
            index: 0,
            line: 1,
            column: 1,
            continuation_at_eof: false,
            manifest_version_major: 1,
            manifest_version_minor: 9,
        }
    }

    pub(crate) fn current(&self) -> Option<u8> {
        self.source.bytes().get(self.index).copied()
    }

    pub(crate) const fn line(&self) -> usize {
        self.line
    }

    pub(crate) const fn set_manifest_version(&mut self, major: i32, minor: i32) {
        self.manifest_version_major = major;
        self.manifest_version_minor = minor;
    }

    pub(crate) fn position(&self) -> ByteSpan {
        ByteSpan {
            source_id: self.source.id(),
            byte_start: self.index,
            byte_end: self.index,
            line: self.line,
            column: self.column,
        }
    }

    pub(crate) fn source_span(&self, span: ByteSpan) -> SourceSpan {
        debug_assert_eq!(span.source_id, self.source.id());
        SourceSpan::new(
            Arc::clone(self.source),
            span.byte_start,
            span.byte_end,
            span.line,
            span.column,
        )
    }

    fn span(&self, byte_start: usize, byte_end: usize, line: usize, column: usize) -> ByteSpan {
        ByteSpan {
            source_id: self.source.id(),
            byte_start,
            byte_end,
            line,
            column,
        }
    }
}

// [spec:samurai:def:scan.scanerror-fn]
// [spec:samurai:sem:scan.scanerror-fn]
pub(crate) fn scanerror(scanner: &Scanner<'_>, kind: ScanErrorKind) -> ScanError {
    ScanError {
        span: scanner.source_span(scanner.position()),
        kind,
    }
}

// [spec:samurai:def:scan.next-fn]
// [spec:samurai:sem:scan.next-fn]
fn next(scanner: &mut Scanner<'_>) -> Option<u8> {
    if scanner.current() == Some(b'\n') {
        scanner.line += 1;
        scanner.column = 1;
    } else {
        scanner.column += 1;
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
fn newline(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
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
fn singlespace(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
    match scanner.current() {
        Some(b' ') => {
            next(scanner);
            Ok(true)
        }
        Some(b'\t') => Err(scanerror(scanner, ScanErrorKind::TabsNotAllowed)),
        Some(b'$') => {
            let index = scanner.index;
            let line = scanner.line;
            let column = scanner.column;
            next(scanner);
            if newline(scanner)? {
                scanner.continuation_at_eof = scanner.current().is_none();
                Ok(true)
            } else {
                scanner.index = index;
                scanner.line = line;
                scanner.column = column;
                Ok(false)
            }
        }
        _ => Ok(false),
    }
}

// [spec:samurai:def:scan.space-fn]
// [spec:samurai:sem:scan.space-fn]
fn space(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
    let mut found = false;
    while singlespace(scanner)? {
        found = true;
    }
    Ok(found)
}

// [spec:samurai:def:scan.comment-fn]
// [spec:samurai:sem:scan.comment-fn]
fn comment(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
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
fn name<'source>(scanner: &mut Scanner<'source>) -> ScanResult<Lexeme<'source>> {
    let source = scanner.source;
    let start = scanner.index;
    let line = scanner.line;
    let column = scanner.column;
    while scanner.current().is_some_and(isvar) {
        next(scanner);
    }
    if scanner.index == start {
        return Err(scanerror(scanner, ScanErrorKind::ExpectedName));
    }
    let end = scanner.index;
    let text = std::str::from_utf8(&source.bytes()[start..end]).expect("variable names are ASCII");
    let span = scanner.span(start, end, line, column);
    space(scanner)?;
    Ok(Lexeme { text, span })
}

// [spec:samurai:def:scan.scankeyword-fn]
// [spec:samurai:sem:scan.scankeyword-fn]
pub(crate) fn scankeyword<'source>(
    scanner: &mut Scanner<'source>,
) -> ScanResult<Option<Token<'source>>> {
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
                let lexeme = name(scanner)?;
                let kind = match lexeme.text {
                    "build" => TokenKind::Build,
                    "default" => TokenKind::Default,
                    "include" => TokenKind::Include,
                    "pool" => TokenKind::Pool,
                    "rule" => TokenKind::Rule,
                    "subninja" => TokenKind::Subninja,
                    _ => TokenKind::Variable,
                };
                return Ok(Some(Token { kind, lexeme }));
            }
        }
    }
}

// [spec:samurai:def:scan.scanname-fn]
// [spec:samurai:sem:scan.scanname-fn]
pub(crate) fn scanname<'source>(scanner: &mut Scanner<'source>) -> ScanResult<Lexeme<'source>> {
    name(scanner)
}

fn push_literal<'source>(
    parts: &mut Vec<ScannedEvalPart<'source>>,
    source: &'source Source,
    start: usize,
    end: usize,
) {
    if start != end {
        parts.push(ScannedEvalPart::Literal(&source.bytes()[start..end]));
    }
}

// [spec:samurai:def:scan.addstringpart-fn]
// [spec:samurai:sem:scan.addstringpart-fn]
// [spec:samurai:def:scan.escape-fn]
// [spec:samurai:sem:scan.escape-fn]
fn escape<'source>(
    scanner: &mut Scanner<'source>,
    parts: &mut Vec<ScannedEvalPart<'source>>,
) -> ScanResult<()> {
    let source = scanner.source;
    match scanner.current() {
        Some(b'$' | b' ' | b':') => {
            let start = scanner.index;
            next(scanner);
            push_literal(parts, source, start, start + 1);
        }
        Some(b'{') => {
            next(scanner);
            let start = scanner.index;
            while scanner.current().is_some_and(isvar) {
                next(scanner);
            }
            if scanner.current() != Some(b'}') {
                return Err(scanerror(scanner, ScanErrorKind::InvalidVariableName));
            }
            let variable = std::str::from_utf8(&source.bytes()[start..scanner.index])
                .expect("variable names are ASCII");
            next(scanner);
            parts.push(ScannedEvalPart::Variable(variable));
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
            parts.push(ScannedEvalPart::EscapedByte(b'\n'));
        }
        _ => {
            let start = scanner.index;
            while scanner.current().is_some_and(issimplevar) {
                next(scanner);
            }
            if scanner.index == start {
                return Err(scanerror(scanner, ScanErrorKind::InvalidDollarEscape));
            }
            let variable = std::str::from_utf8(&source.bytes()[start..scanner.index])
                .expect("variable names are ASCII");
            parts.push(ScannedEvalPart::Variable(variable));
        }
    }
    Ok(())
}

// [spec:samurai:def:scan.scanstring-fn]
// [spec:samurai:sem:scan.scanstring-fn]
pub(crate) fn scanstring<'source>(
    scanner: &mut Scanner<'source>,
    path: bool,
) -> ScanResult<Option<ScannedEvalString<'source>>> {
    let source = scanner.source;
    let mut parts = Vec::new();
    let mut literal_start = scanner.index;
    loop {
        match scanner.current() {
            Some(b'$') => {
                push_literal(&mut parts, source, literal_start, scanner.index);
                next(scanner);
                escape(scanner, &mut parts)?;
                literal_start = scanner.index;
            }
            Some(b':' | b'|' | b' ') if path => break,
            Some(b'\t') if path => {
                return Err(scanerror(scanner, ScanErrorKind::TabsNotAllowed));
            }
            Some(b'\r' | b'\n') | None => break,
            Some(_) => {
                scanner.continuation_at_eof = false;
                next(scanner);
            }
        }
    }
    push_literal(&mut parts, source, literal_start, scanner.index);
    if path {
        space(scanner)?;
    }
    Ok((!parts.is_empty()).then_some(ScannedEvalString { parts }))
}

// [spec:samurai:def:scan.scanpaths-fn]
// [spec:samurai:sem:scan.scanpaths-fn]
pub(crate) fn scanpaths<'source>(
    scanner: &mut Scanner<'source>,
) -> ScanResult<Vec<ScannedEvalString<'source>>> {
    let mut paths = Vec::new();
    while let Some(path) = scanstring(scanner, true)? {
        paths.push(path);
    }
    Ok(paths)
}

// [spec:samurai:def:scan.scanchar-fn]
// [spec:samurai:sem:scan.scanchar-fn]
pub(crate) fn scanchar(scanner: &mut Scanner<'_>, expected: char) -> ScanResult<()> {
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

/// A typed dependency separator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Separator {
    Implicit,
    OrderOnly,
    Validation,
}

impl From<Separator> for SeparatorKind {
    fn from(separator: Separator) -> Self {
        match separator {
            Separator::Implicit => Self::Implicit,
            Separator::OrderOnly => Self::OrderOnly,
            Separator::Validation => Self::Validation,
        }
    }
}

/// The separators accepted at one grammar position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AllowedSeparators(u8);

impl AllowedSeparators {
    pub(crate) const IMPLICIT: Self = Self(0b001);
    pub(crate) const INPUTS: Self = Self(0b111);
    pub(crate) const AFTER_IMPLICIT: Self = Self(0b110);
    pub(crate) const VALIDATION: Self = Self(0b100);

    const fn contains(self, separator: Separator) -> bool {
        let bit = match separator {
            Separator::Implicit => 0b001,
            Separator::OrderOnly => 0b010,
            Separator::Validation => 0b100,
        };
        self.0 & bit != 0
    }
}

// [spec:samurai:def:scan.scanpipe-fn]
// [spec:samurai:sem:scan.scanpipe-fn]
pub(crate) fn scanpipe(
    scanner: &mut Scanner<'_>,
    allowed: AllowedSeparators,
) -> ScanResult<Option<Separator>> {
    if scanner.current() != Some(b'|') {
        return Ok(None);
    }
    next(scanner);
    let separator = match scanner.current() {
        Some(b'|') => {
            next(scanner);
            Separator::OrderOnly
        }
        Some(b'@') => {
            next(scanner);
            Separator::Validation
        }
        _ => Separator::Implicit,
    };
    if !allowed.contains(separator) {
        return Err(scanerror(
            scanner,
            ScanErrorKind::UnexpectedSeparator(separator.into()),
        ));
    }
    space(scanner)?;
    Ok(Some(separator))
}

// [spec:samurai:def:scan.scanindent-fn]
// [spec:samurai:sem:scan.scanindent-fn]
pub(crate) fn scanindent(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
    loop {
        let indent = space(scanner)?;
        if !comment(scanner)? {
            return Ok(indent && !newline(scanner)?);
        }
    }
}

// [spec:samurai:def:scan.scannewline-fn]
// [spec:samurai:sem:scan.scannewline-fn]
pub(crate) fn scannewline(scanner: &mut Scanner<'_>) -> ScanResult<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    // [spec:samurai:req:runtime.borrowed-span-frontend/test]
    #[test]
    fn tokens_and_evaluation_parts_borrow_retained_source_spans() {
        let source = Source::from_bytes("build.ninja", b"rule cc\nvalue $name\n".to_vec());
        let (error, token_source_id) = {
            let mut scanner = Scanner::new(&source);
            let token = scankeyword(&mut scanner).unwrap().unwrap();
            assert_eq!(token.kind, TokenKind::Rule);
            assert_eq!(token.lexeme.text, "rule");
            assert_eq!(token.lexeme.span.source_id, source.id());
            let token_source_id = token.lexeme.span.source_id;
            assert_eq!(
                (
                    token.lexeme.span.byte_start,
                    token.lexeme.span.byte_end,
                    token.lexeme.span.line,
                    token.lexeme.span.column,
                ),
                (0, 4, 1, 1)
            );

            assert_eq!(scanname(&mut scanner).unwrap().text, "cc");
            scannewline(&mut scanner).unwrap();
            let value = scanstring(&mut scanner, false).unwrap().unwrap();
            let ScannedEvalPart::Literal(literal) = value.parts[0] else {
                panic!("first evaluation part was not a literal");
            };
            let source_range = source.bytes().as_ptr_range();
            assert!(source_range.contains(&literal.as_ptr()));

            scannewline(&mut scanner).unwrap();
            (scannewline(&mut scanner).unwrap_err(), token_source_id)
        };
        drop(source);
        assert_eq!(error.span.source_bytes(), &b"rule cc\nvalue $name\n"[..]);
        assert_eq!(error.span.source_id(), token_source_id);
    }
}
