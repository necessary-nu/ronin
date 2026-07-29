//! Manifest parser translated from `parse.c`.

use crate::env::{
    edgevar, envaddrule, enveval, envrule, mkpool, mkrule, poolget, ruleaddvar, EnvState,
    Environment,
};
use crate::graph::{mkedge, mknode, nodeuse, Graph, NodeRef};
use crate::util::{canonpath, BString, ByteSlice, EvalPart, EvalString};
use std::rc::Rc;

// [spec:samurai:def:parse.parseoptions]
#[derive(Clone, Copy, Default)]
pub struct ParseOptions {
    pub dupbuildwarn: bool,
}

pub struct Parser {
    pub options: ParseOptions,
    pub defaults: Vec<NodeRef>,
}

// [spec:samurai:def:parse.parseinit-fn]
// [spec:samurai:sem:parse.parseinit-fn]
pub fn parseinit() -> Parser {
    Parser {
        options: ParseOptions::default(),
        defaults: Vec::new(),
    }
}

// [spec:samurai:def:parse.parselet-fn]
// [spec:samurai:sem:parse.parselet-fn]
fn parselet(line: &str) -> Result<(String, String), String> {
    let (name, value) = line
        .split_once('=')
        .ok_or_else(|| format!("expected assignment: {line}"))?;
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("expected assignment: {line}"));
    }
    Ok((name.to_owned(), value.trim().to_owned()))
}

fn push_literal(parts: &mut Vec<EvalPart>, text: &mut String) {
    if text.is_empty() {
        return;
    }
    let value = BString::from(std::mem::take(text));
    parts.push(EvalPart::Literal(value));
}

fn parsevalue(value: &str) -> Result<EvalString, String> {
    let mut parts = Vec::new();
    let mut literal_part = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '$' {
            literal_part.push(character);
            continue;
        }
        let escaped = characters
            .next()
            .ok_or_else(|| "invalid $ escape".to_owned())?;
        match escaped {
            '$' | ' ' | ':' => literal_part.push(escaped),
            '{' => {
                push_literal(&mut parts, &mut literal_part);
                let mut name = String::new();
                loop {
                    let character = characters
                        .next()
                        .ok_or_else(|| "invalid variable name".to_owned())?;
                    if character == '}' {
                        break;
                    }
                    if !is_variable_character(character) {
                        return Err("invalid variable name".into());
                    }
                    name.push(character);
                }
                parts.push(EvalPart::Variable(name));
            }
            character if is_simple_variable_character(character) => {
                push_literal(&mut parts, &mut literal_part);
                let mut name = String::from(character);
                while let Some(character) = characters.clone().next() {
                    if !is_simple_variable_character(character) {
                        break;
                    }
                    characters.next();
                    name.push(character);
                }
                parts.push(EvalPart::Variable(name));
            }
            _ => return Err("invalid $ escape".into()),
        }
    }
    push_literal(&mut parts, &mut literal_part);
    Ok(EvalString::from_parts(parts))
}

fn is_simple_variable_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
}

fn is_variable_character(character: char) -> bool {
    is_simple_variable_character(character) || character == '.'
}

// [spec:samurai:def:parse.parserule-fn]
// [spec:samurai:sem:parse.parserule-fn]
fn parserule(lines: &[String], index: &mut usize, env: &Rc<Environment>) -> Result<(), String> {
    let name = lines[*index]
        .trim_start()
        .trim_start_matches("rule ")
        .trim()
        .to_owned();
    if name.is_empty() || !name.chars().all(is_variable_character) {
        return Err("expected rule name".into());
    }
    let rule = mkrule(name.clone());
    *index += 1;
    let mut command = false;
    let mut rspfile = false;
    let mut rspfile_content = false;
    while *index < lines.len()
        && lines[*index].starts_with([' ', '\t'])
        && !lines[*index].trim().is_empty()
    {
        let line = lines[*index].trim();
        *index += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = parselet(line)?;
        if !matches!(
            name.as_str(),
            "command"
                | "depfile"
                | "dyndep"
                | "description"
                | "deps"
                | "generator"
                | "pool"
                | "restat"
                | "rspfile"
                | "rspfile_content"
                | "msvc_deps_prefix"
        ) {
            return Err(format!("unexpected rule variable '{name}'"));
        }
        let value = parsevalue(&value)?;
        command |= name == "command";
        rspfile |= name == "rspfile";
        rspfile_content |= name == "rspfile_content";
        ruleaddvar(&rule, name, value);
    }
    if !command {
        return Err(format!("rule '{name}' has no command"));
    }
    if rspfile != rspfile_content {
        return Err(format!(
            "rule '{name}' has rspfile and no rspfile_content or vice versa"
        ));
    }
    envaddrule(env, rule)
}

