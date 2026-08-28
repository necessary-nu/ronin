#![cfg(test)]

use super::{
    Action, ArgumentShape, Invocation, MAKE_OPTION_SURFACE, Shuffle, decode_makefile_makeflags,
    parse,
};
use crate::util::BString;
use kati::bytes::Bytes;
use std::path::Path;

pub(super) fn parsed(arguments: &[&str]) -> Invocation {
    parsed_under(None, arguments)
}

pub(super) fn parsed_under(inherited: Option<&str>, arguments: &[&str]) -> Invocation {
    parsed_with_environment(None, inherited, arguments)
}

/// Both environment option streams, in the order GNU Make reads them.
pub(super) fn parsed_with_environment(
    gnumakeflags: Option<&str>,
    inherited: Option<&str>,
    arguments: &[&str],
) -> Invocation {
    let arguments = arguments
        .iter()
        .map(|argument| BString::from(*argument))
        .collect::<Vec<_>>();
    let diagnostics = std::sync::Arc::new(kati::diagnostics::Diagnostics::collected());
    match parse(&arguments, inherited, gnumakeflags, &diagnostics).unwrap() {
        Action::Execute(invocation) => *invocation,
        Action::Immediate(_) => panic!("these arguments describe a build"),
    }
}

/// The diagnostic an argument list is refused with, or nothing if it
/// described a build after all.
pub(super) fn refused(arguments: &[&str]) -> Option<String> {
    let arguments = arguments
        .iter()
        .map(|argument| BString::from(*argument))
        .collect::<Vec<_>>();
    let diagnostics = std::sync::Arc::new(kati::diagnostics::Diagnostics::collected());
    match parse(&arguments, None, None, &diagnostics).unwrap() {
        Action::Immediate(result) => {
            // An option Make does not know is a build it will not attempt,
            // and GNU Make abandons with two whatever the reason.
            assert_eq!(result.exit_code, super::ABANDONED);
            Some(String::from_utf8_lossy(&result.stderr).into_owned())
        }
        Action::Execute(_) => None,
    }
}

// [spec:ronin:req:make.semantics+1/test]
// [spec:ronin:req:make.recursive-invocation+2/test]
#[test]
fn makefile_makeflags_mutate_switch_table() {
    let decoded = decode_makefile_makeflags(b"", b" -- FOO=bar -rR", b"").unwrap();
    assert_eq!(decoded.makeflags.as_ref(), b"rR");
    assert_eq!(decoded.mflags.as_ref(), b"-rR");

    // Plain `=` adds to the special table just as `+=` does. A contradictory
    // spelling changes the settled state rather than creating a second table.
    let decoded = decode_makefile_makeflags(&decoded.makeflags, b"-w", b"").unwrap();
    assert_eq!(decoded.makeflags.as_ref(), b"rRw");
    let decoded =
        decode_makefile_makeflags(&decoded.makeflags, b"--no-print-directory -k", b"").unwrap();
    assert_eq!(decoded.makeflags.as_ref(), b"krR --no-print-directory");
    assert!(decoded.makeflags.as_ref().starts_with(b"k"));

    // Environment/argv switches are protected and therefore get the last
    // word when a Makefile tries to contradict them.
    let decoded =
        decode_makefile_makeflags(b"w", b"w -- FOO=bar --no-print-directory", b"w").unwrap();
    assert_eq!(decoded.makeflags.as_ref(), b"w");
    assert_eq!(decoded.mflags.as_ref(), b"-w");
}

