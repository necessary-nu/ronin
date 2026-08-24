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
use crate::frontend::{BuildGraph, ManifestOptions, load_manifest};
use crate::graph::PathStyle;
use crate::util::{BStr, ByteSlice};
use kati::anyhow;
use kati::build_sink::{BuildSink, RuleId, SinkCommand, SinkEdge, SinkPool, SinkRule};
use kati::evaluate::{Evaluated, evaluate};
use kati::ninja::{NinjaWriter, NinjaWriterOptions, emit_build};
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
    semantic_command: Option<SemanticCommand>,
    recursive: bool,
}

/// One script as it crossed the sink, kept because the two destinations write
/// it down differently.
struct SemanticCommand {
    /// Whether the writer passes it as an argument or through a file, which is
    /// which of the two bindings carries it.
    response_file: bool,
    /// The manifest has no edge property for an ignored failure and encodes it
    /// by muting the script; the graph binds it and leaves the script alone.
    ignore_errors: bool,
    /// The script holds a newline, so the manifest cannot write it down at
    /// all: a binding ends at one and no escape puts it back. The writer drops
    /// each continuation's backslash and newline, and Ronin's own sink is
    /// handed the recipe's launches instead of a script it would have to
    /// spell. The two paths therefore do not agree about the bytes here, and
    /// cannot, so the comparison judges the script that reached them both.
    respelled: bool,
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
    semantic: BTreeMap<Vec<u8>, SemanticCommand>,
    recursive: BTreeSet<Vec<u8>>,
    deferred: BTreeMap<Vec<u8>, DeferredSemantics>,
}

struct DeferredSemantics {
    outputs: Vec<Vec<u8>>,
    always_dirty_output: bool,
    dates_do_not_decide: bool,
    always_new_inputs: Vec<Vec<u8>>,
    excluded_new_inputs: Vec<Vec<u8>>,
    new_input_names: Vec<Vec<u8>>,
    completion_join: bool,
}

impl BuildSink for Tee<'_> {
    fn start(&mut self, pools: &[SinkPool<'_>]) -> anyhow::Result<()> {
        let _ = self.graph.start(pools);
        self.manifest.start(pools)
    }

    fn declare_rule(&mut self, names: &dyn Interner, rule: &SinkRule<'_>) -> anyhow::Result<()> {
        let (script, response_file) = match rule.command {
            SinkCommand::Inline(script) => (script, false),
            SinkCommand::ResponseFile(script) => (script, true),
        };
        let respelled = script.contains(&b'\n');
        let semantic_command = (rule.ignore_errors || respelled).then_some(SemanticCommand {
            response_file,
            ignore_errors: rule.ignore_errors,
            respelled,
        });
        self.rule_semantics.insert(
            rule.id,
            RuleSemantics {
                semantic_command,
                recursive: !rule.subninjas.is_empty(),
            },
        );
        let _ = self.graph.declare_rule(names, rule);
        self.manifest.declare_rule(names, rule)
    }

    fn declare_edge(&mut self, names: &dyn Interner, edge: &SinkEdge<'_>) -> anyhow::Result<()> {
        let output = names.symtab().name(edge.output).to_vec();
        if let Some(semantics) = edge.rule.and_then(|rule| self.rule_semantics.remove(&rule)) {
            if let Some(command) = semantics.semantic_command {
                self.edge_semantics.semantic.insert(output.clone(), command);
            }
            if semantics.recursive {
                self.edge_semantics.recursive.insert(output.clone());
            }
        }
        if !edge.deferred_freshness_outputs.is_empty() || edge.completion_join {
            self.edge_semantics.deferred.insert(
                output,
                DeferredSemantics {
                    outputs: edge
                        .deferred_freshness_outputs
                        .iter()
                        .map(|output| names.symtab().name(*output).to_vec())
                        .collect(),
                    always_dirty_output: edge.deferred_freshness_always_dirty,
                    dates_do_not_decide: edge.deferred_freshness_ignores_dates,
                    always_new_inputs: edge
                        .deferred_always_new_inputs
                        .iter()
                        .map(|input| names.symtab().name(*input).to_vec())
                        .collect(),
                    excluded_new_inputs: edge
                        .deferred_excluded_new_inputs
                        .iter()
                        .map(|input| names.symtab().name(*input).to_vec())
                        .collect(),
                    new_input_names: edge
                        .deferred_new_input_names
                        .iter()
                        .map(|(input, published)| {
                            let mut pair = names.symtab().name(*input).to_vec();
                            pair.push(b'=');
                            pair.extend_from_slice(&names.symtab().name(*published));
                            pair
                        })
                        .collect(),
                    completion_join: edge.completion_join,
                },
            );
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
    /// Both graphs were built. `differences` is empty when they agree, and
    /// `respelled` counts the rules whose script the manifest could not write
    /// down — the comparison judged those at the sink boundary instead, so the
    /// number is reported rather than left to be inferred from a silence.
    Compared {
        differences: Vec<String>,
        respelled: usize,
    },
}

impl Outcome {
    /// Whether both graphs were built and found to say the same thing.
    fn agreed(&self) -> bool {
        matches!(self, Self::Compared { differences, .. } if differences.is_empty())
    }
}

/// What this outcome is, said once so that every caller reports it alike.
///
/// Written as `Display` rather than as a `match` at each site because there are
/// three of them and a fourth would otherwise have reached for `Err(_)`, which
/// names no reason at all.
impl std::fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Compared { differences, .. } if differences.is_empty() => {
                formatter.write_str("the two graphs agree")
            }
            Self::Compared { differences, .. } => formatter.write_str(&differences.join("\n")),
            Self::NotAccepted(why) => write!(formatter, "make rejected the fixture: {why}"),
            Self::BothRefused { direct, manifest } => {
                write!(
                    formatter,
                    "both paths refused the fixture: {direct} / {manifest}"
                )
            }
            Self::OnlyDirectRefused(why) => write!(formatter, "the direct graph refused: {why}"),
            Self::OnlyManifestRefused(why) => {
                write!(formatter, "ronin refused the manifest: {why}")
            }
        }
    }
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

