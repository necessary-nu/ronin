//! Literal environment, rule, and pool model from `env.c`.

use crate::util::{EvalString, SamuraiString};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

// [spec:samurai:def:env.environment]
pub struct Environment {
    pub parent: Option<Rc<Environment>>,
    bindings: RefCell<BTreeMap<String, SamuraiString>>,
    rules: RefCell<BTreeMap<String, Rc<Rule>>>,
}

// [spec:samurai:def:env.rule]
pub struct Rule {
    pub name: String,
    pub(crate) bindings: RefCell<BTreeMap<String, EvalString>>,
}

// [spec:samurai:def:env.pool]
pub struct Pool {
    pub name: String,
    pub numjobs: i32,
    pub maxjobs: i32,
    /// Scheduler edge IDs blocked by this pool's capacity.
    pub work: Vec<usize>,
}

pub struct EnvState {
    pub root: Rc<Environment>,
    pub phony: Rc<Rule>,
    pub console: Rc<RefCell<Pool>>,
    pools: BTreeMap<String, Rc<RefCell<Pool>>>,
}

// [spec:samurai:def:env.addvar-fn]
// [spec:samurai:sem:env.addvar-fn]
fn addvar<T>(tree: &mut BTreeMap<String, T>, name: String, value: T) {
    tree.insert(name, value);
}

// [spec:samurai:def:env.mkenv-fn]
// [spec:samurai:sem:env.mkenv-fn]
pub fn mkenv(parent: Option<Rc<Environment>>) -> Rc<Environment> {
    Rc::new(Environment {
        parent,
        bindings: RefCell::new(BTreeMap::new()),
        rules: RefCell::new(BTreeMap::new()),
    })
}

// [spec:samurai:def:env.mkrule-fn]
// [spec:samurai:sem:env.mkrule-fn]
pub fn mkrule(name: String) -> Rc<Rule> {
    Rc::new(Rule {
        name,
        bindings: RefCell::new(BTreeMap::new()),
    })
}

// [spec:samurai:def:env.envaddrule-fn]
// [spec:samurai:sem:env.envaddrule-fn]
pub fn envaddrule(env: &Rc<Environment>, rule: Rc<Rule>) -> Result<(), String> {
    let mut rules = env.rules.borrow_mut();
    if rules.contains_key(&rule.name) {
        return Err(format!("rule '{}' redefined", rule.name));
    }
    rules.insert(rule.name.clone(), rule);
    Ok(())
}

// [spec:samurai:def:env.addpool-fn]
// [spec:samurai:sem:env.addpool-fn]
fn addpool(state: &mut EnvState, pool: Rc<RefCell<Pool>>) -> Result<(), String> {
    let name = pool.borrow().name.clone();
    if state.pools.contains_key(&name) {
        return Err(format!("pool '{name}' redefined"));
    }
    state.pools.insert(name, pool);
    Ok(())
}

// [spec:samurai:def:env.envinit-fn]
// [spec:samurai:sem:env.envinit-fn]
pub fn envinit() -> EnvState {
    let root = mkenv(None);
    let phony = mkrule("phony".into());
    envaddrule(&root, phony.clone()).expect("fresh root rule table");
    let console = Rc::new(RefCell::new(Pool {
        name: "console".into(),
        numjobs: 0,
        maxjobs: 1,
        work: Vec::new(),
    }));
    let mut state = EnvState {
        root,
        phony,
        console: console.clone(),
        pools: BTreeMap::new(),
    };
    addpool(&mut state, console).expect("fresh pool table");
    state
}

// [spec:samurai:def:env.envvar-fn]
// [spec:samurai:sem:env.envvar-fn]
pub fn envvar(env: &Rc<Environment>, name: &str) -> Option<SamuraiString> {
    let mut current = Some(env.clone());
    while let Some(scope) = current {
        if let Some(value) = scope.bindings.borrow().get(name) {
            return Some(value.clone());
        }
        current = scope.parent.clone();
    }
    None
}

// [spec:samurai:def:env.envaddvar-fn]
// [spec:samurai:sem:env.envaddvar-fn]
pub fn envaddvar(env: &Rc<Environment>, name: String, value: SamuraiString) {
    addvar(&mut env.bindings.borrow_mut(), name, value);
}

// [spec:samurai:def:env.merge-fn]
// [spec:samurai:sem:env.merge-fn]
fn merge(parts: &[SamuraiString]) -> SamuraiString {
    let n = parts.iter().map(|part| part.n).sum();
    let mut output = Vec::with_capacity(n + 1);
    for part in parts {
        output.extend_from_slice(&part.s[..part.n]);
    }
    output.push(0);
    SamuraiString { n, s: output }
}

// [spec:samurai:def:env.enveval-fn]
// [spec:samurai:sem:env.enveval-fn]
pub fn enveval(env: &Rc<Environment>, string: &mut EvalString) -> SamuraiString {
    let mut parts = Vec::new();
    let mut current = Some(string);
    while let Some(fragment) = current {
        if let Some(name) = &fragment.var {
            if let Ok(name) = std::str::from_utf8(name) {
                fragment.string = envvar(env, name);
            } else {
                fragment.string = None;
            }
        }
        if let Some(value) = &fragment.string {
            parts.push(value.clone());
        }
        current = fragment.next.as_deref_mut();
    }
    merge(&parts)
}

