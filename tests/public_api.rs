use ronin::frontend::{
    load_manifest, Build, BuildGraph, EdgeSpec, FrontendError, Jobs, ManifestOptions, Node,
    Persistence, Template,
};
use ronin::{run, run_os, ErrorKind};
use std::error::Error as _;
use std::ffi::OsString;
use std::num::NonZeroUsize;

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
    let cause = error.source().expect("the I/O failure is the cause");
    assert_eq!(
        error.to_string(),
        cause.to_string().replace(" (os error 2)", ""),
        "Ninja-facing text should remain the underlying I/O diagnostic, as strerror renders it"
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
            always_dirty: false,
            intermediate: false,
            disposable: false,
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
            always_dirty: false,
            intermediate: false,
            disposable: false,
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

/// `out` is copied from `mid`, which is copied from the source file `in`.
///
/// Every path is absolute, because where a build runs is a command-line option
/// rather than something the boundary exposes.
fn copy_graph(directory: &std::path::Path) -> BuildGraph {
    let path = |name: &str| {
        let mut bytes = directory.as_os_str().as_encoded_bytes().to_vec();
        bytes.push(b'/');
        bytes.extend_from_slice(name.as_bytes());
        bytes
    };
    let mut graph = BuildGraph::new();
    let root = graph.root();
    let command = graph.binding(b"command");
    let mut recipe = Template::literal(b"cp ");
    let inputs = graph.binding(b"in");
    recipe.push_variable(inputs);
    recipe.push_literal(b" ");
    let outputs = graph.binding(b"out");
    recipe.push_variable(outputs);
    let copy = graph
        .define_rule(root, b"copy", vec![(command, recipe)])
        .unwrap();

    let source = graph.node(&path("in")).unwrap();
    let middle = graph.node(&path("mid")).unwrap();
    let final_output = graph.node(&path("out")).unwrap();
    for (output, input) in [(middle, source), (final_output, middle)] {
        graph
            .add_edge(EdgeSpec {
                scope: root,
                rule: copy,
                explicit_outputs: &[output],
                implicit_outputs: &[],
                explicit_inputs: &[input],
                implicit_inputs: &[],
                order_only_inputs: &[],
                validations: &[],
                always_dirty: false,
                intermediate: false,
                disposable: false,
                bindings: Vec::new(),
            })
            .unwrap();
    }
    graph.add_default(final_output);
    graph
}

// [spec:ronin:req:frontend.graph-construction/test]
#[test]
fn public_api_runs_a_graph_no_manifest_described() {
    let directory = std::env::temp_dir().join(format!(
        "ronin-execute-api-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("in"), b"source\n").unwrap();

    let mut graph = copy_graph(&directory);
    let targets = graph.default_targets();
    let (mut persistence, warning) = Persistence::open(&mut graph, &directory).unwrap();
    assert!(warning.is_none());

    let mut streamed = Vec::new();
    let planned = Build::new(&mut graph, &mut persistence)
        .jobs(Jobs::Limit(NonZeroUsize::new(2).unwrap()))
        .keep_going(0)
        .output(&mut streamed)
        .plan(&targets)
        .unwrap();
    assert!(!planned.already_up_to_date());
    let outcome = planned.run().unwrap();

    assert_eq!(outcome.stopped(), None);
    assert_eq!(outcome.exit_code(), 0);
    assert_eq!(outcome.regenerated(), targets.as_slice());
    assert!(outcome.output().is_empty(), "the sink took the output");
    assert!(String::from_utf8_lossy(&streamed).contains("cp "));
    assert_eq!(std::fs::read(directory.join("out")).unwrap(), b"source\n");

    // The persistent state the first build appended is what makes the second
    // one know it has nothing to do.
    let planned = Build::new(&mut graph, &mut persistence)
        .plan(&targets)
        .unwrap();
    assert!(planned.already_up_to_date());
    persistence.finish().unwrap();
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
            always_dirty: false,
            intermediate: false,
            disposable: false,
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

/// A Makefile, evaluated and built without a manifest existing anywhere.
///
/// Every path is absolute so the build runs the same wherever the test process
/// happens to be, which is also what lets it run beside every other test.
// [spec:ronin:req:make.graph-direct/test]
#[cfg(all(unix, feature = "make"))]
#[test]
fn public_api_builds_a_makefile_through_ronins_scheduler() {
    use ronin::make::kati::session::Session;
    use ronin::make::{load_makefile, Shuffle};
    use std::ffi::OsString;

    let directory = std::env::temp_dir().join(format!(
        "ronin-make-graph-direct-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("in"), b"source\n").unwrap();
    let at = |name: &str| directory.join(name).to_string_lossy().into_owned();
    let makefile = directory.join("Makefile");
    std::fs::write(
        &makefile,
        format!(
            "all: {out}\n{out}: {mid}\n\tcp {mid} {out}\n{mid}: {source}\n\tcp {source} {mid}\n",
            out = at("out"),
            mid = at("mid"),
            source = at("in"),
        ),
    )
    .unwrap();

    let mut graph = load_makefile(
        Session::from_args(vec![
            OsString::from("ronin"),
            OsString::from("-f"),
            makefile.into_os_string(),
        ]),
        Shuffle::None,
    )
    .unwrap()
    .graph;

    // The Makefile's own default goal is the graph's default target, and
    // nothing in between wrote, read, or reparsed a manifest.
    let targets = graph.default_targets();
    assert_eq!(
        targets
            .iter()
            .map(|node| graph.path(*node).to_vec())
            .collect::<Vec<_>>(),
        [b"all".to_vec()]
    );
    assert!(!directory.join("build.ninja").exists());

    let (mut persistence, warning) = Persistence::open(&mut graph, &directory).unwrap();
    assert!(warning.is_none());
    let mut streamed = Vec::new();
    let planned = Build::new(&mut graph, &mut persistence)
        .jobs(Jobs::Limit(NonZeroUsize::new(2).unwrap()))
        .output(&mut streamed)
        .plan(&targets)
        .unwrap();
    assert!(!planned.already_up_to_date());
    let outcome = planned.run().unwrap();

    assert_eq!(outcome.stopped(), None);
    assert_eq!(outcome.exit_code(), 0);
    assert_eq!(std::fs::read(directory.join("out")).unwrap(), b"source\n");
    // A recipe that says nothing about itself gets kati's default description,
    // and the direct graph expands the `$out` in it against the edge.
    assert!(
        String::from_utf8_lossy(&streamed).contains(&format!("build {}", at("out"))),
        "{}",
        String::from_utf8_lossy(&streamed)
    );

    // The build log the first build appended is what makes the second one know
    // the recipe has already run.
    let planned = Build::new(&mut graph, &mut persistence)
        .plan(&targets)
        .unwrap();
    assert!(planned.already_up_to_date());
    persistence.finish().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}
