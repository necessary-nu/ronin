//! Inputs shared by Make's command-line parser and kati compilation.

use crate::util::BString;
use kati::bytes::Bytes;
use kati::session::Session;

/// Turn `MAKEFLAGS` into an argv-shaped list.
///
/// GNU Make omits the dash from its leading cluster (`ks`, not `-ks`). Every
/// later word already has command-line shape, and `--` still separates
/// assignments. Parsing this list before argv gives argv normal last-word-wins
/// precedence while keeping one option grammar for both inputs.
pub(super) fn makeflags_arguments(inherited: &str) -> Vec<BString> {
    let mut arguments = vec![BString::from("make")];
    for (position, word) in inherited.split_ascii_whitespace().enumerate() {
        if position == 0 && word != "--" && !word.starts_with('-') && !word.contains('=') {
            arguments.push(BString::from(format!("-{word}")));
        } else {
            arguments.push(BString::from(word));
        }
    }
    arguments
}

/// Parse `-E`/`--eval` fragments as Makefile source before the selected file.
///
/// Kati caches a Makefile's parsed statements in its owned session. Prepending
/// the fragments there makes them ordinary compiler input while leaving the
/// selected Makefile's identity, include base, and `MAKEFILE_LIST` unchanged.
// [spec:ronin:req:make.interface-compatibility]
pub(super) fn prepend_command_line_evals(
    session: &mut Session,
    evals: &[Bytes],
) -> Result<(), kati::anyhow::Error> {
    if evals.is_empty() {
        return Ok(());
    }

    let Some(makefile_name) = session.flags.makefile.clone() else {
        return Ok(());
    };
    let Some(makefile) = session.get_makefile(&makefile_name)? else {
        return Ok(());
    };
    let filename = session.intern("*command line eval*");
    let mut statements = Vec::new();
    for source in evals {
        let parsed =
            kati::parser::parse_buf(session, source, kati::loc::Loc { filename, line: 0 })?;
        statements.extend(parsed.lock().iter().cloned());
    }
    makefile.stmts.lock().splice(0..0, statements);
    Ok(())
}
