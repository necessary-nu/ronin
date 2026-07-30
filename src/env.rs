//! Manifest environments, rules, and pools stored in graph-owned arenas.

use crate::error::GraphError;
use crate::graph::{EdgeId, Graph, NodeId, PathStyle};
use crate::scan::{ScannedEvalPart, ScannedEvalString};
use crate::util::{arena_id, BString, ByteSlice, EvalPart, EvalString};
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

arena_id!(EnvironmentId);
arena_id!(RuleId);
arena_id!(PoolId);

// [spec:samurai:def:tree.treenode]
// [spec:samurai:def:tree.deltree-fn]
// [spec:samurai:sem:tree.deltree-fn]
// [spec:samurai:def:tree.height-fn]
// [spec:samurai:sem:tree.height-fn]
// [spec:samurai:def:tree.rot-fn]
// [spec:samurai:sem:tree.rot-fn]
// [spec:samurai:def:tree.balance-fn]
// [spec:samurai:sem:tree.balance-fn]
// [spec:samurai:def:tree.treefind-fn]
// [spec:samurai:sem:tree.treefind-fn]
// [spec:samurai:def:tree.treeinsert-fn]
// [spec:samurai:sem:tree.treeinsert-fn]
// [spec:samurai:def:env.environment]
pub(crate) struct Environment {
    pub(crate) parent: Option<EnvironmentId>,
    pub(crate) bindings: BTreeMap<String, BString>,
    pub(crate) rules: BTreeMap<String, RuleId>,
}

// [spec:samurai:def:env.rule]
pub(crate) struct Rule {
    pub(crate) name: String,
    pub(crate) bindings: BTreeMap<String, EvalString>,
}

// [spec:samurai:def:env.pool]
pub(crate) struct Pool {
    pub(crate) name: String,
    depth: Option<NonZeroUsize>,
}

impl Pool {
    pub(crate) const fn depth(&self) -> Option<NonZeroUsize> {
        self.depth
    }

    pub(crate) const fn set_depth(&mut self, depth: NonZeroUsize) {
        self.depth = Some(depth);
    }
}

pub(crate) struct EnvState {
    pub(crate) root: EnvironmentId,
    pools: BTreeMap<String, PoolId>,
}

impl EnvState {
    // [spec:samurai:def:env.envinit-fn]
    // [spec:samurai:sem:env.envinit-fn]
    pub(crate) fn new(graph: &mut Graph) -> Self {
        let root = mkenv(graph, None);
        let phony = mkrule(graph, "phony".into());
        envaddrule(graph, root, phony).expect("fresh root rule table");
        graph.set_phony_rule(phony);
        let console = graph.push_pool(Pool {
            name: "console".into(),
            depth: NonZeroUsize::new(1),
        });
        graph.set_console_pool(console);
        let mut state = Self {
            root,
            pools: BTreeMap::new(),
        };
        addpool(graph, &mut state, console).expect("fresh pool table");
        state
    }
}

// [spec:samurai:def:env.addvar-fn]
// [spec:samurai:sem:env.addvar-fn]
fn addvar<T>(tree: &mut BTreeMap<String, T>, name: String, value: T) {
    tree.insert(name, value);
}

// [spec:samurai:def:env.mkenv-fn]
// [spec:samurai:sem:env.mkenv-fn]
pub(crate) fn mkenv(graph: &mut Graph, parent: Option<EnvironmentId>) -> EnvironmentId {
    graph.push_environment(Environment {
        parent,
        bindings: BTreeMap::new(),
        rules: BTreeMap::new(),
    })
}

// [spec:samurai:def:env.mkrule-fn]
// [spec:samurai:sem:env.mkrule-fn]
// [spec:samurai:def:env.delrule-fn]
// [spec:samurai:sem:env.delrule-fn]
pub(crate) fn mkrule(graph: &mut Graph, name: String) -> RuleId {
    graph.push_rule(Rule {
        name,
        bindings: BTreeMap::new(),
    })
}

