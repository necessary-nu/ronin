use ronin::frontend::{
    load_manifest, BuildGraph, EdgeSpec, FrontendError, ManifestOptions, Node, Template,
};
use ronin::{run, run_os, ErrorKind};
use std::error::Error as _;
use std::ffi::OsString;

#[test]
fn public_api_classifies_cli_errors() {
    let error = run(&[
        "ronin".to_owned(),
        "-d".to_owned(),
        "not-a-debug-mode".to_owned(),
    ])
    .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Cli);
    assert_eq!(error.to_string(), "unknown debug flag 'not-a-debug-mode'");
    assert!(error.source().is_none());
}

#[test]
fn public_api_preserves_manifest_io_causes() {
    let missing_manifest = std::env::temp_dir().join(format!(
        "ronin-missing-manifest-{}-{}.ninja",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let error = run_os(&[
        OsString::from("ronin"),
        OsString::from("-f"),
        missing_manifest.into_os_string(),
    ])
    .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Manifest);
    assert!(error.source().is_some());
    assert_eq!(
        error.source().unwrap().to_string(),
        error.to_string(),
        "Ninja-facing text should remain the underlying I/O diagnostic"
    );
}

/// The graph described by `manifest_fixture`, built without reading a manifest.
fn build_by_hand() -> BuildGraph {
    let mut graph = BuildGraph::new();
    let root = graph.root();
    let command = graph.binding(b"command");
    let mut recipe = Template::literal(b"cat ");
    let inputs = graph.binding(b"in");
    recipe.push_variable(inputs);
    recipe.push_literal(b" > ");
    let outputs = graph.binding(b"out");
    recipe.push_variable(outputs);
    let cat = graph
        .define_rule(root, b"cat", vec![(command, recipe)])
        .unwrap();

    let source = graph.node(b"in").unwrap();
    let middle = graph.node(b"mid").unwrap();
    let stamp = graph.node(b"mid.stamp").unwrap();
    let final_output = graph.node(b"out").unwrap();
    graph
        .add_edge(EdgeSpec {
            scope: root,
            rule: cat,
            explicit_outputs: &[middle],
            implicit_outputs: &[stamp],
            explicit_inputs: &[source],
            implicit_inputs: &[],
            order_only_inputs: &[],
            validations: &[],
            bindings: Vec::new(),
        })
        .unwrap();
    graph
        .add_edge(EdgeSpec {
            scope: root,
            rule: cat,
            explicit_outputs: &[final_output],
            implicit_outputs: &[],
            explicit_inputs: &[middle],
            implicit_inputs: &[],
            order_only_inputs: &[],
            validations: &[],
            bindings: Vec::new(),
        })
        .unwrap();
    graph.add_default(final_output);
    graph
}

fn manifest_fixture() -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "ronin-frontend-api-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("build.ninja"),
        "rule cat\n  command = cat $in > $out\n\
         build mid | mid.stamp: cat in\nbuild out: cat mid\ndefault out\n",
    )
    .unwrap();
    directory
}

fn shape(graph: &BuildGraph, paths: &[&[u8]]) -> Vec<(Vec<u8>, bool)> {
    paths
        .iter()
        .map(|path| {
            let node = graph.lookup(path).expect("the fixture interns every path");
            (graph.path(node).to_vec(), graph.generator(node).is_some())
        })
        .collect()
}

// [spec:ronin:req:frontend.graph-construction/test]
#[test]
fn public_api_builds_the_same_graph_a_manifest_would() {
    const PATHS: [&[u8]; 4] = [b"in", b"mid", b"mid.stamp", b"out"];

    let directory = manifest_fixture();
    let parsed = load_manifest(&directory, "build.ninja", ManifestOptions::default()).unwrap();
    assert!(parsed.warnings.is_empty());
    let assembled = build_by_hand();

    assert_eq!(shape(&assembled, &PATHS), shape(&parsed.graph, &PATHS));
    let defaults = |graph: &BuildGraph| -> Vec<Vec<u8>> {
        graph
            .default_targets()
            .into_iter()
            .map(|node| graph.path(node).to_vec())
            .collect()
    };
    assert_eq!(defaults(&assembled), [b"out".to_vec()]);
    assert_eq!(defaults(&assembled), defaults(&parsed.graph));
    std::fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:frontend.graph-construction/test]
#[test]
fn public_api_refuses_a_second_generator_for_one_output() {
    let mut graph = build_by_hand();
    let root = graph.root();
    let rule = graph.rule(root, b"cat").unwrap();
    let taken: Vec<Node> = vec![graph.lookup(b"out").unwrap()];
    let error = graph
        .add_edge(EdgeSpec {
            scope: root,
            rule,
            explicit_outputs: &taken,
            implicit_outputs: &[],
            explicit_inputs: &[],
            implicit_inputs: &[],
            order_only_inputs: &[],
            validations: &[],
            bindings: Vec::new(),
        })
        .unwrap_err();

    assert_eq!(
        error,
        FrontendError::DuplicateOutput {
            path: b"out".to_vec()
        }
    );
    assert_eq!(error.to_string(), "multiple rules generate out");
}