fn describe_command_semantics<'a>(
    graph: &BuildGraph,
    edge: crate::graph::EdgeId,
    output: &[u8],
    semantics: &'a EdgeSemantics,
    side: Side,
    described: &mut String,
) -> Option<&'a SemanticCommand> {
    let command = semantics.semantic.get(output)?;
    let ignored = match side {
        Side::Direct => graph
            .arenas()
            .names()
            .lookup(BStr::new(crate::build::IGNORE_ERRORS))
            .and_then(|binding| edgevar(graph.arenas(), edge, binding, PathStyle::Raw))
            .is_some_and(|value| !value.is_empty()),
        // The writer received this property at the common sink boundary and
        // encodes it by muting the script in the manifest.
        Side::Manifest => command.ignore_errors,
    };
    let _ = writeln!(described, "  ignore errors: {ignored}");
    let _ = writeln!(
        described,
        "  respelled for the manifest: {}",
        command.respelled
    );
    Some(command)
}

/// What one side says about a deferred edge, in the words both sides answer in.
///
/// A value rather than the seven-element tuple this used to destructure: the
/// two sides answer the same eight questions from quite different places, and
/// naming them is what lets each answer be read beside its own source. Note
/// that it is not `DeferredSemantics` above: that one is what the writer's
/// output was read back as, and this one is either side's answer in the shape
/// the comparison prints.
struct DeferredAnswer {
    outputs: Vec<Vec<u8>>,
    always_dirty_output: bool,
    dates_do_not_decide: bool,
    always_new_inputs: Vec<Vec<u8>>,
    excluded_new_inputs: Vec<Vec<u8>>,
    new_input_names: Vec<Vec<u8>>,
    completion_join: bool,
    variable: Vec<u8>,
}

/// The paths a graph knows these nodes by, as the bytes the comparison reads.
fn node_path_bytes(arenas: &crate::graph::Graph, nodes: &[crate::graph::NodeId]) -> Vec<Vec<u8>> {
    nodes
        .iter()
        .map(|node| arenas.node_path(*node).as_bytes().to_vec())
        .collect()
}

/// The direct graph's answer, read off the edge's own late-freshness record.
fn direct_deferred_semantics(graph: &BuildGraph, edge: crate::graph::EdgeId) -> DeferredAnswer {
    let arenas = graph.arenas();
    let freshness = arenas.deferred_freshness(edge);
    DeferredAnswer {
        outputs: freshness.map_or_else(Vec::new, |freshness| {
            node_path_bytes(arenas, &freshness.outputs)
        }),
        always_dirty_output: freshness.is_some_and(|freshness| freshness.always_dirty_output),
        dates_do_not_decide: freshness.is_some_and(|freshness| freshness.dates_do_not_decide),
        always_new_inputs: freshness.map_or_else(Vec::new, |freshness| {
            node_path_bytes(arenas, &freshness.always_new_inputs)
        }),
        excluded_new_inputs: freshness.map_or_else(Vec::new, |freshness| {
            node_path_bytes(arenas, &freshness.excluded_new_inputs)
        }),
        new_input_names: freshness.map_or_else(Vec::new, |freshness| {
            freshness
                .new_input_names
                .iter()
                .map(|(node, published)| {
                    let mut pair = arenas.node_path(*node).as_bytes().to_vec();
                    pair.push(b'=');
                    pair.extend_from_slice(published.as_bytes());
                    pair
                })
                .collect()
        }),
        completion_join: arenas.is_completion_join(edge),
        variable: freshness
            .map_or_else(Vec::new, |freshness| freshness.new_inputs_variable.to_vec()),
    }
}