fn evaluated_path(path: &str, env: &Rc<Environment>) -> Result<BString, String> {
    let value = parsevalue(path)?;
    let value = enveval(env, &value);
    if value.is_empty() {
        return Err("empty path".into());
    }
    Ok(value)
}

fn split_path_fields(text: &str) -> Result<Vec<String>, String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        match character {
            ' ' | '\t' if !field.is_empty() => fields.push(std::mem::take(&mut field)),
            ' ' | '\t' => {}
            '$' => {
                field.push('$');
                let escaped = characters
                    .next()
                    .ok_or_else(|| "invalid $ escape".to_owned())?;
                field.push(escaped);
                if escaped == '{' {
                    loop {
                        let character = characters
                            .next()
                            .ok_or_else(|| "invalid variable name".to_owned())?;
                        field.push(character);
                        if character == '}' {
                            break;
                        }
                    }
                }
            }
            _ => field.push(character),
        }
    }
    if !field.is_empty() {
        fields.push(field);
    }
    Ok(fields)
}

fn split_build_outputs(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    for (index, character) in bytes.iter().enumerate() {
        if *character != b':' {
            continue;
        }
        let dollars = bytes[..index]
            .iter()
            .rev()
            .take_while(|character| **character == b'$')
            .count();
        if dollars % 2 == 0 {
            return Some((&line[..index], &line[index + 1..]));
        }
    }
    None
}

fn node_for(graph: &mut Graph, path: &str, env: &Rc<Environment>) -> Result<NodeRef, String> {
    let mut path = evaluated_path(path, env)?;
    canonpath(&mut path);
    Ok(mknode(graph, path))
}

