use super::*;
use crate::env::mkenv;
use crate::graph::{mkedge, mknode, nodeget, nodeuse, Graph};
use crate::names::Names;
use crate::util::{xasprintf, BStr};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_PLAN: AtomicUsize = AtomicUsize::new(0);

fn plan_graph(source: &str) -> Graph {
    let path = std::env::temp_dir().join(format!(
        "ronin-plan-test-{}-{}.ninja",
        std::process::id(),
        NEXT_PLAN.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(
        &path,
        format!("rule cat\n  command = cat $in > $out\n{source}"),
    )
    .unwrap();
    let mut graph = Graph::default();
    let mut parser = crate::parse::Parser::default();
    let mut state = crate::env::EnvState::new(&mut graph);
    crate::parse::parse(
        path.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap();
    fs::remove_file(path).unwrap();
    graph
}

fn mark_dirty(graph: &Graph, paths: &[&str]) -> RuntimeState {
    let mut runtime = RuntimeState::new(graph);
    for path in paths {
        let node = nodeget(graph, path.as_bytes()).unwrap();
        runtime.node_mut(node).set_dirty(true);
    }
    runtime
}

fn output_path(graph: &Graph, edge: EdgeId) -> String {
    let output = graph.edge(edge).out[0];
    String::from_utf8_lossy(graph.node_path(output).as_bytes()).into_owned()
}

fn add_plan_target(plan: &mut Plan, graph: &Graph, runtime: &RuntimeState, path: &[u8]) {
    let node = nodeget(graph, path).unwrap();
    plan.add_target(graph, runtime, node).unwrap();
}

fn build_fixture(label: &str, manifest: &str) -> (Graph, std::path::PathBuf) {
    let directory = std::env::temp_dir().join(format!(
        "ronin-build-{label}-{}-{}",
        std::process::id(),
        NEXT_PLAN.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("build.ninja");
    fs::write(
        &path,
        manifest.replace("$dir", &directory.to_string_lossy()),
    )
    .unwrap();
    let mut graph = Graph::default();
    let mut parser = crate::parse::Parser::default();
    let mut state = crate::env::EnvState::new(&mut graph);
    crate::parse::parse(
        path.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap();
    (graph, directory)
}

fn parse_fixture(directory: &Path) -> Graph {
    let path = directory.join("build.ninja");
    let mut graph = Graph::default();
    let mut parser = crate::parse::Parser::default();
    let mut state = crate::env::EnvState::new(&mut graph);
    crate::parse::parse(
        path.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap();
    graph
}

fn assert_multi_output_deps_log(label: &str, depfile: &str) {
    let (mut graph, directory) = build_fixture(
            label,
            "rule cc\n  command = touch $out\n  depfile = $dir/in.d\n  deps = gcc\nbuild $dir/out1 $dir/out2: cc $dir/in1 $dir/in2\n",
        );
    fs::write(directory.join("in1"), "").unwrap();
    fs::write(directory.join("in2"), "").unwrap();
    fs::write(
        directory.join("in.d"),
        depfile.replace("$dir", &directory.to_string_lossy()),
    )
    .unwrap();
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let expected = vec![
        directory.join("in1").to_string_lossy().into_owned(),
        directory.join("in2").to_string_lossy().into_owned(),
    ];
    let mut deps_log = crate::deps::DepsLog::open(Some(&directory)).unwrap();
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&out1).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    for output in [&out1, &out2] {
        let output = nodeget(&graph, output.as_bytes()).unwrap();
        let entry = crate::deps::depsentry(&deps_log, output).unwrap();
        assert_eq!(
            entry
                .deps
                .nodes
                .iter()
                .map(|node| graph.node_path(*node).to_owned())
                .collect::<Vec<_>>(),
            expected
        );
    }
    deps_log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

fn assert_phony_use_case(case: usize) {
    let manifest = "rule touch\n  command = touch $out\nbuild $dir/notreal: phony $dir/blank\nbuild $dir/phony1: phony $dir/notreal\nbuild $dir/phony2: phony\nbuild $dir/phony3: phony $dir/blank\nbuild $dir/phony4: phony $dir/notreal\nbuild $dir/phony5: phony\nbuild $dir/phony6: phony $dir/blank\nbuild $dir/test1: touch $dir/phony1\nbuild $dir/test2: touch $dir/phony2\nbuild $dir/test3: touch $dir/phony3\nbuild $dir/test4: touch $dir/phony4\nbuild $dir/test5: touch $dir/phony5\nbuild $dir/test6: touch $dir/phony6\n";
    let (mut graph, directory) = build_fixture(&format!("phony-use-case-{case}"), manifest);
    fs::write(directory.join("blank"), "").unwrap();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        for index in 1..=6 {
            builder
                .add_target(
                    directory
                        .join(format!("test{index}"))
                        .to_string_lossy()
                        .as_bytes(),
                )
                .unwrap();
        }
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 6);
    }

    let target = directory
        .join(format!("test{case}"))
        .to_string_lossy()
        .into_owned();
    if case == 2 || case == 5 {
        for _ in 0..2 {
            let mut builder = Builder::new(&mut graph, BuildOptions::default());
            builder.add_target(&target).unwrap();
            assert!(!builder.already_up_to_date());
            builder.build().unwrap();
            assert_eq!(builder.commands_ran.len(), 1);
        }
    } else {
        {
            let mut builder = Builder::new(&mut graph, BuildOptions::default());
            builder.add_target(&target).unwrap();
            assert!(builder.already_up_to_date());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(directory.join("blank"), "changed").unwrap();
        {
            let mut builder = Builder::new(&mut graph, BuildOptions::default());
            builder.add_target(&target).unwrap();
            assert!(!builder.already_up_to_date());
            builder.build().unwrap();
            assert_eq!(builder.commands_ran.len(), 1);
            let phony = nodeget(
                builder.graph,
                directory
                    .join(format!("phony{case}"))
                    .to_string_lossy()
                    .as_bytes(),
            )
            .unwrap();
            let blank = nodeget(
                builder.graph,
                directory.join("blank").to_string_lossy().as_bytes(),
            )
            .unwrap();
            assert_eq!(
                builder.runtime.node(phony).mtime(),
                builder.runtime.node(blank).mtime()
            );
            assert!(!builder.runtime.node(phony).dirty());
        }
    }
    fs::remove_dir_all(directory).unwrap();
}

// Cases adapted from Ninja's src/status_test.cc.
#[test]
fn ninja_status_format_elapsed_and_placeholders() {
    let state = BuildState::new(BuildOptions::default());
    assert_eq!(format_progress_status(&state, "[%%/e%e]"), "[%/e0.000]");
    assert_eq!(format_progress_status(&state, "[%%/e%w]"), "[%/e00:00]");
    assert_eq!(
        format_progress_status(&state, "[%%/s%s/t%t/r%r/u%u/f%f]"),
        "[%/s0/t0/r0/u0/f0]"
    );
}

#[test]
fn ninja_build_status_format_eta() {
    let state = BuildState::new(BuildOptions::default());
    assert_eq!(format_progress_status(&state, "[%%/E%E]"), "[%/E?]");
    assert_eq!(format_progress_status(&state, "[W%W/P%P]"), "[W?/P  0%]");
    assert_eq!(format_progress_status(&state, "[o%o/c%c]"), "[o0.0/c0.0]");
}

#[test]
fn ninja_build_status_format_time_progress() {
    let state = BuildState::new(BuildOptions::default());
    assert_eq!(format_progress_status(&state, "[%%/p%p]"), "[%/p  0%]");
}

#[test]
fn ninja_build_status_format_replace_placeholder() {
    let state = BuildState::new(BuildOptions::default());
    assert_eq!(
        format_progress_status(&state, "[%%/s%s/t%t/r%r/u%u/f%f]"),
        "[%/s0/t0/r0/u0/f0]"
    );
}

#[test]
fn ninja_plan_basic() {
    let graph = plan_graph("build out: cat mid\nbuild mid: cat in\n");
    let runtime = mark_dirty(&graph, &["mid", "out"]);
    let mut plan = Plan::default();
    add_plan_target(&mut plan, &graph, &runtime, b"out");
    plan.prepare_queue(&graph);
    let edge = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, edge), "mid");
    assert!(plan.find_work(&graph).is_none());
    plan.edge_finished(&graph, &runtime, edge, EdgeResult::Succeeded)
        .unwrap();
    let edge = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, edge), "out");
    plan.edge_finished(&graph, &runtime, edge, EdgeResult::Succeeded)
        .unwrap();
    assert!(!plan.more_to_do());
    assert!(plan.find_work(&graph).is_none());
}

#[test]
fn ninja_plan_double_output_direct() {
    let graph = plan_graph("build out: cat mid1 mid2\nbuild mid1 mid2: cat in\n");
    let runtime = mark_dirty(&graph, &["mid1", "mid2", "out"]);
    let mut plan = Plan::default();
    add_plan_target(&mut plan, &graph, &runtime, b"out");
    plan.prepare_queue(&graph);
    let first = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, first), "mid1");
    plan.edge_finished(&graph, &runtime, first, EdgeResult::Succeeded)
        .unwrap();
    let second = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, second), "out");
    plan.edge_finished(&graph, &runtime, second, EdgeResult::Succeeded)
        .unwrap();
    assert!(plan.find_work(&graph).is_none());
}

#[test]
fn ninja_plan_double_output_indirect() {
    let graph = plan_graph(
        "build out: cat b1 b2\nbuild b1: cat a1\nbuild b2: cat a2\nbuild a1 a2: cat in\n",
    );
    let runtime = mark_dirty(&graph, &["a1", "a2", "b1", "b2", "out"]);
    let mut plan = Plan::default();
    add_plan_target(&mut plan, &graph, &runtime, b"out");
    plan.prepare_queue(&graph);
    for expected in ["a1", "b1", "b2", "out"] {
        let edge = plan.find_work(&graph).unwrap();
        assert_eq!(output_path(&graph, edge), expected);
        plan.edge_finished(&graph, &runtime, edge, EdgeResult::Succeeded)
            .unwrap();
    }
    assert!(plan.find_work(&graph).is_none());
}

#[test]
fn ninja_plan_double_dependent() {
    let graph = plan_graph(
        "build out: cat a1 a2\nbuild a1: cat mid\nbuild a2: cat mid\nbuild mid: cat in\n",
    );
    let runtime = mark_dirty(&graph, &["mid", "a1", "a2", "out"]);
    let mut plan = Plan::default();
    add_plan_target(&mut plan, &graph, &runtime, b"out");
    plan.prepare_queue(&graph);
    for expected in ["mid", "a1", "a2", "out"] {
        let edge = plan.find_work(&graph).unwrap();
        assert_eq!(output_path(&graph, edge), expected);
        plan.edge_finished(&graph, &runtime, edge, EdgeResult::Succeeded)
            .unwrap();
    }
    assert!(!plan.more_to_do());
}

fn check_depth_one_pool(pool_definition: &str) {
    let graph = plan_graph(&format!(
            "{pool_definition}rule poolcat\n  command = cat $in > $out\n  pool = selected\nbuild out1: poolcat in\nbuild out2: poolcat in\n"
        ));
    let runtime = mark_dirty(&graph, &["out1", "out2"]);
    let mut plan = Plan::default();
    add_plan_target(&mut plan, &graph, &runtime, b"out1");
    add_plan_target(&mut plan, &graph, &runtime, b"out2");
    plan.prepare_queue(&graph);
    let first = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, first), "out1");
    assert!(plan.find_work(&graph).is_none());
    plan.edge_finished(&graph, &runtime, first, EdgeResult::Succeeded)
        .unwrap();
    let second = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, second), "out2");
    assert!(plan.find_work(&graph).is_none());
    plan.edge_finished(&graph, &runtime, second, EdgeResult::Succeeded)
        .unwrap();
    assert!(!plan.more_to_do());
}

#[test]
fn ninja_plan_pool_with_depth_one() {
    check_depth_one_pool("pool selected\n  depth = 1\n");
}

#[test]
fn ninja_plan_console_pool() {
    let graph = plan_graph(
            "rule poolcat\n  command = cat $in > $out\n  pool = console\nbuild out1: poolcat in\nbuild out2: poolcat in\n",
        );
    let runtime = mark_dirty(&graph, &["out1", "out2"]);
    let mut plan = Plan::default();
    add_plan_target(&mut plan, &graph, &runtime, b"out1");
    add_plan_target(&mut plan, &graph, &runtime, b"out2");
    plan.prepare_queue(&graph);
    let first = plan.find_work(&graph).unwrap();
    assert!(plan.find_work(&graph).is_none());
    plan.edge_finished(&graph, &runtime, first, EdgeResult::Succeeded)
        .unwrap();
    let second = plan.find_work(&graph).unwrap();
    plan.edge_finished(&graph, &runtime, second, EdgeResult::Succeeded)
        .unwrap();
    assert!(!plan.more_to_do());
}

