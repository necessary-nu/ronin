//! Detection of generated depfile inputs without a manifest dependency path.

use crate::env::edgevar;
#[cfg(test)]
use crate::error::GraphError;
use crate::graph::{EdgeId, Graph, NodeId, PathStyle};
use crate::names::Names;
use crate::util::ByteSlice;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MissingDependency {
    pub(crate) consumer: String,
    pub(crate) generated: String,
    pub(crate) generator_rule: String,
}

#[derive(Default)]
pub(crate) struct MissingDependencyScanner {
    dependency_log: Vec<Vec<NodeId>>,
    seen: Vec<bool>,
    nodes_missing_deps: Vec<bool>,
    nodes_missing_deps_count: usize,
    generated_nodes: Vec<bool>,
    generated_nodes_count: usize,
    generator_rules: BTreeSet<String>,
    missing_dep_path_count: usize,
    reports: Vec<MissingDependency>,
    adjacency: HashMap<(EdgeId, EdgeId), bool>,
    generated_edges: Vec<bool>,
    missing_edges: Vec<bool>,
    path_marks: Vec<u32>,
    path_generation: u32,
    path_work: Vec<EdgeId>,
}

// [spec:ronin:req:compat.graph-semantics]
impl MissingDependencyScanner {
    pub(crate) fn record_dependency(&mut self, from: NodeId, to: NodeId) {
        self.dependency_log.resize_with(from.index() + 1, Vec::new);
        self.dependency_log[from.index()].push(to);
    }

    pub(crate) fn process_node(&mut self, graph: &Graph, node: NodeId) {
        enum Work {
            Enter(NodeId),
            Process(NodeId, EdgeId),
        }

        let mut work = vec![Work::Enter(node)];
        while let Some(item) = work.pop() {
            match item {
                Work::Enter(node) => {
                    let Some(edge) = graph.node(node).generator else {
                        continue;
                    };
                    self.seen
                        .resize(self.seen.len().max(node.index() + 1), false);
                    if std::mem::replace(&mut self.seen[node.index()], true) {
                        continue;
                    }
                    work.push(Work::Process(node, edge));
                    for input in graph.edge(edge).input.iter().rev() {
                        work.push(Work::Enter(*input));
                    }
                }
                Work::Process(node, edge) => {
                    if edgevar(graph, edge, Names::DEPS, PathStyle::Raw).is_none() {
                        continue;
                    }
                    if let Some(dependencies) = self.dependency_log.get(node.index()).cloned() {
                        self.process_node_dependencies(graph, node, edge, &dependencies);
                    }
                }
            }
        }
    }

    fn process_node_dependencies(
        &mut self,
        graph: &Graph,
        node: NodeId,
        consumer_edge: EdgeId,
        dependencies: &[NodeId],
    ) {
        if dependencies
            .iter()
            .any(|dependency| graph.node_path(*dependency) == b"build.ninja")
        {
            return;
        }
        self.generated_edges.resize(graph.edge_count(), false);
        self.generated_edges.fill(false);
        for dependency in dependencies {
            if let Some(edge) = graph.node(*dependency).generator {
                self.generated_edges[edge.index()] = true;
            }
        }

        self.missing_edges.resize(graph.edge_count(), false);
        self.missing_edges.fill(false);
        for generator in graph.edge_ids() {
            let index = generator.index();
            let generated = self.generated_edges[index];
            if generated && !self.path_exists_between(graph, generator, consumer_edge) {
                self.missing_edges[index] = true;
            }
        }
        if !self.missing_edges.iter().any(|missing| *missing) {
            return;
        }

        let mut missing_rules = BTreeSet::new();
        for dependency in dependencies {
            let Some(generator) = graph.node(*dependency).generator else {
                continue;
            };
            if !self.missing_edges[generator.index()] {
                continue;
            }
            let rule_name = graph
                .edge(generator)
                .rule
                .map(|rule| graph.rule(rule).name.to_str_lossy().into_owned())
                .unwrap_or_default();
            missing_rules.insert(rule_name.clone());
            self.generated_nodes.resize(
                self.generated_nodes.len().max(dependency.index() + 1),
                false,
            );
            if !std::mem::replace(&mut self.generated_nodes[dependency.index()], true) {
                self.generated_nodes_count += 1;
            }
            self.generator_rules.insert(rule_name.clone());
            self.reports.push(MissingDependency {
                consumer: String::from_utf8_lossy(graph.node_path(node).as_bytes()).into_owned(),
                generated: String::from_utf8_lossy(graph.node_path(*dependency).as_bytes())
                    .into_owned(),
                generator_rule: rule_name,
            });
        }
        self.missing_dep_path_count += missing_rules.len();
        self.nodes_missing_deps
            .resize(self.nodes_missing_deps.len().max(node.index() + 1), false);
        if !std::mem::replace(&mut self.nodes_missing_deps[node.index()], true) {
            self.nodes_missing_deps_count += 1;
        }
    }