// [spec:samurai:def:parse.parseedge-fn]
// [spec:samurai:sem:parse.parseedge-fn]
fn parseedge(
    lines: &[String],
    index: &mut usize,
    graph: &mut Graph,
    env: Rc<Environment>,
    state: &EnvState,
    options: &ParseOptions,
) -> Result<(), String> {
    let line = lines[*index].trim_start();
    let build = line.trim_start_matches("build ");
    let (outputs, rest) = split_build_outputs(build).ok_or_else(|| "build lacks ':'".to_owned())?;
    let mut fields = split_path_fields(rest)?.into_iter();
    let rule_name = fields.next().ok_or_else(|| "build lacks rule".to_owned())?;
    let rule = envrule(&env, &rule_name).ok_or_else(|| format!("undefined rule '{rule_name}'"))?;
    let edge = mkedge(graph, env);
    let edge_env = edge.borrow().env.clone();
    let mut out = Vec::new();
    let mut outimpidx = None;
    for output in split_path_fields(outputs)? {
        if output == "|" {
            outimpidx = Some(out.len());
            continue;
        }
        let node = node_for(graph, &output, &edge_env)?;
        if node.borrow().gen.is_some() {
            if !options.dupbuildwarn {
                return Err(format!("multiple rules generate '{output}'"));
            }
            continue;
        }
        node.borrow_mut().gen = Some(Rc::downgrade(&edge));
        out.push(node);
    }
    if out.is_empty() {
        if options.dupbuildwarn {
            graph.edges.pop();
            *index += 1;
            while *index < lines.len()
                && lines[*index].starts_with([' ', '\t'])
                && !lines[*index].trim().is_empty()
            {
                *index += 1;
            }
            return Ok(());
        }
        return Err("build has no outputs".into());
    }

    let mut input = Vec::new();
    let mut stage = 0;
    let mut inimpidx = None;
    let mut inorderidx = None;
    let mut validation = Vec::new();
    for field in fields {
        match field.as_str() {
            "|" if stage == 0 => {
                inimpidx = Some(input.len());
                stage = 1;
            }
            "||" if stage < 2 => {
                if inimpidx.is_none() {
                    inimpidx = Some(input.len());
                }
                inorderidx = Some(input.len());
                stage = 2;
            }
            "|@" if stage < 3 => stage = 3,
            "|" | "||" | "|@" => return Err("unexpected dependency separator".into()),
            _ => {
                let node = node_for(graph, &field, &edge_env)?;
                if stage == 3 {
                    node.borrow_mut().validation_uses.push(Rc::downgrade(&edge));
                    validation.push(node);
                } else {
                    nodeuse(&node, &edge);
                    input.push(node);
                }
            }
        }
    }
    let inimpidx = inimpidx.unwrap_or(input.len());
    let inorderidx = inorderidx.unwrap_or(input.len());
    {
        let mut edge_mut = edge.borrow_mut();
        edge_mut.rule = Some(rule);
        edge_mut.outimpidx = outimpidx.unwrap_or(out.len());
        edge_mut.out = out;
        edge_mut.inimpidx = inimpidx;
        edge_mut.inorderidx = inorderidx;
        edge_mut.input = input;
        edge_mut.validation = validation;
    }

    *index += 1;
    while *index < lines.len()
        && lines[*index].starts_with([' ', '\t'])
        && !lines[*index].trim().is_empty()
    {
        let line = lines[*index].trim();
        *index += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = parselet(line)?;
        let value = parsevalue(&value)?;
        let value = enveval(&edge_env, &value);
        edge.borrow_mut().bindings.insert(name, value);
    }

    if let Some(pool_name) = edgevar(&edge, "pool", true).filter(|pool| !pool.is_empty()) {
        let pool_name = String::from_utf8_lossy(pool_name.as_bytes());
        edge.borrow_mut().pool = Some(poolget(state, &pool_name)?);
    }

    if let Some(mut dyndep_path) = edgevar(&edge, "dyndep", false).filter(|path| !path.is_empty()) {
        canonpath(&mut dyndep_path);
        let dyndep = mknode(graph, dyndep_path.clone());
        if !edge
            .borrow()
            .input
            .iter()
            .any(|input| Rc::ptr_eq(input, &dyndep))
        {
            return Err(format!(
                "dyndep '{}' is not an input",
                String::from_utf8_lossy(dyndep_path.as_bytes())
            ));
        }
        dyndep.borrow_mut().dyndep_pending = true;
        edge.borrow_mut().dyndep = Some(dyndep);
    }

    let ignore_phony_self_reference = {
        let edge = edge.borrow();
        edge.rule.as_ref().is_some_and(|rule| rule.name == "phony")
            && edge.out.len() == 1
            && edge.outimpidx == 1
            && edge.inimpidx == edge.input.len()
    };
    if ignore_phony_self_reference {
        let output = edge.borrow().out[0].clone();
        let mut removed = 0;
        edge.borrow_mut().input.retain(|input| {
            let keep = !Rc::ptr_eq(input, &output);
            removed += usize::from(!keep);
            keep
        });
        if removed != 0 {
            output.borrow_mut().uses.retain(|candidate| {
                candidate
                    .upgrade()
                    .is_none_or(|candidate| !Rc::ptr_eq(&candidate, &edge))
            });
            let mut edge = edge.borrow_mut();
            edge.inimpidx -= removed;
            edge.inorderidx -= removed;
        }
    }
    Ok(())
}

// [spec:samurai:def:parse.parseinclude-fn]
// [spec:samurai:sem:parse.parseinclude-fn]
fn parseinclude(
    path: &str,
    graph: &mut Graph,
    parser: &mut Parser,
    env: Rc<Environment>,
    state: &mut EnvState,
    newscope: bool,
) -> Result<(), String> {
    let path = evaluated_path(path, &env)?;
    let env = if newscope {
        crate::env::mkenv(Some(env))
    } else {
        env
    };
    parse(
        path.to_path().expect("byte paths are valid on Unix"),
        graph,
        parser,
        env,
        state,
    )
}

