//! Manifest environments, rules, and pools stored in graph-owned arenas.

use crate::error::GraphError;
use crate::graph::{EdgeId, Graph, NodeId, PathStyle};
use crate::names::{Bindings, Names, VarId};
use crate::util::{BStr, BString, ByteSlice, EvalPart, EvalString, arena_id};
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

arena_id!(EnvironmentId);
arena_id!(RuleId);
arena_id!(PoolId);

// [spec:ronin:def:tree.treenode]
// [spec:ronin:def:tree.deltree-fn]
// [spec:ronin:sem:tree.deltree-fn]
// [spec:ronin:def:tree.height-fn]
// [spec:ronin:sem:tree.height-fn]
// [spec:ronin:def:tree.rot-fn]
// [spec:ronin:sem:tree.rot-fn]
// [spec:ronin:def:tree.balance-fn]
// [spec:ronin:sem:tree.balance-fn]
// [spec:ronin:def:tree.treefind-fn]
// [spec:ronin:sem:tree.treefind-fn]
// [spec:ronin:def:tree.treeinsert-fn]
// [spec:ronin:sem:tree.treeinsert-fn]
// [spec:ronin:def:env.environment]
pub(crate) struct Environment {
    pub(crate) parent: Option<EnvironmentId>,
    pub(crate) bindings: Bindings<BString>,
    pub(crate) rules: BTreeMap<BString, RuleId>,
}

// [spec:ronin:def:env.rule]
pub(crate) struct Rule {
    pub(crate) name: BString,
    pub(crate) bindings: Bindings<EvalString>,
}

// [spec:ronin:def:env.pool]
pub(crate) struct Pool {
    pub(crate) name: BString,
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
    pools: BTreeMap<BString, PoolId>,
}

impl EnvState {
    // [spec:ronin:def:env.envinit-fn]
    // [spec:ronin:sem:env.envinit-fn]
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

// [spec:ronin:def:env.mkenv-fn]
// [spec:ronin:sem:env.mkenv-fn]
pub(crate) fn mkenv(graph: &mut Graph, parent: Option<EnvironmentId>) -> EnvironmentId {
    graph.push_environment(Environment {
        parent,
        bindings: Bindings::default(),
        rules: BTreeMap::new(),
    })
}

// [spec:ronin:def:env.mkrule-fn]
// [spec:ronin:sem:env.mkrule-fn]
// [spec:ronin:def:env.delrule-fn]
// [spec:ronin:sem:env.delrule-fn]
pub(crate) fn mkrule(graph: &mut Graph, name: BString) -> RuleId {
    graph.push_rule(Rule {
        name,
        bindings: Bindings::default(),
    })
}

// [spec:ronin:def:env.envaddrule-fn]
// [spec:ronin:sem:env.envaddrule-fn]
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

// [spec:ronin:def:env.addpool-fn]
// [spec:ronin:sem:env.addpool-fn]
fn addpool(graph: &Graph, state: &mut EnvState, pool: PoolId) -> Result<(), GraphError> {
    let name = graph.pool(pool).name.clone();
    if state.pools.contains_key(&name) {
        return Err(GraphError::DuplicatePool { name });
    }
    state.pools.insert(name, pool);
    Ok(())
}

// [spec:ronin:def:env.envvar-fn]
// [spec:ronin:sem:env.envvar-fn]
pub(crate) fn envvar(graph: &Graph, environment: EnvironmentId, name: VarId) -> Option<&BString> {
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

/// Look up a variable for a caller holding a name rather than a symbol.
///
/// A name that was never interned cannot have been bound, so failing to
/// resolve it is the same answer as an unbound variable.
pub(crate) fn envvar_named<'graph>(
    graph: &'graph Graph,
    environment: EnvironmentId,
    name: &BStr,
) -> Option<&'graph BString> {
    envvar(graph, environment, graph.names().lookup(name)?)
}

// [spec:ronin:def:env.envaddvar-fn]
// [spec:ronin:sem:env.envaddvar-fn]
pub(crate) fn envaddvar(
    graph: &mut Graph,
    environment: EnvironmentId,
    name: VarId,
    value: BString,
) {
    graph
        .environment_mut(environment)
        .bindings
        .insert(name, value);
}

// [spec:ronin:def:env.envrule-fn]
// [spec:ronin:sem:env.envrule-fn]
pub(crate) fn envrule(graph: &Graph, environment: EnvironmentId, name: &BStr) -> Option<RuleId> {
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

// [spec:ronin:def:env.ruleaddvar-fn]
// [spec:ronin:sem:env.ruleaddvar-fn]
pub(crate) fn ruleaddvar(graph: &mut Graph, rule: RuleId, name: VarId, value: EvalString) {
    graph.rule_mut(rule).bindings.insert(name, value);
}

// [spec:ronin:def:env.mkpool-fn]
// [spec:ronin:sem:env.mkpool-fn]
// [spec:ronin:def:env.delpool-fn]
// [spec:ronin:sem:env.delpool-fn]
pub(crate) fn mkpool(
    graph: &mut Graph,
    state: &mut EnvState,
    name: BString,
) -> Result<PoolId, GraphError> {
    let pool = graph.push_pool(Pool { name, depth: None });
    addpool(graph, state, pool)?;
    Ok(pool)
}

// [spec:ronin:def:env.poolget-fn]
// [spec:ronin:sem:env.poolget-fn]
pub(crate) fn poolget(state: &EnvState, name: &BStr) -> Result<PoolId, GraphError> {
    state
        .pools
        .get(name)
        .copied()
        .ok_or_else(|| GraphError::UnknownPool {
            name: name.to_owned(),
        })
}

// [spec:ronin:def:env.edgevar-fn]
// [spec:ronin:sem:env.edgevar-fn]
pub(crate) fn edgevar(
    graph: &Graph,
    edge: EdgeId,
    name: VarId,
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
    name: VarId,
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
    active: Vec<VarId>,
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

impl Evaluator<'_> {
    fn append_variable(&mut self, edge_id: EdgeId, name: VarId, out: &mut Vec<u8>) -> bool {
        // Copying the shared graph reference out of `self` keeps the borrows
        // that follow independent of the recursive `&mut self`.
        let graph = self.graph;
        let edge = graph.edge(edge_id);
        let computed: Option<(&[NodeId], u8)> = if name == Names::IN {
            Some((edge.explicit_inputs(), b' '))
        } else if name == Names::IN_NEWLINE {
            Some((edge.explicit_inputs(), b'\n'))
        } else if name == Names::OUT {
            Some((edge.explicit_outputs(), b' '))
        } else {
            None
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
                    self.append_variable(edge_id, *name, out);
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
        let value = graph.names_mut().intern(BStr::new("value"));
        envaddvar(&mut graph, state.root, value, BString::from("root"));
        let child = mkenv(&mut graph, Some(state.root));
        assert_eq!(envvar(&graph, child, value).unwrap(), b"root");
    }
}