    fn path_exists_between(&mut self, graph: &Graph, from: EdgeId, to: EdgeId) -> bool {
        let key = (from, to);
        if let Some(found) = self.adjacency.get(&key) {
            return *found;
        }
        self.path_marks.resize(graph.edge_count(), 0);
        self.path_generation = self.path_generation.wrapping_add(1);
        if self.path_generation == 0 {
            self.path_marks.fill(0);
            self.path_generation = 1;
        }
        self.path_work.clear();
        self.path_work.push(to);
        let mut found = false;
        while let Some(edge) = self.path_work.pop() {
            if self.path_marks[edge.index()] == self.path_generation {
                continue;
            }
            self.path_marks[edge.index()] = self.path_generation;
            for input in &graph.edge(edge).input {
                let Some(generator) = graph.node(*input).generator else {
                    continue;
                };
                if generator == from
                    || self
                        .adjacency
                        .get(&(from, generator))
                        .is_some_and(|cached| *cached)
                {
                    found = true;
                    break;
                }
                if !self.adjacency.contains_key(&(from, generator)) {
                    self.path_work.push(generator);
                }
            }
            if found {
                break;
            }
        }
        self.adjacency.insert(key, found);
        found
    }

    pub(crate) const fn had_missing_dependencies(&self) -> bool {
        self.nodes_missing_deps_count != 0
    }

    pub(crate) fn processed_nodes(&self) -> usize {
        self.seen.iter().filter(|seen| **seen).count()
    }

    pub(crate) const fn nodes_missing_dependencies(&self) -> usize {
        self.nodes_missing_deps_count
    }

    pub(crate) const fn generated_nodes(&self) -> usize {
        self.generated_nodes_count
    }

    pub(crate) fn generator_rules(&self) -> usize {
        self.generator_rules.len()
    }

    pub(crate) const fn missing_dependency_paths(&self) -> usize {
        self.missing_dep_path_count
    }

    pub(crate) fn reports(&self) -> &[MissingDependency] {
        &self.reports
    }
}

#[cfg(test)]
pub(crate) fn root_nodes(graph: &Graph) -> Result<Vec<NodeId>, GraphError> {
    let mut roots = Vec::new();
    let mut has_outputs = false;
    for edge in graph.edge_ids() {
        for output in &graph.edge(edge).out {
            has_outputs = true;
            if graph.node(*output).uses.is_empty() {
                roots.push(*output);
            }
        }
    }
    if roots.is_empty() && has_outputs {
        // Every output is used by something, so there is no root to start from.
        // Ninja reports this without a path too: it never walked one.
        Err(GraphError::DependencyCycle {
            node: None,
            path: Vec::new(),
            phony_self_cycle: false,
        })
    } else {
        Ok(roots)
    }
}