// [spec:samurai:def:parse.parsedefault-fn]
// [spec:samurai:sem:parse.parsedefault-fn]
fn parsedefault(
    line: &str,
    graph: &Graph,
    parser: &mut Parser,
    env: &Rc<Environment>,
) -> Result<(), String> {
    let targets = split_path_fields(line.trim_start_matches("default "))?;
    if targets.is_empty() {
        return Err("expected target name".into());
    }
    for target in targets {
        let target = evaluated_path(&target, env)?;
        parser.defaults.push(
            crate::graph::nodeget(graph, target.as_bytes()).ok_or_else(|| {
                format!(
                    "unknown target '{}'",
                    String::from_utf8_lossy(target.as_bytes())
                )
            })?,
        );
    }
    Ok(())
}

// [spec:samurai:def:parse.parsepool-fn]
// [spec:samurai:sem:parse.parsepool-fn]
fn parsepool(
    lines: &[String],
    index: &mut usize,
    state: &mut EnvState,
    env: &Rc<Environment>,
) -> Result<(), String> {
    let pool = mkpool(
        state,
        lines[*index].trim_start_matches("pool ").trim().to_owned(),
    )?;
    if pool.borrow().name.is_empty() {
        return Err("expected pool name".into());
    }
    *index += 1;
    while *index < lines.len()
        && lines[*index].starts_with([' ', '\t'])
        && !lines[*index].trim().is_empty()
    {
        let line = lines[*index].trim();
        *index += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = parselet(line)?;
        if name != "depth" {
            return Err(format!("unexpected pool variable '{name}'"));
        }
        let value = parsevalue(&value)?;
        let value = enveval(env, &value);
        let text = String::from_utf8_lossy(value.as_bytes());
        pool.borrow_mut().maxjobs = text
            .parse()
            .map_err(|_| format!("invalid pool depth '{text}'"))?;
    }
    if pool.borrow().maxjobs <= 0 {
        return Err("pool has no depth".into());
    }
    Ok(())
}

// [spec:samurai:def:parse.checkversion-fn]
// [spec:samurai:sem:parse.checkversion-fn]
fn checkversion(version: &str) -> Result<(), String> {
    let mut parts = version.split('.');
    let major: i32 = parts
        .next()
        .ok_or_else(|| "invalid ninja_required_version".to_owned())?
        .parse()
        .map_err(|_| "invalid ninja_required_version".to_owned())?;
    let minor: i32 = parts
        .next()
        .unwrap_or("0")
        .parse()
        .map_err(|_| "invalid ninja_required_version".to_owned())?;
    if major > 1 || major == 1 && minor > 9 {
        Err(format!(
            "ninja_required_version {version} is newer than 1.9"
        ))
    } else {
        Ok(())
    }
}

fn manifestlines(source: &str) -> Result<Vec<String>, String> {
    if !source.is_empty() && !source.ends_with('\n') {
        return Err("unexpected EOF".into());
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut continued = false;
    for line in source.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let dollars = line
            .as_bytes()
            .iter()
            .rev()
            .take_while(|character| **character == b'$')
            .count();
        if current.is_empty() {
            current.push_str(line);
        } else {
            current.push_str(line.trim_start());
        }
        if dollars % 2 == 1 {
            current.pop();
            continued = true;
        } else {
            lines.push(std::mem::take(&mut current));
            continued = false;
        }
    }
    if continued {
        return Err("unexpected EOF after continuation".into());
    }
    Ok(lines)
}

