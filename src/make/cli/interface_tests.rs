#![cfg(test)]

use super::{
    Action, ArgumentShape, Invocation, MAKE_OPTION_SURFACE, Shuffle, decode_makefile_makeflags,
    parse,
};
use crate::util::BString;
use std::path::Path;

pub(super) fn parsed(arguments: &[&str]) -> Invocation {
    parsed_under(None, arguments)
}

pub(super) fn parsed_under(inherited: Option<&str>, arguments: &[&str]) -> Invocation {
    let arguments = arguments
        .iter()
        .map(|argument| BString::from(*argument))
        .collect::<Vec<_>>();
    match parse(&arguments, inherited).unwrap() {
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
    match parse(&arguments, None).unwrap() {
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
// [spec:ronin:req:make.recursive-invocation+1/test]
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
        match parse(&arguments, inherited) {
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
    let mut session =
        super::session_for(&invocation, std::slice::from_ref(&makefile), 1, invoked_as);
    super::record_invocation_variables(&mut session, &invocation, 0, 0);
    let context = super::compilation_context(
        &invocation,
        directory.path().canonicalize().unwrap(),
        1,
        0,
        &session,
    );
    let loaded = super::evaluated(
        session,
        &invocation.evals,
        Shuffle::None,
        context,
        "",
        &std::collections::HashSet::new(),
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
    );
    super::record_invocation_variables(&mut session, &invocation, 0, 0);
    let context = super::compilation_context(
        &invocation,
        directory.path().canonicalize().unwrap(),
        1,
        0,
        &session,
    );
    let loaded = super::evaluated(
        session,
        &[],
        Shuffle::None,
        context,
        "",
        &std::collections::HashSet::new(),
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
