//! Manifest environments, rules, and pools stored in graph-owned arenas.

use crate::error::GraphError;
use crate::graph::{EdgeId, Graph, NodeId};
use crate::util::{BString, EvalPart, EvalString};
use std::collections::BTreeMap;

macro_rules! arena_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub(crate) struct $name(usize);

        impl $name {
            pub(crate) const fn from_index(index: usize) -> Self {
                Self(index)
            }

            pub(crate) const fn index(self) -> usize {
                self.0
            }
        }
    };
}

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
    pub(crate) numjobs: i32,
    pub(crate) maxjobs: i32,
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
        let console = graph.push_pool(Pool {
            name: "console".into(),
            numjobs: 0,
            maxjobs: 1,
        });
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
pub(crate) fn envvar(graph: &Graph, environment: EnvironmentId, name: &str) -> Option<BString> {
    let mut current = Some(environment);
    while let Some(scope) = current {
        let environment = graph.environment(scope);
        if let Some(value) = environment.bindings.get(name) {
            return Some(value.clone());
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

// [spec:samurai:def:env.merge-fn]
// [spec:samurai:sem:env.merge-fn]
fn merge(parts: &[BString]) -> BString {
    let mut output = Vec::with_capacity(parts.iter().map(|part| part.len()).sum());
    for part in parts {
        output.extend_from_slice(part);
    }
    output.into()
}

// [spec:samurai:def:env.enveval-fn]
// [spec:samurai:sem:env.enveval-fn]
pub(crate) fn enveval(graph: &Graph, environment: EnvironmentId, string: &EvalString) -> BString {
    let mut parts = Vec::new();
    for part in &string.parts {
        match part {
            EvalPart::Literal(value) => parts.push(value.clone()),
            EvalPart::Variable(name) => {
                if let Some(value) = envvar(graph, environment, name) {
                    parts.push(value);
                }
            }
        }
    }
    merge(&parts)
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

// [spec:samurai:def:env.pathlist-fn]
// [spec:samurai:sem:env.pathlist-fn]
pub(crate) fn pathlist(paths: &[BString], separator: u8) -> Option<BString> {
    if paths.is_empty() {
        return None;
    }
    let mut output =
        Vec::with_capacity(paths.iter().map(|path| path.len()).sum::<usize>() + paths.len() - 1);
    for (index, path) in paths.iter().enumerate() {
        if index != 0 {
            output.push(separator);
        }
        output.extend_from_slice(path);
    }
    Some(output.into())
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
    let pool = graph.push_pool(Pool {
        name,
        numjobs: 0,
        maxjobs: 0,
    });
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
pub(crate) fn edgevar(graph: &Graph, edge: EdgeId, name: &str, escape: bool) -> Option<BString> {
    fn evaluate(
        graph: &Graph,
        edge: EdgeId,
        value: &EvalString,
        escape: bool,
        stack: &mut Vec<String>,
    ) -> BString {
        let mut parts = Vec::with_capacity(value.parts.len());
        for part in &value.parts {
            match part {
                EvalPart::Variable(name) => {
                    if let Some(value) = edgevar_inner(graph, edge, name, escape, stack) {
                        parts.push(value);
                    }
                }
                EvalPart::Literal(value) => parts.push(value.clone()),
            }
        }
        merge(&parts)
    }

    fn edgevar_inner(
        graph: &Graph,
        edge_id: EdgeId,
        name: &str,
        escape: bool,
        stack: &mut Vec<String>,
    ) -> Option<BString> {
        let edge = graph.edge(edge_id);
        let computed: Option<(&[NodeId], u8)> = match name {
            "in" => Some((&edge.input[..edge.inimpidx], b' ')),
            "in_newline" => Some((&edge.input[..edge.inimpidx], b'\n')),
            "out" => Some((&edge.out[..edge.outimpidx], b' ')),
            _ => None,
        };
        if let Some((nodes, separator)) = computed {
            let paths = nodes
                .iter()
                .map(|node| crate::graph::nodepath(graph, *node, escape))
                .collect::<Vec<_>>();
            return pathlist(&paths, separator);
        }
        if let Some(value) = edge.bindings.get(name) {
            return Some(value.clone());
        }
        let environment = edge.env;
        let value = edge
            .rule
            .and_then(|rule| graph.rule(rule).bindings.get(name))
            .cloned();
        let Some(value) = value else {
            return envvar(graph, environment, name);
        };
        if stack.iter().any(|active| active == name) {
            return None;
        }
        stack.push(name.to_owned());
        let result = evaluate(graph, edge_id, &value, escape, stack);
        stack.pop();
        Some(result)
    }

    edgevar_inner(graph, edge, name, escape, &mut Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::mkstr;

    #[test]
    fn resolves_nearest_variable_binding() {
        let mut graph = Graph::default();
        let state = EnvState::new(&mut graph);
        let mut root_value = mkstr(4);
        root_value.copy_from_slice(b"root");
        envaddvar(&mut graph, state.root, "value".into(), root_value);
        let child = mkenv(&mut graph, Some(state.root));
        assert_eq!(envvar(&graph, child, "value").unwrap(), b"root");
    }
}