// [spec:samurai:def:env.envaddrule-fn]
// [spec:samurai:sem:env.envaddrule-fn]
pub(crate) fn envaddrule(
    graph: &mut Graph,
    environment: EnvironmentId,
    rule: RuleId,
) -> Result<(), GraphError> {
    let name = graph.rule(rule).name.clone();
    let rules = &mut graph.environment_mut(environment).rules;
    if rules.contains_key(&name) {
        return Err(GraphError::DuplicateRule { name });
    }
    rules.insert(name, rule);
    Ok(())
}

// [spec:samurai:def:env.addpool-fn]
// [spec:samurai:sem:env.addpool-fn]
fn addpool(graph: &Graph, state: &mut EnvState, pool: PoolId) -> Result<(), GraphError> {
    let name = graph.pool(pool).name.clone();
    if state.pools.contains_key(&name) {
        return Err(GraphError::DuplicatePool { name });
    }
    state.pools.insert(name, pool);
    Ok(())
}

// [spec:samurai:def:env.envvar-fn]
// [spec:samurai:sem:env.envvar-fn]
pub(crate) fn envvar<'graph>(
    graph: &'graph Graph,
    environment: EnvironmentId,
    name: &str,
) -> Option<&'graph BString> {
    let mut current = Some(environment);
    while let Some(scope) = current {
        let environment = graph.environment(scope);
        if let Some(value) = environment.bindings.get(name) {
            return Some(value);
        }
        current = environment.parent;
    }
    None
}

// [spec:samurai:def:env.envaddvar-fn]
// [spec:samurai:sem:env.envaddvar-fn]
pub(crate) fn envaddvar(
    graph: &mut Graph,
    environment: EnvironmentId,
    name: String,
    value: BString,
) {
    addvar(
        &mut graph.environment_mut(environment).bindings,
        name,
        value,
    );
}

// [spec:samurai:def:env.enveval-fn]
// [spec:samurai:sem:env.enveval-fn]
pub(crate) fn enveval(
    graph: &Graph,
    environment: EnvironmentId,
    string: &ScannedEvalString<'_>,
) -> BString {
    let capacity = string
        .parts
        .iter()
        .map(|part| match part {
            ScannedEvalPart::Literal(value) => value.len(),
            ScannedEvalPart::EscapedByte(_) => 1,
            ScannedEvalPart::Variable(name) => {
                envvar(graph, environment, name).map_or(0, |value| value.len())
            }
        })
        .sum();
    let mut output = Vec::with_capacity(capacity);
    for part in &string.parts {
        match part {
            ScannedEvalPart::Literal(value) => output.extend_from_slice(value),
            ScannedEvalPart::EscapedByte(byte) => output.push(*byte),
            ScannedEvalPart::Variable(name) => {
                if let Some(value) = envvar(graph, environment, name) {
                    output.extend_from_slice(value);
                }
            }
        }
    }
    output.into()
}

// [spec:samurai:def:env.envrule-fn]
// [spec:samurai:sem:env.envrule-fn]
pub(crate) fn envrule(graph: &Graph, environment: EnvironmentId, name: &str) -> Option<RuleId> {
    let mut current = Some(environment);
    while let Some(scope) = current {
        let environment = graph.environment(scope);
        if let Some(rule) = environment.rules.get(name) {
            return Some(*rule);
        }
        current = environment.parent;
    }
    None
}

// [spec:samurai:def:env.ruleaddvar-fn]
// [spec:samurai:sem:env.ruleaddvar-fn]
pub(crate) fn ruleaddvar(graph: &mut Graph, rule: RuleId, name: String, value: EvalString) {
    addvar(&mut graph.rule_mut(rule).bindings, name, value);
}

// [spec:samurai:def:env.mkpool-fn]
// [spec:samurai:sem:env.mkpool-fn]
// [spec:samurai:def:env.delpool-fn]
// [spec:samurai:sem:env.delpool-fn]
pub(crate) fn mkpool(
    graph: &mut Graph,
    state: &mut EnvState,
    name: String,
) -> Result<PoolId, GraphError> {
    let pool = graph.push_pool(Pool { name, depth: None });
    addpool(graph, state, pool)?;
    Ok(pool)
}

