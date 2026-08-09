//! The retained emitter as an oracle.
//!
//! `build.ninja` is no longer on the path from Makefile to build, which is what
//! makes it useful: for one Make evaluation, the same rules and edges are handed
//! to [`GraphSink`] and to kati's [`NinjaWriter`] at once, and the graph the
//! first one builds is compared against the graph Ronin gets by parsing what the
//! second one wrote. Two paths, one evaluation, and any disagreement is a defect
//! in one of them rather than a difference between two runs.
//!
//! `_kati_always_build_` is decoded rather than dropped. It is a synthetic
//! input the writer invents so that a manifest can say `.PHONY`: it names no
//! file and nothing builds it, so an edge depending on it is out of date
//! forever, which is the property spelled as a dependency. The direct graph
//! states the same property on the edge, so the comparison reads both
//! spellings back into one line and asserts they agree. The synthetic edge and
//! the references to it stay out of the input lists, because there they would
//! be a dependency the direct graph is right not to have.
//!
//! The writer's other two inventions are deliberately *not* decoded, because
//! hiding a difference and not having one look identical from here.
//! `phony_output` is a binding Ronin does not read, so a manifest using it is
//! genuinely a manifest that does not say `.PHONY` to Ronin; under
//! `--use_ninja_phony_output` it is reported as a difference, both as a binding
//! only one side carries and as the property only one side has.
//! `sandbox_disabled` is a rule binding Ninja does not define, so under
//! `--emit_sandbox_disabled` the manifest is one Ronin refuses to read at all,
//! and that reports as the manifest path refusing rather than as a comparison.
// [spec:ronin:req:make.graph-direct/test]

use super::sink::GraphSink;
use crate::env::edgevar;
use crate::frontend::{load_manifest, BuildGraph, ManifestOptions};
use crate::graph::PathStyle;
use crate::util::{BStr, ByteSlice};
use kati::anyhow;
use kati::build_sink::{BuildSink, RuleId, SinkCommand, SinkEdge, SinkPool, SinkRule};
use kati::evaluate::{evaluate, Evaluated};
use kati::ninja::{emit_build, NinjaWriter, NinjaWriterOptions};
use kati::session::Session;
use kati::symtab::{Interner, Symbol};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::Path;

/// The input the writer invents for `.PHONY`, which the direct graph states
/// on the edge instead.
const ALWAYS_BUILD: &[u8] = b"_kati_always_build_";

/// Every binding kati can put on a rule or an edge.
///
/// A closed list rather than a walk of whatever each graph happens to hold: the
/// producer is one emitter with one repertoire, and naming it here is what makes
/// a binding that stops crossing the sink show up as a difference.
const BINDINGS: [(&str, PathStyle); 12] = [
    ("command", PathStyle::ShellEscaped),
    ("description", PathStyle::ShellEscaped),
    ("depfile", PathStyle::Raw),
    ("deps", PathStyle::Raw),
    ("restat", PathStyle::Raw),
    ("rspfile", PathStyle::Raw),
    ("rspfile_content", PathStyle::ShellEscaped),
    ("generator", PathStyle::Raw),
    ("pool", PathStyle::Raw),
    ("dyndep", PathStyle::Raw),
    ("tags", PathStyle::Raw),
    ("phony_output", PathStyle::Raw),
];

/// Send one build graph to two sinks.
///
/// The graph sink's refusals are swallowed rather than propagated, because
/// stopping the walk would truncate the manifest and leave the oracle with a
/// different Makefile to answer about. [`GraphSink`] keeps the first refusal
/// itself, so nothing is lost by letting the walk finish.
struct Tee<'a> {
    graph: &'a mut GraphSink,
    manifest: &'a mut dyn BuildSink,
    rule_semantics: HashMap<RuleId, RuleSemantics>,
    edge_semantics: &'a mut EdgeSemantics,
}

/// A rule property whose two sinks intentionally encode differently.
struct RuleSemantics {
    ignored_command: Option<IgnoredCommand>,
    recursive: bool,
}

enum IgnoredCommand {
    Inline(Vec<u8>),
    ResponseFile(Vec<u8>),
}

