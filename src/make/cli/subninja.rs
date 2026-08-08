//! Turn one statically expanded `$(MAKE)` command into a child compilation.

use super::{
    compilation_key, default_makefile, flag_environment, parse, path_of,
    prepend_command_line_evals, propagated_makeflags, record_invocation_variables, session_for,
    Action, MAKELEVEL,
};
use crate::make::{Compilation, CompilationContext, MakeError};
use crate::util::{BString, ByteSlice};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Resolve one expanded recursive recipe into another kati compilation unit.
// [spec:ronin:req:make.recursive-invocation+1]
pub(in crate::make) fn compile(
    command: &[u8],
    expanded_make: &[u8],
    parent: &CompilationContext,
) -> Result<Compilation, MakeError> {
    let mut words = shell_words(command)?;
    let mut directory = parent.directory.clone();
    if words.len() >= 4 && words[0].as_bytes() == b"cd" && words[2].as_bytes() == b"&&" {
        let selected =
            path_of(words[1].as_bytes()).map_err(|error| MakeError::Evaluate(error.to_string()))?;
        directory = compilation_directory(&directory, &selected)?;
        words.drain(0..3);
    }
    if words.first().is_some_and(|word| word.as_bytes() == b"exec") {
        words.remove(0);
    }
    let make_words = shell_words(expanded_make)?;
    if make_words.is_empty()
        || words.len() < make_words.len()
        || words[..make_words.len()] != make_words
    {
        return Err(MakeError::Evaluate(format!(
            "recursive MAKE reference is not the invoked command: {}",
            String::from_utf8_lossy(command)
        )));
    }
    if words.is_empty() || words.iter().any(|word| word.as_bytes() == b"&&") {
        return Err(MakeError::Evaluate(format!(
            "recursive Make recipe is not one static invocation: {}",
            String::from_utf8_lossy(command)
        )));
    }

    let inherited = (!parent.makeflags.is_empty()).then_some(parent.makeflags.as_str());
    let invocation =
        match parse(&words, inherited).map_err(|error| MakeError::Evaluate(error.to_string()))? {
            Action::Execute(invocation) => *invocation,
            Action::Immediate(result) => {
                let mut diagnostic = result.stderr;
                diagnostic.extend(result.stdout);
                let diagnostic = String::from_utf8_lossy(&diagnostic);
                return Err(MakeError::Evaluate(format!(
                    "recursive Make invocation does not describe a graph: {}",
                    diagnostic.trim()
                )));
            }
        };
    for selected in &invocation.directories {
        directory = compilation_directory(&directory, selected)?;
    }
    let makefile = invocation
        .makefile
        .clone()
        .or_else(|| default_makefile(&directory))
        .ok_or_else(|| {
            MakeError::Evaluate(format!(
                "no makefile found for recursive compilation in '{}'",
                directory.display()
            ))
        })?;
    let invoked_as =
        path_of(words[0].as_bytes()).map_err(|error| MakeError::Evaluate(error.to_string()))?;
    let mut session = session_for(&invocation, &makefile, parent.jobs, &invoked_as);
    session.invocation_environment = Some(parent.environment.clone());
    let level = parent.level.saturating_add(1);
    record_invocation_variables(&mut session, &invocation, level);
    prepend_command_line_evals(&mut session, &invocation.evals)
        .map_err(|error| MakeError::Evaluate(error.to_string()))?;

    let path_prefix = directory
        .strip_prefix(&parent.root_directory)
        .map_or_else(|_| directory.clone(), Path::to_owned);
    let makeflags = propagated_makeflags(&invocation);
    let mut cache_key = compilation_key(
        &directory,
        makefile.as_os_str().as_encoded_bytes(),
        &makeflags,
    );
    extend_compilation_key(&mut cache_key, command, expanded_make, parent);
    let mut recipe_environment = parent.recipe_environment.clone();
    set_recipe_environment(
        &mut recipe_environment,
        OsString::from(MAKELEVEL),
        Some(OsString::from(level.saturating_add(1).to_string())),
    );
    for (name, value) in flag_environment(&invocation) {
        set_recipe_environment(&mut recipe_environment, OsString::from(name), Some(value));
    }
    let environment = session
        .invocation_environment
        .clone()
        .expect("recording a child invocation preserves its environment");
    Ok(Compilation {
        session,
        shuffle: invocation.shuffle,
        context: CompilationContext {
            root_directory: parent.root_directory.clone(),
            directory,
            path_prefix,
            makeflags,
            level,
            jobs: parent.jobs,
            environment,
            recipe_environment,
        },
        cache_key,
    })
}

