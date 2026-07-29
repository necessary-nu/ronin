//! Detection of generated depfile inputs without a manifest dependency path.

use crate::env::edgevar;
use crate::graph::{EdgeId, Graph, NodeId};
use crate::util::ByteSlice;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingDependency {
    pub consumer: String,
    pub generated: String,
    pub generator_rule: String,
}

#[derive(Default)]
pub struct MissingDependencyScanner {
    dependency_log: BTreeMap<NodeId, Vec<NodeId>>,
    seen: BTreeSet<NodeId>,
    nodes_missing_deps: BTreeSet<NodeId>,
    generated_nodes: BTreeSet<NodeId>,
    generator_rules: BTreeSet<String>,
    missing_dep_path_count: usize,
    reports: Vec<MissingDependency>,
    adjacency: BTreeMap<(EdgeId, EdgeId), bool>,
}

impl MissingDependencyScanner {
    pub fn record_dependency(&mut self, from: NodeId, to: NodeId) {
        self.dependency_log.entry(from).or_default().push(to);
    }

    pub fn process_node(&mut self, graph: &Graph, node: NodeId) {
        let Some(edge) = graph.node(node).gen else {
            return;
        };
        if !self.seen.insert(node) {
            return;
        }
        let inputs = graph.edge(edge).input.clone();
        for input in &inputs {
            self.process_node(graph, *input);
        }
        if edgevar(graph, edge, "deps", false).is_none() {
            return;
        }
        if let Some(dependencies) = self.dependency_log.get(&node).cloned() {
            self.process_node_dependencies(graph, node, edge, &dependencies);
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
            .any(|dependency| graph.node(*dependency).path == b"build.ninja")
        {
            return;
        }
        let mut generated_edges = BTreeSet::new();
        for dependency in dependencies {
            if let Some(edge) = graph.node(*dependency).gen {
                generated_edges.insert(edge);
            }
        }

        let mut missing_edges = BTreeSet::new();
        for generator in generated_edges {
            if !self.path_exists_between(graph, generator, consumer_edge, &mut BTreeSet::new()) {
                missing_edges.insert(generator);
            }
        }
        if missing_edges.is_empty() {
            return;
        }

        let mut missing_rules = BTreeSet::new();
        for dependency in dependencies {
            let Some(generator) = graph.node(*dependency).gen else {
                continue;
            };
            if !missing_edges.contains(&generator) {
                continue;
            }
            let rule_name = graph
                .edge(generator)
                .rule
                .map(|rule| graph.rule(rule).name.clone())
                .unwrap_or_default();
            missing_rules.insert(rule_name.clone());
            self.generated_nodes.insert(*dependency);
            self.generator_rules.insert(rule_name.clone());
            self.reports.push(MissingDependency {
                consumer: String::from_utf8_lossy(graph.node(node).path.as_bytes()).into_owned(),
                generated: String::from_utf8_lossy(graph.node(*dependency).path.as_bytes())
                    .into_owned(),
                generator_rule: rule_name,
            });
        }
        self.missing_dep_path_count += missing_rules.len();
        self.nodes_missing_deps.insert(node);
    }

    fn path_exists_between(
        &mut self,
        graph: &Graph,
        from: EdgeId,
        to: EdgeId,
        visiting: &mut BTreeSet<EdgeId>,
    ) -> bool {
        let key = (from, to);
        if let Some(found) = self.adjacency.get(&key) {
            return *found;
        }
        if !visiting.insert(key.1) {
            return false;
        }
        let inputs = graph.edge(to).input.clone();
        let found = inputs.iter().any(|input| {
            graph.node(*input).gen.is_some_and(|edge| {
                edge == from || self.path_exists_between(graph, from, edge, visiting)
            })
        });
        visiting.remove(&key.1);
        self.adjacency.insert(key, found);
        found
    }

    pub fn had_missing_dependencies(&self) -> bool {
        !self.nodes_missing_deps.is_empty()
    }

    pub fn nodes_missing_dependencies(&self) -> usize {
        self.nodes_missing_deps.len()
    }

    pub fn generated_nodes(&self) -> usize {
        self.generated_nodes.len()
    }

    pub fn generator_rules(&self) -> usize {
        self.generator_rules.len()
    }

    pub fn missing_dependency_paths(&self) -> usize {
        self.missing_dep_path_count
    }

    pub fn reports(&self) -> &[MissingDependency] {
        &self.reports
    }
}

pub fn root_nodes(graph: &Graph) -> Result<Vec<NodeId>, String> {
    let outputs = graph
        .edge_ids()
        .into_iter()
        .flat_map(|edge| graph.edge(edge).out.clone())
        .collect::<Vec<_>>();
    let roots = outputs
        .iter()
        .filter(|node| graph.node(**node).uses.is_empty())
        .copied()
        .collect::<Vec<_>>();
    if roots.is_empty() && !outputs.is_empty() {
        Err("dependency cycle".into())
    } else {
        Ok(roots)
    }
}

pub fn process_all_nodes(
    graph: &Graph,
    scanner: &mut MissingDependencyScanner,
) -> Result<(), String> {
    for node in root_nodes(graph)? {
        scanner.process_node(graph, node);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{envinit, mkrule, ruleaddvar, EnvironmentId, RuleId};
    use crate::graph::{graphinit, mkedge, mknode, nodeuse};
    use crate::util::{xasprintf, EvalString};

    struct Fixture {
        graph: Graph,
        root: EnvironmentId,
        generator_rule: RuleId,
        compile_rule: RuleId,
    }

    impl Fixture {
        fn new() -> Self {
            let mut graph = graphinit();
            let state = envinit(&mut graph);
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
            edge_mut.outimpidx = 1;
            self.graph.node_mut(output).gen = Some(edge);
            output
        }

        fn add_graph_dependency(&mut self, from: NodeId, to: NodeId) {
            let edge = self.graph.node(from).gen.unwrap();
            nodeuse(&mut self.graph, to, edge);
            let edge_mut = self.graph.edge_mut(edge);
            edge_mut.input.push(to);
            let input_count = edge_mut.input.len();
            edge_mut.inimpidx = input_count;
            edge_mut.inorderidx = input_count;
        }
    }

    fn deps_rule(graph: &mut Graph, name: &str) -> RuleId {
        let rule = mkrule(graph, name.into());
        ruleaddvar(graph, rule, "deps".into(), EvalString::literal("gcc"));
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
            Err(error) => assert_eq!(error, "dependency cycle"),
            Ok(_) => panic!("cyclic graph unexpectedly had root nodes"),
        }
    }
}
