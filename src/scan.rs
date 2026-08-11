//! Byte-oriented Ninja manifest lexer.

use crate::error::{FoundToken, NameKind, ScanError, ScanErrorKind, SeparatorKind};
pub(crate) use crate::source::Source;
use crate::source::{SourceId, SourceSpan};
use crate::util::BStr;
use std::sync::Arc;

type ScanResult<T> = Result<T, ScanError>;

/// A scan position, kept so it can be returned to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Mark {
    index: usize,
    line: usize,
    column: usize,
}

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
            manifest_version_major: 1,
            manifest_version_minor: 9,
        }
    }

    pub(crate) fn current(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    /// The byte `offset` positions ahead of the scan position.
    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.index + offset).copied()
    }

    /// The scan position, for a caller that may have to come back to it.
    const fn mark(&self) -> Mark {
        Mark {
            index: self.index,
            line: self.line,
            column: self.column,
        }
    }

    const fn rewind(&mut self, mark: Mark) {
        self.index = mark.index;
        self.line = mark.line;
        self.column = mark.column;
    }

    /// Put back the token that was read, as Ninja's `UnreadToken` does.
    ///
    /// Ninja peeks for one token and rewinds to `last_token_` when it was not
    /// the one wanted, so the scan position after a failed peek is the start of
    /// the token that failed it — which is where the checks a closed block
    /// defers are then located.
    const fn unread_token(&mut self) {
        self.rewind(Mark {
            index: self.last_token.byte_start,
            line: self.last_token.line,
            column: self.last_token.column,
        });
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

/// The same, against a position the caller kept rather than the last token.
fn scanerror_at(scanner: &Scanner<'_>, span: ByteSpan, kind: ScanErrorKind) -> ScanError {
    ScanError {
        span: scanner.source_span(span),
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
/// divide one path from the next.
///
/// A tab is not one of them. Ninja's lexer reads a tab as ordinary text inside
/// an evaluation string, so `build a\tb: r` names a file whose path contains a
/// tab, and only a tab where a *statement* belongs is an error.
const PATH_ENDS: [bool; 256] = ends_table(b"$\r\n:| ");

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
            // Only the pair is a line ending. A carriage return on its own is
            // a byte nothing in the grammar starts with, wherever it turns up,
            // and Ninja says so in the same words it uses for any other.
            let carriage_return = scanner.position();
            next(scanner);
            if scanner.current() != Some(b'\n') {
                return Err(scanerror_at(
                    scanner,
                    carriage_return,
                    ScanErrorKind::LexingError,
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
/// Crosses one unit of whitespace: a space, or a line ending escaped by `$`.
///
/// A tab is not whitespace here. Ninja's lexer eats runs of spaces and nothing
/// else, so a tab never indents a block and never separates two paths — it
/// simply ends the run, which ends the block. Only a tab where a statement
/// belongs is an error, and [`scankeyword`] is where that is raised.
fn singlespace(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
    match scanner.current() {
        Some(b' ') => {
            advance_within_line(scanner);
            Ok(true)
        }
        Some(b'$') => {
            let escape = scanner.mark();
            next(scanner);
            if newline(scanner)? {
                Ok(true)
            } else {
                scanner.rewind(escape);
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

/// What Ninja's lexer reads where a line begins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LineStart {
    /// A run of spaces no other rule claimed.
    Indent,
    /// A line ending, with or without spaces in front of it.
    Newline,
    /// Neither, so the scan position is the first byte of some other token.
    Other,
}

// [spec:ronin:def:scan.comment-fn]
// [spec:ronin:sem:scan.comment-fn]
/// Read the token that begins a line, skipping whole comment lines.
///
/// Three of the lexer's rules can match here and re2c takes the longest match
/// between them, which is the whole of what decides where an `unexpected
/// indent` is raised:
///
/// ```text
/// [ ]*"#"[^\000\n]*"\n"     skipped entirely
/// [ ]*"\r\n" | [ ]*"\n"     one newline
/// [ ]+                      an indent
/// ```
///
/// Two consequences that reading the rules one at a time does not give. The
/// comment rule wants a terminating newline, so a comment that runs to the end
/// of the file matches nothing: `  # x` there is an *indent*, and `# x` there
/// is the byte no token starts with. And a `$`-escaped line ending is part of
/// none of the three — Ninja eats it *after* a token rather than before one —
/// so a line holding only `  $` is an indent rather than a continuation of the
/// line below it.
///
/// The token starts at the first space, which is why an unexpected indent
/// names the indented line and carries no source context: its column is zero.
fn linestart(scanner: &mut Scanner<'_>) -> LineStart {
    /// What a run of spaces that no other rule claimed came to.
    const fn matched(indented: bool) -> LineStart {
        if indented {
            LineStart::Indent
        } else {
            LineStart::Other
        }
    }

    loop {
        scanner.begin_token();
        let mut indented = false;
        while scanner.current() == Some(b' ') {
            advance_within_line(scanner);
            indented = true;
        }
        let after_spaces = scanner.mark();
        match scanner.current() {
            Some(b'#') => {
                while !matches!(scanner.current(), None | Some(b'\n')) {
                    advance_within_line(scanner);
                }
                if scanner.current().is_none() {
                    scanner.rewind(after_spaces);
                    return matched(indented);
                }
                next(scanner);
            }
            Some(b'\n') => {
                next(scanner);
                return LineStart::Newline;
            }
            // A carriage return ends a line only as half of the pair. On its
            // own it is an ordinary byte, so the spaces in front of it are all
            // that matched.
            Some(b'\r') if scanner.peek(1) == Some(b'\n') => {
                advance_within_line(scanner);
                next(scanner);
                return LineStart::Newline;
            }
            _ => return matched(indented),
        }
    }
}

// [spec:ronin:def:scan.name-fn]
// [spec:ronin:sem:scan.name-fn]
fn name<'source>(scanner: &mut Scanner<'source>, kind: NameKind) -> ScanResult<Lexeme<'source>> {
    let source = scanner.source;
    scanner.begin_token();
    let start = scanner.index;
    let line = scanner.line;
    let column = scanner.column;
    while scanner.current().is_some_and(isvar) {
        advance_within_line(scanner);
    }
    if scanner.index == start {
        return Err(scanerror(scanner, ScanErrorKind::ExpectedName(kind)));
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
        match linestart(scanner) {
            LineStart::Newline => continue,
            // Nothing a statement can begin with is indented, and the token
            // that was read starts at the first of the spaces — which is what
            // makes this name the indented line rather than the one above it.
            LineStart::Indent => {
                return Err(scanerror(
                    scanner,
                    ScanErrorKind::UnexpectedToken(FoundToken::Indent),
                ));
            }
            LineStart::Other => {}
        }
        match scanner.current() {
            None => return Ok(None),
            // The one position where a tab is wrong rather than ordinary.
            // Ninja's lexer has no rule that matches it, so reading a statement
            // here yields its error token, and the message names the byte.
            Some(b'\t') => {
                scanner.begin_token();
                return Err(scanerror(scanner, ScanErrorKind::TabsNotAllowed));
            }
            // A token that reads perfectly well and belongs in the middle of a
            // statement rather than at the start of one — and, failing even
            // that, a byte no token begins with at all.
            Some(byte) if !isvar(byte) => {
                scanner.begin_token();
                return Err(scanerror(
                    scanner,
                    match byte {
                        b'=' | b':' | b'|' => ScanErrorKind::UnexpectedToken(found_token(scanner)),
                        _ => ScanErrorKind::LexingError,
                    },
                ));
            }
            _ => {
                let lexeme = name(scanner, NameKind::Variable)?;
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
pub(crate) fn scanname<'source>(
    scanner: &mut Scanner<'source>,
    kind: NameKind,
) -> ScanResult<Lexeme<'source>> {
    name(scanner, kind)
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
/// Read what follows a `$`, which `dollar` locates.
///
/// A malformed escape is reported against the `$` itself, because that is the
/// byte the reader has to go and fix and because it is where Ninja's lexer
/// marks the token it failed on. The version complaint about `$^` is the one
/// exception, and deliberately so: Ninja raises that one without marking
/// anything, so it still points at whatever token was read before the value.
fn escape<'source>(
    scanner: &mut Scanner<'source>,
    parts: &mut ScannedParts<'source>,
    dollar: ByteSpan,
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
                return Err(scanerror_at(
                    scanner,
                    dollar,
                    ScanErrorKind::InvalidVariableName,
                ));
            }
            let variable = BStr::new(&source.bytes()[start..scanner.index]);
            next(scanner);
            parts.push(ScannedEvalPart::Variable(variable));
        }
        Some(b'\r' | b'\n') => {
            // Only a `$` immediately before a complete line ending continues a
            // line; a carriage return with nothing behind it is just a byte the
            // escape does not name, like any other.
            if newline(scanner).is_err() {
                return Err(scanerror_at(
                    scanner,
                    dollar,
                    ScanErrorKind::InvalidDollarEscape,
                ));
            }
            space(scanner)?;
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
                return Err(scanerror_at(
                    scanner,
                    dollar,
                    ScanErrorKind::InvalidDollarEscape,
                ));
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
    // No mark is taken here. Ninja's lexer marks an evaluation string only
    // where it ends, so while one is being read the mark still names the token
    // before it — which is where the `$^` version complaint lands, that being
    // the one error in here Ninja raises without marking anything first.
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
            scanner.column += run;
            scanner.index += run;
        }
        match scanner.current() {
            Some(b'$') => {
                escaped = true;
                push_literal(&mut parts, source, literal_start, scanner.index);
                let dollar = scanner.position();
                next(scanner);
                escape(scanner, &mut parts, dollar)?;
                literal_start = scanner.index;
            }
            // A carriage return ends a value only as half of a line ending;
            // on its own it is a byte the grammar has no rule for, and saying
            // the value ended here would hide that.
            Some(b'\r') if scanner.bytes.get(scanner.index + 1) != Some(&b'\n') => {
                scanner.begin_token();
                return Err(scanerror(scanner, ScanErrorKind::LexingError));
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
    // Nothing ends an evaluation string at the end of the file: Ninja's lexer
    // has a rule for the terminating nul and that rule is an error, so a
    // manifest whose last line has no newline stops here rather than wherever
    // the caller next looks for one.
    if scanner.current().is_none() {
        return Err(scanerror(scanner, ScanErrorKind::UnexpectedEof));
    }
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
            ScanErrorKind::ExpectedCharacter {
                expected: char::from(expected),
                found: found_token(scanner),
            },
        ));
    }
    next(scanner);
    space(scanner)?;
    Ok(())
}

/// Names the token at the scan position the way Ninja names it.
fn found_token(scanner: &Scanner<'_>) -> FoundToken {
    match scanner.current() {
        None => FoundToken::Eof,
        Some(b'\r' | b'\n') => FoundToken::Newline,
        Some(b' ') => FoundToken::Indent,
        // Ninja has no token for a tab, so what it reports having found is the
        // error token, which it names rather than describes.
        Some(b'\t') => FoundToken::LexingError,
        // A pipe is one token with the byte after it, so what was found is
        // `||` or `|@` rather than a `|` with something unexplained behind it.
        Some(b'|') => FoundToken::Separator(separator_at(scanner).0.into()),
        Some(byte) if isvar(byte) => FoundToken::Identifier,
        Some(byte) => FoundToken::Character(char::from(byte)),
    }
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

/// The separator beginning at a `|` the caller has already seen, and how many
/// bytes it spans.
fn separator_at(scanner: &Scanner<'_>) -> (Separator, usize) {
    match scanner.bytes.get(scanner.index + 1) {
        Some(b'|') => (Separator::OrderOnly, 2),
        Some(b'@') => (Separator::Validation, 2),
        _ => (Separator::Implicit, 1),
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
/// Read the separator at the scan position, if it is one this position takes.
///
/// A separator this position does not take is *left where it is* rather than
/// rejected here. Ninja only ever peeks for the one separator it wants next,
/// and a peek that fails puts the token back — so the complaint about `build a
/// || b: r` comes from the colon that was expected instead, naming the `||` it
/// found. Consuming it to complain about it here would say something Ninja
/// never says, and would say it about a token the reader can see is a pipe.
pub(crate) fn scanpipe(
    scanner: &mut Scanner<'_>,
    allowed: AllowedSeparators,
) -> ScanResult<Option<Separator>> {
    if scanner.current() != Some(b'|') {
        return Ok(None);
    }
    let (separator, width) = separator_at(scanner);
    if !allowed.contains(separator) {
        return Ok(None);
    }
    for _ in 0..width {
        advance_within_line(scanner);
    }
    space(scanner)?;
    Ok(Some(separator))
}

// [spec:ronin:def:scan.scanindent-fn]
// [spec:ronin:sem:scan.scanindent-fn]
/// Whether the next line continues an indented block, consuming it if it does.
///
/// This is Ninja's `PeekToken(INDENT)`, and the putting-back matters as much
/// as the reading: what a block's deferred checks name is wherever the failed
/// peek left the scanner, which is the start of the line that ended the block.
/// Consuming a `[ ]*"\n"` there instead would name the line below it.
pub(crate) fn scanindent(scanner: &mut Scanner<'_>) -> ScanResult<bool> {
    if linestart(scanner) == LineStart::Indent {
        // Ninja eats whitespace after every token but a newline, which is what
        // carries a `$`-escaped line ending into the binding that follows.
        space(scanner)?;
        return Ok(true);
    }
    scanner.unread_token();
    Ok(false)
}

// [spec:ronin:def:scan.scannewline-fn]
// [spec:ronin:sem:scan.scannewline-fn]
pub(crate) fn scannewline(scanner: &mut Scanner<'_>) -> ScanResult<()> {
    scanner.begin_token();
    if newline(scanner)? {
        Ok(())
    } else {
        // The end of the file is a token here rather than a failure to read
        // one: Ninja asks for a newline and names whatever it got, and at the
        // end of a file that is `eof`. Running off the end *inside* a value is
        // the other thing, and `scanstring` has already raised it by now.
        Err(scanerror(
            scanner,
            ScanErrorKind::ExpectedNewline(found_token(scanner)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_a_run_of_ordinary_bytes_leaves_the_column_where_stepping_would() {
        // The bulk skip advances the column by the whole run at once, so the
        // position it reports has to match what a byte-at-a-time walk gave.
        // A tab is ordinary inside a value and inside a path alike.
        for (bytes, path, expected_column) in [
            (&b"abcdef ghi\n"[..], false, 11),
            // A path stops at the space, then `scanstring` eats the separator.
            (&b"abcdef ghi\n"[..], true, 8),
            (&b"a\tb\n"[..], false, 4),
            (&b"a\tb\n"[..], true, 4),
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

        // A tab inside a path is part of it, exactly as Ninja reads it.
        let source = Source::from_bytes("build.ninja", b"obj/\tx.o out\n".to_vec());
        let mut scanner = Scanner::new(&source);
        assert_eq!(
            scanstring(&mut scanner, true).unwrap(),
            Some(ScannedEvalString::Plain(b"obj/\tx.o"))
        );

        // A tab where a statement belongs is the one place it is an error, and
        // it is reported against the tab rather than against whatever token was
        // read last.
        let source = Source::from_bytes("build.ninja", b"rule r\n\tcommand = c\n".to_vec());
        let mut scanner = Scanner::new(&source);
        scankeyword(&mut scanner).unwrap();
        scanname(&mut scanner, NameKind::Rule).unwrap();
        scannewline(&mut scanner).unwrap();
        // The tab does not indent, so the rule's block is already over.
        assert!(!scanindent(&mut scanner).unwrap());
        let error = scankeyword(&mut scanner).unwrap_err();
        assert_eq!(error.kind, ScanErrorKind::TabsNotAllowed);
        assert_eq!((error.span.line, error.span.column), (2, 1));
    }

    /// Read one `name = value` line, the way the parser's `parselet` does.
    fn skip_binding(scanner: &mut Scanner<'_>) {
        scanname(scanner, NameKind::Variable).unwrap();
        scanchar(scanner, '=').unwrap();
        scanstring(scanner, false).unwrap();
        scannewline(scanner).unwrap();
    }

    // [spec:ronin:sem:scan.scankeyword-fn/test]
    // [spec:ronin:sem:scan.comment-fn/test]
    #[test]
    fn indent_starts_at_its_first_space() {
        // Every one of these was reported against the *preceding* token, so it
        // named the line above and dragged that line's source context onto a
        // diagnostic Ninja shows none for. The column is one — zero counting
        // Ninja's way — which is exactly what suppresses the context.
        for (bytes, line) in [
            (&b"x = 1\n  y = 2\n"[..], 2),
            (&b"x = 1\n\n  y = 2\n"[..], 3),
            (&b"x = 1\n# c\n  y = 2\n"[..], 3),
            // A `$`-escaped line ending is eaten after a token, never in front
            // of one, so neither of these is a continuation of anything.
            (&b"x = 1\n  $\nbuild a: phony\n"[..], 2),
            (&b"x = 1\n  $\n\ny = 2\n"[..], 2),
            // Half a line ending is not one: the spaces are the whole token.
            (&b"x = 1\n  \ry = 2\n"[..], 2),
            // The comment rule wants a terminating newline it never got.
            (&b"x = 1\n  # c"[..], 2),
        ] {
            let source = Source::from_bytes("build.ninja", bytes.to_vec());
            let mut scanner = Scanner::new(&source);
            skip_binding(&mut scanner);
            let error = scankeyword(&mut scanner).unwrap_err();
            assert_eq!(
                (error.kind, error.span.line, error.span.column),
                (ScanErrorKind::UnexpectedToken(FoundToken::Indent), line, 1),
                "scanning {:?}",
                bstr::BStr::new(bytes)
            );
        }

        // With no comment to run off the end of, a `#` where a statement
        // belongs is simply a byte no token begins with.
        let source = Source::from_bytes("build.ninja", b"x = 1\n# c".to_vec());
        let mut scanner = Scanner::new(&source);
        skip_binding(&mut scanner);
        let error = scankeyword(&mut scanner).unwrap_err();
        assert_eq!(error.kind, ScanErrorKind::LexingError);
        assert_eq!((error.span.line, error.span.column), (2, 1));
    }

    // [spec:ronin:sem:scan.scanindent-fn/test]
    #[test]
    fn failed_indent_peek_puts_it_back() {
        // What ends a block is where the block's deferred checks are reported,
        // so the spaces of an indented blank line have to still be ahead of
        // the scanner afterwards. Consuming the newline instead named the line
        // below, one past where Ninja stands.
        for bytes in [
            &b"rule cc\n  depfile = y\n  \nbuild a: cc\n"[..],
            &b"rule cc\n  depfile = y\n# c"[..],
        ] {
            let source = Source::from_bytes("build.ninja", bytes.to_vec());
            let mut scanner = Scanner::new(&source);
            scankeyword(&mut scanner).unwrap();
            scanname(&mut scanner, NameKind::Rule).unwrap();
            scannewline(&mut scanner).unwrap();
            assert!(scanindent(&mut scanner).unwrap());
            skip_binding(&mut scanner);
            assert!(!scanindent(&mut scanner).unwrap());
            assert_eq!(
                (scanner.line, scanner.column),
                (3, 1),
                "after the block of {:?}",
                bstr::BStr::new(bytes)
            );
        }

        // An indented comment at the end of the file *is* an indent, so the
        // block goes on and the binding's name is looked for at the `#`.
        let source = Source::from_bytes("build.ninja", b"rule cc\n  command = x\n  # c".to_vec());
        let mut scanner = Scanner::new(&source);
        scankeyword(&mut scanner).unwrap();
        scanname(&mut scanner, NameKind::Rule).unwrap();
        scannewline(&mut scanner).unwrap();
        assert!(scanindent(&mut scanner).unwrap());
        skip_binding(&mut scanner);
        assert!(scanindent(&mut scanner).unwrap());
        let error = scanname(&mut scanner, NameKind::Variable).unwrap_err();
        assert_eq!(error.kind, ScanErrorKind::ExpectedName(NameKind::Variable));
        assert_eq!((error.span.line, error.span.column), (3, 3));
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

            assert_eq!(
                scanname(&mut scanner, NameKind::Variable).unwrap().text,
                "cc"
            );
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