#[test]
fn ninja_plan_pools_with_depth_two() {
    let graph = plan_graph(
            "pool foobar\n  depth = 2\npool bazbin\n  depth = 2\nrule foocat\n  command = cat\n  pool = foobar\nrule bazcat\n  command = cat\n  pool = bazbin\nbuild out1: foocat in\nbuild out2: foocat in\nbuild out3: foocat in\nbuild outb1: bazcat in\nbuild outb2: bazcat in\nbuild outb3: bazcat in\n  pool =\nbuild allTheThings: cat out1 out2 out3 outb1 outb2 outb3\n",
        );
    let runtime = mark_dirty(
        &graph,
        &[
            "out1",
            "out2",
            "out3",
            "outb1",
            "outb2",
            "outb3",
            "allTheThings",
        ],
    );
    let mut plan = Plan::default();
    add_plan_target(&mut plan, &graph, &runtime, b"allTheThings");
    plan.prepare_queue(&graph);
    let mut initial = Vec::new();
    while let Some(edge) = plan.find_work(&graph) {
        initial.push(edge);
    }
    assert_eq!(
        initial
            .iter()
            .map(|edge| output_path(&graph, *edge))
            .collect::<Vec<_>>(),
        ["out1", "out2", "outb1", "outb2", "outb3"]
    );
    plan.edge_finished(&graph, &runtime, initial[0], EdgeResult::Succeeded)
        .unwrap();
    let out3 = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, out3), "out3");
    plan.edge_finished(&graph, &runtime, out3, EdgeResult::Succeeded)
        .unwrap();
    for edge in &initial[1..] {
        plan.edge_finished(&graph, &runtime, *edge, EdgeResult::Succeeded)
            .unwrap();
    }
    let final_edge = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, final_edge), "allTheThings");
    plan.edge_finished(&graph, &runtime, final_edge, EdgeResult::Succeeded)
        .unwrap();
    assert!(!plan.more_to_do());
}

#[test]
// [spec:ronin:req:runtime.typed-runtime-state/test]
fn ninja_plan_pool_with_failing_edge() {
    let graph = plan_graph(
            "pool foobar\n  depth = 1\nrule poolcat\n  command = cat\n  pool = foobar\nbuild out1: poolcat in\nbuild out2: poolcat in\n",
        );
    let runtime = mark_dirty(&graph, &["out1", "out2"]);
    let mut plan = Plan::default();
    add_plan_target(&mut plan, &graph, &runtime, b"out1");
    add_plan_target(&mut plan, &graph, &runtime, b"out2");
    plan.prepare_queue(&graph);
    let first = plan.find_work(&graph).unwrap();
    assert!(plan.find_work(&graph).is_none());
    plan.edge_finished(&graph, &runtime, first, EdgeResult::Failed)
        .unwrap();
    let second = plan.find_work(&graph).unwrap();
    assert!(plan.find_work(&graph).is_none());
    plan.edge_finished(&graph, &runtime, second, EdgeResult::Failed)
        .unwrap();
    assert!(plan.more_to_do());
    assert!(plan.find_work(&graph).is_none());
}

#[test]
fn ninja_plan_pool_with_redundant_edges() {
    let graph = plan_graph(
            "pool compile\n  depth = 1\nrule generate\n  command = touch $out\nrule echo\n  command = echo $out\nbuild foo.obj: echo foo || foo\n  pool = compile\nbuild bar.obj: echo bar || bar\n  pool = compile\nbuild lib: echo foo.obj bar.obj\nbuild foo: generate\nbuild bar: generate\nbuild all: phony lib\n",
        );
    let runtime = mark_dirty(&graph, &["foo", "bar", "foo.obj", "bar.obj", "lib", "all"]);
    let mut plan = Plan::default();
    add_plan_target(&mut plan, &graph, &runtime, b"all");
    plan.prepare_queue(&graph);

    let first = plan.find_work(&graph).unwrap();
    let second = plan.find_work(&graph).unwrap();
    assert_eq!(
        [output_path(&graph, first), output_path(&graph, second)],
        ["foo", "bar"]
    );
    assert!(plan.find_work(&graph).is_none());

    plan.edge_finished(&graph, &runtime, first, EdgeResult::Succeeded)
        .unwrap();
    let foo_object = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, foo_object), "foo.obj");
    assert!(plan.find_work(&graph).is_none());
    plan.edge_finished(&graph, &runtime, foo_object, EdgeResult::Succeeded)
        .unwrap();

    plan.edge_finished(&graph, &runtime, second, EdgeResult::Succeeded)
        .unwrap();
    let bar_object = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, bar_object), "bar.obj");
    assert!(plan.find_work(&graph).is_none());
    plan.edge_finished(&graph, &runtime, bar_object, EdgeResult::Succeeded)
        .unwrap();

    let library = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, library), "lib");
    plan.edge_finished(&graph, &runtime, library, EdgeResult::Succeeded)
        .unwrap();
    let all = plan.find_work(&graph).unwrap();
    assert_eq!(output_path(&graph, all), "all");
    plan.edge_finished(&graph, &runtime, all, EdgeResult::Succeeded)
        .unwrap();
    assert!(!plan.more_to_do());
}

#[test]
fn ninja_plan_priority_without_build_log() {
    let graph = plan_graph(
            "rule r\n  command = unused\nbuild out: r a0 b0 c0\nbuild a0: r a1\nbuild a1: r a2\nbuild b0: r b1\nbuild c0: r b1\n",
        );
    let runtime = mark_dirty(&graph, &["a1", "a0", "b0", "c0", "out"]);
    let mut plan = Plan::default();
    add_plan_target(&mut plan, &graph, &runtime, b"out");
    plan.prepare_queue(&graph);
    assert_eq!(
        [("out", 1), ("a0", 2), ("b0", 2), ("c0", 2), ("a1", 3)].map(|(path, weight)| {
            let node = nodeget(&graph, path.as_bytes()).unwrap();
            let edge = graph.node(node).gen.unwrap();
            let actual = plan.weight[edge.index()].0;
            (actual, weight)
        }),
        [(1, 1), (2, 2), (2, 2), (2, 2), (3, 3)]
    );
    for expected in ["a1", "a0", "b0", "c0", "out"] {
        let edge = plan.find_work(&graph).unwrap();
        assert_eq!(output_path(&graph, edge), expected);
        plan.edge_finished(&graph, &runtime, edge, EdgeResult::Succeeded)
            .unwrap();
    }
    assert!(plan.find_work(&graph).is_none());
}

#[test]
fn ronin_plan_handles_deep_graphs_without_recursion() {
    const DEPTH: usize = 20_000;

    let mut graph = Graph::default();
    let root = mkenv(&mut graph, None);
    let mut input = mknode(&mut graph, xasprintf(format_args!("source")));
    let mut target = input;
    for index in 0..DEPTH {
        let output = mknode(&mut graph, xasprintf(format_args!("node/{index}")));
        let edge = mkedge(&mut graph, root);
        {
            let edge = graph.edge_mut(edge);
            edge.input.push(input);
            edge.set_input_partitions(1, 1);
            edge.out.push(output);
            edge.set_explicit_output_count(1);
        }
        nodeuse(&mut graph, input, edge);
        graph.node_mut(output).gen = Some(edge);
        input = output;
        target = output;
    }

    let mut runtime = RuntimeState::new(&graph);
    for node in graph.node_ids() {
        if graph.node(node).gen.is_some() {
            runtime.node_mut(node).set_dirty(true);
        }
    }
    let mut plan = Plan::default();
    plan.add_target(&graph, &runtime, target).unwrap();
    plan.prepare_queue(&graph);
    let mut scheduled = 0;
    while let Some(edge) = plan.find_work(&graph) {
        plan.edge_finished(&graph, &runtime, edge, EdgeResult::Succeeded)
            .unwrap();
        scheduled += 1;
    }
    assert_eq!(scheduled, DEPTH);
    assert!(!plan.more_to_do());
}