// [spec:samurai:def:env.envrule-fn]
// [spec:samurai:sem:env.envrule-fn]
pub fn envrule(env: &Rc<Environment>, name: &str) -> Option<Rc<Rule>> {
    let mut current = Some(env.clone());
    while let Some(scope) = current {
        if let Some(rule) = scope.rules.borrow().get(name) {
            return Some(rule.clone());
        }
        current = scope.parent.clone();
    }
    None
}

// [spec:samurai:def:env.pathlist-fn]
// [spec:samurai:sem:env.pathlist-fn]
pub fn pathlist(paths: &[SamuraiString], separator: u8) -> Option<SamuraiString> {
    if paths.is_empty() {
        return None;
    }
    let n = paths.iter().map(|path| path.n).sum::<usize>() + paths.len() - 1;
    let mut output = Vec::with_capacity(n + 1);
    for (index, path) in paths.iter().enumerate() {
        if index != 0 {
            output.push(separator);
        }
        output.extend_from_slice(&path.s[..path.n]);
    }
    output.push(0);
    Some(SamuraiString { n, s: output })
}

// [spec:samurai:def:env.ruleaddvar-fn]
// [spec:samurai:sem:env.ruleaddvar-fn]
pub fn ruleaddvar(rule: &Rc<Rule>, name: String, value: EvalString) {
    addvar(&mut rule.bindings.borrow_mut(), name, value);
}

// [spec:samurai:def:env.mkpool-fn]
// [spec:samurai:sem:env.mkpool-fn]
pub fn mkpool(state: &mut EnvState, name: String) -> Result<Rc<RefCell<Pool>>, String> {
    let pool = Rc::new(RefCell::new(Pool {
        name,
        numjobs: 0,
        maxjobs: 0,
        work: Vec::new(),
    }));
    addpool(state, pool.clone())?;
    Ok(pool)
}

// [spec:samurai:def:env.delrule-fn]
// [spec:samurai:sem:env.delrule-fn]
pub fn delrule(_rule: Rc<Rule>) {}

// [spec:samurai:def:env.delpool-fn]
// [spec:samurai:sem:env.delpool-fn]
pub fn delpool(_pool: Rc<RefCell<Pool>>) {}

// [spec:samurai:def:env.poolget-fn]
// [spec:samurai:sem:env.poolget-fn]
pub fn poolget(state: &EnvState, name: &str) -> Result<Rc<RefCell<Pool>>, String> {
    state
        .pools
        .get(name)
        .cloned()
        .ok_or_else(|| format!("unknown pool '{name}'"))
}

// [spec:samurai:def:env.edgevar-fn]
// [spec:samurai:sem:env.edgevar-fn]
pub fn edgevar(edge: &crate::graph::EdgeRef, name: &str, escape: bool) -> Option<SamuraiString> {
    fn evaluate(
        edge: &crate::graph::EdgeRef,
        value: &EvalString,
        escape: bool,
        stack: &mut Vec<String>,
    ) -> SamuraiString {
        let mut parts = Vec::new();
        let mut current = Some(value);
        while let Some(fragment) = current {
            if let Some(name) = &fragment.var {
                if let Ok(name) = std::str::from_utf8(name) {
                    if let Some(value) = edgevar_inner(edge, name, escape, stack) {
                        parts.push(value);
                    }
                }
            } else if let Some(value) = &fragment.string {
                parts.push(value.clone());
            }
            current = fragment.next.as_deref();
        }
        merge(&parts)
    }

    fn edgevar_inner(
        edge: &crate::graph::EdgeRef,
        name: &str,
        escape: bool,
        stack: &mut Vec<String>,
    ) -> Option<SamuraiString> {
        let (env, rule, direct, paths, separator) = {
            let edge = edge.borrow();
            let computed = match name {
                "in" => Some((&edge.input[..edge.inimpidx], b' ')),
                "in_newline" => Some((&edge.input[..edge.inimpidx], b'\n')),
                "out" => Some((&edge.out[..edge.outimpidx], b' ')),
                _ => None,
            };
            if let Some((nodes, separator)) = computed {
                let paths = nodes
                    .iter()
                    .map(|node| crate::graph::nodepath(node, escape))
                    .collect();
                (edge.env.clone(), None, None, paths, Some(separator))
            } else {
                (
                    edge.env.clone(),
                    edge.rule.clone(),
                    edge.bindings.get(name).cloned(),
                    Vec::new(),
                    None,
                )
            }
        };
        if let Some(separator) = separator {
            return pathlist(&paths, separator);
        }
        if direct.is_some() {
            return direct;
        }
        let value = rule.and_then(|rule| rule.bindings.borrow().get(name).cloned());
        let Some(value) = value else {
            return envvar(&env, name);
        };
        if stack.iter().any(|active| active == name) {
            return None;
        }
        stack.push(name.to_owned());
        let result = evaluate(edge, &value, escape, stack);
        stack.pop();
        Some(result)
    }

    edgevar_inner(edge, name, escape, &mut Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::mkstr;

    #[test]
    fn resolves_nearest_variable_binding() {
        let state = envinit();
        let mut root_value = mkstr(4);
        root_value.s[..4].copy_from_slice(b"root");
        envaddvar(&state.root, "value".into(), root_value);
        let child = mkenv(Some(state.root.clone()));
        assert_eq!(&envvar(&child, "value").unwrap().s[..4], b"root");
    }
}
