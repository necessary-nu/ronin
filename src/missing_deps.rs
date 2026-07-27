//! Detection of generated depfile inputs without a manifest dependency path.

use crate::env::edgevar;
use crate::graph::{EdgeRef, Graph, NodeRef};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingDependency {
    pub consumer: String,
    pub generated: String,
    pub generator_rule: String,
}

#[derive(Default)]
pub struct MissingDependencyScanner {
    dependency_log: BTreeMap<Vec<u8>, Vec<NodeRef>>,
    seen: BTreeSet<usize>,
    nodes_missing_deps: BTreeSet<Vec<u8>>,
    generated_nodes: BTreeSet<Vec<u8>>,
    generator_rules: BTreeSet<String>,
    missing_dep_path_count: usize,
    reports: Vec<MissingDependency>,
    adjacency: BTreeMap<(usize, usize), bool>,
}

fn node_key(node: &NodeRef) -> Vec<u8> {
    let node = node.borrow();
    node.path.s[..node.path.n].to_vec()
}

fn edge_identity(edge: &EdgeRef) -> usize {
    Rc::as_ptr(edge) as usize
}

impl MissingDependencyScanner {
    pub fn record_dependency(&mut self, from: &NodeRef, to: &NodeRef) {
        self.dependency_log
            .entry(node_key(from))
            .or_default()
            .push(to.clone());
    }

    pub fn process_node(&mut self, node: &NodeRef) {
        let Some(edge) = node.borrow().gen.as_ref().and_then(|edge| edge.upgrade()) else {
            return;
        };
        if !self.seen.insert(Rc::as_ptr(node) as usize) {
            return;
        }
        let inputs = edge.borrow().input.clone();
        for input in &inputs {
            self.process_node(input);
        }
        if edgevar(&edge, "deps", false).is_none() {
            return;
        }
        if let Some(dependencies) = self.dependency_log.get(&node_key(node)).cloned() {
            self.process_node_dependencies(node, &edge, &dependencies);
        }
    }

    fn process_node_dependencies(
        &mut self,
        node: &NodeRef,
        consumer_edge: &EdgeRef,
        dependencies: &[NodeRef],
    ) {
        if dependencies
            .iter()
            .any(|dependency| node_key(dependency) == b"build.ninja")
        {
            return;
        }
        let mut generated_edges = BTreeMap::<usize, EdgeRef>::new();
        for dependency in dependencies {
            if let Some(edge) = dependency
                .borrow()
                .gen
                .as_ref()
                .and_then(|edge| edge.upgrade())
            {
                generated_edges.insert(edge_identity(&edge), edge);
            }
        }

        let mut missing_edges = BTreeSet::new();
        for (identity, generator) in &generated_edges {
            if !self.path_exists_between(generator, consumer_edge, &mut BTreeSet::new()) {
                missing_edges.insert(*identity);
            }
        }
        if missing_edges.is_empty() {
            return;
        }

        let mut missing_rules = BTreeSet::new();
        for dependency in dependencies {
            let Some(generator) = dependency
                .borrow()
                .gen
                .as_ref()
                .and_then(|edge| edge.upgrade())
            else {
                continue;
            };
            if !missing_edges.contains(&edge_identity(&generator)) {
                continue;
            }
            let rule_name = generator
                .borrow()
                .rule
                .as_ref()
                .map(|rule| rule.name.clone())
                .unwrap_or_default();
            missing_rules.insert(rule_name.clone());
            self.generated_nodes.insert(node_key(dependency));
            self.generator_rules.insert(rule_name.clone());
            self.reports.push(MissingDependency {
                consumer: String::from_utf8_lossy(&node_key(node)).into_owned(),
                generated: String::from_utf8_lossy(&node_key(dependency)).into_owned(),
                generator_rule: rule_name,
            });
        }
        self.missing_dep_path_count += missing_rules.len();
        self.nodes_missing_deps.insert(node_key(node));
    }

