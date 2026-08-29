//! Turn one statically expanded `$(MAKE)` command into a child compilation.

use super::{
    Action, GNUMAKEFLAGS, MAKELEVEL, carry_command_line_evals, compilation_key, named_makefiles,
    parse, path_of, propagated_makeflags, record_invocation_variables, session_for,
};
use crate::make::{Compilation, CompilationContext, MakeError};
use crate::util::{BString, ByteSlice};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Resolve one expanded recursive recipe into another kati compilation unit.
// [spec:ronin:req:make.recursive-invocation+2]
pub(in crate::make) fn compile(
    command: &[u8],
    expanded_make: &[u8],
    shell: &[u8],
    shell_flags: &[u8],
    parent: &CompilationContext,
) -> Result<Compilation, MakeError> {
    let (words, mut directory) =
        invocation_words(command, expanded_make, shell, shell_flags, parent)?;
    let inherited = (!parent.makeflags.is_empty()).then_some(parent.makeflags.as_str());
    // A composed child reads the environment its parent settled, where
    // `GNUMAKEFLAGS` has already been emptied — so its own decode finds nothing
    // and the switches reach it once, through `MAKEFLAGS`, exactly as they
    // reach a child GNU Make starts. Read rather than assumed empty, because a
    // Makefile is free to write the name again before the `$(MAKE)` line.
    let gnumakeflags = parent
        .environment
        .iter()
        .rev()
        .find(|(name, _)| name == GNUMAKEFLAGS)
        .map(|(_, value)| value.to_string_lossy().into_owned());
    let invocation = match parse(
        &words,
        inherited,
        gnumakeflags.as_deref(),
        &parent.diagnostics,
    )
    .map_err(|error| MakeError::Evaluate(error.to_string()))?
    {
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
    let makefiles = named_makefiles(&invocation, &directory);
    if makefiles.is_empty() {
        return Err(MakeError::MissingChildMakefile { directory });
    }
    let invoked_as =
        path_of(words[0].as_bytes()).map_err(|error| MakeError::Evaluate(error.to_string()))?;
    let mut session = session_for(
        &invocation,
        &makefiles,
        parent.jobs,
        &invoked_as,
        &parent.diagnostics,
        &parent.census,
    );
    session.invocation_environment = Some(std::sync::Arc::clone(&parent.environment));
    let level = parent.level.saturating_add(1);
    record_invocation_variables(&mut session, &invocation, level, 0);
    carry_command_line_evals(&mut session, &invocation.evals);

    let path_prefix = directory
        .strip_prefix(&parent.root_directory)
        .map_or_else(|_| directory.clone(), Path::to_owned);
    // So a census can say which `Makefile` a line is in: this child reads its
    // own from its own directory, under the same name its parent used.
    session.unit_prefix = path_prefix.as_os_str().as_encoded_bytes().to_vec();
    let makeflags = propagated_makeflags(&invocation);
    let mut cache_key = compilation_key(&directory, &makefiles, &makeflags);
    extend_compilation_key(
        &mut cache_key,
        command,
        expanded_make,
        shell,
        shell_flags,
        parent,
    );
    let mut recipe_environment = parent.recipe_environment.clone();
    set_recipe_environment(
        &mut recipe_environment,
        OsString::from(MAKELEVEL),
        Some(OsString::from(level.saturating_add(1).to_string())),
    );
    let environment = session
        .invocation_environment
        .clone()
        .expect("recording a child invocation preserves its environment");
    Ok(Compilation {
        session,
        shuffle: invocation.shuffle,
        context: CompilationContext {
            diagnostics: std::sync::Arc::clone(&parent.diagnostics),
            interrupts: std::sync::Arc::clone(&parent.interrupts),
            census: std::sync::Arc::clone(&parent.census),
            reporting: parent.reporting,
            root_directory: parent.root_directory.clone(),
            directory,
            path_prefix,
            makeflags,
            always_make: parent.always_make,
            restarted: parent.restarted,
            // Deliberately not `parent.assumed_new` or `parent.assumed_old`.
            // GNU Make puts neither `-W` nor `-o` in `MAKEFLAGS`, so a
            // recursive child is never told about a file the parent was asked
            // to pretend was new or old — and the names are the parent's own
            // directory's in any case. Measured: under `make -o sub` the child
            // remakes `sub` exactly as it would have without the switch.
            assumed_new: Vec::new(),
            assumed_old: Vec::new(),
            level,
            jobs: parent.jobs,
            parallel_reads: parent.parallel_reads,
            environment,
            recipe_environment,
        },
        cache_key,
    })
}

fn invocation_words(
    command: &[u8],
    expanded_make: &[u8],
    shell: &[u8],
    shell_flags: &[u8],
    parent: &CompilationContext,
) -> Result<(Vec<BString>, PathBuf), MakeError> {
    let mut expand = |script: &[u8]| shell_command_substitution(script, shell, shell_flags, parent);
    let mut words = shell_words_with(command, Some(&mut expand))?;
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

    Ok((words, directory))
}

fn extend_compilation_key(
    key: &mut Vec<u8>,
    command: &[u8],
    expanded_make: &[u8],
    shell: &[u8],
    shell_flags: &[u8],
    parent: &CompilationContext,
) {
    key.push(0);
    key.extend_from_slice(command);
    key.push(0);
    key.extend_from_slice(expanded_make);
    key.push(0);
    key.extend_from_slice(shell);
    key.push(0);
    key.extend_from_slice(shell_flags);
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
///
/// A backslash before a newline is the one escape that stands for nothing.
/// The shell removes the pair and joins what is on either side of it, and it
/// is how a long invocation is written over two lines — the kernel's
/// `__sub-make` writes `$(MAKE) … -C $(abs_objtree) \` and its `-f` on the
/// line below. Reading it as an escaped newline puts that byte inside the
/// next word, and the child is then asked to build a goal spelled with a
/// newline in front of it.
fn shell_words(command: &[u8]) -> Result<Vec<BString>, MakeError> {
    shell_words_with(command, None)
}

type CommandExpansion<'a> = dyn FnMut(&[u8]) -> Result<Vec<u8>, MakeError> + 'a;

fn shell_words_with(
    command: &[u8],
    mut expansion: Option<&mut CommandExpansion<'_>>,
) -> Result<Vec<BString>, MakeError> {
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
                    if escaped != b'\n' {
                        word.push(escaped);
                    }
                }
                b'$' if command.get(index + 1) == Some(&b'(') => {
                    let end = command_substitution_end(command, index + 2).ok_or_else(&failure)?;
                    let expand = expansion.as_deref_mut().ok_or_else(&failure)?;
                    let output = expand(&command[index + 2..end])?;
                    word.extend(output);
                    index = end;
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
                    if escaped != b'\n' {
                        word.push(escaped);
                    }
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
                b'$' if command.get(index + 1) == Some(&b'(') => {
                    let end = command_substitution_end(command, index + 2).ok_or_else(&failure)?;
                    let expand = expansion.as_deref_mut().ok_or_else(&failure)?;
                    let output = expand(&command[index + 2..end])?;
                    append_unquoted_substitution(&mut words, &mut word, &output)
                        .ok_or_else(&failure)?;
                    index = end;
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

/// Find the close of one shell command substitution without mistaking quoted
/// parentheses for its delimiter. The shell still parses and executes the
/// body; this scanner only isolates the bytes it receives.
fn command_substitution_end(command: &[u8], start: usize) -> Option<usize> {
    #[derive(Clone, Copy)]
    enum Quote {
        Single,
        Double,
        Backtick,
    }

    let mut depth = 1usize;
    let mut quote = None;
    let mut index = start;
    while index < command.len() {
        let byte = command[index];
        match quote {
            Some(Quote::Single) => {
                if byte == b'\'' {
                    quote = None;
                }
            }
            Some(Quote::Double) => match byte {
                b'"' => quote = None,
                b'\\' => index += 1,
                b'$' if command.get(index + 1) == Some(&b'(') => {
                    depth += 1;
                    index += 1;
                }
                b')' if depth > 1 => depth -= 1,
                _ => {}
            },
            Some(Quote::Backtick) => match byte {
                b'`' => quote = None,
                b'\\' => index += 1,
                _ => {}
            },
            None => match byte {
                b'\'' => quote = Some(Quote::Single),
                b'"' => quote = Some(Quote::Double),
                b'`' => quote = Some(Quote::Backtick),
                b'\\' => index += 1,
                b'$' if command.get(index + 1) == Some(&b'(') => {
                    depth += 1;
                    index += 1;
                }
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            },
        }
        index += 1;
    }
    None
}

/// Apply the default shell's field splitting to unquoted substitution output.
/// Globbing remains dynamic, so a result containing a glob is refused rather
/// than composed as a different argv from the one the shell would produce.
fn append_unquoted_substitution(
    words: &mut Vec<BString>,
    word: &mut Vec<u8>,
    output: &[u8],
) -> Option<()> {
    let fields = output
        .split(u8::is_ascii_whitespace)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    if fields
        .iter()
        .any(|field| field.iter().any(|byte| matches!(byte, b'*' | b'?' | b'[')))
    {
        return None;
    }
    let Some((last, preceding)) = fields.split_last() else {
        return Some(());
    };
    for field in preceding {
        word.extend_from_slice(field);
        words.push(BString::from(std::mem::take(word)));
    }
    word.extend_from_slice(last);
    Some(())
}

/// Execute the computation Kati deferred out of `$(shell ...)` when it expanded
/// the recursive recipe. This happens only after the parent prerequisites have
/// crossed the recursive evaluation boundary, in the exact directory and
/// exported environment the recipe would have received.
fn shell_command_substitution(
    script: &[u8],
    shell: &[u8],
    shell_flags: &[u8],
    parent: &CompilationContext,
) -> Result<Vec<u8>, MakeError> {
    // The recipe's own shell, and the build's own where that is the default:
    // a computation deferred out of a recipe is read by the shell that would
    // have read the recipe.
    // [spec:ronin:req:product.builtin-shell]
    let mut command = kati::simple_command::shell_process(
        OsStr::from_bytes(shell),
        crate::subprocess::builtin_shell(),
    );
    command
        .arg(OsStr::from_bytes(shell_flags))
        .arg(OsStr::from_bytes(script))
        .current_dir(&parent.directory)
        .env_clear()
        .envs(parent.environment.iter().map(|(name, value)| (name, value)))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    for (name, value) in &parent.recipe_environment {
        if let Some(value) = value {
            command.env(name, value);
        } else {
            command.env_remove(name);
        }
    }
    let mut output = command.output().map_err(|error| {
        MakeError::Evaluate(format!(
            "running recursive Make command substitution '{}': {error}",
            String::from_utf8_lossy(script)
        ))
    })?;
    while output.stdout.last() == Some(&b'\n') {
        output.stdout.pop();
    }
    output.stdout.retain(|byte| *byte != 0);
    Ok(output.stdout)
}
