//! Byte-oriented Ninja manifest lexer.

use crate::error::{ScanError, ScanErrorKind, SeparatorKind};
use crate::names::Names;
pub(crate) use crate::source::Source;
use crate::source::{SourceId, SourceSpan};
use crate::util::{BStr, BString, EvalPart, EvalString};
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
    pub(crate) text: &'source BStr,
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

// [spec:ronin:def:scan.token]
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
    Variable(&'source BStr),
}

/// The fragments of one scanned evaluation string that needed expanding.
///
/// A single literal run — the overwhelmingly common shape — is `Plain` and
/// never reaches here, so inline capacity would only widen the enum and every
/// vector of them. Reaching this type at all means the value held a `$`.
pub(crate) type ScannedParts<'source> = Vec<ScannedEvalPart<'source>>;

/// An evaluation string whose source-backed fragments have not been interned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScannedEvalString<'source> {
    /// One unbroken run of manifest bytes.
    ///
    /// No `$` appeared, so there is nothing to expand: the bytes are the
    /// value. Almost every path in a real manifest takes this shape, and it
    /// lets interning skip evaluation and copying entirely.
    Plain(&'source [u8]),
    Parts(ScannedParts<'source>),
}

impl Default for ScannedEvalString<'_> {
    fn default() -> Self {
        Self::Plain(&[])
    }
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
        let parts = match self {
            Self::Plain(b"") => return EvalString::from_parts(Vec::new()),
            Self::Plain(bytes) => {
                return EvalString::from_parts(vec![EvalPart::Literal(BString::from(bytes))])
            }
            Self::Parts(parts) => parts,
        };
        let mut scanned = parts.into_iter().peekable();
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

// [spec:ronin:def:scan.scanner]
pub(crate) struct Scanner<'source> {
    source: &'source Arc<Source>,
    /// The manifest bytes, resolved once.
    ///
    /// `Source` is immutable for `'source`, so holding the slice avoids
    /// walking the `Arc` and the `Vec` behind it on every single byte read.
    bytes: &'source [u8],
    index: usize,
    line: usize,
    column: usize,
    /// Where the token being read started.
    ///
    /// Ninja's `last_token_`: every diagnostic is reported against this rather
    /// than against the scan position, which by the time an error is raised has
    /// usually moved on to the following token.
    last_token: ByteSpan,
    continuation_at_eof: bool,
    manifest_version_major: i32,
    manifest_version_minor: i32,
}

impl<'source> Scanner<'source> {
    // [spec:ronin:def:scan.scaninit-fn]
    // [spec:ronin:sem:scan.scaninit-fn]
    // [spec:ronin:def:scan.scanclose-fn]
    // [spec:ronin:sem:scan.scanclose-fn]
    pub(crate) fn new(source: &'source Arc<Source>) -> Self {
        Self {
            source,
            bytes: source.bytes(),
            index: 0,
            line: 1,
            column: 1,
            last_token: ByteSpan {
                source_id: source.id(),
                byte_start: 0,
                byte_end: 0,
                line: 1,
                column: 1,
            },
            continuation_at_eof: false,
            manifest_version_major: 1,
            manifest_version_minor: 9,
        }
    }