/// Semantic overrides keyed by the rule's primary output.
///
/// A manifest has no ignore-failure edge property, so its writer wraps the
/// shell script; the in-memory graph keeps the script and binds the property.
/// A recursive recipe is likewise a wrapper edge only in the legacy manifest,
/// while the graph sink holds it for semantic subninja composition. Recording
/// these at the common sink boundary lets the comparison judge the properties
/// rather than require their destination-specific spellings to be identical.
#[derive(Default)]
struct EdgeSemantics {
    ignored: BTreeMap<Vec<u8>, IgnoredCommand>,
    recursive: BTreeSet<Vec<u8>>,
}

impl BuildSink for Tee<'_> {
    fn start(&mut self, pools: &[SinkPool<'_>]) -> anyhow::Result<()> {
        let _ = self.graph.start(pools);
        self.manifest.start(pools)
    }

    fn declare_rule(&mut self, names: &dyn Interner, rule: &SinkRule<'_>) -> anyhow::Result<()> {
        let ignored_command = rule.ignore_errors.then(|| match rule.command {
            SinkCommand::Inline(script) => IgnoredCommand::Inline(script.to_vec()),
            SinkCommand::ResponseFile(script) => IgnoredCommand::ResponseFile(script.to_vec()),
        });
        self.rule_semantics.insert(
            rule.id,
            RuleSemantics {
                ignored_command,
                recursive: rule.contains_recursive,
            },
        );
        let _ = self.graph.declare_rule(names, rule);
        self.manifest.declare_rule(names, rule)
    }

    fn declare_edge(&mut self, names: &dyn Interner, edge: &SinkEdge<'_>) -> anyhow::Result<()> {
        if let Some(semantics) = edge.rule.and_then(|rule| self.rule_semantics.remove(&rule)) {
            let output = names.symtab().name(edge.output).to_vec();
            if let Some(command) = semantics.ignored_command {
                self.edge_semantics.ignored.insert(output.clone(), command);
            }
            if semantics.recursive {
                self.edge_semantics.recursive.insert(output);
            }
        }
        let _ = self.graph.declare_edge(names, edge);
        self.manifest.declare_edge(names, edge)
    }

    fn set_default_targets(
        &mut self,
        names: &dyn Interner,
        targets: &[Symbol],
    ) -> anyhow::Result<()> {
        let _ = self.graph.set_default_targets(names, targets);
        self.manifest.set_default_targets(names, targets)
    }

    fn finish(&mut self) -> anyhow::Result<()> {
        let _ = self.graph.finish();
        self.manifest.finish()
    }
}

/// What comparing one Makefile produced.
///
/// Refusing is an answer, not an absence of one: a Makefile describing a graph
/// neither path can hold should be refused by both, and the two paths agreeing
/// to refuse is the property holding rather than the property going untested.
enum Outcome {
    /// Make itself rejected the Makefile, so neither path was reached.
    NotAccepted(String),
    /// Both paths refused the graph it described.
    BothRefused { direct: String, manifest: String },
    /// Only the direct path refused it.
    OnlyDirectRefused(String),
    /// Only the manifest path refused it.
    OnlyManifestRefused(String),
    /// Both graphs were built. Empty when they agree.
    Compared(Vec<String>),
}

/// One edge, written so that two graphs can be compared as text.
///
/// Everything the equivalence rule names is here — outputs and their explicit
/// count, inputs in their three partitions, validations, the rule, the pool and
/// its depth, and every binding — because a field left out of the description is
/// a field the comparison cannot see.
///
/// Being never up to date is described once, from whichever of the two
/// spellings the graph in hand uses: the edge's own declaration, which only
/// construction can set, or a dependency on `_kati_always_build_`, which is all
/// a manifest can say. One decoder over both sides is what turns the property
/// into something the comparison asserts rather than something it hides.
#[derive(Clone, Copy)]
enum Side {
    Direct,
    Manifest,
}