/// A word of the write that binds a name is handed to the evaluator, and the
/// leading-cluster rule belongs to the first word of the value and to no other.
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn a_makeflags_write_hands_back_names() {
    let decoded = decode_makefile_makeflags(b"", b"FOO=bar", b"").unwrap();
    assert_eq!(decoded.assignments, vec![Bytes::from("FOO=bar")]);
    assert_eq!(decoded.makeflags.as_ref(), b"");

    // Every operator the scanner knows arrives whole; what an operator means is
    // the evaluator's answer, not this grammar's.
    let decoded = decode_makefile_makeflags(b"", b"-k A:=1 B+=2 C?=3 D!=echo", b"").unwrap();
    assert_eq!(
        decoded.assignments,
        vec![
            Bytes::from("A:=1"),
            Bytes::from("B+=2"),
            Bytes::from("C?=3"),
            Bytes::from("D!=echo"),
        ]
    );
    assert_eq!(decoded.makeflags.as_ref(), b"k");

    // The switch table and the protected state are read back through the same
    // grammar, so a name bound out of either would be bound again on every
    // write. Only the write itself binds.
    let decoded = decode_makefile_makeflags(b"KEPT=1", b"-w", b"HELD=2").unwrap();
    assert!(decoded.assignments.is_empty());

    // A word that is not a switch and binds nothing is dropped, and the dash
    // the leading cluster is missing goes to the first word of the value —
    // never to a word an assignment happens to stand in front of. `-ran` is
    // three switches, one of them a dry run.
    let decoded = decode_makefile_makeflags(b"", b"FOO=bar ran", b"").unwrap();
    assert_eq!(decoded.assignments, vec![Bytes::from("FOO=bar")]);
    assert!(!decoded.is_dry_run);
    assert_eq!(decoded.makeflags.as_ref(), b"");

    let decoded = decode_makefile_makeflags(b"", b"ran FOO=bar", b"").unwrap();
    assert!(decoded.is_dry_run);
    assert_eq!(decoded.makeflags.as_ref(), b"nr");
}

/// GNU Make settles `--shuffle` before it reads a makefile, so a makefile's
/// write reaches only the switch table: republished exactly as written, never
/// examined, and unable to take back the order the command line asked for.
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn a_makefiles_shuffle_word_is_republished() {
    // Nothing looks at it, so a word naming no mode is not an error.
    let decoded = decode_makefile_makeflags(b"", b"--shuffle=bogus", b"").unwrap();
    assert_eq!(decoded.makeflags.as_ref(), b" --shuffle=bogus");

    // The command line's `random` settles on a seed and travels as one; a
    // makefile's is stored as the word, because the block that would have
    // rewritten it has already run.
    let decoded = decode_makefile_makeflags(b"", b"--shuffle=random", b"").unwrap();
    assert_eq!(decoded.makeflags.as_ref(), b" --shuffle=random");

    // An empty argument leaves the table entry empty, which publishes nothing.
    let decoded = decode_makefile_makeflags(b"", b"--shuffle=", b"").unwrap();
    assert_eq!(decoded.makeflags.as_ref(), b"");

    // Unlike every switch that acts, this one is not protected: the command
    // line settled the order already, and the word the table ends up holding
    // is the makefile's.
    let decoded = decode_makefile_makeflags(
        b" --shuffle=reverse",
        b" --shuffle=reverse --shuffle=none",
        b" --shuffle=reverse",
    )
    .unwrap();
    assert_eq!(decoded.makeflags.as_ref(), b" --shuffle=none");
}