    pub(crate) fn current(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
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

    /// Records that a token starts here.
    ///
    /// Ninja reports every diagnostic against the token it was reading, not
    /// against wherever scanning had got to — which is usually the *next*
    /// token, on the next line. Marking the start is what makes an error name
    /// the line the reader has to go and fix, and puts the caret under the
    /// word rather than past the end of it.
    // [spec:ronin:req:compat.manifest-semantics]
    pub(crate) fn begin_token(&mut self) {
        self.last_token = self.position();
    }

    /// Where the token being read started, for a diagnostic to point at.
    pub(crate) const fn last_token(&self) -> ByteSpan {
        self.last_token
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

// [spec:ronin:def:scan.scanerror-fn]
// [spec:ronin:sem:scan.scanerror-fn]
pub(crate) fn scanerror(scanner: &Scanner<'_>, kind: ScanErrorKind) -> ScanError {
    ScanError {
        span: scanner.source_span(scanner.last_token()),
        kind,
    }
}

// [spec:ronin:def:scan.next-fn]
// [spec:ronin:sem:scan.next-fn]
fn next(scanner: &mut Scanner<'_>) {
    if scanner.current() == Some(b'\n') {
        scanner.line += 1;
        scanner.column = 1;
    } else {
        scanner.column += 1;
    }
    scanner.index += 1;
}

/// Advance past a byte the caller has already established is not a newline.
///
/// The hot loops all match the newline case in an earlier arm, so the newline
/// test inside `next` is dead work for them — as is re-reading a byte the
/// caller is holding.
const fn advance_within_line(scanner: &mut Scanner<'_>) {
    scanner.column += 1;
    scanner.index += 1;
}

/// Bytes that end a run of ordinary text inside a value.
///
/// `$` begins an expansion and the line endings end the value; everything else
/// is literal, so a value can be crossed by looking for the next of these
/// rather than by classifying every byte on the way.
const VALUE_ENDS: [bool; 256] = ends_table(b"$\r\n");

/// The same, for a path, which additionally ends at any of the separators that
/// divide one path from the next. A tab is included so it can be rejected:
/// Ninja does not allow one here, and the scan has to stop to say so.
const PATH_ENDS: [bool; 256] = ends_table(b"$\r\n:| \t");

/// A membership table, rather than a match over the bytes or a byte-set search.
///
/// The match this replaced tested up to seven alternatives per byte. Both
/// obvious replacements were measured in the built tool rather than in a
/// microbenchmark, because a standalone harness for this ranked them
/// inconsistently between runs: `bstr`'s byte-set search and this table. The
/// table won at every manifest size — parsing 100,000 statements, 787.9
/// million cycles before, 744.6 with the byte-set search and 722.6 with the
/// table; at 25,000, 209.2, 185.3 and 173.2. It also executes the fewest
/// instructions of the three, which the byte-set search does not, because that
/// one rebuilds its set on every call and most runs here are short.
const fn ends_table(bytes: &[u8]) -> [bool; 256] {
    let mut table = [false; 256];
    let mut index = 0;
    while index < bytes.len() {
        table[bytes[index] as usize] = true;
        index += 1;
    }
    table
}

// [spec:ronin:def:scan.issimplevar-fn]
// [spec:ronin:sem:scan.issimplevar-fn]
const fn issimplevar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

// [spec:ronin:def:scan.isvar-fn]
// [spec:ronin:sem:scan.isvar-fn]
const fn isvar(byte: u8) -> bool {
    issimplevar(byte) || byte == b'.'
}

// [spec:ronin:def:scan.newline-fn]
// [spec:ronin:sem:scan.newline-fn]
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

// [spec:ronin:def:scan.singlespace-fn]
// [spec:ronin:sem:scan.singlespace-fn]
fn singlespace(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
    match scanner.current() {
        Some(b' ') => {
            advance_within_line(scanner);
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

// [spec:ronin:def:scan.space-fn]
// [spec:ronin:sem:scan.space-fn]
fn space(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
    let mut found = false;
    while singlespace(scanner)? {
        found = true;
    }
    Ok(found)
}

// [spec:ronin:def:scan.comment-fn]
// [spec:ronin:sem:scan.comment-fn]
fn comment(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
    if scanner.current() != Some(b'#') {
        return Ok(false);
    }
    while scanner.current().is_some() && !newline(scanner)? {
        next(scanner);
    }
    Ok(true)
}

// [spec:ronin:def:scan.name-fn]
// [spec:ronin:sem:scan.name-fn]
fn name<'source>(scanner: &mut Scanner<'source>) -> ScanResult<Lexeme<'source>> {
    let source = scanner.source;
    scanner.begin_token();
    let start = scanner.index;
    let line = scanner.line;
    let column = scanner.column;
    while scanner.current().is_some_and(isvar) {
        advance_within_line(scanner);
    }
    if scanner.index == start {
        return Err(scanerror(scanner, ScanErrorKind::ExpectedName));
    }
    let end = scanner.index;
    let text = BStr::new(&source.bytes()[start..end]);
    let span = scanner.span(start, end, line, column);
    space(scanner)?;
    Ok(Lexeme { text, span })
}

// [spec:ronin:def:scan.scankeyword-fn]
// [spec:ronin:sem:scan.scankeyword-fn]
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
                let kind = match &**lexeme.text {
                    b"build" => TokenKind::Build,
                    b"default" => TokenKind::Default,
                    b"include" => TokenKind::Include,
                    b"pool" => TokenKind::Pool,
                    b"rule" => TokenKind::Rule,
                    b"subninja" => TokenKind::Subninja,
                    _ => TokenKind::Variable,
                };
                return Ok(Some(Token { kind, lexeme }));
            }
        }
    }
}

// [spec:ronin:def:scan.scanname-fn]
// [spec:ronin:sem:scan.scanname-fn]
pub(crate) fn scanname<'source>(scanner: &mut Scanner<'source>) -> ScanResult<Lexeme<'source>> {
    name(scanner)
}

fn push_literal<'source>(
    parts: &mut ScannedParts<'source>,
    source: &'source Source,
    start: usize,
    end: usize,
) {
    if start != end {
        parts.push(ScannedEvalPart::Literal(&source.bytes()[start..end]));
    }
}

// [spec:ronin:def:scan.addstringpart-fn]
// [spec:ronin:sem:scan.addstringpart-fn]
// [spec:ronin:def:scan.escape-fn]
// [spec:ronin:sem:scan.escape-fn]
fn escape<'source>(
    scanner: &mut Scanner<'source>,
    parts: &mut ScannedParts<'source>,
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
            let variable = BStr::new(&source.bytes()[start..scanner.index]);
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
            let variable = BStr::new(&source.bytes()[start..scanner.index]);
            parts.push(ScannedEvalPart::Variable(variable));
        }
    }
    Ok(())
}