/// The manifest graph's answer, read back out of what the writer emitted.
fn manifest_deferred_semantics(recorded: Option<&DeferredSemantics>) -> DeferredAnswer {
    DeferredAnswer {
        outputs: recorded.map_or_else(Vec::new, |semantic| semantic.outputs.clone()),
        always_dirty_output: recorded.is_some_and(|semantic| semantic.always_dirty_output),
        dates_do_not_decide: recorded.is_some_and(|semantic| semantic.dates_do_not_decide),
        always_new_inputs: recorded
            .map_or_else(Vec::new, |semantic| semantic.always_new_inputs.clone()),
        excluded_new_inputs: recorded
            .map_or_else(Vec::new, |semantic| semantic.excluded_new_inputs.clone()),
        new_input_names: recorded
            .map_or_else(Vec::new, |semantic| semantic.new_input_names.clone()),
        completion_join: recorded.is_some_and(|semantic| semantic.completion_join),
        variable: recorded
            .filter(|semantic| !semantic.outputs.is_empty())
            .map_or_else(Vec::new, |_| b"KATI_NEW_INPUTS".to_vec()),
    }
}

fn describe_deferred_semantics(
    graph: &BuildGraph,
    edge: crate::graph::EdgeId,
    output: &[u8],
    semantics: &EdgeSemantics,
    side: Side,
    described: &mut String,
) {
    let answered = match side {
        Side::Direct => direct_deferred_semantics(graph, edge),
        Side::Manifest => manifest_deferred_semantics(semantics.deferred.get(output)),
    };
    let list = |paths: &[Vec<u8>]| {
        paths
            .iter()
            .map(|path| path.as_bstr().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let _ = writeln!(described, "  deferred outputs: {}", list(&answered.outputs));
    let _ = writeln!(
        described,
        "  deferred output always dirty: {}",
        answered.always_dirty_output
    );
    let _ = writeln!(
        described,
        "  deferred freshness ignores dates: {}",
        answered.dates_do_not_decide
    );
    let _ = writeln!(
        described,
        "  deferred always-new inputs: {}",
        list(&answered.always_new_inputs)
    );
    let _ = writeln!(
        described,
        "  deferred excluded new inputs: {}",
        list(&answered.excluded_new_inputs)
    );
    let _ = writeln!(
        described,
        "  deferred new-input names: {}",
        list(&answered.new_input_names)
    );
    let _ = writeln!(
        described,
        "  deferred variable: {:?}",
        answered.variable.as_bstr()
    );
    let _ = writeln!(described, "  completion join: {}", answered.completion_join);
}

fn destination_specific_binding(name: &str, command: Option<&SemanticCommand>) -> bool {
    let Some(command) = command else {
        return false;
    };
    match name {
        "command" => !command.response_file,
        "rspfile_content" => command.response_file,
        // A script the manifest had to respell cannot be its own narration
        // there: the binding holds one line, and the reconstruction that
        // carries the recipe is plumbing rather than the recipe. The sink that
        // keeps the script in memory narrates it with its own bytes, and the
        // two spellings are as destination-specific as the command is.
        "description" => command.respelled,
        _ => false,
    }
}

/// Return the Make-visible path for a graph node.
///
/// A direct grouped-action graph gives its public completion edge a private
/// proxy output so a recursively compiled child can still generate the real
/// member.  The retained manifest spells that same completion point with the
/// member path itself, so compare the proxy as the observed member.
fn semantic_path(graph: &BuildGraph, node: crate::graph::NodeId) -> Vec<u8> {
    let arenas = graph.arenas();
    let node = arenas
        .node(node)
        .generator
        .and_then(|edge| arenas.completion_join_output(edge))
        .unwrap_or(node);
    arenas.node_path(node).as_bytes().to_vec()
}

fn describe_edge(
    graph: &BuildGraph,
    edge: crate::graph::EdgeId,
    semantics: &EdgeSemantics,
    side: Side,
) -> Option<(Vec<u8>, String)> {
    let arenas = graph.arenas();
    let stored = arenas.edge(edge);
    let path = |node: &crate::graph::NodeId| semantic_path(graph, *node);
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
    describe_deferred_semantics(graph, edge, &outputs[0], semantics, side, &mut described);
    let semantic_command =
        describe_command_semantics(graph, edge, &outputs[0], semantics, side, &mut described);
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
        if destination_specific_binding(name, semantic_command) {
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
        .map(|node| {
            graph
                .completion_join_observed_output(node)
                .map_or_else(|| graph.path(node), |observed| graph.path(observed))
                .to_vec()
        })
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

/// What one Makefile evaluation produced, when it produced two graphs.
struct Both {
    direct: BuildGraph,
    parsed: BuildGraph,
    semantics: EdgeSemantics,
    /// Recursive rules the sink is still holding, which the boundary should
    /// have recognised exactly as many of.
    pending_subninjas: usize,
}

/// Evaluate one Makefile into both graphs and compare them.
///
/// `directory` is where the manifest is written and read back from; `argv` is a
/// whole kati command line, program name included.
// [spec:ronin:req:make.manifest-equivalence+1]
// [spec:ronin:req:make.semantics+1]
fn compare(directory: &Path, argv: Vec<OsString>) -> Outcome {
    match build_both(directory, argv) {
        Ok(Both {
            direct,
            parsed,
            semantics,
            pending_subninjas,
        }) => {
            let mut found = differences(&direct, &parsed, &semantics);
            if pending_subninjas != semantics.recursive.len() {
                found.push(format!(
                    "recursive rules: sink held {pending_subninjas}, boundary declared {}",
                    semantics.recursive.len()
                ));
            }
            Outcome::Compared {
                differences: found,
                respelled: semantics
                    .semantic
                    .values()
                    .filter(|command| command.respelled)
                    .count(),
            }
        }
        Err(outcome) => outcome,
    }
}

/// Send one evaluation to both sinks and read back what each produced.
///
/// Separate from the comparison so a test can ask about a property the
/// comparison deliberately does not describe, and assert which side has it.
fn build_both(directory: &Path, argv: Vec<OsString>) -> Result<Both, Outcome> {
    let _directory = super::compilation_directory_guard();
    // A front end that will not take the argv has no graph to compare, and
    // says so where every other unusable command line says it.
    let session = match Session::from_args(argv) {
        Ok(session) => session,
        Err(refusal) => return Err(Outcome::NotAccepted(refusal.to_string())),
    };
    let manifest_path = directory.join("build.ninja");
    let options = NinjaWriterOptions::from_flags(&session.flags);

    let Evaluated {
        mut ev,
        mut nodes,
        regeneration_nodes,
        mut refusals,
    } = match evaluate(session) {
        Ok(evaluated) => evaluated,
        Err(error) => return Err(Outcome::NotAccepted(format!("{error:#}"))),
    };
    // A read that ends in a refusal has no graph to compare: the frontend
    // builds what it collected and then dies, and this comparison is about
    // what the two emitters make of a graph.
    if let Some(refusal) = refusals.pop() {
        return Err(Outcome::NotAccepted(format!("{:#}", refusal.error)));
    }
    // The two paths have to be handed the same roots, generated Makefiles
    // included, or the comparison stops covering the edges that make them.
    nodes.extend(regeneration_nodes.into_iter().map(|root| root.node));
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
        return Err(Outcome::NotAccepted(format!("{error:#}")));
    }
    let pending_subninjas = sink.take_unit().subninjas.len();
    let direct = sink.into_graph();
    let parsed = load_manifest(directory, "build.ninja", ManifestOptions::default());
    match (direct, parsed) {
        (Ok(direct), Ok(parsed)) => Ok(Both {
            direct,
            parsed: parsed.graph,
            semantics,
            pending_subninjas,
        }),
        (Err(direct), Err(manifest)) => Err(Outcome::BothRefused {
            direct: direct.to_string(),
            manifest: manifest.to_string(),
        }),
        (Err(direct), Ok(_)) => Err(Outcome::OnlyDirectRefused(direct.to_string())),
        (Ok(_), Err(manifest)) => Err(Outcome::OnlyManifestRefused(manifest.to_string())),
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

    fn compare(&self) -> Outcome {
        compare(self.directory.path(), self.argv.clone())
    }

    fn both(&self) -> Both {
        build_both(self.directory.path(), self.argv.clone())
            .unwrap_or_else(|outcome| panic!("the fixture did not produce two graphs: {outcome}"))
    }
}

/// Every output either graph would withdraw if the command that makes it
/// failed, as a sorted list of paths.
fn withdrawn(graph: &BuildGraph) -> Vec<String> {
    let arenas = graph.arenas();
    let mut paths = arenas
        .edge_ids()
        .flat_map(|edge| arenas.delete_on_error(edge))
        .map(|node| arenas.node_path(*node).to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

/// Every output either graph makes only on the way to making one that was
/// asked for, as a sorted list of paths.
fn peers(graph: &BuildGraph) -> Vec<String> {
    let arenas = graph.arenas();
    let mut paths = arenas
        .edge_ids()
        .flat_map(|edge| arenas.peer_outputs(edge))
        .map(|node| arenas.node_path(*node).to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

/// Every output either graph would delete once the build has finished with it,
/// as a sorted list of paths.
fn disposable(graph: &BuildGraph) -> Vec<String> {
    outputs_where(graph, |edge| edge.disposable)
}

/// Every output either graph reads off the disk again once its command has
/// run, as a sorted list of paths.
fn reobserved(graph: &BuildGraph) -> Vec<String> {
    outputs_where(graph, |edge| edge.outputs_reobserved)
}

/// Every wait either graph forgives a failure of, as a sorted list of
/// `consumer <- input` pairs.
fn forgiven_order(graph: &BuildGraph) -> Vec<String> {
    let arenas = graph.arenas();
    let mut pairs = arenas
        .edge_ids()
        .flat_map(|edge| {
            arenas
                .edge(edge)
                .input
                .iter()
                .filter(move |input| arenas.order_is_forgiven(edge, **input))
                .flat_map(move |input| {
                    arenas.edge(edge).out.iter().map(move |output| {
                        format!(
                            "{} <- {}",
                            arenas.node_path(*output),
                            arenas.node_path(*input)
                        )
                    })
                })
        })
        .collect::<Vec<_>>();
    pairs.sort();
    pairs
}

/// Every output either graph is allowed to find missing, as a sorted list of
/// paths.
fn intermediate(graph: &BuildGraph) -> Vec<String> {
    outputs_where(graph, |edge| edge.intermediate)
}

/// Every output either graph counts as absent when it is not on disk, rather
/// than as an alias for what its edge reads, as a sorted list of paths.
fn unaliased(graph: &BuildGraph) -> Vec<String> {
    outputs_where(graph, |edge| edge.outputs_unaliased)
}

/// Every output either graph reads as the end of its second when the edge that
/// makes it is deciding whether to, as a sorted list of paths.
fn low_resolution(graph: &BuildGraph) -> Vec<String> {
    outputs_where(graph, |edge| edge.outputs_low_resolution)
}

/// Every output either graph was given a second place to look for, as sorted
/// `output -> found` pairs.
fn searched(graph: &BuildGraph) -> Vec<String> {
    let arenas = graph.arenas();
    let mut pairs = arenas
        .node_ids()
        .filter_map(|node| {
            let found = arenas.searched_at(node)?;
            Some(format!("{} -> {}", arenas.node_path(node), found))
        })
        .collect::<Vec<_>>();
    pairs.sort();
    pairs
}

fn outputs_where(graph: &BuildGraph, wanted: impl Fn(&crate::graph::Edge) -> bool) -> Vec<String> {
    let arenas = graph.arenas();
    let mut paths = arenas
        .edge_ids()
        .filter(|edge| wanted(arenas.edge(*edge)))
        .flat_map(|edge| arenas.edge(edge).out.iter())
        .map(|node| arenas.node_path(*node).to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

/// A command line kati's front end will not take is an outcome, not a crash.
///
/// This gate hands its flags straight to `Session::from_args`, so the option
/// table's refusals arrive here rather than in a process anyone can exit from.
/// Every letter of GNU Make's short-option row that kati cannot honour used to
/// reach `panic!("Unknown flag: ...")` there — sixteen of them, including
/// `-rR`, which is not one letter but two and which the corpus under
/// tests/make/ writes seven times. A panic inside this gate takes the gate
/// down; a refusal is a value it can report.
///
/// `-q` is the letter that would be worst to accept quietly: it says answer
/// whether there is work instead of doing it, and a front end that compiled
/// the graph and reported success would be answering for a build that never
/// ran. `-rR` is the cluster beside it, and both sinks have to see both
/// letters — `no_builtin_rules` and `no_builtin_variables` are compiler input,
/// so a cluster read as one unknown word would compile a different graph.
#[test]
fn a_refused_switch_is_an_outcome() {
    let makefile = "all: out\nout:\n\t@touch m1\n";
    let outcome = Case::new(makefile, &["-q"]).compare();
    assert_eq!(
        outcome.to_string(),
        "make rejected the fixture: rkati: unsupported option -- 'q'"
    );
    // And the cluster beside it is read as the two switches it is, all the way
    // through to a graph both sinks agree on.
    agrees(
        "\
all: out
out: out.c
\t@printf '%s\\n' '$(CC)' '$(AR)' > $@
out.c:
\t@touch $@
",
        &["-rR"],
    );
}

/// Assert that one Makefile produces the same graph both ways.
#[track_caller]
fn agrees(makefile: &str, flags: &[&str]) {
    let outcome = Case::new(makefile, flags).compare();
    assert!(outcome.agreed(), "{outcome}");
}

/// A `.ONESHELL` recipe is the one recipe whose script holds a newline, and a
/// `build.ninja` binding ends at one.
///
/// This used to emit a manifest that was not a manifest: the recipe's own
/// separators went into the binding as they were, the stanza broke in two, and
/// stock ninja answered `expected 'command =' line`. Reaching `both()` at all
/// is the assertion — the manifest is written here and read back by Ronin's
/// own reader, which refused it too.
///
/// GNU Make 4.4.1 over this recipe: m1 and m2 both made, and make succeeds.
/// One shell for the whole recipe and no `-e` anywhere, so the failing line
/// stops nothing — which the two sinks have to keep agreeing about across a
/// respelling only one of them needs.
#[test]
fn one_shell_recipe_reaches_a_reader() {
    let both = Case::new(
        "\
.ONESHELL:
all: out
out:
\t@touch m1; false
\t@touch m2
",
        &[],
    )
    .both();
    assert!(
        both.semantics
            .semantic
            .values()
            .all(|command| command.respelled),
        "the script holds a newline, which is what the manifest cannot say"
    );
    assert_eq!(
        differences(&both.direct, &both.parsed, &both.semantics),
        Vec::<String>::new()
    );
}

/// `-i` is GNU Make's `--ignore-errors`, and the letter has to reach both
/// sinks rather than merely set a field somewhere.
///
/// This comparison drives kati's own command line — `Case` hands the flags
/// straight to `Session::from_args` — and that table read `-i` as the dry run.
/// So this Makefile compiled to a manifest with no muting anywhere and a graph
/// that bound nothing: the two sides agreed with each other and both disagreed
/// with Make, which is the shape a comparison cannot catch on its own. Ronin's
/// Make mode never had it, because its front end parses its own options.
///
/// GNU Make 4.4.1, `make -i` over this recipe: both markers made, exit 0.
#[test]
fn ignore_errors_switch_reaches_both_sinks() {
    let makefile = "\
all: out
out:
\t@touch m1; false
\t@touch m2
";
    let both = Case::new(makefile, &["-i"]).both();
    assert!(
        !both.semantics.semantic.is_empty()
            && both
                .semantics
                .semantic
                .values()
                .all(|command| command.ignore_errors),
        "-i has to forgive every recipe, and the manifest says so by muting it"
    );
    assert_eq!(
        differences(&both.direct, &both.parsed, &both.semantics),
        Vec::<String>::new()
    );
    // A semantic override is recorded only for a rule that ignores errors or
    // had to be respelled, and this recipe holds no newline — so an empty map
    // is the whole statement that nothing was forgiven. The dry run is the
    // letter that was standing here, and it must not acquire this meaning.
    for flags in [&[] as &[&str], &["-n"]] {
        assert!(
            Case::new(makefile, flags)
                .both()
                .semantics
                .semantic
                .is_empty(),
            "only -i forgives a failure, not {flags:?}"
        );
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

#[test]
fn deferred_edges_cross_the_common_sink() {
    agrees(
        "\
all: a
a b &:: p | ordered
\t@printf '%s\\n' '$?' > a
\t@cp a b
p ordered:
\t@touch $@
",
        &[],
    );
}

/// `.DELETE_ON_ERROR` is a property only the direct graph can hold, and this
/// says so out loud rather than leaving it to be inferred from a comparison
/// that passes.
///
/// Ninja has no notion of an output the build takes back when the command
/// fails, so the writer has nothing to write and the parsed manifest carries
/// none of it. That is the same bounded divergence `intermediate` and
/// `disposable` already have, and like them it is outside what
/// `make.manifest-equivalence` enumerates — the comparison below still agrees,
/// which is the property that matters for the emitted manifest as an oracle.
///
/// The exclusions are asserted here too, because they are what makes the list
/// per output rather than per edge: `kept` is `.PRECIOUS` and stays, `all` is
/// `.PHONY` and names no file to take back.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.manifest-equivalence+1/test]
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn withdrawal_reaches_only_the_direct_graph() {
    let case = Case::new(
        "\
.DELETE_ON_ERROR:
.PRECIOUS: kept
.PHONY: all
all: gone kept
gone kept &:
\t@echo g > gone; echo k > kept; exit 1
",
        &[],
    );
    let both = case.both();
    assert_eq!(withdrawn(&both.direct), vec!["gone".to_owned()]);
    assert!(
        withdrawn(&both.parsed).is_empty(),
        "a manifest has no way to say this, so reading one back must not invent it"
    );
    assert_eq!(
        differences(&both.direct, &both.parsed, &both.semantics),
        Vec::<String>::new()
    );
}

/// A `.PRECIOUS` pattern takes an invented file out of the disposable set
/// without taking away its being intermediate, and only the direct graph can
/// say either.
///
/// Two properties in one Makefile, because they are separable and GNU Make
/// separates them: `hello.z` was invented to complete the chain, so its absence
/// is still no reason to remake what reads it, and the pattern that made it is
/// spelled on `.PRECIOUS`, so the build does not sweep it up afterwards. The
/// control is `hello.y`, invented the same way and named by no pattern.
///
/// `.PRECIOUS` is matched against the target pattern of the rule that made the
/// file rather than against the file, so `%.z` protecting a `%.z: %.x` output
/// says nothing about a name an explicit rule wrote.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.manifest-equivalence+1/test]
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn a_precious_pattern_spares_an_intermediate() {
    let case = Case::new(
        "\
.PRECIOUS: %.z
all: hello.tsk other.wsk
hello.x other.x:
\t@echo body > $@
%.z: %.x
\t@cat $< > $@
%.y: %.x
\t@cat $< > $@
%.tsk: %.z
\t@cat $< > $@
%.wsk: %.y
\t@cat $< > $@
",
        &[],
    );
    let both = case.both();
    assert_eq!(disposable(&both.direct), vec!["other.y".to_owned()]);
    assert_eq!(
        intermediate(&both.direct),
        vec!["hello.z".to_owned(), "other.y".to_owned()],
        "protection is from the deletion and not from being intermediate"
    );
    assert!(
        disposable(&both.parsed).is_empty() && intermediate(&both.parsed).is_empty(),
        "a manifest has no way to say either, so reading one back must not invent them"
    );
    assert_eq!(
        differences(&both.direct, &both.parsed, &both.semantics),
        Vec::<String>::new()
    );
}

/// A target whose recipe writes nothing compiles to the same commandless edge
/// a manifest's `phony` parses to, and only the direct graph knows the two mean
/// opposite things.
///
/// `empty` has an empty recipe and `bare` has none at all; GNU Make treats both
/// the same, so both carry the property. `alias` is the control that shows the
/// property is not simply "every edge": it has a command, so nothing about it
/// is commandless.
///
/// The manifest side cannot carry it — `build empty: phony src` is the only
/// thing the writer can emit, and Ninja reads that as the alias it spells —
/// which is the same bounded divergence `intermediate` and `disposable` already
/// have. The comparison below still agrees, because the shape both sides
/// describe is unchanged.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.manifest-equivalence+1/test]
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn empty_recipes_reach_the_direct_graph() {
    let case = Case::new(
        "\
all: empty bare alias
src:
\t@echo body > $@
empty: src ;
bare: src
alias: src
\t@cat $< > $@
",
        &[],
    );
    let both = case.both();
    assert_eq!(
        unaliased(&both.direct),
        vec!["all".to_owned(), "bare".to_owned(), "empty".to_owned()],
        "a goal with no recipe of its own is one too"
    );
    assert!(
        unaliased(&both.parsed).is_empty(),
        "a manifest has no way to say this, so reading one back must not invent it"
    );
    assert_eq!(
        differences(&both.direct, &both.parsed, &both.semantics),
        Vec::<String>::new()
    );
}

/// An archive index dates its members in whole seconds, and only the direct
/// graph knows which outputs come from one.
///
/// `mylib.a(a.o)` is filed by the rule that reads `a.o`, so that edge is the
/// one that has to read the member's date as the end of its second — GNU Make's
/// `low_resolution_time`, which applies to the file being updated. `mylib.a`
/// itself is the control: the same members are its prerequisites, and a
/// prerequisite keeps the plain date, so its edge does not carry the property.
///
/// A manifest cannot say it — a member is a path there like any other — which
/// is the same bounded divergence `intermediate` and `disposable` have.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.manifest-equivalence+1/test]
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn archive_members_reach_the_direct_graph() {
    let case = Case::new(
        "\
mylib.a: mylib.a(a.o)
(%): %
\t@ar -rcU $@ $<
a.o:
\t@echo body > $@
",
        &[],
    );
    let both = case.both();
    assert_eq!(
        low_resolution(&both.direct),
        vec!["mylib.a(a.o)".to_owned()],
        "the archive that reads the member keeps the plain date"
    );
    assert!(
        low_resolution(&both.parsed).is_empty(),
        "a manifest has no way to say this, so reading one back must not invent it"
    );
    assert_eq!(
        differences(&both.direct, &both.parsed, &both.semantics),
        Vec::<String>::new()
    );
}

/// A pattern rule's other target is an output of the same edge, and only the
/// direct graph knows it is one the recipe merely writes on the way.
///
/// Two names from one recipe, one of them asked for. `hello.z` completes the
/// chain to `hello.tsk`, so the search invented it and the build sweeps it up;
/// `hello.w` is the peer, which GNU Make enters as a target of its own and
/// therefore neither sweeps up nor consults when it decides whether the recipe
/// has to run. Both are outputs on both sides — a manifest can say that much —
/// and which of them is the peer is the part only the direct graph carries.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.manifest-equivalence+1/test]
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn peers_reach_only_the_direct_graph() {
    let case = Case::new(
        "\
all: hello.tsk
hello.x:
\t@echo body > $@
%.z %.w: %.x
\t@cat $< > $*.z; cat $< > $*.w
%.tsk: %.z
\t@cat $< > $@
",
        &[],
    );
    let both = case.both();
    assert_eq!(peers(&both.direct), vec!["hello.w".to_owned()]);
    assert_eq!(
        disposable(&both.direct),
        vec!["hello.w".to_owned(), "hello.z".to_owned()],
        "the edge is disposable; sparing the peer is the build's reading of this list"
    );
    assert!(
        peers(&both.parsed).is_empty(),
        "a manifest has no way to say this, so reading one back must not invent it"
    );
    assert_eq!(
        differences(&both.direct, &both.parsed, &both.semantics),
        Vec::<String>::new()
    );
}

/// Being reached by name is what stops an output being a peer, whichever of
/// the rule's names the search matched first.
///
/// `hello.z` is the goal, so the search matches `%.z` and `hello.w` arrives
/// beside it — but `use` then asks for `hello.w` itself, and from there GNU
/// Make decides that name from that name. Nothing is left to spare.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.semantics+1/test]
#[test]
fn asking_by_name_makes_a_target() {
    let case = Case::new(
        "\
all: hello.z use
hello.x:
\t@echo body > $@
%.z %.w: %.x
\t@cat $< > $*.z; cat $< > $*.w
use: hello.w
\t@cat $< > $@
",
        &[],
    );
    let both = case.both();
    assert_eq!(peers(&both.direct), Vec::<String>::new());
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

/// Every Make edge is one whose outputs the build looks at again once its
/// command has run, and only the direct graph says so.
///
/// A target the directory search found elsewhere keeps both names, and only the
/// direct graph is told the second one.
///
/// GNU Make hangs the found path off the file object beside the written name
/// and lets `update_file_1` choose once the prerequisites have settled, so the
/// compiler cannot fold the choice in. A manifest has one path per name and no
/// way to say a second, which is the same bounded divergence `intermediate` and
/// `disposable` have — and the manifest names the target as written, which is
/// where the build would put it if it had to make it.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.manifest-equivalence+1/test]
#[test]
fn searched_name_reaches_only_direct_graph() {
    let found = tempfile::tempdir().expect("a directory for the search to find");
    std::fs::write(found.path().join("vpath-equivalence.o"), "old")
        .expect("the scratch directory is writable");
    let case = Case::new(
        &format!(
            "\
VPATH = {}
all: vpath-equivalence.o
\t@printf '%s' '$^' > seen
vpath-equivalence.o: vpath-equivalence.c
\t@printf 'made' > $@
vpath-equivalence.c:
\t@printf 'source' > $@
",
            found.path().display()
        ),
        &[],
    );
    let both = case.both();
    assert_eq!(
        searched(&both.direct),
        vec![format!(
            "vpath-equivalence.o -> {}/vpath-equivalence.o",
            found.path().display()
        )],
        "the target the search answered about carries both names"
    );
    assert!(
        searched(&both.parsed).is_empty(),
        "a manifest has no way to say this, so reading one back must not invent it"
    );
    assert_eq!(
        differences(&both.direct, &both.parsed, &both.semantics),
        Vec::<String>::new()
    );
}

/// The manifest keeps saying `restat` for `.KATI_RESTAT` and for nothing else,
/// because that binding is what the Makefile asked for and the property here
/// is what Make is. The comparison is deliberately silent about the property,
/// as it is about peers and `.DELETE_ON_ERROR`: a manifest has no way to state
/// it, so a graph read back from one must not appear to have lost it.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.manifest-equivalence+1/test]
// [spec:ronin:req:make.remade-target-re-observed/test]
#[test]
fn reobservation_reaches_only_the_direct_graph() {
    let case = Case::new(
        "\
all: out asked
out: in
\t@cat in > out
asked: in
\t@cat in > asked
in:
\t@touch in
asked: .KATI_RESTAT := 1
",
        &[],
    );
    let both = case.both();
    assert_eq!(
        reobserved(&both.direct),
        vec![
            "all".to_owned(),
            "asked".to_owned(),
            "in".to_owned(),
            "out".to_owned()
        ],
        "every Make edge carries it, whether or not the Makefile asked for restat"
    );
    assert!(
        reobserved(&both.parsed).is_empty(),
        "a manifest has no way to say this, so reading one back must not invent it"
    );
    assert_eq!(
        differences(&both.direct, &both.parsed, &both.semantics),
        Vec::<String>::new()
    );
}

/// The chain between two entries of a double-colon target is an ordering with
/// the status taken out of it, and only the direct graph can say so.
///
/// Ninja's manifest has one word for a wait and it means "after it succeeded",
/// so a graph read back from `build.ninja` has an ordinary order-only edge
/// there. The comparison is deliberately silent about the difference, as it is
/// about re-observation and peers: what a manifest cannot state, a graph parsed
/// from one must not appear to have lost.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.manifest-equivalence+1/test]
#[test]
fn forgiven_chain_edge_reaches_direct_graph() {
    let case = Case::new(
        "\
out:: n1
\t@echo first
out:: n2
\t@echo second
n1: ; @touch n1
n2: ; @touch n2
",
        &[],
    );
    let both = case.both();
    assert_eq!(
        forgiven_order(&both.direct),
        vec![".ronin_grouped_double/1 <- .ronin_grouped_double/0".to_owned()],
        "the second entry waits for the first and outlives its failure"
    );
    assert!(
        forgiven_order(&both.parsed).is_empty(),
        "a manifest has no way to say this, so reading one back must not invent it"
    );
    assert_eq!(
        differences(&both.direct, &both.parsed, &both.semantics),
        Vec::<String>::new()
    );
}

// [spec:ronin:req:make.compiler-boundary/test]
// [spec:ronin:req:make.graph-direct/test]
#[test]
fn unaddressable_phony_aliases_agree() {
    let target = "x".repeat(66 * 1024);
    let case = Case::new(
        &format!(
            "\
.PHONY: all {target}
all: {target}
"
        ),
        &[],
    );
    match case.compare() {
        Outcome::Compared { differences, .. } => {
            assert!(differences.is_empty(), "{differences:?}");
        }
        _ => panic!("expected both graph paths to accept the alias"),
    }

    let manifest = std::fs::read(case.directory.path().join("build.ninja")).unwrap();
    assert!(
        manifest
            .windows(b"_kati_unaddressable_phony_".len())
            .any(|part| part == b"_kati_unaddressable_phony_")
    );
    assert!(
        !manifest
            .windows(target.len())
            .any(|part| part == target.as_bytes())
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

/// One edge carrying the whole binding repertoire the equivalence rule
/// enumerates: pool, depfile and restat as the Makefile states them, and the
/// generator control every recipe rule acquires. The corpus pass reaches the
/// same property over every testcase and is ignored by default, so this is
/// where a binding that stops crossing the sink is caught on an ordinary run.
// [spec:ronin:req:make.graph-direct/test]
// [spec:ronin:req:make.manifest-equivalence+1/test]
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
        Outcome::Compared { differences, .. } => {
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
// [spec:ronin:req:make.manifest-equivalence+1/test]
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
            // Named relatively, because a testcase can observe the name it
            // was given, and an absolute path fails a match Make's own
            // invocation would pass.
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
    respelled: usize,
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
            Outcome::Compared {
                differences,
                respelled,
            } => {
                self.compared += 1;
                self.respelled += respelled;
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
        println!("manifest respellings: {}", self.respelled);
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