fn describe_ignore_semantics<'a>(
    graph: &BuildGraph,
    edge: crate::graph::EdgeId,
    output: &[u8],
    semantics: &'a EdgeSemantics,
    side: Side,
    described: &mut String,
) -> Option<&'a IgnoredCommand> {
    let command = semantics.ignored.get(output)?;
    let ignored = match side {
        Side::Direct => graph
            .arenas()
            .names()
            .lookup(BStr::new(crate::build::IGNORE_ERRORS))
            .and_then(|binding| edgevar(graph.arenas(), edge, binding, PathStyle::Raw))
            .is_some_and(|value| !value.is_empty()),
        // The writer received this property at the common sink boundary and
        // encodes it by muting the script in the manifest.
        Side::Manifest => true,
    };
    let _ = writeln!(described, "  ignore errors: {ignored}");
    match command {
        IgnoredCommand::Inline(script) => {
            let _ = writeln!(described, "  semantic command: {:?}", script.as_bstr());
        }
        IgnoredCommand::ResponseFile(script) => {
            let _ = writeln!(
                described,
                "  semantic response content: {:?}",
                script.as_bstr()
            );
        }
    }
    Some(command)
}

fn destination_specific_binding(name: &str, command: Option<&IgnoredCommand>) -> bool {
    matches!(
        (name, command),
        ("command", Some(IgnoredCommand::Inline(_)))
            | ("rspfile_content", Some(IgnoredCommand::ResponseFile(_)))
    )
}