// [spec:samurai:def:parse.parse-fn]
// [spec:samurai:sem:parse.parse-fn]
pub fn parse(
    name: impl AsRef<std::path::Path>,
    graph: &mut Graph,
    parser: &mut Parser,
    env: Rc<Environment>,
    state: &mut EnvState,
) -> Result<(), String> {
    let source = std::fs::read_to_string(name).map_err(|error| error.to_string())?;
    let lines = manifestlines(&source)?;
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim_end();
        let content = line.trim_start();
        if content.is_empty() || content.starts_with('#') {
            index += 1;
            continue;
        }
        if content.len() != line.len() {
            return Err(format!("unexpected indent: {line}"));
        }
        if content.starts_with("rule ") {
            parserule(&lines, &mut index, &env)?;
            continue;
        }
        if content.starts_with("pool ") {
            parsepool(&lines, &mut index, state, &env)?;
            continue;
        }
        if content.starts_with("build ") {
            parseedge(
                &lines,
                &mut index,
                graph,
                env.clone(),
                state,
                &parser.options,
            )?;
            continue;
        } else if content.starts_with("default ") {
            parsedefault(content, graph, parser, &env)?;
        } else if let Some(path) = content.strip_prefix("include ") {
            parseinclude(path.trim(), graph, parser, env.clone(), state, false)?;
        } else if let Some(path) = content.strip_prefix("subninja ") {
            parseinclude(path.trim(), graph, parser, env.clone(), state, true)?;
        } else if let Ok((name, value)) = parselet(content) {
            let value = parsevalue(&value)?;
            let value = enveval(&env, &value);
            if name == "ninja_required_version" {
                checkversion(&String::from_utf8_lossy(value.as_bytes()))?;
            }
            crate::env::envaddvar(&env, name, value);
        } else {
            return Err(format!("unrecognized manifest line: {line}"));
        }
        index += 1;
    }
    Ok(())
}

// [spec:samurai:def:parse.defaultnodes-fn]
// [spec:samurai:sem:parse.defaultnodes-fn]
pub fn defaultnodes(parser: &Parser, graph: &Graph) -> Vec<NodeRef> {
    if !parser.defaults.is_empty() {
        parser.defaults.clone()
    } else {
        graph
            .nodes()
            .into_iter()
            .filter(|node| {
                let node = node.borrow();
                node.gen.is_some() && node.uses.is_empty()
            })
            .collect()
    }
}

