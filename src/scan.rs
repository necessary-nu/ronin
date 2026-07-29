//! Ninja manifest scanner translated from `scan.c`.

use crate::util::{BString, EvalPart, EvalString};
use std::fs;

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
    pub path: String,
    chars: Vec<char>,
    index: usize,
    pub line: usize,
    pub col: usize,
    pub paths: Vec<EvalString>,
    pub manifest_version_major: i32,
    pub manifest_version_minor: i32,
}

impl Scanner {
    fn chr(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }
}

// [spec:samurai:def:scan.scaninit-fn]
// [spec:samurai:sem:scan.scaninit-fn]
pub fn scaninit(path: &str) -> Result<Scanner, String> {
    Ok(Scanner {
        path: path.into(),
        chars: fs::read_to_string(path)
            .map_err(|e| e.to_string())?
            .chars()
            .collect(),
        index: 0,
        line: 1,
        col: 1,
        paths: Vec::new(),
        manifest_version_major: 1,
        manifest_version_minor: 9,
    })
}

// [spec:samurai:def:scan.scanclose-fn]
// [spec:samurai:sem:scan.scanclose-fn]
pub fn scanclose(_scanner: Scanner) {}

// [spec:samurai:def:scan.scanerror-fn]
// [spec:samurai:sem:scan.scanerror-fn]
pub fn scanerror(scanner: &Scanner, message: &str) -> String {
    format!(
        "{}:{}:{}: {message}",
        scanner.path, scanner.line, scanner.col
    )
}

// [spec:samurai:def:scan.next-fn]
// [spec:samurai:sem:scan.next-fn]
fn next(scanner: &mut Scanner) -> Option<char> {
    if scanner.chr() == Some('\n') {
        scanner.line += 1;
        scanner.col = 1;
    } else {
        scanner.col += 1;
    }
    scanner.index += 1;
    scanner.chr()
}