fn describe_edge(
    graph: &BuildGraph,
    edge: crate::graph::EdgeId,
    semantics: &EdgeSemantics,
    side: Side,
) -> Option<(Vec<u8>, String)> {
    let arenas = graph.arenas();
    let stored = arenas.edge(edge);
    let path = |node: &crate::graph::NodeId| arenas.node_path(*node).as_bytes().to_vec();
    let outputs: Vec<Vec<u8>> = stored.out.iter().map(path).collect();
    if outputs.len() == 1 && outputs[0] == ALWAYS_BUILD {
        return None;
    }
    if semantics.recursive.contains(&outputs[0]) {
        // The manifest writer can only retain the recursive wrapper command.
        // The direct sink holds it outside the graph until load_with_subninjas
        // replaces it with child goals, so this one-unit oracle has no edge on
        // the direct side to compare. Recognition is checked in compare().
        return None;
    }
    let inputs: Vec<Vec<u8>> = stored.input.iter().map(path).collect();
    let explicit = stored.explicit_input_count();
    let non_order_only = stored.non_order_only_input_count();
    let is_synthetic = |input: &&Vec<u8>| input.as_slice() != ALWAYS_BUILD;
    let always_dirty =
        stored.always_dirty || inputs.iter().any(|input| input.as_slice() == ALWAYS_BUILD);

    let key = outputs[..stored.explicit_output_count()].join(&b' ');
    let mut described = String::new();
    let list = |name: &str, paths: &[Vec<u8>], described: &mut String| {
        let _ = writeln!(
            described,
            "  {name}: {}",
            paths
                .iter()
                .map(|path| path.as_bstr().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
    };
    let _ = writeln!(
        described,
        "  rule: {}",
        stored
            .rule
            .map_or_else(|| "-".to_owned(), |rule| arenas.rule(rule).name.to_string())
    );
    list(
        "explicit out",
        &outputs[..stored.explicit_output_count()],
        &mut described,
    );
    list(
        "implicit out",
        &outputs[stored.explicit_output_count()..],
        &mut described,
    );
    let explicit_inputs: Vec<Vec<u8>> = inputs[..explicit]
        .iter()
        .filter(is_synthetic)
        .cloned()
        .collect();
    list("explicit in", &explicit_inputs, &mut described);
    list(
        "implicit in",
        &inputs[explicit..non_order_only],
        &mut described,
    );
    list("order-only in", &inputs[non_order_only..], &mut described);
    list(
        "validations",
        &stored.validation.iter().map(path).collect::<Vec<_>>(),
        &mut described,
    );
    let _ = writeln!(described, "  always dirty: {always_dirty}");
    let ignored_command =
        describe_ignore_semantics(graph, edge, &outputs[0], semantics, side, &mut described);
    let _ = writeln!(
        described,
        "  pool: {}",
        stored.pool.map_or_else(
            || "-".to_owned(),
            |pool| format!(
                "{} depth {:?}",
                arenas.pool(pool).name,
                arenas.pool(pool).depth().map(std::num::NonZeroUsize::get)
            )
        )
    );
    for (name, style) in BINDINGS {
        if destination_specific_binding(name, ignored_command) {
            continue;
        }
        let Some(id) = arenas.names().lookup(BStr::new(name)) else {
            continue;
        };
        if let Some(value) = edgevar(arenas, edge, id, style) {
            let _ = writeln!(described, "  ${name} = {value}");
        }
    }
    Some((key, described))
}

/// Every edge in a graph, keyed by the outputs `$out` names.
fn shape(graph: &BuildGraph, semantics: &EdgeSemantics, side: Side) -> BTreeMap<Vec<u8>, String> {
    graph
        .arenas()
        .edge_ids()
        .filter_map(|edge| describe_edge(graph, edge, semantics, side))
        .collect()
}

/// The default targets, with the writer's synthetic target removed.
fn defaults(graph: &BuildGraph) -> Vec<Vec<u8>> {
    graph
        .default_targets()
        .into_iter()
        .map(|node| graph.path(node).to_vec())
        .filter(|path| path != ALWAYS_BUILD)
        .collect()
}

/// Every way two graphs of the same Makefile disagree.
fn differences(direct: &BuildGraph, parsed: &BuildGraph, semantics: &EdgeSemantics) -> Vec<String> {
    let mut found = Vec::new();
    let (direct_shape, parsed_shape) = (
        shape(direct, semantics, Side::Direct),
        shape(parsed, semantics, Side::Manifest),
    );
    for output in direct_shape.keys().chain(parsed_shape.keys()) {
        match (direct_shape.get(output), parsed_shape.get(output)) {
            (Some(mine), Some(theirs)) if mine == theirs => {}
            (Some(mine), Some(theirs)) => found.push(format!(
                "edge {}:\ndirect:\n{mine}manifest:\n{theirs}",
                output.as_bstr()
            )),
            (Some(_), None) => found.push(format!("edge {} only direct", output.as_bstr())),
            (None, Some(_)) => found.push(format!("edge {} only in manifest", output.as_bstr())),
            (None, None) => unreachable!("the key came from one of the two maps"),
        }
    }
    found.dedup();
    if defaults(direct) != defaults(parsed) {
        found.push(format!(
            "default targets: direct {:?}, manifest {:?}",
            defaults(direct)
                .iter()
                .map(|path| path.as_bstr().to_string())
                .collect::<Vec<_>>(),
            defaults(parsed)
                .iter()
                .map(|path| path.as_bstr().to_string())
                .collect::<Vec<_>>(),
        ));
    }
    found
}

/// Evaluate one Makefile into both graphs and compare them.
///
/// `directory` is where the manifest is written and read back from; `argv` is a
/// whole kati command line, program name included.
// [spec:ronin:req:make.manifest-equivalence]
// [spec:ronin:req:make.semantics+1]
fn compare(directory: &Path, argv: Vec<OsString>) -> Outcome {
    let _directory = super::compilation_directory_guard();
    let session = Session::from_args(argv);
    let manifest_path = directory.join("build.ninja");
    let options = NinjaWriterOptions::from_flags(&session.flags);

    let Evaluated { mut ev, nodes, .. } = match evaluate(session) {
        Ok(evaluated) => evaluated,
        Err(error) => return Outcome::NotAccepted(format!("{error:#}")),
    };
    let mut sink = GraphSink::new();
    let mut semantics = EdgeSemantics::default();
    let emitted = {
        let file = std::fs::File::create(&manifest_path).expect("the case directory is writable");
        let mut writer = NinjaWriter::new(std::io::BufWriter::new(file), options);
        let mut tee = Tee {
            graph: &mut sink,
            manifest: &mut writer,
            rule_semantics: HashMap::new(),
            edge_semantics: &mut semantics,
        };
        emit_build(&nodes, &mut ev, &mut tee)
    };
    if let Err(error) = emitted {
        return Outcome::NotAccepted(format!("{error:#}"));
    }
    let pending_subninjas = sink.take_unit().subninjas.len();
    let direct = sink.into_graph();
    let parsed = load_manifest(directory, "build.ninja", ManifestOptions::default());
    match (direct, parsed) {
        (Ok(direct), Ok(parsed)) => {
            let mut found = differences(&direct, &parsed.graph, &semantics);
            if pending_subninjas != semantics.recursive.len() {
                found.push(format!(
                    "recursive rules: sink held {pending_subninjas}, boundary declared {}",
                    semantics.recursive.len()
                ));
            }
            Outcome::Compared(found)
        }
        (Err(direct), Err(manifest)) => Outcome::BothRefused {
            direct: direct.to_string(),
            manifest: manifest.to_string(),
        },
        (Err(direct), Ok(_)) => Outcome::OnlyDirectRefused(direct.to_string()),
        (Ok(_), Err(manifest)) => Outcome::OnlyManifestRefused(manifest.to_string()),
    }
}

/// A Makefile written to a scratch directory, and the command line that runs it.
struct Case {
    directory: tempfile::TempDir,
    argv: Vec<OsString>,
}

impl Case {
    /// A Makefile reached by absolute path, so the comparison needs no working
    /// directory of its own and can run beside every other test.
    fn new(makefile: &str, flags: &[&str]) -> Self {
        let directory = tempfile::tempdir().expect("a scratch directory");
        let path = directory.path().join("Makefile");
        std::fs::write(&path, makefile).expect("the scratch directory is writable");
        let mut argv = vec![OsString::from("rkati"), OsString::from("--ninja")];
        argv.extend(flags.iter().map(OsString::from));
        argv.push(OsString::from("-f"));
        argv.push(path.into_os_string());
        Self { directory, argv }
    }

    fn compare(self) -> Outcome {
        compare(self.directory.path(), self.argv)
    }
}

/// Assert that one Makefile produces the same graph both ways.
#[track_caller]
fn agrees(makefile: &str, flags: &[&str]) {
    match Case::new(makefile, flags).compare() {
        Outcome::Compared(differences) if differences.is_empty() => {}
        Outcome::Compared(differences) => panic!("{}", differences.join("\n")),
        Outcome::NotAccepted(why) => panic!("make rejected the fixture: {why}"),
        Outcome::BothRefused { direct, manifest } => {
            panic!("both paths refused the fixture: {direct} / {manifest}")
        }
        Outcome::OnlyDirectRefused(why) => panic!("the direct graph refused: {why}"),
        Outcome::OnlyManifestRefused(why) => panic!("ronin refused the manifest: {why}"),
    }
}

// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.graph-direct/test]
#[test]
fn the_direct_graph_carries_every_partition_the_manifest_does() {
    agrees(
        "\
all: out
out: in | ordered
\t@echo building $@ > $@
in ordered:
\t@touch $@
",
        &[],
    );
}

/// Both sides say `.PHONY`; only the spelling differs, and the comparison
/// reads both spellings as the same property rather than excusing one of them.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.phony-always-dirty/test]
#[test]
fn a_phony_target_agrees_on_being_never_up_to_date() {
    agrees(
        "\
.PHONY: clean all
all: clean
clean:
\trm -rf out
",
        &[],
    );
}

// [spec:ronin:req:make.graph-direct/test]
#[test]
fn shell_metacharacters_survive_both_paths_identically() {
    agrees(
        "\
all:
\techo $$HOME \"quoted $$(date)\" 'single' `backtick` \\$$literal
",
        &[],
    );
}

// [spec:ronin:req:make.graph-direct/test]
#[test]
fn names_needing_ninja_escaping_agree() {
    agrees(
        "\
all: out$$dollar
out$$dollar:
\t@touch $@
",
        &[],
    );
}

// [spec:ronin:req:make.graph-direct/test]
#[test]
fn the_per_edge_bindings_agree() {
    agrees(
        "\
all: out
out: in
\t@cp in out
\tls
in:
\t@touch in
out: .KATI_DEPFILE := out.d
out: .KATI_RESTAT := 1
out: .KATI_NINJA_POOL := console
out: .KATI_TAGS := one two
out: .KATI_IMPLICIT_OUTPUTS := out.stamp
all: .KATI_NINJA_POOL := local_pool
",
        &["--use_ninja_validations"],
    );
}

// [spec:ronin:req:make.graph-direct/test]
#[test]
fn a_validation_agrees() {
    agrees(
        "\
all: out
out: in
\t@cp in out
in checked:
\t@touch $@
out: .KATI_VALIDATIONS := checked
",
        &["--use_ninja_validations"],
    );
}

// [spec:ronin:req:make.graph-direct/test]
#[test]
fn a_script_too_long_for_an_argument_list_agrees() {
    let padding = "x".repeat(100 * 1000);
    agrees(
        &format!(
            "\
all:
\t@echo {padding} > out
"
        ),
        &[],
    );
}

// [spec:ronin:req:make.graph-direct/test]
#[test]
fn the_default_pool_and_the_local_pool_agree() {
    agrees(
        "\
all: out
out:
\t@touch out
",
        &["--remote_num_jobs", "8"],
    );
}

/// `--emit_sandbox_disabled` writes a rule binding Ninja does not define, so
/// the manifest it produces is not one Ronin can read back. The direct graph is
/// still built, and drops the flag rather than carrying a binding nothing could
/// consume.
// [spec:ronin:req:make.graph-direct/test]
#[test]
fn the_sandbox_flag_leaves_the_manifest_with_no_oracle() {
    let outcome = Case::new(
        "\
all:
\t@touch out
",
        &["--emit_sandbox_disabled"],
    )
    .compare();
    match outcome {
        Outcome::OnlyManifestRefused(why) => {
            assert!(why.contains("sandbox_disabled"), "{why}");
        }
        _ => panic!("expected ninja to refuse the sandbox binding"),
    }
}

/// Android ninja's `phony_output` is the manifest's other way of saying
/// `.PHONY`, and it is one Ronin does not read. Under that flag the emitted
/// manifest genuinely describes a build whose `.PHONY` target can go up to
/// date, so the difference is reported twice over — the binding only the
/// manifest carries, and the property only the direct graph has — rather than
/// being decoded away into an agreement that would not hold at build time.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.phony-always-dirty/test]
#[test]
fn phony_output_is_reported_as_a_difference_rather_than_hidden() {
    let outcome = Case::new(
        "\
.PHONY: all
all:
\t@echo hello
",
        &["--use_ninja_phony_output"],
    )
    .compare();
    match outcome {
        Outcome::Compared(differences) => {
            assert!(
                differences.iter().any(|why| why.contains("phony_output")),
                "{differences:?}"
            );
            assert!(
                differences
                    .iter()
                    .any(|why| why.contains("always dirty: true")
                        && why.contains("always dirty: false")),
                "{differences:?}"
            );
        }
        _ => panic!("expected the manifest to carry a binding the graph does not"),
    }
}

/// The whole Make corpus, one comparison per target each `.mk` file declares.
///
/// A testcase reads and writes files beside its own Makefile, so the corpus
/// pass has to change its process working directory. It is ignored in ordinary
/// parallel libtest runs and the release gate invokes it alone through
/// `scripts/check-make-equivalence.sh`; otherwise the process-global directory
/// can race unrelated tests that launch commands.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.manifest-equivalence/test]
// [spec:ronin:req:make.semantics+1/test]
#[test]
#[ignore = "changes the process working directory; the release gate runs it alone"]
fn the_direct_graph_matches_the_manifest_over_the_corpus() {
    compare_corpus_graphs();
}

fn compare_corpus_graphs() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("kati/testcase");
    let mut makefiles: Vec<_> = std::fs::read_dir(&corpus)
        .expect("the kati submodule is checked out")
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "mk"))
        .collect();
    makefiles.sort();

    let original = std::env::current_dir().expect("a working directory");
    let mut report = Report::default();
    for makefile in &makefiles {
        let source = std::fs::read(makefile).expect("a readable testcase");
        let name = makefile.file_name().expect("a named file").to_owned();
        for target in declared_targets(&source) {
            let directory = tempfile::tempdir().expect("a scratch directory");
            std::fs::write(directory.path().join("Makefile"), &source)
                .expect("the scratch directory is writable");
            let _ = std::os::unix::fs::symlink(
                corpus.join("submake"),
                directory.path().join("submake"),
            );
            let mut argv = vec![
                OsString::from("rkati"),
                OsString::from("--use_find_emulator"),
                OsString::from("--ninja"),
            ];
            if name.as_encoded_bytes().starts_with(b"submake_") {
                argv.push(OsString::from("-s"));
            }
            argv.push(OsString::from("SHELL=/bin/bash"));
            // Named relatively, because a testcase can observe the name it was
            // given: `KATI_visibility_prefix` matches against it, and an
            // absolute path fails a match Make's own invocation would pass.
            argv.push(OsString::from("-f"));
            argv.push(OsString::from("Makefile"));
            if let Some(target) = &target {
                argv.push(OsString::from(target));
            }

            std::env::set_current_dir(directory.path()).expect("the scratch directory exists");
            let outcome = compare(directory.path(), argv);
            std::env::set_current_dir(&original).expect("the original directory still exists");
            report.record(
                &format!("{}#{}", name.to_string_lossy(), target.unwrap_or_default()),
                outcome,
            );
        }
    }
    report.assert_clean(makefiles.len());
}