#[cfg(test)]
pub(crate) fn process_all_nodes(
    graph: &Graph,
    scanner: &mut MissingDependencyScanner,
) -> Result<(), GraphError> {
    for node in root_nodes(graph)? {
        scanner.process_node(graph, node);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{EnvState, EnvironmentId, RuleId, mkrule, ruleaddvar};
    use crate::graph::{mkedge, mknode, nodeuse};
    use crate::util::{EvalString, xasprintf};

    struct Fixture {
        graph: Graph,
        root: EnvironmentId,
        generator_rule: RuleId,
        compile_rule: RuleId,
    }

    impl Fixture {
        fn new() -> Self {
            let mut graph = Graph::default();
            let state = EnvState::new(&mut graph);
            let generator_rule = deps_rule(&mut graph, "generator_rule");
            let compile_rule = deps_rule(&mut graph, "compile_rule");
            Self {
                graph,
                root: state.root,
                generator_rule,
                compile_rule,
            }
        }

        fn create_initial_state(&mut self) -> (NodeId, NodeId) {
            let generated = self.add_output("generated_header", self.generator_rule);
            let compiled = self.add_output("compiled_object", self.compile_rule);
            (generated, compiled)
        }

        fn add_output(&mut self, path: &str, rule: RuleId) -> NodeId {
            let output = mknode(&mut self.graph, xasprintf(format_args!("{path}")));
            let edge = mkedge(&mut self.graph, self.root);
            let edge_mut = self.graph.edge_mut(edge);
            edge_mut.rule = Some(rule);
            edge_mut.out.push(output);
            edge_mut.set_explicit_output_count(1);
            self.graph.node_mut(output).generator = Some(edge);
            output
        }

        fn add_graph_dependency(&mut self, from: NodeId, to: NodeId) {
            let edge = self.graph.node(from).generator.unwrap();
            nodeuse(&mut self.graph, to, edge);
            let edge_mut = self.graph.edge_mut(edge);
            edge_mut.input.push(to);
            let input_count = edge_mut.input.len();
            edge_mut.set_input_partitions(input_count, input_count);
        }
    }

    fn deps_rule(graph: &mut Graph, name: &str) -> RuleId {
        let rule = mkrule(graph, name.into());
        ruleaddvar(graph, rule, Names::DEPS, EvalString::literal("gcc"));
        rule
    }

    #[test]
    fn ninja_missing_deps_empty_graph() {
        let fixture = Fixture::new();
        let mut scanner = MissingDependencyScanner::default();
        process_all_nodes(&fixture.graph, &mut scanner).unwrap();
        assert!(!scanner.had_missing_dependencies());
    }

    #[test]
    fn ninja_missing_deps_none_missing() {
        let mut fixture = Fixture::new();
        fixture.create_initial_state();
        let mut scanner = MissingDependencyScanner::default();
        process_all_nodes(&fixture.graph, &mut scanner).unwrap();
        assert!(!scanner.had_missing_dependencies());
    }

    #[test]
    fn ninja_missing_deps_detects_generated_depfile_input() {
        let mut fixture = Fixture::new();
        let (generated, compiled) = fixture.create_initial_state();
        let mut scanner = MissingDependencyScanner::default();
        scanner.record_dependency(compiled, generated);
        process_all_nodes(&fixture.graph, &mut scanner).unwrap();
        assert!(scanner.had_missing_dependencies());
        assert_eq!(scanner.nodes_missing_dependencies(), 1);
        assert_eq!(scanner.missing_dependency_paths(), 1);
        assert_eq!(scanner.generated_nodes(), 1);
        assert_eq!(scanner.generator_rules(), 1);
        assert_eq!(
            scanner.reports(),
            [MissingDependency {
                consumer: "compiled_object".into(),
                generated: "generated_header".into(),
                generator_rule: "generator_rule".into(),
            }]
        );
    }

    #[test]
    fn ninja_missing_deps_direct_path_fixes_issue() {
        let mut fixture = Fixture::new();
        let (generated, compiled) = fixture.create_initial_state();
        fixture.add_graph_dependency(compiled, generated);
        let mut scanner = MissingDependencyScanner::default();
        scanner.record_dependency(compiled, generated);
        process_all_nodes(&fixture.graph, &mut scanner).unwrap();
        assert!(!scanner.had_missing_dependencies());
    }

    #[test]
    fn ninja_missing_deps_indirect_path_fixes_issue() {
        let mut fixture = Fixture::new();
        let (generated, compiled) = fixture.create_initial_state();
        let intermediate = fixture.add_output("intermediate", fixture.generator_rule);
        fixture.add_graph_dependency(compiled, intermediate);
        fixture.add_graph_dependency(intermediate, generated);
        let mut scanner = MissingDependencyScanner::default();
        scanner.record_dependency(compiled, generated);
        process_all_nodes(&fixture.graph, &mut scanner).unwrap();
        assert!(!scanner.had_missing_dependencies());
    }

    #[test]
    fn deep_dependency_queries_reuse_dense_scratch_storage() {
        const DEPTH: usize = 20_000;

        let mut fixture = Fixture::new();
        let (generated, compiled) = fixture.create_initial_state();
        let second_compiled = fixture.add_output("compiled_object_2", fixture.compile_rule);
        let mut previous = generated;
        for index in 0..DEPTH {
            let intermediate =
                fixture.add_output(&format!("intermediate_{index}"), fixture.generator_rule);
            fixture.add_graph_dependency(intermediate, previous);
            previous = intermediate;
        }
        fixture.add_graph_dependency(compiled, previous);
        fixture.add_graph_dependency(second_compiled, previous);

        let mut scanner = MissingDependencyScanner::default();
        scanner.record_dependency(compiled, generated);
        scanner.record_dependency(second_compiled, generated);
        scanner.process_node(&fixture.graph, compiled);
        assert!(!scanner.had_missing_dependencies());
        let capacities = (
            scanner.generated_edges.capacity(),
            scanner.missing_edges.capacity(),
            scanner.path_marks.capacity(),
            scanner.path_work.capacity(),
        );
        let generation = scanner.path_generation;

        scanner.process_node(&fixture.graph, second_compiled);
        assert!(!scanner.had_missing_dependencies());
        assert_eq!(
            (
                scanner.generated_edges.capacity(),
                scanner.missing_edges.capacity(),
                scanner.path_marks.capacity(),
                scanner.path_work.capacity(),
            ),
            capacities
        );
        assert!(scanner.path_generation > generation);
    }

    #[test]
    fn ninja_missing_deps_reports_both_sides_of_deps_log_cycle() {
        let mut fixture = Fixture::new();
        let (generated, compiled) = fixture.create_initial_state();
        let mut scanner = MissingDependencyScanner::default();
        scanner.record_dependency(generated, compiled);
        scanner.record_dependency(compiled, generated);
        process_all_nodes(&fixture.graph, &mut scanner).unwrap();
        assert!(scanner.had_missing_dependencies());
        assert_eq!(scanner.nodes_missing_dependencies(), 2);
        assert_eq!(scanner.missing_dependency_paths(), 2);
        assert_eq!(scanner.generated_nodes(), 2);
        assert_eq!(scanner.generator_rules(), 2);
    }

    #[test]
    fn ninja_missing_deps_graph_cycle_has_no_roots() {
        let mut fixture = Fixture::new();
        let (generated, compiled) = fixture.create_initial_state();
        fixture.add_graph_dependency(compiled, generated);
        fixture.add_graph_dependency(generated, compiled);
        match root_nodes(&fixture.graph) {
            Err(error) => assert_eq!(error.to_string(), "dependency cycle"),
            Ok(_) => panic!("cyclic graph unexpectedly had root nodes"),
        }
    }
}