// [spec:ronin:req:make.interface-compatibility/test]
#[test]
fn accepts_every_make_option_shape() {
    fn sample(spelling: &str) -> &'static str {
        match spelling {
            "-C" | "--directory" | "-I" | "--include-dir" => ".",
            "-E" | "--eval" => "SURFACE:=accepted",
            "-f" | "--file" | "--makefile" => "Makefile",
            "--jobserver-style" => "fifo",
            "--jobserver-auth" => "fifo:/tmp/ronin-ignored",
            "--jobserver-fds" => "3,4",
            "--sync-mutex" => "fnm:/tmp/ronin-ignored",
            "-o" | "--old-file" | "--assume-old" => "old",
            "-W" | "--what-if" | "--new-file" | "--assume-new" => "new",
            "--debug" => "n",
            "-O" | "--output-sync" | "--shuffle" => "none",
            "-j" | "--jobs" | "-l" | "--load-average" | "--max-load" => "2",
            _ => "value",
        }
    }

    fn accepted(arguments: &[String], inherited: Option<&str>) -> bool {
        let arguments = arguments
            .iter()
            .map(|argument| BString::from(argument.as_str()))
            .collect::<Vec<_>>();
        let diagnostics = std::sync::Arc::new(kati::diagnostics::Diagnostics::collected());
        match parse(&arguments, inherited, None, &diagnostics) {
            Ok(Action::Execute(_)) => true,
            Ok(Action::Immediate(result)) => result.exit_code == 0,
            Err(_) => false,
        }
    }

    for declared in MAKE_OPTION_SURFACE {
        for spelling in declared.spellings {
            let value = sample(spelling);
            let mut separate = vec!["make".to_owned(), (*spelling).to_owned()];
            if matches!(
                declared.argument,
                ArgumentShape::Required | ArgumentShape::OptionalNumeric
            ) {
                separate.push(value.to_owned());
            }
            separate.push("goal".to_owned());
            assert!(
                accepted(&separate, None),
                "argv refused {spelling} as {:?}/{:?}",
                declared.class,
                declared.argument
            );

            let inherited = separate[1..separate.len() - 1].join(" ");
            assert!(
                accepted(&["make".to_owned(), "goal".to_owned()], Some(&inherited)),
                "MAKEFLAGS refused {spelling} as {:?}/{:?}",
                declared.class,
                declared.argument
            );

            if declared.argument != ArgumentShape::None {
                let attached = if spelling.starts_with("--") {
                    format!("{spelling}={value}")
                } else {
                    format!("{spelling}{value}")
                };
                assert!(
                    accepted(
                        &["make".to_owned(), attached.clone(), "goal".to_owned()],
                        None
                    ),
                    "argv refused attached {attached}"
                );
                assert!(
                    accepted(&["make".to_owned(), "goal".to_owned()], Some(&attached)),
                    "MAKEFLAGS refused attached {attached}"
                );
            }
        }
    }
}

// [spec:ronin:req:make.interface-compatibility/test]
#[test]
fn eval_fragments_reach_compilation() {
    let invocation = parsed_under(
        Some("--eval=FLAG_GOAL:=from_flags"),
        &["make", "--eval", "$(FLAG_GOAL): ; @:", "from_flags"],
    );
    assert_eq!(
        invocation.evals,
        [
            kati::bytes::Bytes::from_static(b"FLAG_GOAL:=from_flags"),
            kati::bytes::Bytes::from_static(b"$(FLAG_GOAL): ; @:")
        ]
    );

    let directory = tempfile::tempdir().unwrap();
    let makefile = directory.path().join("Makefile");
    std::fs::write(&makefile, "").unwrap();
    let invoked_as = Path::new("make");
    let mut session = super::session_for(
        &invocation,
        std::slice::from_ref(&makefile),
        1,
        invoked_as,
        &std::sync::Arc::new(kati::diagnostics::Diagnostics::to_stderr()),
        &std::sync::Arc::new(kati::census::Census::ignored()),
    );
    super::record_invocation_variables(&mut session, &invocation, 0, 0);
    let context = super::compilation_context(
        &invocation,
        directory.path().canonicalize().unwrap(),
        super::JobCounts {
            carried: 1,
            parallel_reads: 1,
        },
        0,
        &session,
        false,
        0,
    );
    let loaded = super::evaluated(
        session,
        &invocation.evals,
        Shuffle::None,
        context,
        "",
        &crate::make::Groundwork::default(),
    )
    .expect("the goal supplied only by --eval must compile into the graph");
    let target = loaded
        .graph
        .lookup(b"from_flags")
        .expect("the inherited eval names the command-line goal");
    assert!(
        loaded.graph.generator(target).is_some(),
        "the argv eval compiles a producing edge for the inherited name"
    );
}

