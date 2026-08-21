//! What a Ninja manifest says that parsing it accepts and building it would
//! never question.
//!
//! The parser refuses a great deal outright — a duplicate output, an unknown
//! rule, an unexpected rule variable — and what it refuses is already
//! reported, as the refusal it is, by the read that failed. What is left is
//! the manifest that parses and still does not say what its author meant: a
//! binding nothing expands, a phony statement carrying a command that can
//! never run, a cycle in a corner of the graph no target asked for.
//!
//! Nothing here changes what the parser accepts. A manifest that lints with
//! findings builds exactly as it did before, and that is the point: a lint
//! that tightened the language would be a second, stricter Ninja rather than a
//! report about this one.
// [spec:ronin:req:tools.manifest-lint]

use super::{Finding, Report};
use crate::frontend::BuildGraph;
use crate::graph::Graph;
use crate::names::{Names, VarId};
use crate::util::{ByteSlice, EvalPart};
use std::collections::HashSet;

/// Report everything a parsed manifest has to answer for.
///
/// In one order every time — the rules, then the build statements in the
/// order they were read, then the cycles — so two runs over one manifest
/// print the same report.
pub(super) fn check(graph: &BuildGraph, report: &mut Report) {
    let arenas = graph.arenas();
    shadowed_phony(arenas, report);
    unread_bindings(arenas, report);
    for cycle in crate::graph::dependency_cycles(arenas) {
        report.raise(&Finding::error(cycle.to_string()));
    }
}

/// A rule of one's own called `phony`.
///
/// Ninja dispatches the built-in by identity rather than by name, so a rule
/// named `phony` in a `subninja` scope is an ordinary rule that runs its
/// command — which is the opposite of what its name tells every reader of the
/// statements that use it.
fn shadowed_phony(graph: &Graph, report: &mut Report) {
    for rule in graph.rule_ids() {
        if graph.rule(rule).name.as_bytes() == b"phony" && !graph.is_phony_rule(Some(rule)) {
            report.raise(&Finding::warning(
                "a rule of its own named `phony` shadows the built-in one: a build statement \
                 using it runs its command, where the name says it runs nothing",
            ));
        }
    }
}

/// Bindings on a build statement that nothing will ever expand.
///
/// A build statement's binding is reached in exactly two ways: the engine
/// reads it under one of the names Ninja reserves, or a template belonging to
/// the statement's own rule names it. A binding reached neither way expands
/// nowhere, and the build runs precisely as it would with the line deleted —
/// which is almost always a misspelling of a name that would have been read.
fn unread_bindings(graph: &Graph, report: &mut Report) {
    for edge_id in graph.edge_ids() {
        let edge = graph.edge(edge_id);
        if edge.bindings.iter().next().is_none() {
            continue;
        }
        let phony = graph.is_phony_rule(edge.rule);
        let read = edge.rule.map(|rule| expanded_names(graph, rule));
        let output = edge.out.first().map_or_else(String::new, |node| {
            graph.node_path(*node).to_str_lossy().into_owned()
        });
        for (name, _) in edge.bindings.iter() {
            if Names::is_reserved(name) && !phony {
                continue;
            }
            if read.as_ref().is_some_and(|read| read.contains(&name)) {
                continue;
            }
            let name = graph.names().name(name);
            report.raise(&Finding::warning(if phony {
                format!(
                    "build `{output}`: the binding `{name}` runs nothing, because a phony \
                     statement runs nothing"
                )
            } else {
                format!(
                    "build `{output}`: the binding `{name}` is expanded by nothing its rule \
                     writes, so the build runs as it would without the line"
                )
            }));
        }
    }
}

/// Every name the templates of one rule expand.
fn expanded_names(graph: &Graph, rule: crate::env::RuleId) -> HashSet<VarId> {
    let mut names = HashSet::new();
    for (_, template) in graph.rule(rule).bindings.iter() {
        for part in &template.parts {
            if let EvalPart::Variable(name) = part {
                names.insert(*name);
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::super::Report;
    use crate::scratch_directory::Scratch;
    use std::fs;

    /// What the checks say about one manifest, as the lines a reader sees.
    fn checked(source: &str) -> String {
        let directory = Scratch::named("ronin-manifest-lint-");
        let path = directory.join("build.ninja");
        fs::write(&path, source).unwrap();
        let graph = crate::parse::load_manifest_reporting(
            &path,
            crate::os::WorkingDirectory::default(),
            crate::frontend::ManifestOptions::default(),
            &mut Vec::new(),
        )
        .expect("the source parses");
        let mut report = Report::default();
        super::check(&graph, &mut report);
        String::from_utf8(report.finish("done").stdout).expect("findings are text")
    }

    /// A binding the engine reads under a reserved name, and one a rule
    /// template expands, are both read. Ninja refuses a rule binding under any
    /// other name, so those templates are the whole of what a build statement
    /// can be read by, and a binding no template names is read by nothing.
    // [spec:ronin:req:tools.manifest-lint/test]
    #[test]
    fn only_an_unexpanded_binding_is_reported() {
        assert_eq!(
            checked(
                "rule cc\n  command = gcc $cflags -c $in -o $out\n\
                 build a.o: cc a.c\n  cflags = -O2\n"
            ),
            "ronin: done\n"
        );
        assert!(
            checked(
                "rule cc\n  command = gcc $cflags -c $in -o $out\n\
                 build a.o: cc a.c\n  cflag = -O2\n"
            )
            .contains("the binding `cflag` is expanded by nothing"),
        );
    }

    /// A reserved name on a build statement is read by the engine whatever the
    /// rule's templates say — except on a phony statement, which runs nothing
    /// for any of them to govern.
    // [spec:ronin:req:tools.manifest-lint/test]
    #[test]
    fn a_reserved_name_is_read() {
        assert_eq!(
            checked(
                "pool slow\n  depth = 1\n\
                 rule cc\n  command = gcc -c $in -o $out\n\
                 build a.o: cc a.c\n  pool = slow\n  description = CC\n"
            ),
            "ronin: done\n"
        );
        assert!(
            checked("build all: phony a.c\n  description = ALL\n")
                .contains("the binding `description` runs nothing"),
        );
    }

    /// A cycle the requested targets never reach, which is the one a build
    /// cannot find because a build walks only what it was asked for.
    // [spec:ronin:req:tools.manifest-lint/test]
    #[test]
    fn an_unreached_cycle_is_reported() {
        assert_eq!(
            checked(
                "rule cc\n  command = gcc $in -o $out\n\
                 build a: cc b\nbuild b: cc a\nbuild z: cc y\ndefault z\n"
            ),
            "ronin: dependency cycle: a -> b -> a\nronin: done\n"
        );
    }
}