// [spec:samurai:def:scan.issimplevar-fn]
// [spec:samurai:sem:scan.issimplevar-fn]
fn issimplevar(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

// [spec:samurai:def:scan.isvar-fn]
// [spec:samurai:sem:scan.isvar-fn]
fn isvar(c: char) -> bool {
    issimplevar(c) || c == '.'
}

// [spec:samurai:def:scan.newline-fn]
// [spec:samurai:sem:scan.newline-fn]
fn newline(scanner: &mut Scanner) -> Result<bool, String> {
    match scanner.chr() {
        Some('\r') => {
            next(scanner);
            if scanner.chr() != Some('\n') {
                return Err(scanerror(scanner, "expected '\\n' after '\\r'"));
            }
            next(scanner);
            Ok(true)
        }
        Some('\n') => {
            next(scanner);
            Ok(true)
        }
        _ => Ok(false),
    }
}

// [spec:samurai:def:scan.singlespace-fn]
// [spec:samurai:sem:scan.singlespace-fn]
fn singlespace(scanner: &mut Scanner) -> Result<bool, String> {
    match scanner.chr() {
        Some(' ') => {
            next(scanner);
            Ok(true)
        }
        Some('\t') => Err(scanerror(scanner, "tabs are not allowed, use spaces")),
        Some('$') => {
            next(scanner);
            if newline(scanner)? {
                Ok(true)
            } else {
                scanner.index -= 1;
                scanner.col -= 1;
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
    if scanner.chr() != Some('#') {
        return Ok(false);
    }
    while scanner.chr().is_some() && !newline(scanner)? {
        next(scanner);
    }
    Ok(true)
}

// [spec:samurai:def:scan.name-fn]
// [spec:samurai:sem:scan.name-fn]
fn name(scanner: &mut Scanner) -> Result<String, String> {
    let mut name = String::new();
    while scanner.chr().is_some_and(isvar) {
        name.push(scanner.chr().unwrap());
        next(scanner);
    }
    if name.is_empty() {
        Err(scanerror(scanner, "expected name"))
    } else {
        space(scanner)?;
        Ok(name)
    }
}

// [spec:samurai:def:scan.scankeyword-fn]
// [spec:samurai:sem:scan.scankeyword-fn]
pub fn scankeyword(scanner: &mut Scanner) -> Result<Option<Token>, String> {
    loop {
        match scanner.chr() {
            None => return Ok(None),
            Some(' ') => {
                space(scanner)?;
                if !comment(scanner)? && !newline(scanner)? {
                    return Err(scanerror(scanner, "unexpected indent"));
                }
            }
            Some('#') => {
                comment(scanner)?;
            }
            Some('\r' | '\n') => {
                newline(scanner)?;
            }
            _ => {
                return Ok(Some(match name(scanner)?.as_str() {
                    "build" => Token::Build,
                    "default" => Token::Default,
                    "include" => Token::Include,
                    "pool" => Token::Pool,
                    "rule" => Token::Rule,
                    "subninja" => Token::Subninja,
                    _ => Token::Variable,
                }))
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
fn addstringpart(parts: &mut Vec<EvalPart>, text: String, variable: bool) {
    let part = if variable {
        EvalPart::Variable(text)
    } else {
        EvalPart::Literal(BString::from(text))
    };
    parts.push(part);
}

// [spec:samurai:def:scan.escape-fn]
// [spec:samurai:sem:scan.escape-fn]
fn escape(
    scanner: &mut Scanner,
    parts: &mut Vec<EvalPart>,
    literal: &mut String,
) -> Result<(), String> {
    match scanner.chr() {
        Some('$' | ' ' | ':') => {
            literal.push(scanner.chr().unwrap());
            next(scanner);
        }
        Some('{') => {
            if !literal.is_empty() {
                addstringpart(parts, std::mem::take(literal), false);
            }
            next(scanner);
            let mut variable = String::new();
            while scanner.chr().is_some_and(isvar) {
                variable.push(scanner.chr().unwrap());
                next(scanner);
            }
            if scanner.chr() != Some('}') {
                return Err(scanerror(scanner, "invalid variable name"));
            }
            next(scanner);
            addstringpart(parts, variable, true);
        }
        Some('\r' | '\n') => {
            newline(scanner)?;
            space(scanner)?;
        }
        Some('^') => {
            if scanner.manifest_version_major < 1
                || scanner.manifest_version_major == 1 && scanner.manifest_version_minor < 14
            {
                return Err(scanerror(
                    scanner,
                    "using $^ escape requires specifying 'ninja_required_version' with version greater or equal 1.14",
                ));
            }
            next(scanner);
            literal.push('\n');
        }
        _ => {
            if !literal.is_empty() {
                addstringpart(parts, std::mem::take(literal), false);
            }
            let mut variable = String::new();
            while scanner.chr().is_some_and(issimplevar) {
                variable.push(scanner.chr().unwrap());
                next(scanner);
            }
            if variable.is_empty() {
                return Err(scanerror(scanner, "invalid $ escape"));
            }
            addstringpart(parts, variable, true);
        }
    }
    Ok(())
}

// [spec:samurai:def:scan.scanstring-fn]
// [spec:samurai:sem:scan.scanstring-fn]
pub fn scanstring(scanner: &mut Scanner, path: bool) -> Result<Option<EvalString>, String> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    loop {
        match scanner.chr() {
            Some('$') => {
                next(scanner);
                escape(scanner, &mut parts, &mut literal)?;
            }
            Some(':' | '|' | ' ') if path => break,
            Some('\r' | '\n') | None => break,
            Some(c) => {
                literal.push(c);
                next(scanner);
            }
        }
    }
    if !literal.is_empty() {
        addstringpart(&mut parts, literal, false);
    }
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
    if scanner.chr() != Some(expected) {
        return Err(scanerror(scanner, &format!("expected '{expected}'")));
    }
    next(scanner);
    space(scanner)?;
    Ok(())
}

// [spec:samurai:def:scan.scanpipe-fn]
// [spec:samurai:sem:scan.scanpipe-fn]
pub fn scanpipe(scanner: &mut Scanner, allowed: i32) -> Result<i32, String> {
    if scanner.chr() != Some('|') {
        return Ok(0);
    }
    next(scanner);
    if scanner.chr() != Some('|') {
        if allowed & 1 == 0 {
            return Err(scanerror(scanner, "expected '||'"));
        }
        space(scanner)?;
        return Ok(1);
    }
    if allowed & 2 == 0 {
        return Err(scanerror(scanner, "unexpected '||'"));
    }
    next(scanner);
    space(scanner)?;
    Ok(2)
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
        Ok(())
    } else {
        Err(scanerror(scanner, "expected newline"))
    }
}
