#![cfg(test)]

use super::{parse, Action, ArgumentShape, Invocation, Shuffle, MAKE_OPTION_SURFACE};
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
    let session = super::session_for(&invocation, &makefile, 1, invoked_as);
    let loaded = super::evaluated(session, &invocation.evals, Shuffle::None, "")
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

// [spec:ronin:req:make.interface-compatibility/test]
#[test]
fn unknown_make_option_is_refused() {
    let diagnostic = refused(&["make", "--not-a-make-option"]).expect("an unknown option");
    assert!(
        diagnostic.starts_with("ronin: unrecognized option '--not-a-make-option'"),
        "{diagnostic}"
    );
}