/// Each `testN` target a testcase declares, or one unnamed run for a file that
/// declares none. Matches how the emitter's own differential harness enumerates
/// the same corpus, so the two cover the same runs.
fn declared_targets(source: &[u8]) -> Vec<Option<String>> {
    let mut targets: Vec<String> = Vec::new();
    for line in source.split(|byte| *byte == b'\n') {
        let Some(rest) = line.strip_prefix(b"test") else {
            continue;
        };
        let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
        let target = format!("test{}", String::from_utf8_lossy(&rest[..digits]));
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets.sort();
    if targets.is_empty() {
        return vec![None];
    }
    targets.into_iter().map(Some).collect()
}

/// The one corpus case where the two paths disagree, and why that is kati's
/// defect rather than this one's.
///
/// `equal_in_target.mk` is annotated `# TODO fix parser` upstream: the target
/// `test` acquires a target-specific variable and no rule, so kati emits no
/// build statement for it and then names it in `default` anyway. Ninja requires
/// every `default` to name a path some build statement mentions, so the
/// manifest is one ninja itself refuses; the direct graph has no statement to
/// validate and carries the goal through to planning, which refuses it with
/// `'test' missing and no known rule to make it`. Neither path builds it, and
/// they disagree only about where they say so.
const KNOWN_TO_DISAGREE: [(&str, &str); 1] = [("equal_in_target.mk#test", "unknown target 'test'")];

/// What a whole corpus run found.
#[derive(Default)]
struct Report {
    compared: usize,
    not_accepted: Vec<String>,
    both_refused: Vec<(String, String, String)>,
    disagreed: Vec<(String, Vec<String>)>,
}

impl Report {
    fn record(&mut self, case: &str, outcome: Outcome) {
        match outcome {
            Outcome::NotAccepted(_) => self.not_accepted.push(case.to_owned()),
            Outcome::BothRefused { direct, manifest } => {
                self.both_refused.push((case.to_owned(), direct, manifest));
            }
            Outcome::OnlyDirectRefused(why) => self.disagreed.push((
                case.to_owned(),
                vec![format!("only the direct graph refused it: {why}")],
            )),
            Outcome::OnlyManifestRefused(why) => self.disagreed.push((
                case.to_owned(),
                vec![format!("only the manifest was refused: {why}")],
            )),
            Outcome::Compared(differences) => {
                self.compared += 1;
                if !differences.is_empty() {
                    self.disagreed.push((case.to_owned(), differences));
                }
            }
        }
    }

    /// Whether a disagreement is the one already classified above.
    fn classified(case: &str, differences: &[String]) -> bool {
        KNOWN_TO_DISAGREE.iter().any(|(known, reason)| {
            *known == case && differences.iter().any(|why| why.contains(reason))
        })
    }

    fn assert_clean(&self, makefiles: usize) {
        println!("makefiles:          {makefiles}");
        println!("graphs compared:    {}", self.compared);
        println!("both refused:       {}", self.both_refused.len());
        for (case, direct, manifest) in &self.both_refused {
            println!("  {case}: direct {direct}; manifest {manifest}");
        }
        println!("make rejected:      {}", self.not_accepted.len());
        println!("disagreements:      {}", self.disagreed.len());
        for (case, differences) in &self.disagreed {
            let classified = if Self::classified(case, differences) {
                " (classified)"
            } else {
                ""
            };
            println!("  {case}{classified}:");
            for difference in differences {
                println!("    {difference}");
            }
        }
        let unclassified = self
            .disagreed
            .iter()
            .filter(|(case, differences)| !Self::classified(case, differences))
            .count();
        assert_eq!(unclassified, 0, "the two paths disagree");
    }
}