// [spec:samurai:def:env.poolget-fn]
// [spec:samurai:sem:env.poolget-fn]
pub(crate) fn poolget(state: &EnvState, name: &str) -> Result<PoolId, GraphError> {
    state
        .pools
        .get(name)
        .copied()
        .ok_or_else(|| GraphError::UnknownPool {
            name: name.to_owned(),
        })
}

// [spec:samurai:def:env.edgevar-fn]
// [spec:samurai:sem:env.edgevar-fn]
pub(crate) fn edgevar(
    graph: &Graph,
    edge: EdgeId,
    name: &str,
    style: PathStyle,
) -> Option<BString> {
    let mut value = Vec::new();
    edgevar_into(graph, edge, name, style, &mut value).then(|| BString::from(value))
}

/// Append one edge variable's value to `out`, reporting whether it resolved.
///
/// A caller evaluating several bindings for the same edge reuses one buffer.
/// Resolving to nothing and resolving to an empty value stay distinct, because
/// Ninja treats an absent `$in` differently from an empty one.
pub(crate) fn edgevar_into(
    graph: &Graph,
    edge: EdgeId,
    name: &str,
    style: PathStyle,
    out: &mut Vec<u8>,
) -> bool {
    Evaluator {
        graph,
        style,
        active: Vec::new(),
    }
    .append_variable(edge, name, out)
}

/// Appending evaluator for edge variables.
///
/// Evaluating into one buffer removes the intermediate part and path vectors,
/// and borrowing rule bindings out of the graph removes an `EvalString` clone
/// per lookup. Ninja and C samurai both evaluate this way; the clones were an
/// artifact of returning owned values from every recursion level.
struct Evaluator<'a> {
    graph: &'a Graph,
    style: PathStyle,
    /// Rule bindings currently being expanded, for cycle detection. Real
    /// manifests nest two or three deep, so a linear scan is the cheapest
    /// structure and it needs no allocation for the names.
    active: Vec<&'a str>,
}

fn append_paths(
    graph: &Graph,
    nodes: &[NodeId],
    style: PathStyle,
    separator: u8,
    out: &mut Vec<u8>,
) -> bool {
    if nodes.is_empty() {
        return false;
    }
    for (index, node) in nodes.iter().enumerate() {
        if index != 0 {
            out.push(separator);
        }
        out.extend_from_slice(crate::graph::nodepath_bytes(graph, *node, style));
    }
    true
}

impl<'a> Evaluator<'a> {
    fn append_variable(&mut self, edge_id: EdgeId, name: &'a str, out: &mut Vec<u8>) -> bool {
        // Copying the shared graph reference out of `self` keeps the borrows
        // that follow independent of the recursive `&mut self`.
        let graph = self.graph;
        let edge = graph.edge(edge_id);
        let computed: Option<(&[NodeId], u8)> = match name {
            "in" => Some((edge.explicit_inputs(), b' ')),
            "in_newline" => Some((edge.explicit_inputs(), b'\n')),
            "out" => Some((edge.explicit_outputs(), b' ')),
            _ => None,
        };
        if let Some((nodes, separator)) = computed {
            return append_paths(graph, nodes, self.style, separator, out);
        }
        if let Some(value) = edge.bindings.get(name) {
            out.extend_from_slice(value.as_bytes());
            return true;
        }
        let Some(value) = edge
            .rule
            .and_then(|rule| graph.rule(rule).bindings.get(name))
        else {
            let Some(value) = envvar(graph, edge.env, name) else {
                return false;
            };
            out.extend_from_slice(value.as_bytes());
            return true;
        };
        if self.active.contains(&name) {
            return false;
        }
        self.active.push(name);
        for part in &value.parts {
            match part {
                EvalPart::Literal(literal) => out.extend_from_slice(literal.as_bytes()),
                EvalPart::Variable(name) => {
                    self.append_variable(edge_id, name, out);
                }
            }
        }
        self.active.pop();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_nearest_variable_binding() {
        let mut graph = Graph::default();
        let state = EnvState::new(&mut graph);
        envaddvar(
            &mut graph,
            state.root,
            "value".into(),
            BString::from("root"),
        );
        let child = mkenv(&mut graph, Some(state.root));
        assert_eq!(envvar(&graph, child, "value").unwrap(), b"root");
    }
}