/// A generated include is compiler input, so the first compilation exposes
/// its producer as a graph root without changing the Makefile's default goal.
/// The CLI builds this root with the ordinary scheduler and compiles again.
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn generated_include_is_provisional_graph_root() {
    let directory = tempfile::tempdir().unwrap();
    let makefile = directory.path().join("Makefile");
    std::fs::write(
        &makefile,
        "all: ; @printf '%s\\n' '$(GENERATED)' > out\n\
         include gen.mk\n\
         gen.mk: ; @printf 'GENERATED := yes\\n' > $@\n",
    )
    .unwrap();

    let invocation = parsed(&["make"]);
    let mut session = super::session_for(
        &invocation,
        std::slice::from_ref(&makefile),
        1,
        Path::new("make"),
        &std::sync::Arc::new(kati::diagnostics::Diagnostics::to_stderr()),
        &std::sync::Arc::new(kati::census::Census::ignored()),
    );
    super::record_invocation_variables(&mut session, &invocation, 0, 0);
    let context = super::compilation_context(
        &invocation,
        directory.path().canonicalize().unwrap(),
        super::JobCounts {
            carried: 1,
            parallel_reads: 1,
        },
        0,
        &session,
        false,
        0,
    );
    let loaded = super::evaluated(
        session,
        &[],
        Shuffle::None,
        context,
        "",
        &crate::make::Groundwork::default(),
    )
    .expect("the missing include's rule should compile provisionally");

    let [include] = loaded.regeneration_targets() else {
        panic!("the provisional graph should name exactly one generated include");
    };
    assert!(loaded.graph.generator(*include).is_some());
    assert!(!loaded.graph.default_targets().contains(include));
}

// [spec:ronin:req:make.interface-compatibility/test]
#[test]
fn unknown_make_option_is_refused() {
    let diagnostic = refused(&["make", "--not-a-make-option"]).expect("an unknown option");
    assert!(
        diagnostic.starts_with("ronin: unrecognized option '--not-a-make-option'"),
        "{diagnostic}"
    );
}

/// Every switch argument is quoted on its way into the switch table, because
/// GNU Make's `define_makeflags` runs one `quote_for_env` over `flags->arg`
/// without asking which switch it belongs to. Two switches carry arbitrary
/// text — `-I` and `--debug` — so those are the two that can show it.
///
/// What the quoting is for is the round trip: `MAKEFLAGS` is read back as a
/// command line, so a blank must not end the word and a backslash must not be
/// taken as quoting the byte after it. A `$` is doubled by the same rule, and
/// what Ronin does with the doubling when the variable is READ is a decision
/// of its own — see `docs/make-oracle-divergences.md`.
// [spec:ronin:req:make.recursive-invocation+2/test]
#[test]
fn a_switch_argument_reaches_makeflags_quoted() {
    let flags = super::compiler_flag_variables(&parsed(&["make", "--debug=b\\x$y"]));
    assert_eq!(flags.base, " --debug=b\\\\x$$y");
    assert_eq!(flags.mflags, "--debug=b\\\\x$$y");

    let flags = super::compiler_flag_variables(&parsed(&["make", "-I", "a b$c\\d"]));
    assert_eq!(flags.base, " -Ia\\ b$$c\\\\d");

    // A spec with nothing to quote is published as it was written, which is
    // what keeps the ordinary case readable in a child's environment.
    let flags = super::compiler_flag_variables(&parsed(&["make", "--debug=basic"]));
    assert_eq!(flags.base, " --debug=basic");

    // And it survives being read back: the decoder unquotes exactly what the
    // publisher quoted, so a makefile's own write settles against the same
    // word the command line supplied.
    let decoded = decode_makefile_makeflags(b"", b" --debug=b\\\\x$$y", b"").unwrap();
    assert_eq!(decoded.makeflags.as_ref(), b" --debug=b\\\\x$$y");
}