fn extend_compilation_key(
    key: &mut Vec<u8>,
    command: &[u8],
    expanded_make: &[u8],
    parent: &CompilationContext,
) {
    key.push(0);
    key.extend_from_slice(command);
    key.push(0);
    key.extend_from_slice(expanded_make);
    append_environment_key(key, &parent.environment);
    append_recipe_environment_key(key, &parent.recipe_environment);
}

fn set_recipe_environment(
    environment: &mut Vec<(OsString, Option<OsString>)>,
    name: OsString,
    value: Option<OsString>,
) {
    environment.retain(|(candidate, _)| candidate != &name);
    environment.push((name, value));
}

fn append_environment_key(key: &mut Vec<u8>, environment: &[(OsString, OsString)]) {
    let mut environment = environment.to_vec();
    environment.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    for (name, value) in environment {
        key.push(0);
        key.extend_from_slice(name.as_encoded_bytes());
        key.push(b'=');
        key.extend_from_slice(value.as_encoded_bytes());
    }
}

fn append_recipe_environment_key(key: &mut Vec<u8>, environment: &[(OsString, Option<OsString>)]) {
    let mut environment = environment.to_vec();
    environment.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    for (name, value) in environment {
        key.push(0);
        key.extend_from_slice(name.as_encoded_bytes());
        key.push(b'=');
        if let Some(value) = value {
            key.extend_from_slice(value.as_encoded_bytes());
        } else {
            key.push(0xff);
        }
    }
}

fn compilation_directory(base: &Path, selected: &Path) -> Result<PathBuf, MakeError> {
    let selected = if selected.is_absolute() {
        selected.to_owned()
    } else {
        base.join(selected)
    };
    let directory = std::fs::canonicalize(&selected).map_err(|error| {
        MakeError::Evaluate(format!(
            "cannot enter recursive Make directory '{}': {error}",
            selected.display()
        ))
    })?;
    if !directory.is_dir() {
        return Err(MakeError::Evaluate(format!(
            "recursive Make directory '{}' is not a directory",
            directory.display()
        )));
    }
    Ok(directory)
}

/// Split the shell words a static recursive invocation may contain.
///
/// Quotes and backslash escapes are resolved because the nested process would
/// have received their results as argv. Shell expansion, pipelines, lists,
/// redirections and globbing are rejected: those are runtime programs, not a
/// statically selected subninja.
fn shell_words(command: &[u8]) -> Result<Vec<BString>, MakeError> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Quote {
        Single,
        Double,
    }

    let failure = || {
        MakeError::Evaluate(format!(
            "recursive Make recipe is not a static invocation: {}",
            String::from_utf8_lossy(command)
        ))
    };
    let mut words = Vec::new();
    let mut word = Vec::new();
    let mut quote = None;
    let mut index = 0;
    while index < command.len() {
        let byte = command[index];
        match quote {
            Some(Quote::Single) => {
                if byte == b'\'' {
                    quote = None;
                } else {
                    word.push(byte);
                }
            }
            Some(Quote::Double) => match byte {
                b'"' => quote = None,
                b'\\' => {
                    index += 1;
                    let escaped = command.get(index).copied().ok_or_else(&failure)?;
                    word.push(escaped);
                }
                b'$' | b'`' => return Err(failure()),
                _ => word.push(byte),
            },
            None => match byte {
                b'\'' => quote = Some(Quote::Single),
                b'"' => quote = Some(Quote::Double),
                b'\\' => {
                    index += 1;
                    let escaped = command.get(index).copied().ok_or_else(&failure)?;
                    word.push(escaped);
                }
                b'&' if command.get(index + 1) == Some(&b'&') => {
                    if !word.is_empty() {
                        words.push(BString::from(std::mem::take(&mut word)));
                    }
                    words.push(BString::from("&&"));
                    index += 1;
                }
                byte if byte.is_ascii_whitespace() => {
                    if !word.is_empty() {
                        words.push(BString::from(std::mem::take(&mut word)));
                    }
                }
                b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')' | b'{' | b'}' | b'$' | b'`'
                | b'*' | b'?' | b'[' | b']' | b'~' | b'#' => return Err(failure()),
                _ => word.push(byte),
            },
        }
        index += 1;
    }
    if quote.is_some() {
        return Err(failure());
    }
    if !word.is_empty() {
        words.push(BString::from(word));
    }
    Ok(words)
}