#[test]
fn ninja_build_no_work() {
    let (mut graph, directory) = build_fixture(
        "no-work",
        "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/in\n",
    );
    fs::write(directory.join("in"), "input").unwrap();
    fs::write(directory.join("out"), "output").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        assert!(builder.already_up_to_date());
        builder.build().unwrap();
        assert!(builder.commands_ran.is_empty());
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_one_step() {
    let (mut graph, directory) = build_fixture(
        "one-step",
        "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/in\n",
    );
    fs::write(directory.join("in"), "hello").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        assert!(!builder.already_up_to_date());
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "hello");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_two_step() {
    let (mut graph, directory) = build_fixture(
            "two-step",
            "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/mid\nbuild $dir/mid: copy $dir/in\n",
        );
    fs::write(directory.join("in"), "hello").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
        assert!(builder.commands_ran[0].contains_str("/mid"));
        assert!(builder.commands_ran[1].contains_str("/out"));
    }
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "hello");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_two_outputs() {
    let (mut graph, directory) = build_fixture(
        "two-outputs",
        "rule touch\n  command = touch $out\nbuild $dir/out1 $dir/out2: touch $dir/in\n",
    );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out1").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    assert!(directory.join("out1").exists());
    assert!(directory.join("out2").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_implicit_output() {
    let (mut graph, directory) = build_fixture(
            "implicit-output",
            "rule touch\n  command = touch $out $dir/out.imp\nbuild $dir/out | $dir/out.imp: touch $dir/in\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out.imp").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    assert!(directory.join("out").exists());
    assert!(directory.join("out.imp").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_multi_output_input_rebuilds_consistently() {
    let (mut graph, directory) = build_fixture(
            "multi-output-input",
            "rule touch\n  command = touch $out\nbuild $dir/in1 $dir/otherfile: touch $dir/in\nbuild $dir/out: touch $dir/in | $dir/in1\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in1"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
    }
    assert!(directory.join("otherfile").exists());
    assert!(directory.join("out").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_chain_is_clean_on_second_scan() {
    let (mut graph, directory) = build_fixture(
            "chain",
            "rule copy\n  command = cp $in $out\nbuild $dir/c2: copy $dir/c1\nbuild $dir/c3: copy $dir/c2\nbuild $dir/c4: copy $dir/c3\nbuild $dir/c5: copy $dir/c4\n",
        );
    fs::write(directory.join("c1"), "chain").unwrap();
    let target = directory.join("c5").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 4);
    }
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        assert!(builder.already_up_to_date());
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_missing_input_and_target() {
    let (mut graph, directory) = build_fixture(
        "missing",
        "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/missing\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    assert!(builder.add_target(&target).is_err());
    assert!(builder.add_target("not-in-the-graph").is_err());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_stat_failure_aborts_target_scan() {
    let output_name = "o".repeat(512);
    let (mut graph, directory) = build_fixture(
        "stat-failure",
        &format!("rule touch\n  command = touch $out\nbuild $dir/{output_name}: touch $dir/in\n"),
    );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join(output_name).to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    assert!(builder.add_target(&target).is_err());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_makes_output_directories() {
    let (mut graph, directory) = build_fixture(
        "make-dirs",
        "rule copy\n  command = cp $in $out\nbuild $dir/subdir/deeper/out: copy $dir/in\n",
    );
    fs::write(directory.join("in"), "hello").unwrap();
    let target = directory
        .join("subdir/deeper/out")
        .to_string_lossy()
        .into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    assert_eq!(
        fs::read_to_string(directory.join("subdir/deeper/out")).unwrap(),
        "hello"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_failure_is_reported() {
    let (mut graph, directory) = build_fixture(
        "failure",
        "rule fail\n  command = echo failure >&2; false\nbuild $dir/out: fail $dir/in\n",
    );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        assert!(builder.build().is_err());
        assert!(String::from_utf8_lossy(&builder.command_output).contains("failure"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
// [spec:ronin:req:compat.command-runtime/test]
fn ronin_build_streams_description_status_and_buffered_output_in_order() {
    let (mut graph, directory) = build_fixture(
        "streamed-output",
        "rule emit\n  command = printf child; touch $out\n  description = describe output\nbuild $dir/out: emit\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut output = Vec::new();
    {
        let mut builder = Builder::with_output(&mut graph, BuildOptions::default(), &mut output);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert!(builder.build_output.is_empty());
        assert!(builder.command_output.is_empty());
    }
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "[1/1] describe output\nchild"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[derive(Default)]
struct FlushCountingWriter {
    bytes: Vec<u8>,
    flushes: usize,
}

impl std::io::Write for FlushCountingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[test]
// [spec:ronin:req:runtime.process-supervisor-scalability/test]
fn ronin_build_flushes_each_completed_output_batch_once() {
    let (mut graph, directory) = build_fixture(
        "batched-output-flush",
        "rule emit\n  command = printf child; touch $out\n  description = emitting\nbuild $dir/one: emit\nbuild $dir/two: emit\nbuild all: phony $dir/one $dir/two\n",
    );
    let mut output = FlushCountingWriter::default();
    {
        let mut builder = Builder::with_output(&mut graph, BuildOptions::default(), &mut output);
        builder.add_target(b"all").unwrap();
        builder.build().unwrap();
    }

    assert_eq!(output.flushes, 2);
    let output = String::from_utf8(output.bytes).unwrap();
    assert_eq!(output, "[1/2] emitting\nchild[2/2] emitting\nchild");
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
// [spec:ronin:req:runtime.process-supervisor-scalability/test]
fn ronin_console_build_flushes_its_status_batch_once() {
    let (mut graph, directory) = build_fixture(
        "console-output-flush",
        "rule emit\n  command = touch $out\n  description = console\n  pool = console\nbuild $dir/out: emit\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut output = FlushCountingWriter::default();
    {
        let mut builder = Builder::with_output(&mut graph, BuildOptions::default(), &mut output);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }

    assert_eq!(output.flushes, 1);
    assert_eq!(String::from_utf8(output.bytes).unwrap(), "[0/1] console\n");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ronin_command_cache_recomputes_after_explicit_binding_invalidation() {
    let (mut graph, directory) = build_fixture(
        "command-cache-invalidation",
        "rule emit\n  command = printf $value > $out\nbuild $dir/out: emit\n  value = first\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&target).unwrap();
    let output = nodeget(builder.graph, target.as_bytes()).unwrap();
    let edge = builder.graph.node(output).gen.unwrap();
    builder.refresh_command_hash(edge).unwrap();
    let first_hash = builder.runtime.edge(edge).command_hash();
    assert!(builder.command_cache[edge.index()]
        .as_ref()
        .unwrap()
        .command
        .contains_str("first"));

    let value_name = builder.graph.names_mut().intern(BStr::new("value"));
    builder
        .graph
        .edge_mut(edge)
        .bindings
        .insert(value_name, BString::from("second"));
    builder.invalidate_command(edge);
    builder.refresh_command_hash(edge).unwrap();
    assert_ne!(builder.runtime.edge(edge).command_hash(), first_hash);
    assert!(builder.command_cache[edge.index()]
        .as_ref()
        .unwrap()
        .command
        .contains_str("second"));
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ronin_build_verbose_status_uses_the_expanded_command() {
    let (mut graph, directory) = build_fixture(
        "verbose-status",
        "rule emit\n  command = touch $out\n  description = hidden description\nbuild $dir/out: emit\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut output = Vec::new();
    {
        let options = BuildOptions {
            verbose: true,
            ..BuildOptions::default()
        };
        let mut builder = Builder::with_output(&mut graph, options, &mut output);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    let output = String::from_utf8(output).unwrap();
    assert!(output.starts_with("[1/1] touch "));
    assert!(!output.contains("hidden description"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ronin_build_explain_reports_the_dirty_reason_before_status() {
    let (mut graph, directory) = build_fixture(
        "explain-status",
        "rule emit\n  command = touch $out\n  description = create output\nbuild $dir/out: emit\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut output = Vec::new();
    {
        let options = BuildOptions {
            explain: true,
            ..BuildOptions::default()
        };
        let mut builder = Builder::with_output(&mut graph, options, &mut output);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    let output = String::from_utf8(output).unwrap();
    let explanation = output.find("ronin explain: output ").unwrap();
    let status = output.find("[1/1] create output").unwrap();
    assert!(explanation < status);
    assert!(output.contains("doesn't exist"), "{output:?}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ronin_build_prints_failure_context_before_child_output() {
    let (mut graph, directory) = build_fixture(
        "failure-output-order",
        "rule fail\n  command = printf child; false\n  description = failing action\nbuild $dir/out: fail\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut output = Vec::new();
    {
        let mut builder = Builder::with_output(&mut graph, BuildOptions::default(), &mut output);
        builder.add_target(&target).unwrap();
        assert!(builder.build().is_err());
    }
    let output = String::from_utf8(output).unwrap();
    let status = output.find("[1/1] failing action\n").unwrap();
    let failure = output.find("FAILED: [code=1]").unwrap();
    let command = output.find("printf child; false\n").unwrap();
    let child = output.rfind("child").unwrap();
    assert!(status < failure && failure < command && command < child);
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn ninja_build_interrupted_command_cleans_only_changed_outputs() {
    let manifest = "rule interrupt\n  command = kill -INT $$$$\nrule touch_interrupt\n  command = touch $out; kill -INT $$$$\nbuild $dir/out1: interrupt $dir/in1\nbuild $dir/out2: touch_interrupt $dir/in2\n";
    let (mut graph, directory) = build_fixture("interrupt-cleanup", manifest);
    fs::write(directory.join("out1"), "keep").unwrap();
    fs::write(directory.join("out2"), "remove").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in1"), "").unwrap();
    fs::write(directory.join("in2"), "").unwrap();
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&out1).unwrap();
        assert_eq!(
            builder.build().unwrap_err().to_string(),
            "build stopped: interrupted by user."
        );
    }
    assert!(directory.join("out1").exists());

    let mut graph = parse_fixture(&directory);
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&out2).unwrap();
        assert_eq!(
            builder.build().unwrap_err().to_string(),
            "build stopped: interrupted by user."
        );
    }
    assert!(!directory.join("out2").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_response_file_success() {
    let (mut graph, directory) = build_fixture(
            "rsp-success",
            "rule rsp\n  command = cat $rspfile > $out\n  rspfile = $dir/args.rsp\n  rspfile_content = response contents\nbuild $dir/out: rsp $dir/in\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    assert_eq!(
        fs::read_to_string(directory.join("out")).unwrap(),
        "response contents"
    );
    assert!(!directory.join("args.rsp").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_response_file_failure_cleanup() {
    let (mut graph, directory) = build_fixture(
            "rsp-failure",
            "rule rsp\n  command = false\n  rspfile = $dir/args.rsp\n  rspfile_content = response contents\nbuild $dir/out: rsp $dir/in\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        assert!(builder.build().is_err());
    }
    assert!(!directory.join("args.rsp").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_phony_dependency() {
    let (mut graph, directory) = build_fixture(
            "phony",
            "rule copy\n  command = cp $in $out\nbuild $dir/real: copy $dir/in\nbuild $dir/alias: phony $dir/real\n",
        );
    fs::write(directory.join("in"), "hello").unwrap();
    let target = directory.join("alias").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    assert_eq!(fs::read_to_string(directory.join("real")).unwrap(), "hello");
    assert!(!directory.join("alias").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_phony_no_work() {
    let (mut graph, directory) = build_fixture(
            "phony-no-work",
            "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/in\nbuild $dir/all: phony $dir/out\n",
        );
    fs::write(directory.join("in"), "hello").unwrap();
    fs::write(directory.join("out"), "hello").unwrap();
    let target = directory.join("all").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&target).unwrap();
    assert!(builder.already_up_to_date());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_phony_self_reference_has_no_work() {
    let (mut graph, directory) =
        build_fixture("phony-self-reference", "build $dir/a: phony $dir/a\n");
    let target = directory.join("a").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&target).unwrap();
    assert!(builder.already_up_to_date());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_phony_use_case_1() {
    assert_phony_use_case(1);
}

#[test]
fn ninja_build_phony_use_case_2() {
    assert_phony_use_case(2);
}

#[test]
fn ninja_build_phony_use_case_3() {
    assert_phony_use_case(3);
}

#[test]
fn ninja_build_phony_use_case_4() {
    assert_phony_use_case(4);
}

#[test]
fn ninja_build_phony_use_case_5() {
    assert_phony_use_case(5);
}

#[test]
fn ninja_build_phony_use_case_6() {
    assert_phony_use_case(6);
}

#[test]
fn ninja_build_missing_depfile_forces_rebuild() {
    let (mut graph, directory) = build_fixture(
        "depfile-missing",
        "rule copy\n  command = cp $in $out\n  depfile = $out.d\nbuild $dir/out: copy $dir/in\n",
    );
    fs::write(directory.join("in"), "hello").unwrap();
    fs::write(directory.join("out"), "old").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        assert!(!builder.already_up_to_date());
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "hello");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_loads_existing_depfile() {
    let (mut graph, directory) = build_fixture(
        "depfile-existing",
        "rule copy\n  command = cp $in $out\n  depfile = $out.d\nbuild $dir/out: copy $dir/in\n",
    );
    fs::write(directory.join("in"), "hello").unwrap();
    fs::write(directory.join("header1"), "").unwrap();
    fs::write(directory.join("header2"), "").unwrap();
    fs::write(directory.join("out"), "hello").unwrap();
    fs::write(
        directory.join("out.d"),
        format!(
            "{}: {} {}\n",
            directory.join("out").display(),
            directory.join("header1").display(),
            directory.join("header2").display()
        ),
    )
    .unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        assert!(builder.already_up_to_date());
    }
    let output = nodeget(&graph, target.as_bytes()).unwrap();
    let edge = graph.node(output).gen.unwrap();
    assert_eq!(graph.edge(edge).input.len(), 3);
    let command = crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::Raw).unwrap();
    assert_eq!(
        String::from_utf8_lossy(command.as_bytes()),
        format!("cp {} {}", directory.join("in").display(), target)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_rejects_invalid_depfile() {
    let (mut graph, directory) = build_fixture(
        "depfile-invalid",
        "rule copy\n  command = cp $in $out\n  depfile = $out.d\nbuild $dir/out: copy $dir/in\n",
    );
    fs::write(directory.join("in"), "hello").unwrap();
    fs::write(directory.join("out"), "hello").unwrap();
    fs::write(directory.join("out.d"), "random text\n").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    assert!(builder.add_target(&target).is_err());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_wrong_output_in_depfile_forces_rebuild() {
    let (mut graph, directory) = build_fixture(
            "depfile-wrong-output",
            "rule copy\n  command = cp $in $out; printf '$out: $dir/header\\n' > $out.d\n  depfile = $out.d\nbuild $dir/out: copy $dir/in\n",
        );
    fs::write(directory.join("in"), "new").unwrap();
    fs::write(directory.join("header"), "").unwrap();
    fs::write(directory.join("out"), "old").unwrap();
    fs::write(
        directory.join("out.d"),
        format!(
            "{}: {}\n",
            directory.join("wrong").display(),
            directory.join("header").display()
        ),
    )
    .unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        assert!(!builder.already_up_to_date());
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "new");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_depfile_rejects_undeclared_extra_output() {
    let (mut graph, directory) = build_fixture(
        "depfile-extra-output",
        "rule copy\n  command = cp $in $out\n  depfile = $out.d\nbuild $dir/out: copy $dir/in\n",
    );
    fs::write(directory.join("in"), "source").unwrap();
    fs::write(directory.join("out"), "source").unwrap();
    fs::write(directory.join("header"), "").unwrap();
    fs::write(
        directory.join("out.d"),
        format!(
            "{}: {}\n{}: {}\n",
            directory.join("out").display(),
            directory.join("header").display(),
            directory.join("extra").display(),
            directory.join("header").display()
        ),
    )
    .unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    let error = builder.add_target(&target).unwrap_err();
    assert!(error.to_string().contains("no such output was declared"));
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_failed_depfile_parse_after_command() {
    let (mut graph, directory) = build_fixture(
            "depfile-command-parse-error",
            "rule copy\n  command = cp $in $out\n  depfile = $out.d\n  deps = gcc\nbuild $dir/out: copy $dir/in\n",
        );
    fs::write(directory.join("in"), "source").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&target).unwrap();
    fs::write(directory.join("out.d"), "AAA BBB\n").unwrap();
    assert!(builder.build().is_err());
    assert_eq!(builder.commands_ran.len(), 1);
    assert!(String::from_utf8_lossy(&builder.build_output).contains("FAILED: [code=1]"));
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_gcc_deps_without_depfile_errors_after_command() {
    let (mut graph, directory) = build_fixture(
        "gcc-deps-no-depfile",
        "rule cc\n  command = true\n  deps = gcc\nbuild $dir/out: cc\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&target).unwrap();
    let error = builder.build().unwrap_err();
    assert!(error.to_string().contains("dependency file is missing"));
    assert_eq!(builder.commands_ran, ["true"]);
    assert!(
        String::from_utf8_lossy(&builder.build_output).contains("FAILED: [code=1]"),
        "{:?}",
        String::from_utf8_lossy(&builder.build_output)
    );
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_records_generated_depfile() {
    let (mut graph, directory) = build_fixture(
            "depfile-generated",
            "rule copy\n  command = cp $in $out; printf '$out: $dir/header\\n' > $out.d\n  depfile = $out.d\nbuild $dir/out: copy $dir/in\n",
        );
    fs::write(directory.join("in"), "hello").unwrap();
    fs::write(directory.join("header"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    assert!(directory.join("out.d").exists());
    let mut graph = parse_fixture(&directory);
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        assert!(builder.already_up_to_date());
    }
    let output = nodeget(&graph, target.as_bytes()).unwrap();
    let edge = graph.node(output).gen.unwrap();
    assert_eq!(graph.edge(edge).input.len(), 2);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_newer_order_only_input_does_not_rebuild() {
    let (mut graph, directory) = build_fixture(
        "order-only-clean",
        "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/in || $dir/order\n",
    );
    fs::write(directory.join("in"), "hello").unwrap();
    fs::write(directory.join("out"), "hello").unwrap();
    fs::write(directory.join("order"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&target).unwrap();
    assert!(builder.already_up_to_date());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_rebuilds_dirty_order_only_generator() {
    let (mut graph, directory) = build_fixture(
            "order-only-generated",
            "rule copy\n  command = cp $in $out\nbuild $dir/order: copy $dir/order.in\nbuild $dir/out: copy $dir/in || $dir/order\n",
        );
    fs::write(directory.join("order.in"), "order").unwrap();
    fs::write(directory.join("in"), "hello").unwrap();
    fs::write(directory.join("out"), "hello").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        assert!(!builder.already_up_to_date());
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
        assert!(builder.commands_ran[0].contains_str("order.in"));
    }
    assert!(directory.join("order").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_encounter_ready_twice() {
    let (mut graph, directory) = build_fixture(
            "encounter-ready-twice",
            "rule touch\n  command = touch $out\nbuild $dir/c: touch\nbuild $dir/b: touch || $dir/c\nbuild $dir/a: touch | $dir/b || $dir/c\n",
        );
    fs::write(directory.join("b"), "").unwrap();
    let target = directory.join("a").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
        assert!(builder.commands_ran[0].contains_str("/c"));
        assert!(builder.commands_ran[1].contains_str("/a"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_phony_with_no_inputs_respects_order_only() {
    let (mut graph, directory) = build_fixture(
            "phony-empty",
            "rule touch\n  command = touch $out\nbuild $dir/nonexistent: phony\nbuild $dir/out1: touch || $dir/nonexistent\nbuild $dir/out2: touch $dir/nonexistent\n",
        );
    fs::write(directory.join("out1"), "").unwrap();
    fs::write(directory.join("out2"), "").unwrap();
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&out1).unwrap();
        assert!(builder.already_up_to_date());
    }
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&out2).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_swallow_failures() {
    let (mut graph, directory) = build_fixture(
        "swallow-failures",
        "rule fail\n  command = false\nbuild $dir/out1: fail\nbuild $dir/out2: fail\n",
    );
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let options = BuildOptions {
        maxfail: 2,
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&out1).unwrap();
    builder.add_target(&out2).unwrap();
    assert!(builder.build().is_err());
    assert_eq!(builder.commands_ran.len(), 2);
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_swallow_failures_limit() {
    let (mut graph, directory) = build_fixture(
        "failure-limit",
        "rule fail\n  command = false\nbuild $dir/out1: fail\nbuild $dir/out2: fail\n",
    );
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&out1).unwrap();
    builder.add_target(&out2).unwrap();
    assert!(builder.build().is_err());
    assert_eq!(builder.commands_ran.len(), 1);
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_swallow_failures_releases_pool() {
    let (mut graph, directory) = build_fixture(
            "failure-pool",
            "pool serial\n  depth = 1\nrule fail\n  command = false\n  pool = serial\nbuild $dir/out1: fail\nbuild $dir/out2: fail\n",
        );
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let options = BuildOptions {
        maxfail: 2,
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&out1).unwrap();
    builder.add_target(&out2).unwrap();
    assert!(builder.build().is_err());
    assert_eq!(builder.commands_ran.len(), 2);
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_runs_independent_edges_in_parallel() {
    let (mut graph, directory) = build_fixture(
            "parallel-edges",
            "rule sync\n  command = touch $out.started; i=0; while [ ! -e $other.started ] && [ $$i -lt 100 ]; do sleep 0.01; i=$$((i + 1)); done; test -e $other.started; touch $out\nbuild $dir/out1: sync\n  other = $dir/out2\nbuild $dir/out2: sync\n  other = $dir/out1\n",
        );
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let options = BuildOptions {
        jobs: JobLimit::fixed(2).unwrap(),
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&out1).unwrap();
    builder.add_target(&out2).unwrap();
    builder.build().unwrap();
    assert_eq!(builder.commands_ran.len(), 2);
    drop(builder);
    assert!(directory.join("out1").exists());
    assert!(directory.join("out2").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
// [spec:ronin:req:runtime.process-supervisor-scalability/test]
fn ronin_build_acquires_and_releases_jobserver_tokens() {
    let (mut graph, directory) = build_fixture(
        "jobserver-tokens",
        "rule sync\n  command = touch $out.started; i=0; while [ ! -e $other.started ] && [ $$i -lt 100 ]; do sleep 0.01; i=$$((i + 1)); done; test -e $other.started; touch $out\nbuild $dir/out1: sync\n  other = $dir/out2\nbuild $dir/out2: sync\n  other = $dir/out1\n",
    );

    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let options = BuildOptions {
        jobs: JobLimit::Unlimited,
        // GNU Make contributes one implicit slot; one transport token permits
        // the second command to run concurrently.
        jobserver: Some(jobserver::Client::new(1).unwrap()),
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&out1).unwrap();
    builder.add_target(&out2).unwrap();
    builder.build().unwrap();
    assert_eq!(builder.commands_ran.len(), 2);
    drop(builder);
    assert!(directory.join("out1").exists());
    assert!(directory.join("out2").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
// [spec:ronin:req:compat.scheduling/test]
fn ronin_scheduler_releases_dependents_on_each_completion() {
    let (mut graph, directory) = build_fixture(
        "completion-driven",
        "rule fast\n  command = sleep 0.03; touch $out\nrule slow\n  command = sleep 0.30; touch $out\nrule after\n  command = test ! -e $dir/slow && touch $out\nbuild $dir/fast: fast\nbuild $dir/slow: slow\nbuild $dir/after: after $dir/fast\n",
    );
    let after = directory.join("after").to_string_lossy().into_owned();
    let slow = directory.join("slow").to_string_lossy().into_owned();
    let options = BuildOptions {
        jobs: JobLimit::fixed(2).unwrap(),
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&after).unwrap();
    builder.add_target(&slow).unwrap();
    builder.build().unwrap();
    assert_eq!(builder.commands_ran.len(), 3);
    drop(builder);
    assert!(directory.join("after").exists());
    assert!(directory.join("slow").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_pool_depth_serializes_parallel_commands() {
    let (mut graph, directory) = build_fixture(
            "parallel-pool-depth",
            "pool serial\n  depth = 1\nrule locked\n  command = mkdir $dir/lock; sleep 0.02; rmdir $dir/lock; touch $out\n  pool = serial\nbuild $dir/out1: locked\nbuild $dir/out2: locked\n",
        );
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let options = BuildOptions {
        jobs: JobLimit::fixed(2).unwrap(),
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&out1).unwrap();
    builder.add_target(&out2).unwrap();
    builder.build().unwrap();
    assert_eq!(builder.commands_ran.len(), 2);
    drop(builder);
    assert!(directory.join("out1").exists());
    assert!(directory.join("out2").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
// [spec:ronin:req:runtime.process-supervisor-scalability/test]
fn ninja_build_console_pool_is_exclusive() {
    let (mut graph, directory) = build_fixture(
            "parallel-console-exclusive",
            "rule regular\n  command = mkdir $dir/lock; sleep 0.02; rmdir $dir/lock; touch $out\nrule console\n  command = mkdir $dir/lock; sleep 0.02; rmdir $dir/lock; touch $out\n  pool = console\nbuild $dir/out1: regular\nbuild $dir/out2: console\n",
        );
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let options = BuildOptions {
        jobs: JobLimit::fixed(2).unwrap(),
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&out1).unwrap();
    builder.add_target(&out2).unwrap();
    builder.build().unwrap();
    assert_eq!(builder.commands_ran.len(), 2);
    drop(builder);
    assert!(directory.join("out1").exists());
    assert!(directory.join("out2").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_dry_run_shows_all_commands() {
    let (mut graph, directory) = build_fixture(
            "dry-run",
            "rule touch\n  command = touch $out\nbuild $dir/mid: touch $dir/in\nbuild $dir/out: touch $dir/mid\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let options = BuildOptions {
        dryrun: true,
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&target).unwrap();
    builder.build().unwrap();
    assert_eq!(builder.commands_ran.len(), 2);
    assert!(!directory.join("mid").exists());
    assert!(!directory.join("out").exists());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_ready_dyndep_implicit_connection() {
    let (mut graph, directory) = build_fixture(
            "dyndep-ready",
            "rule touch\n  command = touch $out\nbuild $dir/out1: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\nbuild $dir/out2: touch $dir/in\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    fs::write(
        directory.join("dd"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep | {}\n",
            directory.join("out1").display(),
            directory.join("out2").display()
        ),
    )
    .unwrap();
    let target = directory.join("out1").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
        assert!(builder.commands_ran[0].contains_str("out2"));
        assert!(builder.commands_ran[1].contains_str("out1"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_ready_dyndep_syntax_error() {
    let (mut graph, directory) = build_fixture(
            "dyndep-syntax",
            "rule touch\n  command = touch $out\nbuild $dir/out: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    fs::write(directory.join("dd"), "not a dyndep file\n").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    assert!(builder.add_target(&target).is_err());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_ready_dyndep_discovers_cycle() {
    let (mut graph, directory) = build_fixture(
            "dyndep-ready-cycle",
            "rule touch\n  command = touch $out\nbuild $dir/out: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\nbuild $dir/in: touch $dir/circ\n",
        );
    fs::write(
        directory.join("dd"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep\n",
            directory.join("out").display(),
            directory.join("circ").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("out"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    assert!(builder.add_target(&target).is_err());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_dyndep_missing_and_no_rule() {
    let (mut graph, directory) = build_fixture(
            "dyndep-missing",
            "rule touch\n  command = touch $out\nbuild $dir/out: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    assert!(builder.add_target(&target).is_err());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_dry_run_with_dyndep() {
    let (mut graph, directory) = build_fixture(
            "dry-dyndep",
            "rule touch\n  command = touch $out\nbuild $dir/out1: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\nbuild $dir/out2: touch $dir/in\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    fs::write(
        directory.join("dd"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep | {}\n",
            directory.join("out1").display(),
            directory.join("out2").display()
        ),
    )
    .unwrap();
    let target = directory.join("out1").to_string_lossy().into_owned();
    let options = BuildOptions {
        dryrun: true,
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&target).unwrap();
    builder.build().unwrap();
    assert_eq!(builder.commands_ran.len(), 2);
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_validation() {
    let (mut graph, directory) = build_fixture(
            "validation",
            "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/in |@ $dir/validate\nbuild $dir/validate: copy $dir/in2\n",
        );
    fs::write(directory.join("in"), "out").unwrap();
    fs::write(directory.join("in2"), "validation").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
    }
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "out");
    assert_eq!(
        fs::read_to_string(directory.join("validate")).unwrap(),
        "validation"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_validation_depends_on_output() {
    let (mut graph, directory) = build_fixture(
            "validation-output",
            "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/in |@ $dir/validate\nbuild $dir/validate: copy $dir/out\n",
        );
    fs::write(directory.join("in"), "out").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
        assert!(builder.commands_ran[0].contains_str("/in "));
        assert!(builder.commands_ran[1].contains_str("/out "));
    }
    assert_eq!(
        fs::read_to_string(directory.join("validate")).unwrap(),
        "out"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_circular_validations() {
    let (mut graph, directory) = build_fixture(
            "validation-circular",
            "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/in |@ $dir/out2\nbuild $dir/out2: copy $dir/in2 |@ $dir/out\n",
        );
    fs::write(directory.join("in"), "out").unwrap();
    fs::write(directory.join("in2"), "out2").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
    }
    assert!(directory.join("out").exists());
    assert!(directory.join("out2").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_validation_with_dependency_cycle() {
    let (mut graph, directory) = build_fixture(
            "validation-dependency-cycle",
            "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/in |@ $dir/validate\nbuild $dir/validate: copy $dir/validate_in | $dir/out\nbuild $dir/validate_in: copy $dir/validate\n",
        );
    fs::write(directory.join("in"), "out").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    assert!(builder.add_target(&target).is_err());
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_rebuilds_output_not_in_log() {
    let (mut graph, directory) = build_fixture(
        "log-not-present",
        "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/in\n",
    );
    fs::write(directory.join("in"), "hello").unwrap();
    fs::write(directory.join("out"), "hello").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    assert!(crate::log::logentry(&log, &target).is_some());

    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        assert!(builder.already_up_to_date());
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_generator_rebuilds_for_newer_implicit_input() {
    let (mut graph, directory) = build_fixture(
            "log-generator-implicit-newer",
            "rule generate\n  command = touch $out\n  generator = 1\nbuild $dir/out: generate | $dir/in\n",
        );
    fs::write(directory.join("out"), "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        assert!(!builder.already_up_to_date());
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_generator_implicit_inputs_are_clean_after_each_rebuild() {
    let manifest = "rule generate\n  command = touch $dir/in1; sleep 0.02; touch $out\n  generator = 1\nbuild $dir/out: generate | $dir/in1 $dir/in2\n";
    let (mut graph, directory) = build_fixture("log-generator-implicit-cycle", manifest);
    fs::write(directory.join("in1"), "").unwrap();
    fs::write(directory.join("out"), "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in2"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    log.finish().unwrap();

    let mut graph = parse_fixture(&directory);
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        assert!(builder.already_up_to_date());
    }
    log.finish().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in1"), "changed").unwrap();
    let mut graph = parse_fixture(&directory);
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    log.finish().unwrap();

    let mut graph = parse_fixture(&directory);
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        assert!(builder.already_up_to_date());
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_does_not_record_failure() {
    let (mut graph, directory) = build_fixture(
        "log-failure",
        "rule fail\n  command = false\nbuild $dir/out: fail\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        assert!(builder.build().is_err());
    }
    assert!(crate::log::logentry(&log, &target).is_none());
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_rspfile_content_change_rebuilds() {
    let (mut graph, directory) = build_fixture(
            "log-rsp-change",
            "rule rsp\n  command = cat $rspfile > $out\n  rspfile = $dir/args.rsp\n  rspfile_content = first\nbuild $dir/out: rsp\n",
        );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    log.finish().unwrap();
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "first");

    fs::write(
            directory.join("build.ninja"),
            format!(
                "rule rsp\n  command = cat $rspfile > $out\n  rspfile = {}/args.rsp\n  rspfile_content = second\nbuild {}/out: rsp\n",
                directory.display(),
                directory.display()
            ),
        )
        .unwrap();
    let mut graph = parse_fixture(&directory);
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "second");
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_generator_command_change_is_ignored() {
    let (mut graph, directory) = build_fixture(
        "log-generator",
        "rule generate\n  command = touch $out\n  generator = 1\nbuild $dir/out: generate\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    log.finish().unwrap();

    fs::write(
            directory.join("build.ninja"),
            format!(
                "rule generate\n  command = echo changed > $out\n  generator = 1\nbuild {}/out: generate\n",
                directory.display()
            ),
        )
        .unwrap();
    let mut graph = parse_fixture(&directory);
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
    builder.add_target(&target).unwrap();
    assert!(builder.already_up_to_date());
    drop(builder);
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_restat_prunes_downstream_edge() {
    let (mut graph, directory) = build_fixture(
            "log-restat",
            "rule steady\n  command = true\n  restat = 1\nrule touch\n  command = touch $out\nbuild $dir/mid: steady $dir/in\nbuild $dir/out: touch $dir/mid\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    fs::write(directory.join("mid"), "").unwrap();
    fs::write(directory.join("out"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
    }

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in"), "changed").unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran, ["true"]);
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_restat_does_not_hide_missing_sibling_output() {
    let (mut graph, directory) = build_fixture(
            "log-restat-sibling",
            "rule steady\n  command = true\n  restat = 1\nrule touch\n  command = touch $out\nbuild $dir/mid: steady $dir/in\nbuild $dir/out1 $dir/out2: touch $dir/mid\nbuild $dir/final: touch $dir/out1\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    fs::write(directory.join("mid"), "").unwrap();
    fs::write(directory.join("out1"), "").unwrap();
    fs::write(directory.join("out2"), "").unwrap();
    fs::write(directory.join("final"), "").unwrap();
    let target = directory.join("final").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }

    std::thread::sleep(std::time::Duration::from_millis(2));
    fs::write(directory.join("in"), "changed").unwrap();
    fs::remove_file(directory.join("out2")).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_rebuild_with_no_inputs() {
    let (mut graph, directory) = build_fixture(
            "log-no-inputs",
            "rule touch\n  command = touch $out\nbuild $dir/out1: touch\nbuild $dir/out2: touch $dir/in\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&out1).unwrap();
        builder.add_target(&out2).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
    }

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in"), "changed").unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&out1).unwrap();
        builder.add_target(&out2).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
        assert!(builder.commands_ran[0].contains_str("out2"));
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_rebuilds_after_failed_output_update() {
    let (mut graph, directory) = build_fixture(
            "log-rebuild-failure",
            "rule conditional\n  command = touch $out; test ! -e $dir/fail\nbuild $dir/out: conditional $dir/in\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in"), "changed").unwrap();
    fs::write(directory.join("fail"), "").unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        assert!(builder.build().is_err());
    }

    fs::remove_file(directory.join("fail")).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        assert!(!builder.already_up_to_date());
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_detects_input_changed_during_command() {
    let manifest = "rule race\n  command = cp $in $out; if [ ! -e $dir/raced ]; then sleep 0.05; touch $in; touch $dir/raced; fi\nbuild $dir/out: race $dir/in\n";
    let (mut graph, directory) = build_fixture("log-input-mtime-race", manifest);
    fs::write(directory.join("in"), "source").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    let first_recorded_mtime = crate::log::logentry(&log, &target).unwrap().mtime;
    assert!(
        RealDiskInterface::default()
            .stat(&directory.join("in"))
            .unwrap()
            > first_recorded_mtime
    );
    log.finish().unwrap();

    let mut graph = parse_fixture(&directory);
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        assert!(!builder.already_up_to_date());
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    log.finish().unwrap();

    let mut graph = parse_fixture(&directory);
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        assert!(builder.already_up_to_date());
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_restat_missing_output_prunes_dependent() {
    let manifest = "rule steady\n  command = true\n  restat = 1\nrule touch\n  command = touch $out\nbuild $dir/mid: steady $dir/in\nbuild $dir/out: touch $dir/mid\n";
    let (mut graph, directory) = build_fixture("log-restat-missing-output", manifest);
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
    }
    log.finish().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in"), "changed").unwrap();
    let mut graph = parse_fixture(&directory);
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran, ["true"]);
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_restat_missing_discovered_input_prunes_dependent() {
    let manifest = "rule steady\n  command = true\n  depfile = $dir/mid.d\n  restat = 1\nrule touch\n  command = touch $out\nbuild $dir/mid: steady $dir/in\nbuild $dir/out: touch $dir/mid\n";
    let (mut graph, directory) = build_fixture("log-restat-missing-input", manifest);
    fs::write(directory.join("in"), "").unwrap();
    fs::write(directory.join("mid"), "").unwrap();
    fs::write(directory.join("discovered"), "").unwrap();
    fs::write(
        directory.join("mid.d"),
        format!(
            "{}: {}\n",
            directory.join("mid").display(),
            directory.join("discovered").display()
        ),
    )
    .unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
    }
    log.finish().unwrap();

    fs::remove_file(directory.join("discovered")).unwrap();
    let mut graph = parse_fixture(&directory);
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran, ["true"]);
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_generated_plain_depfile_mtime() {
    let manifest = "rule generate\n  command = touch $out; printf '$out: $dir/header\\n' > $out.d\n  depfile = $out.d\nbuild $dir/out: generate\n";
    let (mut graph, directory) = build_fixture("log-plain-depfile", manifest);
    fs::write(directory.join("header"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    log.finish().unwrap();
    assert!(directory.join("out.d").exists());

    let mut graph = parse_fixture(&directory);
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        assert!(builder.already_up_to_date());
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_log_dyndep_discovers_restat() {
    let (mut graph, directory) = build_fixture(
            "log-dyndep-restat",
            "rule steady\n  command = true\nrule copy\n  command = cp $in $out\nbuild $dir/dd: copy $dir/dd-in\nbuild $dir/out1: steady $dir/in || $dir/dd\n  dyndep = $dir/dd\nbuild $dir/out2: copy $dir/out1\n",
        );
    fs::write(directory.join("out1"), "").unwrap();
    fs::write(directory.join("out2"), "").unwrap();
    fs::write(directory.join("in"), "").unwrap();
    fs::write(
        directory.join("dd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep\n  restat = 1\n",
            directory.join("out1").display()
        ),
    )
    .unwrap();
    let target = directory.join("out2").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
    }

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in"), "changed").unwrap();
    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran, ["true"]);
    }
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_deps_log_straightforward() {
    let manifest = "rule cc\n  command = cp $in $out; printf '$out: $dir/header\\n' > $out.d\n  depfile = $out.d\n  deps = gcc\nbuild $dir/out: cc $dir/in\n";
    let (mut graph, directory) = build_fixture("deps-log-straightforward", manifest);
    fs::write(directory.join("in"), "hello").unwrap();
    fs::write(directory.join("header"), "header").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let deps_path = directory.join(".ninja_deps");
    {
        let mut deps_log = crate::deps::DepsLog::open(Some(&directory)).unwrap();
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
        assert!(!directory.join("out.d").exists());
        drop(builder);
        deps_log.finish().unwrap();
    }

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("header"), "changed").unwrap();
    let mut graph = parse_fixture(&directory);
    let (mut deps_log, warning) = crate::deps::depsloadlog(&deps_path, &mut graph).unwrap();
    assert!(warning.is_none());
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        assert!(!builder.already_up_to_date());
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    deps_log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_deps_log_obsolete_entry_forces_rebuild() {
    let manifest = "rule cc\n  command = cp $in $out; printf '$out:\\n' > $out.d\n  depfile = $out.d\n  deps = gcc\nbuild $dir/out: cc $dir/in\n";
    let (mut graph, directory) = build_fixture("deps-log-obsolete", manifest);
    fs::write(directory.join("in"), "first").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let deps_path = directory.join(".ninja_deps");
    let mut deps_log = crate::deps::DepsLog::open(Some(&directory)).unwrap();
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    deps_log.finish().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in"), "second").unwrap();
    fs::write(directory.join("out"), "second").unwrap();
    let mut graph = parse_fixture(&directory);
    let (mut deps_log, warning) = crate::deps::depsloadlog(&deps_path, &mut graph).unwrap();
    assert!(warning.is_none());
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        assert!(!builder.already_up_to_date());
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    deps_log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_deps_log_detects_discovered_input_changed_during_command() {
    let manifest = "rule cc\n  command = touch $out; printf '$out: $dir/header\\n' > $out.d; if [ ! -e $dir/raced ]; then sleep 0.05; touch $dir/header; touch $dir/raced; fi\n  depfile = $out.d\n  deps = gcc\nbuild $dir/out: cc\n";
    let (mut graph, directory) = build_fixture("deps-log-input-mtime-race", manifest);
    fs::write(directory.join("header"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let deps_path = directory.join(".ninja_deps");
    let mut build_log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    let mut deps_log = crate::deps::DepsLog::open(Some(&directory)).unwrap();
    {
        let mut builder = Builder::with_logs(
            &mut graph,
            BuildOptions::default(),
            &mut build_log,
            &mut deps_log,
        );
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    assert!(
        RealDiskInterface::default()
            .stat(&directory.join("header"))
            .unwrap()
            > crate::log::logentry(&build_log, &target).unwrap().mtime
    );
    build_log.finish().unwrap();
    deps_log.finish().unwrap();

    let mut graph = parse_fixture(&directory);
    let mut build_log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    let (mut deps_log, warning) = crate::deps::depsloadlog(&deps_path, &mut graph).unwrap();
    assert!(warning.is_none());
    {
        let mut builder = Builder::with_logs(
            &mut graph,
            BuildOptions::default(),
            &mut build_log,
            &mut deps_log,
        );
        builder.add_target(&target).unwrap();
        assert!(!builder.already_up_to_date());
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    build_log.finish().unwrap();
    deps_log.finish().unwrap();

    let mut graph = parse_fixture(&directory);
    let mut build_log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    let (mut deps_log, warning) = crate::deps::depsloadlog(&deps_path, &mut graph).unwrap();
    assert!(warning.is_none());
    {
        let mut builder = Builder::with_logs(
            &mut graph,
            BuildOptions::default(),
            &mut build_log,
            &mut deps_log,
        );
        builder.add_target(&target).unwrap();
        assert!(builder.already_up_to_date());
    }
    build_log.finish().unwrap();
    deps_log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_deps_are_ignored_in_dry_run() {
    let (mut graph, directory) = build_fixture(
            "deps-log-dry-run",
            "rule cc\n  command = cp $in $out\n  depfile = $out.d\n  deps = gcc\nbuild $dir/out: cc $dir/in\n",
        );
    fs::write(directory.join("out"), "old").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in"), "new").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let options = BuildOptions {
        dryrun: true,
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&target).unwrap();
    builder.build().unwrap();
    assert_eq!(builder.commands_ran.len(), 1);
    drop(builder);
    assert_eq!(fs::read_to_string(directory.join("out")).unwrap(), "old");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_deps_log_records_all_outputs() {
    let (mut graph, directory) = build_fixture(
            "deps-log-multiple-outputs",
            "rule cc\n  command = touch $out; printf '$dir/out1: $dir/header\\n' > $dir/out.d\n  depfile = $dir/out.d\n  deps = gcc\nbuild $dir/out1 $dir/out2: cc $dir/in\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    fs::write(directory.join("header"), "").unwrap();
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let mut deps_log = crate::deps::DepsLog::open(Some(&directory)).unwrap();
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&out2).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    let out1_node = nodeget(&graph, out1.as_bytes()).unwrap();
    let out2_node = nodeget(&graph, out2.as_bytes()).unwrap();
    assert_eq!(
        crate::deps::depsentry(&deps_log, out1_node)
            .unwrap()
            .deps
            .nodes
            .len(),
        1
    );
    assert_eq!(
        crate::deps::depsentry(&deps_log, out2_node)
            .unwrap()
            .deps
            .nodes
            .len(),
        1
    );
    deps_log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_deps_log_two_outputs_gcc_one_line() {
    assert_multi_output_deps_log(
        "deps-log-gcc-one-line",
        "$dir/out1 $dir/out2: $dir/in1 $dir/in2\n",
    );
}

#[test]
fn ninja_build_deps_log_two_outputs_gcc_line_per_input() {
    assert_multi_output_deps_log(
        "deps-log-gcc-line-per-input",
        "$dir/out1 $dir/out2: $dir/in1\n$dir/out1 $dir/out2: $dir/in2\n",
    );
}

#[test]
fn ninja_build_deps_log_two_outputs_gcc_line_per_output() {
    assert_multi_output_deps_log(
        "deps-log-gcc-line-per-output",
        "$dir/out1: $dir/in1 $dir/in2\n$dir/out2: $dir/in1 $dir/in2\n",
    );
}

#[test]
fn ninja_build_deps_log_two_outputs_gcc_only_main_output() {
    assert_multi_output_deps_log("deps-log-gcc-only-main", "$dir/out1: $dir/in1 $dir/in2\n");
}

#[test]
fn ninja_build_deps_log_two_outputs_gcc_only_secondary_output() {
    assert_multi_output_deps_log(
        "deps-log-gcc-only-secondary",
        "$dir/out2: $dir/in1 $dir/in2\n",
    );
}

#[test]
fn ninja_build_deps_log_msvc_records_all_outputs() {
    let (mut graph, directory) = build_fixture(
            "deps-log-msvc-multiple-outputs",
            "rule cc\n  command = printf 'using $dir/in\\n'; touch $out\n  deps = msvc\n  msvc_deps_prefix = using\nbuild $dir/out1 $dir/out2: cc $dir/in\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let mut deps_log = crate::deps::DepsLog::open(Some(&directory)).unwrap();
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&out1).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
        assert!(builder.command_output.is_empty());
    }
    for output in [&out1, &out2] {
        let output = nodeget(&graph, output.as_bytes()).unwrap();
        let entry = crate::deps::depsentry(&deps_log, output).unwrap();
        assert_eq!(entry.deps.nodes.len(), 1);
        assert_eq!(
            graph.node_path(entry.deps.nodes[0]),
            directory.join("in").to_string_lossy().as_ref()
        );
    }
    deps_log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_deps_log_escaped_output_preserves_command_inputs() {
    let manifest = "rule cc\n  command = cp $in $out\n  depfile = $out.d\n  deps = gcc\nbuild $dir/fo$ o.o: cc $dir/foo.c\n";
    let (mut graph, directory) = build_fixture("deps-log-escaped-output", manifest);
    fs::write(directory.join("foo.c"), "source").unwrap();
    let target = directory.join("fo o.o").to_string_lossy().into_owned();
    fs::write(
        directory.join("fo o.o.d"),
        format!(
            "{}: {} {}\n",
            target.replace(' ', "\\ "),
            directory.join("blah.h").display(),
            directory.join("bar.h").display()
        ),
    )
    .unwrap();
    let deps_path = directory.join(".ninja_deps");
    let mut deps_log = crate::deps::DepsLog::open(Some(&directory)).unwrap();
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    deps_log.finish().unwrap();

    fs::write(directory.join("blah.h"), "").unwrap();
    fs::write(directory.join("bar.h"), "").unwrap();
    let mut graph = parse_fixture(&directory);
    let (mut deps_log, warning) = crate::deps::depsloadlog(&deps_path, &mut graph).unwrap();
    assert!(warning.is_none());
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
    }
    let output = nodeget(&graph, target.as_bytes()).unwrap();
    let edge = graph.node(output).gen.unwrap();
    assert_eq!(graph.edge(edge).input.len(), 3);
    let command = crate::env::edgevar(&graph, edge, Names::COMMAND, PathStyle::Raw).unwrap();
    let command = String::from_utf8_lossy(command.as_bytes());
    assert!(command.contains("foo.c"));
    assert!(!command.contains("blah.h"));
    assert!(!command.contains("bar.h"));
    deps_log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_deps_log_restat_prunes_discovered_dependency() {
    let manifest = "rule steady\n  command = true\n  restat = 1\nrule cc\n  command = cp $in $out; printf '$out: $dir/header.h\\n' > $out.d\n  depfile = $out.d\n  deps = gcc\nbuild $dir/header.h: steady $dir/header.in\nbuild $dir/out: cc $dir/in\n";
    let (mut graph, directory) = build_fixture("deps-log-restat", manifest);
    fs::write(directory.join("header.in"), "").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("header.h"), "").unwrap();
    fs::write(directory.join("in"), "source").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let deps_path = directory.join(".ninja_deps");
    let mut deps_log = crate::deps::DepsLog::open(Some(&directory)).unwrap();
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    deps_log.finish().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("header.in"), "changed").unwrap();
    let mut graph = parse_fixture(&directory);
    let (mut deps_log, warning) = crate::deps::depsloadlog(&deps_path, &mut graph).unwrap();
    assert!(warning.is_none());
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran, ["true"]);
    }
    deps_log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_deps_log_missing_entry_survives_restat_cleanup() {
    let manifest = "rule steady\n  command = true\n  restat = 1\nrule cc\n  command = cp $in $out; printf '$out: $dir/header.h\\n' > $out.d\n  depfile = $out.d\n  deps = gcc\nbuild $dir/header.h: steady $dir/header.in\nbuild $dir/out: cc $dir/header.h\n";
    let (mut graph, directory) = build_fixture("deps-log-restat-missing", manifest);
    fs::write(directory.join("header.in"), "").unwrap();
    fs::write(directory.join("header.h"), "header").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut deps_log = crate::deps::DepsLog::open(Some(&directory)).unwrap();
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
    }
    deps_log.finish().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("header.in"), "changed").unwrap();
    let blank_log_directory = directory.join("blank-log");
    fs::create_dir_all(&blank_log_directory).unwrap();
    let mut graph = parse_fixture(&directory);
    let mut deps_log = crate::deps::DepsLog::open(Some(&blank_log_directory)).unwrap();
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
        assert_eq!(builder.commands_ran[0], "true");
    }
    deps_log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_deps_log_validation_through_discovered_input() {
    let manifest = "rule copy\n  command = cp $in $out\nrule cc\n  command = cp $in $out; printf '$out: $dir/out\\n' > $out.d\n  deps = gcc\n  depfile = $out.d\nbuild $dir/out: copy $dir/in |@ $dir/validate\nbuild $dir/validate: copy $dir/in2 | $dir/out\nbuild $dir/out2: cc $dir/in3\n";
    let (mut graph, directory) = build_fixture("deps-log-validation", manifest);
    fs::write(directory.join("in"), "out").unwrap();
    fs::write(directory.join("in2"), "validation").unwrap();
    fs::write(directory.join("in3"), "out2").unwrap();
    let target = directory.join("out2").to_string_lossy().into_owned();
    let deps_path = directory.join(".ninja_deps");
    let mut deps_log = crate::deps::DepsLog::open(Some(&directory)).unwrap();
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    deps_log.finish().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in"), "changed").unwrap();
    fs::write(directory.join("in2"), "changed").unwrap();
    let mut graph = parse_fixture(&directory);
    let (mut deps_log, warning) = crate::deps::depsloadlog(&deps_path, &mut graph).unwrap();
    assert!(warning.is_none());
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
        assert!(builder.commands_ran[0].contains_str("/in "));
    }
    deps_log.finish().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("in2"), "changed again").unwrap();
    fs::write(directory.join("in3"), "changed").unwrap();
    let mut graph = parse_fixture(&directory);
    let (mut deps_log, warning) = crate::deps::depsloadlog(&deps_path, &mut graph).unwrap();
    assert!(warning.is_none());
    {
        let mut builder =
            Builder::with_deps_log(&mut graph, BuildOptions::default(), &mut deps_log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
        assert!(builder.commands_ran[0].contains_str("/in3 "));
    }
    deps_log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_restat_depfile_dependency() {
    let (mut graph, directory) = build_fixture(
            "restat-depfile-dependency",
            "rule steady\n  command = true\n  restat = 1\nrule copy\n  command = cp $in $out\nbuild $dir/header.h: steady $dir/header.in\nbuild $dir/out: copy $dir/in\n  depfile = $dir/out.d\n",
        );
    fs::write(directory.join("in"), "source").unwrap();
    fs::write(directory.join("header.h"), "").unwrap();
    fs::write(directory.join("out"), "source").unwrap();
    fs::write(
        directory.join("out.d"),
        format!(
            "{}: {}\n",
            directory.join("out").display(),
            directory.join("header.h").display()
        ),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("header.in"), "changed").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran, ["true"]);
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_restat_missing_depfile_does_not_prune_dependent() {
    let (mut graph, directory) = build_fixture(
            "restat-missing-depfile",
            "rule steady\n  command = true\n  restat = 1\nrule copy\n  command = cp $in $out\nbuild $dir/header.h: steady $dir/header.in\nbuild $dir/out: copy $dir/header.h\n  depfile = $dir/out.d\n",
        );
    fs::write(directory.join("header.h"), "header").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("out"), "header").unwrap();
    fs::write(directory.join("header.in"), "changed").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
        assert_eq!(builder.commands_ran[0], "true");
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_stale_depfile_does_not_introduce_cycle() {
    let (mut graph, directory) = build_fixture(
            "stale-depfile-cycle",
            "rule copy\n  command = cp $in $out\nrule copy_deps\n  command = cp $in $out; printf '$dir/b: $dir/X\\n' > $dir/d.d\nbuild $dir/b: copy_deps $dir/a\n  depfile = $dir/d.d\nbuild $dir/c: copy $dir/b\nbuild $dir/d: copy $dir/c\n",
        );
    fs::write(directory.join("a"), "source").unwrap();
    fs::write(directory.join("X"), "").unwrap();
    fs::write(
        directory.join("d.d"),
        format!(
            "{}: {}\n",
            directory.join("b").display(),
            directory.join("d").display()
        ),
    )
    .unwrap();
    let target = directory.join("d").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        let target_node = nodeget(builder.graph, target.as_bytes()).unwrap();
        builder.load_depfiles_for(target_node).unwrap();
        let b = nodeget(
            builder.graph,
            directory.join("b").to_string_lossy().as_bytes(),
        )
        .unwrap();
        let edge = builder.graph.node(b).gen.unwrap();
        assert_eq!(builder.graph.edge(edge).input.len(), 1);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
    }
    assert_eq!(
        fs::read_to_string(directory.join("d.d")).unwrap(),
        format!(
            "{}: {}\n",
            directory.join("b").display(),
            directory.join("X").display()
        )
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep() {
    let (mut graph, directory) = build_fixture(
            "dyndep-generated",
            "rule generate_dd\n  command = printf 'ninja_dyndep_version = 1\\nbuild $dir/out: dyndep\\n' > $out\nrule touch\n  command = touch $out\nbuild $dir/dd: generate_dd\nbuild $dir/out: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
        let dyndep = nodeget(
            builder.graph,
            directory.join("dd").to_string_lossy().as_bytes(),
        )
        .unwrap();
        assert!(!builder.runtime.node(dyndep).dyndep_pending());
    }
    assert!(directory.join("dd").exists());
    assert!(directory.join("out").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_syntax_error() {
    let (mut graph, directory) = build_fixture(
            "dyndep-generated-error",
            "rule generate_dd\n  command = printf 'not a dyndep file\\n' > $out\nrule touch\n  command = touch $out\nbuild $dir/dd: generate_dd\nbuild $dir/out: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&target).unwrap();
    assert!(builder.build().is_err());
    assert_eq!(builder.commands_ran.len(), 1);
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_with_unrelated_dependent() {
    let (mut graph, directory) = build_fixture(
            "dyndep-unrelated-output",
            "rule touch\n  command = touch $out\nrule copy\n  command = cp $in $out\nbuild $dir/dd: copy $dir/dd-in\nbuild $dir/unrelated: touch || $dir/dd\nbuild $dir/out: touch $dir/unrelated || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(
        directory.join("dd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep\n",
            directory.join("out").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("out"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
        assert!(builder.commands_ran[0].contains_str("/dd-in"));
        assert!(builder.commands_ran[1].contains_str("/unrelated"));
        assert!(builder.commands_ran[2].contains_str("/out"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_discovers_new_output() {
    let (mut graph, directory) = build_fixture(
            "dyndep-new-output",
            "rule touch\n  command = touch $out $out.imp\nrule copy\n  command = cp $in $out\nbuild $dir/dd: copy $dir/dd-in\nbuild $dir/out: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    fs::write(
        directory.join("dd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep\n",
            directory.join("out").display(),
            directory.join("out.imp").display()
        ),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(directory.join("out"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 2);
        assert!(builder.commands_ran[0].contains_str("/dd-in"));
        assert!(builder.commands_ran[1].contains_str("/out.imp"));
    }
    assert!(directory.join("out.imp").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_rejects_existing_static_output() {
    let (mut graph, directory) = build_fixture(
            "dyndep-duplicate-static-output",
            "rule touch\n  command = touch $out $out.imp\nrule copy\n  command = cp $in $out\nbuild $dir/dd: copy $dir/dd-in\nbuild $dir/out1 | $dir/out-twice.imp: touch $dir/in\nbuild $dir/out2: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    fs::write(
        directory.join("dd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep\n",
            directory.join("out2").display(),
            directory.join("out-twice.imp").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("out1"), "").unwrap();
    fs::write(directory.join("out2"), "").unwrap();
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&out1).unwrap();
    builder.add_target(&out2).unwrap();
    let error = builder.build().unwrap_err();
    assert!(error.to_string().contains("multiple rules generate"));
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_rejects_output_from_other_dyndep() {
    let (mut graph, directory) = build_fixture(
            "dyndep-duplicate-dynamic-output",
            "rule touch\n  command = touch $out $out.imp\nrule copy\n  command = cp $in $out\nbuild $dir/dd1: copy $dir/dd1-in\nbuild $dir/out1: touch || $dir/dd1\n  dyndep = $dir/dd1\nbuild $dir/dd2: copy $dir/dd2-in || $dir/dd1\nbuild $dir/out2: touch || $dir/dd2\n  dyndep = $dir/dd2\n",
        );
    fs::write(
        directory.join("dd1-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep\n",
            directory.join("out1").display(),
            directory.join("out-twice.imp").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("dd2-in"), "").unwrap();
    fs::write(
        directory.join("dd2"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep\n",
            directory.join("out2").display(),
            directory.join("out-twice.imp").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("out1"), "").unwrap();
    fs::write(directory.join("out2"), "").unwrap();
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&out1).unwrap();
    builder.add_target(&out2).unwrap();
    let error = builder.build().unwrap_err();
    assert!(error.to_string().contains("multiple rules generate"));
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_rejects_validation_syntax() {
    let (mut graph, directory) = build_fixture(
            "dyndep-validation-syntax",
            "rule touch\n  command = touch $out\nrule copy\n  command = cp $in $out\nbuild $dir/dd: copy $dir/dd-in\nbuild $dir/out: touch || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(
        directory.join("dd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep |@ {}\n",
            directory.join("out").display(),
            directory.join("validation").display()
        ),
    )
    .unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&target).unwrap();
    let error = builder.build().unwrap_err();
    assert!(error.to_string().contains("expected newline, got '|@'"));
    assert_eq!(builder.commands_ran.len(), 1);
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_discovers_transitive_validation() {
    let (mut graph, directory) = build_fixture(
            "dyndep-transitive-validation",
            "rule touch\n  command = touch $out\nrule copy\n  command = cp $in $out\nbuild $dir/dd: copy $dir/dd-in\nbuild $dir/in: touch |@ $dir/validation\nbuild $dir/validation: touch $dir/in $dir/out\nbuild $dir/out: touch || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(
        directory.join("dd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep | {}\n",
            directory.join("out").display(),
            directory.join("in").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("out"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 4);
        assert!(builder.commands_ran[0].contains_str("/dd-in"));
        assert!(builder.commands_ran[1].contains_str("/in"));
        assert!(builder.commands_ran[2].contains_str("/out"));
        assert!(builder.commands_ran[3].contains_str("/validation"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_discovers_implicit_connection() {
    let (mut graph, directory) = build_fixture(
            "dyndep-implicit-connection",
            "rule touch\n  command = touch $out $out.imp\nrule copy\n  command = cp $in $out\nbuild $dir/dd: copy $dir/dd-in\nbuild $dir/tmp: touch || $dir/dd\n  dyndep = $dir/dd\nbuild $dir/out: touch || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(
        directory.join("dd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep | {}\nbuild {} | {}: dyndep\n",
            directory.join("out").display(),
            directory.join("out.imp").display(),
            directory.join("tmp.imp").display(),
            directory.join("tmp").display(),
            directory.join("tmp.imp").display()
        ),
    )
    .unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
        assert!(builder.commands_ran[0].contains_str("/dd-in"));
        assert!(builder.commands_ran[1].contains_str("/tmp.imp"));
        assert!(builder.commands_ran[2].contains_str("/out.imp"));
    }
    assert!(directory.join("tmp.imp").exists());
    assert!(directory.join("out.imp").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_connects_depfile_input() {
    let (mut graph, directory) = build_fixture(
            "dyndep-depfile-connection",
            "rule touch\n  command = touch $out $out.imp\nrule copy\n  command = cp $in $out\nbuild $dir/dd: copy $dir/dd-in\nbuild $dir/tmp: touch || $dir/dd\n  dyndep = $dir/dd\nbuild $dir/out: copy $dir/tmp\n  depfile = $dir/out.d\n",
        );
    fs::write(
        directory.join("out.d"),
        format!(
            "{}: {}\n",
            directory.join("out").display(),
            directory.join("tmp.imp").display()
        ),
    )
    .unwrap();
    fs::write(
        directory.join("dd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep\n",
            directory.join("tmp").display(),
            directory.join("tmp.imp").display()
        ),
    )
    .unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
    }
    let implicit = nodeget(
        &graph,
        directory.join("tmp.imp").to_string_lossy().as_bytes(),
    )
    .unwrap();
    let generator = graph.node(implicit).gen.unwrap();
    assert!(graph
        .edge(generator)
        .rule
        .is_none_or(|rule| graph.rule(rule).name != "phony"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_now_wants_clean_edge() {
    let (mut graph, directory) = build_fixture(
            "dyndep-now-want-edge",
            "rule touch\n  command = touch $out $out.imp\nrule copy\n  command = cp $in $out\nbuild $dir/dd: copy $dir/dd-in\nbuild $dir/tmp: touch || $dir/dd\n  dyndep = $dir/dd\nbuild $dir/out: touch $dir/tmp || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(directory.join("tmp"), "").unwrap();
    fs::write(directory.join("out"), "").unwrap();
    fs::write(
        directory.join("dd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep\nbuild {} | {}: dyndep\n",
            directory.join("out").display(),
            directory.join("tmp").display(),
            directory.join("tmp.imp").display()
        ),
    )
    .unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
        assert!(builder.commands_ran[0].contains_str("/dd-in"));
        assert!(builder.commands_ran[1].contains_str("/tmp.imp"));
        assert!(builder.commands_ran[2].contains_str("/out.imp"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_now_wants_edge_and_dependent() {
    let (mut graph, directory) = build_fixture(
            "dyndep-now-want-dependent",
            "rule touch\n  command = touch $out $out.imp\nrule copy\n  command = cp $in $out\nbuild $dir/dd: copy $dir/dd-in\nbuild $dir/tmp: touch || $dir/dd\n  dyndep = $dir/dd\nbuild $dir/out: touch $dir/tmp\n",
        );
    fs::write(directory.join("tmp"), "").unwrap();
    fs::write(directory.join("out"), "").unwrap();
    fs::write(
        directory.join("dd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep\n",
            directory.join("tmp").display(),
            directory.join("tmp.imp").display()
        ),
    )
    .unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
        assert!(builder.commands_ran[0].contains_str("/dd-in"));
        assert!(builder.commands_ran[1].contains_str("/tmp.imp"));
        assert!(builder.commands_ran[2].contains_str("/out.imp"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_generated_dyndep_does_not_reschedule_completed_edge() {
    let (mut graph, directory) = build_fixture(
            "dyndep-scheduled-edge",
            "rule touch\n  command = touch $out $out.imp\nrule copy\n  command = cp $in $out\nbuild $dir/out1 | $dir/out1.imp: touch\nbuild $dir/zdd: copy $dir/zdd-in\nbuild $dir/out2: copy $dir/out1 || $dir/zdd\n  dyndep = $dir/zdd\n",
        );
    fs::write(
        directory.join("zdd-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep | {}\n",
            directory.join("out2").display(),
            directory.join("out1.imp").display()
        ),
    )
    .unwrap();
    let out1 = directory.join("out1").to_string_lossy().into_owned();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&out1).unwrap();
        builder.add_target(&out2).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
        assert_eq!(
            builder
                .commands_ran
                .iter()
                .filter(|command| command.starts_with(b"touch "))
                .count(),
            1
        );
        assert!(builder.commands_ran[2].contains_str("/out1"));
        assert!(builder.commands_ran[2].contains_str("/out2"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_two_level_dyndep_direct() {
    let (mut graph, directory) = build_fixture(
            "dyndep-two-level-direct",
            "rule touch\n  command = touch $out $out.imp\nrule copy\n  command = cp $in $out\nbuild $dir/dd1: copy $dir/dd1-in\nbuild $dir/out1 | $dir/out1.imp: touch || $dir/dd1\n  dyndep = $dir/dd1\nbuild $dir/dd2: copy $dir/dd2-in || $dir/dd1\nbuild $dir/out2: touch || $dir/dd2\n  dyndep = $dir/dd2\n",
        );
    fs::write(directory.join("out1.imp"), "").unwrap();
    fs::write(directory.join("out2"), "").unwrap();
    fs::write(directory.join("out2.imp"), "").unwrap();
    fs::write(
        directory.join("dd1-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep\n",
            directory.join("out1").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("dd2-in"), "").unwrap();
    fs::write(
        directory.join("dd2"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep | {}\n",
            directory.join("out2").display(),
            directory.join("out2.imp").display(),
            directory.join("out1.imp").display()
        ),
    )
    .unwrap();
    let target = directory.join("out2").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
        assert!(builder.commands_ran[0].contains_str("/dd1-in"));
        assert!(builder.commands_ran[1].contains_str("/out1.imp"));
        assert!(builder.commands_ran[2].contains_str("/out2.imp"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_two_level_dyndep_indirect() {
    let (mut graph, directory) = build_fixture(
            "dyndep-two-level-indirect",
            "rule touch\n  command = touch $out $out.imp\nrule copy\n  command = cp $in $out\nbuild $dir/dd1: copy $dir/dd1-in\nbuild $dir/out1: touch || $dir/dd1\n  dyndep = $dir/dd1\nbuild $dir/dd2: copy $dir/dd2-in || $dir/out1\nbuild $dir/out2: touch || $dir/dd2\n  dyndep = $dir/dd2\n",
        );
    fs::write(directory.join("out1.imp"), "").unwrap();
    fs::write(directory.join("out2"), "").unwrap();
    fs::write(directory.join("out2.imp"), "").unwrap();
    fs::write(
        directory.join("dd1-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep\n",
            directory.join("out1").display(),
            directory.join("out1.imp").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("dd2-in"), "").unwrap();
    fs::write(
        directory.join("dd2"),
        format!(
            "ninja_dyndep_version = 1\nbuild {} | {}: dyndep | {}\n",
            directory.join("out2").display(),
            directory.join("out2.imp").display(),
            directory.join("out1.imp").display()
        ),
    )
    .unwrap();
    let target = directory.join("out2").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
        assert!(builder.commands_ran[0].contains_str("/dd1-in"));
        assert!(builder.commands_ran[1].contains_str("/out1.imp"));
        assert!(builder.commands_ran[2].contains_str("/out2.imp"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_two_level_dyndep_discovered_ready() {
    let (mut graph, directory) = build_fixture(
            "dyndep-two-level-ready",
            "rule touch\n  command = touch $out\nrule copy\n  command = cp $in $out\nbuild $dir/dd0: copy $dir/dd0-in\nbuild $dir/dd1: copy $dir/dd1-in\nbuild $dir/in: touch\nbuild $dir/tmp: touch || $dir/dd0\n  dyndep = $dir/dd0\nbuild $dir/out: touch || $dir/dd1\n  dyndep = $dir/dd1\n",
        );
    fs::write(
        directory.join("dd1-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep | {}\n",
            directory.join("out").display(),
            directory.join("tmp").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("dd0-in"), "").unwrap();
    fs::write(
        directory.join("dd0"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep | {}\n",
            directory.join("tmp").display(),
            directory.join("in").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("out"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 4);
        assert!(builder.commands_ran[0].contains_str("/dd1-in"));
        assert!(builder.commands_ran[1].contains_str("/in"));
        assert!(builder.commands_ran[2].contains_str("/tmp"));
        assert!(builder.commands_ran[3].contains_str("/out"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_two_level_dyndep_discovered_dirty() {
    let (mut graph, directory) = build_fixture(
            "dyndep-two-level-dirty",
            "rule touch\n  command = touch $out\nrule copy\n  command = cp $in $out\nbuild $dir/dd0: copy $dir/dd0-in\nbuild $dir/dd1: copy $dir/dd1-in\nbuild $dir/in: touch\nbuild $dir/tmp: touch || $dir/dd0\n  dyndep = $dir/dd0\nbuild $dir/out: touch || $dir/dd1\n  dyndep = $dir/dd1\n",
        );
    fs::write(
        directory.join("dd1-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep | {}\n",
            directory.join("out").display(),
            directory.join("tmp").display()
        ),
    )
    .unwrap();
    fs::write(
        directory.join("dd0-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep | {}\n",
            directory.join("tmp").display(),
            directory.join("in").display()
        ),
    )
    .unwrap();
    fs::write(directory.join("out"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 5);
        assert!(builder.commands_ran[0].contains_str("/dd1-in"));
        assert!(builder.commands_ran[1].contains_str("/dd0-in"));
        assert!(builder.commands_ran[2].contains_str("/in"));
        assert!(builder.commands_ran[3].contains_str("/tmp"));
        assert!(builder.commands_ran[4].contains_str("/out"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_multiple_dyndeps_from_one_edge() {
    let (mut graph, directory) = build_fixture(
            "dyndep-multiple-files",
            "rule touch\n  command = touch $out\nrule generate\n  command = cp $dir/dd3-in $dir/dd3; cp $dir/dd2-in $dir/dd2\nrule copy_out1\n  command = cp $dir/out1 $out\nbuild $dir/dd3 $dir/dd2: generate $dir/dd3-in $dir/dd2-in\nbuild $dir/out3: touch $dir/in || $dir/dd3\n  dyndep = $dir/dd3\nbuild $dir/out2: copy_out1 || $dir/dd2\n  dyndep = $dir/dd2\nbuild $dir/out1: touch $dir/in\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    fs::write(
        directory.join("dd3-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep | {}\n",
            directory.join("out3").display(),
            directory.join("out2").display()
        ),
    )
    .unwrap();
    fs::write(
        directory.join("dd2-in"),
        format!(
            "ninja_dyndep_version = 1\nbuild {}: dyndep | {}\n",
            directory.join("out2").display(),
            directory.join("out1").display()
        ),
    )
    .unwrap();
    let out2 = directory.join("out2").to_string_lossy().into_owned();
    let out3 = directory.join("out3").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&out2).unwrap();
        builder.add_target(&out3).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 4);
        assert!(builder.commands_ran[0].contains_str("dd3-in"));
        assert!(builder.commands_ran[1].contains_str("/out1"));
        assert!(builder.commands_ran[2].contains_str("/out2"));
        assert!(builder.commands_ran[3].contains_str("/out3"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_dyndep_discovers_new_generated_input() {
    let (mut graph, directory) = build_fixture(
            "dyndep-new-input",
            "rule generate_dd\n  command = printf 'ninja_dyndep_version = 1\\nbuild $dir/out: dyndep | $dir/implicit\\n' > $out\nrule touch\n  command = touch $out\nbuild $dir/dd: generate_dd\nbuild $dir/implicit: touch $dir/source\nbuild $dir/out: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    fs::write(directory.join("source"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    {
        let mut builder = Builder::new(&mut graph, BuildOptions::default());
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 3);
        assert!(builder.commands_ran[0].contains_str("/dd"));
        assert!(builder.commands_ran[1].contains_str("/implicit"));
        assert!(builder.commands_ran[2].contains_str("/out"));
    }
    assert!(directory.join("implicit").exists());
    assert!(directory.join("out").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ninja_build_dyndep_discovers_dependency_cycle() {
    let (mut graph, directory) = build_fixture(
            "dyndep-cycle",
            "rule generate_dd\n  command = printf 'ninja_dyndep_version = 1\\nbuild $dir/out: dyndep | $dir/circular\\n' > $out\nrule touch\n  command = touch $out\nbuild $dir/dd: generate_dd\nbuild $dir/circular: touch $dir/out\nbuild $dir/out: touch $dir/in || $dir/dd\n  dyndep = $dir/dd\n",
        );
    fs::write(directory.join("in"), "").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(&target).unwrap();
    assert!(builder.build().is_err());
    assert_eq!(builder.commands_ran.len(), 1);
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:product.output-style/test]
#[test]
fn cargo_style_renders_a_whole_build_in_the_verb_column() {
    let (mut graph, directory) = build_fixture(
        "cargo-style-build",
        "rule cc\n  command = touch $out\n  description = Building $out\nbuild $dir/out: cc $dir/in\n",
    );
    fs::write(directory.join("in"), "source").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let options = BuildOptions {
        style: crate::build::OutputStyle::Cargo,
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&target).unwrap();
    builder.build().unwrap();
    let output = String::from_utf8_lossy(&builder.build_output).into_owned();
    let expected = format!("    Building {} (1/1)\n", directory.join("out").display());
    assert!(output.starts_with(&expected), "{output:?}");
    assert!(
        output.contains("\n    Finished 1 command in "),
        "{output:?}"
    );
    assert!(!output.contains("[1/1]"), "{output:?}");
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:product.output-style/test]
#[test]
fn cargo_style_names_the_failed_output_and_shows_its_command() {
    let (mut graph, directory) = build_fixture(
        "cargo-style-failure",
        "rule cc\n  command = false\n  description = Building $out\nbuild $dir/out: cc\n",
    );
    let target = directory.join("out").to_string_lossy().into_owned();
    let options = BuildOptions {
        style: crate::build::OutputStyle::Cargo,
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&target).unwrap();
    assert!(builder.build().is_err());
    let output = String::from_utf8_lossy(&builder.build_output).into_owned();
    let failure = format!(
        "      Failed {} (exit 1)\n",
        directory.join("out").display()
    );
    assert!(output.contains(&failure), "{output:?}");
    assert!(output.contains("\n             false\n"), "{output:?}");
    assert!(!output.contains("FAILED:"), "{output:?}");
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:product.output-style/test]
#[test]
fn a_styled_build_gives_the_bars_line_back_when_it_ends() {
    let (mut graph, directory) = build_fixture(
        "cargo-style-bar",
        "rule cc\n  command = touch $out\n  description = Building $out\nbuild $dir/out: cc $dir/in\n",
    );
    fs::write(directory.join("in"), "source").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let options = BuildOptions {
        style: crate::build::OutputStyle::Cargo,
        color: crate::build::ColorChoice::Always,
        ..BuildOptions::default()
    };
    let mut builder = Builder::new(&mut graph, options);
    builder.add_target(&target).unwrap();
    builder.build().unwrap();
    let output = String::from_utf8_lossy(&builder.build_output).into_owned();
    // The gauge's leading marker identifies a painted bar; the verb before it
    // is separated from the bracket by a reset escape, so match on the gauge.
    assert!(output.contains("[>"), "no bar was painted: {output:?}");
    let (_, tail) = output
        .rsplit_once("\r\u{1b}[K")
        .expect("the bar is taken back");
    // A reset escape sits between the verb and its text when colour is on.
    assert!(
        tail.contains("Finished") && tail.contains(" 1 command in "),
        "{tail:?}"
    );
    assert!(
        !tail.contains("[>") && !tail.contains("[="),
        "a bar was left on screen: {tail:?}"
    );
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:compat.persistent-state/test]
#[test]
fn ninja_dry_run_records_nothing_in_the_build_log() {
    let (mut graph, directory) = build_fixture(
        "dry-run-log",
        "rule copy\n  command = cp $in $out\nbuild $dir/out: copy $dir/in\n",
    );
    fs::write(directory.join("in"), "hello").unwrap();
    let target = directory.join("out").to_string_lossy().into_owned();
    let mut log = crate::log::BuildLog::open(Some(&directory)).unwrap();
    let options = BuildOptions {
        dryrun: true,
        ..BuildOptions::default()
    };
    {
        let mut builder = Builder::with_build_log(&mut graph, options, &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        // A dry run still reports the command it would have run.
        assert_eq!(builder.commands_ran.len(), 1);
    }
    // Ninja records a command it did not run nowhere; neither does ronin, so a
    // repeated dry run cannot grow the log and a later real build still runs.
    assert!(crate::log::logentry(&log, &target).is_none());
    assert!(!directory.join("out").exists());

    {
        let mut builder = Builder::with_build_log(&mut graph, BuildOptions::default(), &mut log);
        builder.add_target(&target).unwrap();
        builder.build().unwrap();
        assert_eq!(builder.commands_ran.len(), 1);
    }
    assert!(crate::log::logentry(&log, &target).is_some());
    log.finish().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

// [spec:ronin:req:compat.scheduling/test]
#[test]
fn a_plan_of_only_phony_work_is_already_up_to_date() {
    // Ninja's `more_to_do` wants a command edge as well as a wanted edge, so a
    // default target that is a phony over other phonies has nothing to do.
    let (mut graph, directory) = build_fixture(
        "phony-only-plan",
        "build inner: phony\nbuild all: phony inner\ndefault all\n",
    );
    let mut builder = Builder::new(&mut graph, BuildOptions::default());
    builder.add_target(b"all").unwrap();
    assert!(
        builder.already_up_to_date(),
        "a plan with no command edges has nothing to do"
    );
    drop(builder);
    fs::remove_dir_all(directory).unwrap();
}