// [spec:ronin:def:scan.scanstring-fn]
// [spec:ronin:sem:scan.scanstring-fn]
pub(crate) fn scanstring<'source>(
    scanner: &mut Scanner<'source>,
    path: bool,
) -> ScanResult<Option<ScannedEvalString<'source>>> {
    let source = scanner.source;
    scanner.begin_token();
    let mut parts = ScannedParts::new();
    let start = scanner.index;
    let mut literal_start = start;
    let mut escaped = false;
    let terminators = if path { &PATH_ENDS } else { &VALUE_ENDS };
    loop {
        // Cross the run of ordinary bytes in one step. Nothing here keeps
        // per-byte state — `advance_within_line` only counts the column, and
        // the run cannot contain a line ending because one would end it — so
        // the bulk skip is exactly the loop it replaces.
        let rest = &scanner.bytes[scanner.index..];
        let run = rest
            .iter()
            .position(|byte| terminators[*byte as usize])
            .unwrap_or(rest.len());
        if run > 0 {
            scanner.continuation_at_eof = false;
            scanner.column += run;
            scanner.index += run;
        }
        match scanner.current() {
            Some(b'$') => {
                escaped = true;
                push_literal(&mut parts, source, literal_start, scanner.index);
                next(scanner);
                escape(scanner, &mut parts)?;
                literal_start = scanner.index;
            }
            Some(b'\t') if path => {
                return Err(scanerror(scanner, ScanErrorKind::TabsNotAllowed));
            }
            // A separator when scanning a path, a line ending, or the end of
            // the source; everything else was consumed by the skip above.
            _ => break,
        }
    }
    let end = scanner.index;
    // Ninja reads an evaluation string in chunks and marks each one, so once
    // the value is read its mark sits on whatever ended it — the newline, or
    // the separator after a path. A diagnostic the *caller* then raises about
    // the binding points there rather than at the value, and this is where
    // that difference is reproduced.
    scanner.begin_token();
    if !escaped {
        // Nothing was expanded, so the run of source bytes is the whole value.
        if path {
            space(scanner)?;
        }
        return Ok((start != end).then(|| ScannedEvalString::Plain(&source.bytes()[start..end])));
    }
    push_literal(&mut parts, source, literal_start, end);
    if path {
        space(scanner)?;
    }
    Ok((!parts.is_empty()).then_some(ScannedEvalString::Parts(parts)))
}

// [spec:ronin:def:scan.scanpaths-fn]
// [spec:ronin:sem:scan.scanpaths-fn]
pub(crate) fn scanpaths<'source>(
    scanner: &mut Scanner<'source>,
) -> ScanResult<Vec<ScannedEvalString<'source>>> {
    let mut paths = Vec::new();
    while let Some(path) = scanstring(scanner, true)? {
        paths.push(path);
    }
    Ok(paths)
}

// [spec:ronin:def:scan.scanchar-fn]
// [spec:ronin:sem:scan.scanchar-fn]
pub(crate) fn scanchar(scanner: &mut Scanner<'_>, expected: char) -> ScanResult<()> {
    scanner.begin_token();
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

// [spec:ronin:def:scan.scanpipe-fn]
// [spec:ronin:sem:scan.scanpipe-fn]
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

// [spec:ronin:def:scan.scanindent-fn]
// [spec:ronin:sem:scan.scanindent-fn]
pub(crate) fn scanindent(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
    loop {
        let indent = space(scanner)?;
        if !comment(scanner)? {
            return Ok(indent && !newline(scanner)?);
        }
    }
}

// [spec:ronin:def:scan.scannewline-fn]
// [spec:ronin:sem:scan.scannewline-fn]
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

    #[test]
    fn crossing_a_run_of_ordinary_bytes_leaves_the_column_where_stepping_would() {
        // The bulk skip advances the column by the whole run at once, so the
        // position it reports has to match what a byte-at-a-time walk gave.
        // A tab is ordinary inside a value and forbidden inside a path.
        for (bytes, path, expected_column) in [
            (&b"abcdef ghi\n"[..], false, 11),
            // A path stops at the space, then `scanstring` eats the separator.
            (&b"abcdef ghi\n"[..], true, 8),
            (&b"a\tb\n"[..], false, 4),
            (&b"obj/x.o: cc\n"[..], true, 8),
            (&b"|| dep\n"[..], true, 1),
        ] {
            let source = Source::from_bytes("build.ninja", bytes.to_vec());
            let mut scanner = Scanner::new(&source);
            scanstring(&mut scanner, path).unwrap();
            assert_eq!(
                scanner.column,
                expected_column,
                "column after scanning {:?} as path={path}",
                bstr::BStr::new(bytes)
            );
            assert_eq!(scanner.line, 1, "an ordinary run never crosses a line");
        }

        // A tab inside a path is rejected rather than crossed.
        let source = Source::from_bytes("build.ninja", b"obj/\tx.o\n".to_vec());
        let mut scanner = Scanner::new(&source);
        assert!(matches!(
            scanstring(&mut scanner, true).unwrap_err().kind,
            ScanErrorKind::TabsNotAllowed
        ));
    }

    // [spec:ronin:req:runtime.borrowed-span-frontend/test]
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
            let ScannedEvalString::Parts(parts) = &value else {
                panic!("a value holding an expansion should scan as parts");
            };
            let ScannedEvalPart::Literal(literal) = parts[0] else {
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