#[cfg(test)]
mod ninja_manifest_tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_MANIFEST: AtomicUsize = AtomicUsize::new(0);

    fn parse_source(source: &str) -> Result<(Graph, Parser, EnvState), String> {
        let path = std::env::temp_dir().join(format!(
            "ronin-manifest-parser-{}-{}.ninja",
            std::process::id(),
            NEXT_MANIFEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, source).unwrap();
        let mut graph = crate::graph::graphinit();
        let mut parser = parseinit();
        let mut state = crate::env::envinit();
        let result = parse(
            path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root.clone(),
            &mut state,
        );
        fs::remove_file(path).unwrap();
        result.map(|()| (graph, parser, state))
    }

    fn parse_path(path: &std::path::Path) -> Result<(Graph, Parser, EnvState), String> {
        let mut graph = crate::graph::graphinit();
        let mut parser = parseinit();
        let mut state = crate::env::envinit();
        parse(
            path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root.clone(),
            &mut state,
        )?;
        Ok((graph, parser, state))
    }

    fn temporary_directory(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ronin-manifest-{label}-{}-{}",
            std::process::id(),
            NEXT_MANIFEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn output_edge(graph: &Graph, output: &[u8]) -> crate::graph::EdgeRef {
        crate::graph::nodeget(graph, output)
            .unwrap()
            .borrow()
            .gen
            .as_ref()
            .unwrap()
            .upgrade()
            .unwrap()
    }

    fn parse_error(source: &str) -> String {
        match parse_source(source) {
            Ok(_) => panic!("manifest unexpectedly parsed"),
            Err(error) => error,
        }
    }

    fn assert_dyndep(source: &str, expected: &[u8]) {
        let (graph, _, _) = parse_source(source).unwrap();
        let edge = output_edge(&graph, b"result");
        let dyndep = edge.borrow().dyndep.clone().unwrap();
        assert_eq!(dyndep.borrow().path.as_bytes(), expected);
        assert!(dyndep.borrow().dyndep_pending);
    }

    #[test]
    fn ninja_manifest_parser_validations() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild foo: cat bar |@ baz\n")
                .unwrap();
        let edge = output_edge(&graph, b"foo");
        assert_eq!(edge.borrow().input.len(), 1);
        assert_eq!(edge.borrow().validation.len(), 1);
        assert_eq!(edge.borrow().validation[0].borrow().path.as_bytes(), b"baz");
        assert_eq!(
            crate::graph::nodeget(&graph, b"baz")
                .unwrap()
                .borrow()
                .validation_uses
                .len(),
            1
        );
    }

    #[test]
    fn ninja_manifest_parser_implicit_output() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild foo | imp: cat bar\n")
                .unwrap();
        let edge = output_edge(&graph, b"imp");
        assert_eq!(edge.borrow().out.len(), 2);
        assert_eq!(edge.borrow().outimpidx, 1);
        assert!(Rc::ptr_eq(&edge, &output_edge(&graph, b"foo")));
    }

    #[test]
    fn ninja_manifest_parser_implicit_output_empty() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild foo | : cat bar\n").unwrap();
        let edge = output_edge(&graph, b"foo");
        assert_eq!(edge.borrow().out.len(), 1);
        assert_eq!(edge.borrow().outimpidx, 1);
    }

    #[test]
    fn ninja_manifest_parser_no_explicit_output() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild | imp: cat bar\n").unwrap();
        let edge = output_edge(&graph, b"imp");
        assert_eq!(edge.borrow().out.len(), 1);
        assert_eq!(edge.borrow().outimpidx, 0);
    }

    #[test]
    fn ninja_manifest_parser_implicit_output_duplicate_error() {
        let error = parse_error(
            "rule cat\n  command = cat $in > $out\nbuild foo baz | foo baq foo: cat bar\n",
        );
        assert!(error.contains("multiple rules generate 'foo'"));
    }

    #[test]
    fn ninja_manifest_parser_phony_self_reference_ignored() {
        let (graph, _, _) = parse_source("build a: phony a\n").unwrap();
        let edge = output_edge(&graph, b"a");
        assert!(edge.borrow().input.is_empty());
        assert!(crate::graph::nodeget(&graph, b"a")
            .unwrap()
            .borrow()
            .uses
            .is_empty());
    }

    #[test]
    fn ninja_manifest_parser_reserved_words() {
        let (graph, parser, _) = parse_source(
            "rule build\n  command = rule run $out\nbuild subninja: build include default foo.cc\ndefault subninja\n",
        )
        .unwrap();
        assert!(crate::graph::nodeget(&graph, b"subninja").is_some());
        assert_eq!(defaultnodes(&parser, &graph).len(), 1);
    }

    #[test]
    fn ninja_manifest_parser_dyndep_not_specified() {
        let (graph, _, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild result: cat in\n").unwrap();
        assert!(output_edge(&graph, b"result").borrow().dyndep.is_none());
    }

    #[test]
    fn ninja_manifest_parser_dyndep_not_input() {
        let error = parse_error(
            "rule touch\n  command = touch $out\nbuild result: touch\n  dyndep = notin\n",
        );
        assert_eq!(error, "dyndep 'notin' is not an input");
    }

    #[test]
    fn ninja_manifest_parser_dyndep_explicit_input() {
        assert_dyndep(
            "rule cat\n  command = cat $in > $out\nbuild result: cat in\n  dyndep = in\n",
            b"in",
        );
    }

    #[test]
    fn ninja_manifest_parser_dyndep_implicit_input() {
        assert_dyndep(
            "rule cat\n  command = cat $in > $out\nbuild result: cat in | dd\n  dyndep = dd\n",
            b"dd",
        );
    }

    #[test]
    fn ninja_manifest_parser_dyndep_order_only_input() {
        assert_dyndep(
            "rule cat\n  command = cat $in > $out\nbuild result: cat in || dd\n  dyndep = dd\n",
            b"dd",
        );
    }

    #[test]
    fn ninja_manifest_parser_dyndep_rule_input() {
        assert_dyndep(
            "rule cat\n  command = cat $in > $out\n  dyndep = $in\nbuild result: cat in\n",
            b"in",
        );
    }

    #[test]
    fn ninja_manifest_parser_selects_pool() {
        let (graph, _, _) = parse_source(
            "pool link_pool\n  depth = 15\nrule link\n  command = link\n  pool = link_pool\nbuild result: link input\n",
        )
        .unwrap();
        let pool = output_edge(&graph, b"result")
            .borrow()
            .pool
            .clone()
            .unwrap();
        assert_eq!(pool.borrow().name, "link_pool");
        assert_eq!(pool.borrow().maxjobs, 15);
    }

    #[test]
    fn ninja_manifest_parser_rejects_bad_pools() {
        assert!(parse_source("pool foo\n  depth = -1\n").is_err());
        assert!(parse_source("pool foo\n  depth = word\n").is_err());
        assert!(parse_source("pool foo\n  bar = 1\n").is_err());
        assert!(parse_source(
            "rule run\n  command = echo\n  pool = unnamed_pool\nbuild out: run in\n"
        )
        .is_err());
    }

    #[test]
    fn ninja_manifest_parser_rejects_unknown_rule_binding() {
        let error = parse_error("rule cc\n  command = foo\n  othervar = bar\n");
        assert_eq!(error, "unexpected rule variable 'othervar'");
    }

    #[test]
    fn ninja_manifest_parser_default_cycle_has_no_root() {
        let (graph, parser, _) =
            parse_source("rule cat\n  command = cat $in > $out\nbuild a: cat a\n").unwrap();
        assert!(defaultnodes(&parser, &graph).is_empty());
    }

    #[test]
    fn ninja_manifest_parser_utf8_and_crlf() {
        parse_source(
            "# comment with crlf\r\npool link_pool\r\n  depth = 15\r\n\r\nrule utf8\r\n  command = true\r\n  description = compilación\r\n",
        )
        .unwrap();
    }

    #[test]
    fn ninja_manifest_parser_subninja_scope() {
        let directory = temporary_directory("subninja-scope");
        let child = directory.join("child.ninja");
        let root = directory.join("build.ninja");
        fs::write(&child, "var = inner\nbuild $builddir/inner: varref input\n").unwrap();
        fs::write(
            &root,
            format!(
                "builddir = some_dir\nrule varref\n  command = varref $var\nvar = outer\nbuild $builddir/outer: varref input\nsubninja {}\nbuild $builddir/outer2: varref input\n",
                child.display()
            ),
        )
        .unwrap();
        let (graph, _, _) = parse_path(&root).unwrap();
        let inner = output_edge(&graph, b"some_dir/inner");
        let outer = output_edge(&graph, b"some_dir/outer");
        let outer2 = output_edge(&graph, b"some_dir/outer2");
        let inner_command = edgevar(&inner, "command", false).unwrap();
        let outer_command = edgevar(&outer, "command", false).unwrap();
        let outer2_command = edgevar(&outer2, "command", false).unwrap();
        assert_eq!(inner_command.as_bytes(), b"varref inner");
        assert_eq!(outer_command.as_bytes(), b"varref outer");
        assert_eq!(outer2_command.as_bytes(), b"varref outer");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_duplicate_rule_in_different_subninjas() {
        let directory = temporary_directory("subninja-rules");
        let child = directory.join("child.ninja");
        let root = directory.join("build.ninja");
        fs::write(&child, "rule cat\n  command = child\n").unwrap();
        fs::write(
            &root,
            format!(
                "rule cat\n  command = parent\nsubninja {}\nbuild out: cat input\n",
                child.display()
            ),
        )
        .unwrap();
        parse_path(&root).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_duplicate_rule_across_include_scopes() {
        let directory = temporary_directory("subninja-includes");
        let rules = directory.join("rules.ninja");
        let child = directory.join("child.ninja");
        let root = directory.join("build.ninja");
        fs::write(&rules, "rule cat\n  command = cat\n").unwrap();
        fs::write(
            &child,
            format!("include {}\nbuild x: cat input\n", rules.display()),
        )
        .unwrap();
        fs::write(
            &root,
            format!(
                "include {}\nsubninja {}\nbuild y: cat input\n",
                rules.display(),
                child.display()
            ),
        )
        .unwrap();
        let (graph, _, _) = parse_path(&root).unwrap();
        assert!(crate::graph::nodeget(&graph, b"x").is_some());
        assert!(crate::graph::nodeget(&graph, b"y").is_some());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_include_updates_current_scope() {
        let directory = temporary_directory("include-scope");
        let include = directory.join("include.ninja");
        let root = directory.join("build.ninja");
        fs::write(&include, "var = inner\n").unwrap();
        fs::write(
            &root,
            format!("var = outer\ninclude {}\n", include.display()),
        )
        .unwrap();
        let (_, _, state) = parse_path(&root).unwrap();
        let value = crate::env::envvar(&state.root, "var").unwrap();
        assert_eq!(value.as_bytes(), b"inner");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_broken_and_missing_includes() {
        let directory = temporary_directory("broken-include");
        let broken = directory.join("broken.ninja");
        let root = directory.join("build.ninja");
        fs::write(&broken, "build\n").unwrap();
        fs::write(&root, format!("include {}\n", broken.display())).unwrap();
        assert!(parse_path(&root).is_err());
        fs::write(
            &root,
            format!("subninja {}\n", directory.join("missing.ninja").display()),
        )
        .unwrap();
        assert!(parse_path(&root).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_duplicate_edge_in_included_file() {
        let directory = temporary_directory("duplicate-include");
        let child = directory.join("child.ninja");
        let root = directory.join("build.ninja");
        fs::write(
            &child,
            "rule cat\n  command = cat\nbuild out1 out2: cat in1\nbuild out1: cat in2\n",
        )
        .unwrap();
        fs::write(&root, format!("subninja {}\n", child.display())).unwrap();
        let error = match parse_path(&root) {
            Ok(_) => panic!("duplicate output unexpectedly parsed"),
            Err(error) => error,
        };
        assert!(error.contains("multiple rules generate 'out1'"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_duplicate_output_warning_mode() {
        let path = std::env::temp_dir().join(format!(
            "ronin-manifest-duplicate-warning-{}-{}.ninja",
            std::process::id(),
            NEXT_MANIFEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &path,
            "rule cat\n  command = cat\nbuild out: cat in1\nbuild out: cat in2\n",
        )
        .unwrap();
        let mut graph = crate::graph::graphinit();
        let mut parser = parseinit();
        parser.options.dupbuildwarn = true;
        let mut state = crate::env::envinit();
        parse(
            path.to_str().unwrap(),
            &mut graph,
            &mut parser,
            state.root.clone(),
            &mut state,
        )
        .unwrap();
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(output_edge(&graph, b"out").borrow().input.len(), 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn ninja_manifest_parser_rejects_unterminated_lines() {
        assert_eq!(parse_error("x = 3"), "unexpected EOF");
        assert_eq!(parse_error("x = $\n"), "unexpected EOF after continuation");
        assert_eq!(
            parse_error("x = a$\n b$\n $\n"),
            "unexpected EOF after continuation"
        );
    }

    #[test]
    fn ninja_manifest_parser_indented_blank_terminates_rule() {
        assert!(parse_source("rule r\n  command = r\n  \n  generator = 1\n").is_err());
    }

    #[test]
    fn ninja_manifest_parser_default_escaped_space() {
        let (graph, parser, _) = parse_source(
            "rule cat\n  command = cat\nbuild foo$ bar: cat input\ndefault foo$ bar\n",
        )
        .unwrap();
        let defaults = defaultnodes(&parser, &graph);
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].borrow().path.as_bytes(), b"foo bar");
    }

    #[test]
    fn ninja_state_complex_target_is_preserved() {
        let (graph, _, _) = parse_source(
            "rule copy\n  command = cp $in $out\nname = foo %2F bar?baz&x=1\nbuild $name: copy foo\n",
        )
        .unwrap();
        let node = crate::graph::nodeget(&graph, b"foo %2F bar?baz&x=1").unwrap();
        assert_eq!(node.borrow().path.as_bytes(), b"foo %2F bar?baz&x=1");
    }
}