    fn path_exists_between(
        &mut self,
        from: &EdgeRef,
        to: &EdgeRef,
        visiting: &mut BTreeSet<usize>,
    ) -> bool {
        let key = (edge_identity(from), edge_identity(to));
        if let Some(found) = self.adjacency.get(&key) {
            return *found;
        }
        if !visiting.insert(key.1) {
            return false;
        }
        let inputs = to.borrow().input.clone();
        let found = inputs.iter().any(|input| {
            input
                .borrow()
                .gen
                .as_ref()
                .and_then(|edge| edge.upgrade())
                .is_some_and(|edge| {
                    Rc::ptr_eq(&edge, from) || self.path_exists_between(from, &edge, visiting)
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

pub fn root_nodes(graph: &Graph) -> Result<Vec<NodeRef>, String> {
    let outputs = graph
        .edges
        .iter()
        .flat_map(|edge| edge.borrow().out.clone())
        .collect::<Vec<_>>();
    let roots = outputs
        .iter()
        .filter(|node| node.borrow().uses.is_empty())
        .cloned()
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
        scanner.process_node(&node);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{envinit, mkrule, ruleaddvar, Environment, Rule};
    use crate::graph::{graphinit, mkedge, mknode, nodeuse};
    use crate::util::{xasprintf, EvalString};

    struct Fixture {
        graph: Graph,
        root: Rc<Environment>,
        generator_rule: Rc<Rule>,
        compile_rule: Rc<Rule>,
    }

    impl Fixture {
        fn new() -> Self {
            let state = envinit();
            let generator_rule = deps_rule("generator_rule");
            let compile_rule = deps_rule("compile_rule");
            Self {
                graph: graphinit(),
                root: state.root,
                generator_rule,
                compile_rule,
            }
        }

        fn create_initial_state(&mut self) -> (NodeRef, NodeRef) {
            let generated = self.add_output("generated_header", self.generator_rule.clone());
            let compiled = self.add_output("compiled_object", self.compile_rule.clone());
            (generated, compiled)
        }

        fn add_output(&mut self, path: &str, rule: Rc<Rule>) -> NodeRef {
            let output = mknode(&mut self.graph, xasprintf(format_args!("{path}")));
            let edge = mkedge(&mut self.graph, self.root.clone());
            edge.borrow_mut().rule = Some(rule);
            edge.borrow_mut().out.push(output.clone());
            edge.borrow_mut().outimpidx = 1;
            output.borrow_mut().gen = Some(Rc::downgrade(&edge));
            output
        }

        fn add_graph_dependency(&mut self, from: &NodeRef, to: &NodeRef) {
            let edge = from
                .borrow()
                .gen
                .as_ref()
                .and_then(|edge| edge.upgrade())
                .unwrap();
            nodeuse(to, &edge);
            edge.borrow_mut().input.push(to.clone());
            let input_count = edge.borrow().input.len();
            edge.borrow_mut().inimpidx = input_count;
            edge.borrow_mut().inorderidx = input_count;
        }
    }

    fn deps_rule(name: &str) -> Rc<Rule> {
        let rule = mkrule(name.into());
        ruleaddvar(
            &rule,
            "deps".into(),
            EvalString {
                var: None,
                string: Some(xasprintf(format_args!("gcc"))),
                next: None,
            },
        );
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
        scanner.record_dependency(&compiled, &generated);
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
        fixture.add_graph_dependency(&compiled, &generated);
        let mut scanner = MissingDependencyScanner::default();
        scanner.record_dependency(&compiled, &generated);
        process_all_nodes(&fixture.graph, &mut scanner).unwrap();
        assert!(!scanner.had_missing_dependencies());
    }

    #[test]
    fn ninja_missing_deps_indirect_path_fixes_issue() {
        let mut fixture = Fixture::new();
        let (generated, compiled) = fixture.create_initial_state();
        let intermediate = fixture.add_output("intermediate", fixture.generator_rule.clone());
        fixture.add_graph_dependency(&compiled, &intermediate);
        fixture.add_graph_dependency(&intermediate, &generated);
        let mut scanner = MissingDependencyScanner::default();
        scanner.record_dependency(&compiled, &generated);
        process_all_nodes(&fixture.graph, &mut scanner).unwrap();
        assert!(!scanner.had_missing_dependencies());
    }

    #[test]
    fn ninja_missing_deps_reports_both_sides_of_deps_log_cycle() {
        let mut fixture = Fixture::new();
        let (generated, compiled) = fixture.create_initial_state();
        let mut scanner = MissingDependencyScanner::default();
        scanner.record_dependency(&generated, &compiled);
        scanner.record_dependency(&compiled, &generated);
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
        fixture.add_graph_dependency(&compiled, &generated);
        fixture.add_graph_dependency(&generated, &compiled);
        match root_nodes(&fixture.graph) {
            Err(error) => assert_eq!(error, "dependency cycle"),
            Ok(_) => panic!("cyclic graph unexpectedly had root nodes"),
        }
    }
}
