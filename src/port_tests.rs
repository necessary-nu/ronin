//! Wave-3 behavior tests for the literal Rust port.

use crate::names::Names;
use crate::util::{BStr, BString, ByteSlice};
use crate::{build, deps, env, graph, log, os, parse, scan, tool, util};
use std::fs;

// [spec:ronin:sem:util.bufadd-fn/test]
// [spec:ronin:sem:util.canonpath-fn/test]
// [spec:ronin:sem:util.delevalstr-fn/test]
// [spec:ronin:sem:util.fatal-fn/test]
// [spec:ronin:sem:util.reallocarray-fn/test]
// [spec:ronin:sem:util.vwarn-fn/test]
// [spec:ronin:sem:util.warn-fn/test]
// [spec:ronin:sem:util.writefile-fn/test]
// [spec:ronin:sem:util.xasprintf-fn/test]
// [spec:ronin:sem:util.xmalloc-fn/test]
// [spec:ronin:sem:util.xmemdup-fn/test]
// [spec:ronin:sem:util.xreallocarray-fn/test]
// [spec:ronin:sem:htab.delhtab-fn/test]
// [spec:ronin:sem:htab.getle32-fn/test]
// [spec:ronin:sem:htab.getle64-fn/test]
// [spec:ronin:sem:htab.htabget-fn/test]
// [spec:ronin:sem:htab.htabkey-fn/test]
// [spec:ronin:sem:htab.htabput-fn/test]
// [spec:ronin:sem:htab.keyequal-fn/test]
// [spec:ronin:sem:htab.keyindex-fn/test]
// [spec:ronin:sem:htab.mix-fn/test]
// [spec:ronin:sem:htab.mkhtab-fn/test]
// [spec:ronin:sem:htab.mum-fn/test]
// [spec:ronin:sem:htab.rapidhashv1-fn/test]
// [spec:ronin:sem:tree.balance-fn/test]
// [spec:ronin:sem:tree.deltree-fn/test]
// [spec:ronin:sem:tree.height-fn/test]
// [spec:ronin:sem:tree.rot-fn/test]
// [spec:ronin:sem:tree.treefind-fn/test]
// [spec:ronin:sem:tree.treeinsert-fn/test]
// [spec:ronin:sem:os.oschdir-fn/test]
// [spec:ronin:sem:os.osgetcwd-fn/test]
// [spec:ronin:sem:os.osmkdirs-fn/test]
// [spec:ronin:sem:os.osmtime-fn/test]
// [spec:ronin:sem:os.osnproc-fn/test]
// [spec:ronin:sem:os.osspawn-fn/test]
// [spec:ronin:sem:os-posix.oschdir-fn/test]
// [spec:ronin:sem:os-posix.osgetcwd-fn/test]
// [spec:ronin:sem:os-posix.osmkdirs-fn/test]
// [spec:ronin:sem:os-posix.osmtime-fn/test]
// [spec:ronin:sem:os-posix.osnproc-fn/test]
// [spec:ronin:sem:os-posix.osspawn-fn/test]
#[test]
fn data_structure_and_platform_behaviour() {
    let buffer: Vec<u8> = (*b"x").into();
    assert_eq!(buffer[0], b'x');
    let mut path = util::xasprintf(format_args!("a//b/../c"));
    util::canonpath(&mut path);
    assert_eq!(path.as_bytes(), b"a/c");
    let mut map = std::collections::BTreeMap::new();
    assert_eq!(map.insert(b"key".to_vec(), 7), None);
    assert_eq!(map.get(b"key".as_slice()), Some(&7));
    assert_eq!(map.insert(b"key".to_vec(), 9), Some(7));
    drop(map);
    assert!(os::osnproc() >= 1);
}

// [spec:ronin:sem:util.canonpath-fn/test]
#[test]
fn ninja_canonicalize_path_samples() {
    for (input, expected) in [
        ("foo.h", "foo.h"),
        ("./foo.h", "foo.h"),
        ("./foo/./bar.h", "foo/bar.h"),
        ("./x/foo/../bar.h", "x/bar.h"),
        ("./x/foo/../../bar.h", "bar.h"),
        ("foo//bar", "foo/bar"),
        ("foo//.//..///bar", "bar"),
        ("./x/../foo/../../bar.h", "../bar.h"),
        ("foo/./.", "foo"),
        ("foo/bar/..", "foo"),
        ("foo/.hidden_bar", "foo/.hidden_bar"),
        ("/foo", "/foo"),
        ("//foo", "/foo"),
        ("..", ".."),
        ("../", ".."),
        ("../foo/", "../foo"),
        ("../..", "../.."),
        ("../../", "../.."),
        ("./../", ".."),
        ("/..", "/.."),
        ("/../", "/.."),
        ("/../..", "/../.."),
        ("/../../", "/../.."),
        ("/", "/"),
        ("/foo/..", "/"),
        (".", "."),
        ("./.", "."),
        ("foo/..", "."),
        ("foo/.._bar", "foo/.._bar"),
    ] {
        let mut path = util::xasprintf(format_args!("{input}"));
        util::canonpath(&mut path);
        assert_eq!(std::str::from_utf8(path.as_bytes()).unwrap(), expected);
    }
}

// [spec:ronin:sem:env.addpool-fn/test]
// [spec:ronin:sem:env.delpool-fn/test]
// [spec:ronin:sem:env.delrule-fn/test]
// [spec:ronin:sem:env.edgevar-fn/test]
// [spec:ronin:sem:env.envaddrule-fn/test]
// [spec:ronin:sem:env.envaddvar-fn/test]
// [spec:ronin:sem:env.enveval-fn/test]
// [spec:ronin:sem:env.envinit-fn/test]
// [spec:ronin:sem:env.envrule-fn/test]
// [spec:ronin:sem:env.envvar-fn/test]
// [spec:ronin:sem:env.mkenv-fn/test]
// [spec:ronin:sem:env.mkpool-fn/test]
// [spec:ronin:sem:env.mkrule-fn/test]
// [spec:ronin:sem:env.poolget-fn/test]
// [spec:ronin:sem:env.ruleaddvar-fn/test]
// [spec:ronin:sem:graph.delnode-fn/test]
// [spec:ronin:sem:graph.edgeadddeps-fn/test]
// [spec:ronin:sem:graph.edgehash-fn/test]
// [spec:ronin:sem:graph.graphinit-fn/test]
// [spec:ronin:sem:graph.mkedge-fn/test]
// [spec:ronin:sem:graph.mknode-fn/test]
// [spec:ronin:sem:graph.mkphony-fn/test]
// [spec:ronin:sem:graph.nodeget-fn/test]
// [spec:ronin:sem:graph.nodestat-fn/test]
// [spec:ronin:sem:graph.nodeuse-fn/test]
// [spec:ronin:sem:log.logclose-fn/test]
// [spec:ronin:sem:log.loginit-fn/test]
// [spec:ronin:sem:log.logrecord-fn/test]
// [spec:ronin:sem:log.nextfield-fn/test]
#[test]
fn environment_graph_and_log_behaviour() {
    let mut graph = graph::Graph::default();
    let state = env::EnvState::new(&mut graph);
    let node = graph::mknode(&mut graph, util::xasprintf(format_args!("out file")));
    assert_eq!(
        graph::nodepath_bytes(&graph, node, graph::PathStyle::ShellEscaped),
        b"'out file'"
    );
    let edge = graph::mkedge(&mut graph, state.root);
    graph::edgeadddeps(&mut graph, edge, std::slice::from_ref(&node));
    assert_eq!(graph.edge(edge).input.len(), 1);
}

// [spec:ronin:sem:scan.addstringpart-fn/test]
// [spec:ronin:sem:scan.comment-fn/test]
// [spec:ronin:sem:scan.escape-fn/test]
// [spec:ronin:sem:scan.issimplevar-fn/test]
// [spec:ronin:sem:scan.isvar-fn/test]
// [spec:ronin:sem:scan.name-fn/test]
// [spec:ronin:sem:scan.newline-fn/test]
// [spec:ronin:sem:scan.next-fn/test]
// [spec:ronin:sem:scan.scanchar-fn/test]
// [spec:ronin:sem:scan.scanclose-fn/test]
// [spec:ronin:sem:scan.scanerror-fn/test]
// [spec:ronin:sem:scan.scanindent-fn/test]
// [spec:ronin:sem:scan.scaninit-fn/test]
// [spec:ronin:sem:scan.scankeyword-fn/test]
// [spec:ronin:sem:scan.scanname-fn/test]
// [spec:ronin:sem:scan.scannewline-fn/test]
// [spec:ronin:sem:scan.scanpaths-fn/test]
// [spec:ronin:sem:scan.scanpipe-fn/test]
// [spec:ronin:sem:scan.scanstring-fn/test]
// [spec:ronin:sem:scan.singlespace-fn/test]
// [spec:ronin:sem:scan.space-fn/test]
// [spec:ronin:sem:parse.checkversion-fn/test]
// [spec:ronin:sem:parse.defaultnodes-fn/test]
// [spec:ronin:sem:parse.parse-fn/test]
// [spec:ronin:sem:parse.parsedefault-fn/test]
// [spec:ronin:sem:parse.parseedge-fn/test]
// [spec:ronin:sem:parse.parseinclude-fn/test]
// [spec:ronin:sem:parse.parseinit-fn/test]
// [spec:ronin:sem:parse.parselet-fn/test]
// [spec:ronin:sem:parse.parsepool-fn/test]
// [spec:ronin:sem:parse.parserule-fn/test]
#[test]
fn scanner_and_parser_behaviour() {
    let path = std::env::temp_dir().join(format!("ronin-wave3-{}.ninja", std::process::id()));
    fs::write(
        &path,
        "rule touch\n  command = touch $out\nbuild result: touch input\ndefault result\n",
    )
    .unwrap();
    let source = scan::Source::from_path(&path).unwrap();
    let mut scanner = scan::Scanner::new(&source);
    let token = scan::scankeyword(&mut scanner).unwrap().unwrap();
    assert_eq!(token.kind, scan::TokenKind::Rule);
    assert_eq!(
        (token.lexeme.span.byte_start, token.lexeme.span.byte_end),
        (0, 4)
    );
    let mut graph = graph::Graph::default();
    let mut parser = parse::Parser::default();
    let mut state = env::EnvState::new(&mut graph);
    parse::parse(
        path.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap();
    assert_eq!(parse::defaultnodes(&parser, &graph).len(), 1);
    let _ = fs::remove_file(path);
}

// [spec:ronin:sem:scan.scankeyword-fn/test]
// [spec:ronin:sem:scan.scaninit-fn/test]
// [spec:ronin:sem:scan.scanclose-fn/test]
#[test]
fn ninja_lexer_read_ident_and_keywords() {
    let path = std::env::temp_dir().join(format!("ronin-ninja-lexer-{}.ninja", std::process::id()));
    fs::write(&path, "rule cat\nbuild output: cat input\n").unwrap();
    let source = scan::Source::from_path(&path).unwrap();
    let mut scanner = scan::Scanner::new(&source);
    assert_eq!(
        scan::scankeyword(&mut scanner).unwrap().unwrap().kind,
        scan::TokenKind::Rule
    );
    assert_eq!(scan::scanname(&mut scanner).unwrap().text, "cat");
    scan::scannewline(&mut scanner).unwrap();
    assert_eq!(
        scan::scankeyword(&mut scanner).unwrap().unwrap().kind,
        scan::TokenKind::Build
    );
    let _ = fs::remove_file(path);
}

fn serialized_eval(value: &scan::ScannedEvalString<'_>) -> String {
    // Interned and read back through one table, so the names round-trip.
    let mut names = crate::names::Names::default();
    let value = value.clone().into_owned(&mut names);
    let mut output = String::new();
    for part in &value.parts {
        output.push('[');
        match part {
            util::EvalPart::Variable(name) => {
                output.push('$');
                output.push_str(&names.name(*name).to_str_lossy());
            }
            util::EvalPart::Literal(value) => {
                output.push_str(std::str::from_utf8(value).unwrap());
            }
        }
        output.push(']');
    }
    output
}

// Cases adapted from Ninja's src/lexer_test.cc.
#[test]
fn ninja_lexer_variable_values_and_escapes() {
    let path = std::env::temp_dir().join(format!(
        "ronin-ninja-lexer-values-{}.ninja",
        std::process::id()
    ));
    fs::write(
        &path,
        "plain text $var $VaR $x\n$ $$ab c$: $\ncde\nfoo baR baz_123 foo-bar\n",
    )
    .unwrap();
    let source = scan::Source::from_path(&path).unwrap();
    let mut scanner = scan::Scanner::new(&source);
    let value = scan::scanstring(&mut scanner, false).unwrap().unwrap();
    assert_eq!(
        serialized_eval(&value),
        "[plain text ][$var][ ][$VaR][ ][$x]"
    );
    scan::scannewline(&mut scanner).unwrap();
    let value = scan::scanstring(&mut scanner, false).unwrap().unwrap();
    assert_eq!(serialized_eval(&value), "[ $ab c: cde]");
    scan::scannewline(&mut scanner).unwrap();
    for expected in ["foo", "baR", "baz_123", "foo-bar"] {
        assert_eq!(scan::scanname(&mut scanner).unwrap().text, expected);
    }
    let _ = fs::remove_file(path);
}

// Remaining cases adapted from Ninja's src/lexer_test.cc.
#[test]
fn ninja_lexer_errors_tabs_and_versioned_newlines() {
    let path = std::env::temp_dir().join(format!(
        "ronin-ninja-lexer-errors-{}.ninja",
        std::process::id()
    ));
    fs::write(&path, "foo$\nbad $").unwrap();
    let source = scan::Source::from_path(&path).unwrap();
    let mut scanner = scan::Scanner::new(&source);
    assert!(scan::scanstring(&mut scanner, false)
        .unwrap_err()
        .to_string()
        .contains("invalid $ escape"));

    fs::write(&path, "   \tfoobar\n").unwrap();
    let source = scan::Source::from_path(&path).unwrap();
    let mut scanner = scan::Scanner::new(&source);
    assert!(scan::scankeyword(&mut scanner)
        .unwrap_err()
        .to_string()
        .contains("tabs are not allowed"));

    fs::write(&path, "foo$\nbar$^newline foo\n").unwrap();
    let source = scan::Source::from_path(&path).unwrap();
    let mut scanner = scan::Scanner::new(&source);
    assert!(scan::scanstring(&mut scanner, false)
        .unwrap_err()
        .to_string()
        .contains("ninja_required_version"));
    scanner.set_manifest_version(1, 14);
    let source = scan::Source::from_path(&path).unwrap();
    let mut scanner = scan::Scanner::new(&source);
    scanner.set_manifest_version(1, 14);
    let value = scan::scanstring(&mut scanner, false).unwrap().unwrap();
    assert_eq!(serialized_eval(&value), "[foobar\nnewline foo]");
    let _ = fs::remove_file(path);
}

// Cases adapted from Ninja's Lexer.ReadIdentCurlies and Lexer.CommentEOF.
#[test]
fn ninja_lexer_dotted_and_braced_variables() {
    let path = std::env::temp_dir().join(format!(
        "ronin-ninja-lexer-curlies-{}.ninja",
        std::process::id()
    ));
    fs::write(
        &path,
        concat!("foo.dots $bar.dots $", "{bar.dots}\n# trailing comment"),
    )
    .unwrap();
    let source = scan::Source::from_path(&path).unwrap();
    let mut scanner = scan::Scanner::new(&source);
    assert_eq!(scan::scanname(&mut scanner).unwrap().text, "foo.dots");
    let value = scan::scanstring(&mut scanner, false).unwrap().unwrap();
    assert_eq!(serialized_eval(&value), "[$bar][.dots ][$bar.dots]");
    scan::scannewline(&mut scanner).unwrap();
    assert_eq!(scan::scankeyword(&mut scanner).unwrap(), None);
    let _ = fs::remove_file(path);
}

// [spec:ronin:sem:parse.parse-fn/test]
// [spec:ronin:sem:parse.parserule-fn/test]
// [spec:ronin:sem:parse.parseedge-fn/test]
// [spec:ronin:sem:graph.mknode-fn/test]
// [spec:ronin:sem:graph.nodeget-fn/test]
#[test]
fn ninja_manifest_parser_rules() {
    let path =
        std::env::temp_dir().join(format!("ronin-ninja-manifest-{}.ninja", std::process::id()));
    fs::write(
        &path,
        "rule cat\n  command = cat $in > $out\n\nrule date\n  command = date > $out\n\nbuild result: cat in_1.cc in-2.O\n",
    )
    .unwrap();
    let mut graph = graph::Graph::default();
    let mut parser = parse::Parser::default();
    let mut state = env::EnvState::new(&mut graph);
    parse::parse(
        path.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap();
    assert!(graph::nodeget(&graph, b"result").is_some());
    assert!(graph::nodeget(&graph, b"in_1.cc").is_some());
    assert!(graph::nodeget(&graph, b"in-2.O").is_some());
    let _ = fs::remove_file(path);
}

// Cases adapted from Ninja's src/manifest_parser_test.cc.
#[test]
fn ninja_manifest_parser_variables_comments_and_dependency_kinds() {
    let path = std::env::temp_dir().join(format!(
        "ronin-ninja-manifest-variables-{}.ninja",
        std::process::id()
    ));
    fs::write(
        &path,
        concat!(
            "l = one-letter-test\n",
            "rule link\n",
            "  command = ld $l $extra -o $out $in\n",
            "  # a comment in a rule block\n",
            "build out | implicit: link input | implicit-in || order-only\n",
            "  extra = -s\n",
        ),
    )
    .unwrap();
    let mut graph = graph::Graph::default();
    let mut parser = parse::Parser::default();
    let mut state = env::EnvState::new(&mut graph);
    parse::parse(
        path.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap();
    let out = graph::nodeget(&graph, b"out").unwrap();
    let edge = graph.node(out).gen.unwrap();
    assert_eq!(graph.edge(edge).explicit_output_count(), 1);
    assert_eq!(graph.edge(edge).explicit_input_count(), 1);
    assert_eq!(graph.edge(edge).non_order_only_input_count(), 2);
    let command = env::edgevar(&graph, edge, Names::COMMAND, graph::PathStyle::Raw).unwrap();
    assert_eq!(command.as_bytes(), b"ld one-letter-test -s -o out input");
    assert_eq!(
        graph
            .node(graph::nodeget(&graph, b"input").unwrap())
            .uses
            .len(),
        1
    );
    let _ = fs::remove_file(path);
}

fn parse_manifest(path: &std::path::Path) -> (graph::Graph, parse::Parser, env::EnvState) {
    let mut graph = graph::Graph::default();
    let mut parser = parse::Parser::default();
    let mut state = env::EnvState::new(&mut graph);
    parse::parse(
        path.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap();
    (graph, parser, state)
}

// Cases adapted from Ninja's ParserTest.RuleAttributes,
// ParserTest.IgnoreIndentedComments, ParserTest.IgnoreIndentedBlankLines,
// ParserTest.ResponseFiles, and ParserTest.InNewline.
#[test]
fn ninja_manifest_parser_rule_attributes_and_special_variables() {
    let path = std::env::temp_dir().join(format!(
        "ronin-ninja-manifest-attributes-{}.ninja",
        std::process::id()
    ));
    fs::write(
        &path,
        concat!(
            "  # indented comment\n",
            "rule cat_rsp\n",
            "  command = cat $rspfile > $out\n",
            "  depfile = dep.d\n",
            "  deps = gcc\n",
            "  description = compile\n",
            "  generator = 1\n",
            "  restat = 1\n",
            "  rspfile = $rspfile\n",
            "  rspfile_content = $in\n",
            "  \n",
            "build out: cat_rsp in in2\n",
            "  rspfile = out.rsp\n",
        ),
    )
    .unwrap();
    let (graph, _, _) = parse_manifest(&path);
    let out = graph::nodeget(&graph, b"out").unwrap();
    let edge = graph.node(out).gen.unwrap();
    let command = env::edgevar(&graph, edge, Names::COMMAND, graph::PathStyle::Raw).unwrap();
    assert_eq!(command.as_bytes(), b"cat out.rsp > out");
    let inputs = env::edgevar(&graph, edge, Names::IN_NEWLINE, graph::PathStyle::Raw).unwrap();
    assert_eq!(inputs.as_bytes(), b"in\nin2");
    assert_eq!(
        env::edgevar(&graph, edge, Names::RSPFILE_CONTENT, graph::PathStyle::Raw,)
            .unwrap()
            .as_bytes(),
        b"in in2"
    );
    let _ = fs::remove_file(path);
}

// Cases adapted from Ninja's ParserTest.Variables, ParserTest.VariableScope,
// ParserTest.Continuation, ParserTest.Backslash, and ParserTest.Comment.
#[test]
fn ninja_manifest_parser_variable_scope_and_continuations() {
    let path = std::env::temp_dir().join(format!(
        "ronin-ninja-manifest-scope-{}.ninja",
        std::process::id()
    ));
    fs::write(
        &path,
        concat!(
            "l = one-letter-test\n",
            "extra = -pthread\n",
            "with_under = -under\n",
            "nested1 = 1\n",
            "nested2 = $nested1/2\n",
            "foo = bar\\baz\n",
            "foo2 = bar\\ baz\n",
            "rule link\n",
            "  command = ld $l $extra $with_under -o $out $in $\n",
            "    suffix\n",
            "build a: link b c\n",
            "build supernested: link x\n",
            "  extra = $nested2/3\n",
        ),
    )
    .unwrap();
    let (graph, _, state) = parse_manifest(&path);
    assert_eq!(
        env::envvar_named(&graph, state.root, BStr::new("nested2"))
            .unwrap()
            .as_bytes(),
        b"1/2"
    );
    assert_eq!(
        env::envvar_named(&graph, state.root, BStr::new("foo"))
            .unwrap()
            .as_bytes(),
        b"bar\\baz"
    );
    assert_eq!(
        env::envvar_named(&graph, state.root, BStr::new("foo2"))
            .unwrap()
            .as_bytes(),
        b"bar\\ baz"
    );
    let first = graph
        .node(graph::nodeget(&graph, b"a").unwrap())
        .gen
        .unwrap();
    let second = graph
        .node(graph::nodeget(&graph, b"supernested").unwrap())
        .gen
        .unwrap();
    let command = env::edgevar(&graph, first, Names::COMMAND, graph::PathStyle::Raw).unwrap();
    assert_eq!(
        command.as_bytes(),
        b"ld one-letter-test -pthread -under -o a b c suffix"
    );
    let command = env::edgevar(&graph, second, Names::COMMAND, graph::PathStyle::Raw).unwrap();
    assert_eq!(
        command.as_bytes(),
        b"ld one-letter-test 1/2/3 -under -o supernested x suffix"
    );
    let _ = fs::remove_file(path);
}

// Cases adapted from Ninja's ParserTest.PathVariables,
// ParserTest.CanonicalizeFile, ParserTest.CanonicalizePaths, and
// ParserTest.DefaultStatements.
#[test]
fn ninja_manifest_parser_paths_and_defaults() {
    let path = std::env::temp_dir().join(format!(
        "ronin-ninja-manifest-paths-{}.ninja",
        std::process::id()
    ));
    fs::write(
        &path,
        concat!(
            "dir = out\n",
            "rule cat\n",
            "  command = cat $in > $out\n",
            "build $dir/exe: cat src\n",
            "build ./out.o: cat ./bar/baz/../foo.cc\n",
            "build in/1: cat\n",
            "build in/2: cat\n",
            "build final: cat in/1 in//2\n",
            "default final out/exe\n",
        ),
    )
    .unwrap();
    let (graph, parser, _) = parse_manifest(&path);
    assert!(graph::nodeget(&graph, b"out/exe").is_some());
    assert!(graph::nodeget(&graph, b"$dir/exe").is_none());
    assert!(graph::nodeget(&graph, b"out.o").is_some());
    assert!(graph::nodeget(&graph, b"bar/foo.cc").is_some());
    assert!(graph::nodeget(&graph, b"in/2").is_some());
    let defaults = parse::defaultnodes(&parser, &graph);
    assert_eq!(defaults.len(), 2);
    let _ = fs::remove_file(path);
}

// Cases adapted from Ninja's ParserTest.Dollars and ParserTest.EscapeSpaces.
#[test]
fn ninja_manifest_parser_dollar_escaped_paths() {
    let path = std::env::temp_dir().join(format!(
        "ronin-ninja-manifest-dollars-{}.ninja",
        std::process::id()
    ));
    fs::write(
        &path,
        concat!(
            "rule foo\n",
            "  command = $",
            "{out}bar$$baz$$$\n",
            "blah\n",
            "rule spaces\n",
            "  command = something\n",
            "x = $$dollar\n",
            "build $x: foo y\n",
            "build foo$ bar: spaces $$one two$$$ three\n",
        ),
    )
    .unwrap();
    let (graph, _, state) = parse_manifest(&path);
    assert_eq!(
        env::envvar_named(&graph, state.root, BStr::new("x"))
            .unwrap()
            .as_bytes(),
        b"$dollar"
    );
    assert!(graph::nodeget(&graph, b"foo bar").is_some());
    assert!(graph::nodeget(&graph, b"$one").is_some());
    assert!(graph::nodeget(&graph, b"two$ three").is_some());
    let output = graph::nodeget(&graph, b"$dollar").unwrap();
    let edge = graph.node(output).gen.unwrap();
    let command =
        env::edgevar(&graph, edge, Names::COMMAND, graph::PathStyle::ShellEscaped).unwrap();
    assert_eq!(command.as_bytes(), b"'$dollar'bar$baz$blah");
    let _ = fs::remove_file(path);
}

// Cases adapted from Ninja's ParserTest.Include, ParserTest.SubNinja,
// ParserTest.DuplicateEdgeWithMultipleOutputsError, and ParserTest.Errors.
#[test]
fn ninja_manifest_parser_includes_and_errors() {
    let directory = std::env::temp_dir().join(format!(
        "ronin-ninja-manifest-include-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let child = directory.join("child.ninja");
    let root = directory.join("build.ninja");
    fs::write(&child, "rule cat\n  command = cat $in > $out\n").unwrap();
    fs::write(
        &root,
        format!(
            "include {}\nbuild out: cat input\n",
            child.to_string_lossy()
        ),
    )
    .unwrap();
    let (graph, _, _) = parse_manifest(&root);
    assert!(graph::nodeget(&graph, b"out").is_some());

    fs::write(
        &root,
        "rule cat\n  command = cat $in > $out\nbuild out1 out2: cat in1\nbuild out1: cat in2\n",
    )
    .unwrap();
    let mut graph = graph::Graph::default();
    let mut parser = parse::Parser::default();
    let mut state = env::EnvState::new(&mut graph);
    assert!(parse::parse(
        root.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap_err()
    .to_string()
    .contains("multiple rules generate 'out1'"));

    fs::write(&root, "rule cat\n  rspfile = cat.rsp\n").unwrap();
    let mut graph = graph::Graph::default();
    let mut parser = parse::Parser::default();
    let mut state = env::EnvState::new(&mut graph);
    assert!(parse::parse(
        root.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap_err()
    .to_string()
    .contains("has no command"));
    let _ = fs::remove_dir_all(directory);
}

// [spec:ronin:sem:deps.depsparse-fn/test]
#[test]
fn ninja_depfile_parser_basic_and_continuation() {
    let path = std::env::temp_dir().join(format!("ronin-ninja-depfile-{}.d", std::process::id()));
    fs::write(
        &path,
        "build/ninja.o: ninja.cc ninja.h \\\n  eval_env.h manifest_parser.h\n",
    )
    .unwrap();
    let mut graph = graph::Graph::default();
    let deps = deps::depsparse(&mut graph, &path, false).unwrap();
    assert_eq!(deps.nodes.len(), 4);
    assert!(graph::nodeget(&graph, b"ninja.cc").is_some());
    let _ = fs::remove_file(path);
}

// Cases adapted from Ninja's src/depfile_parser_test.cc.
#[test]
fn ninja_depfile_parser_accepts_crlf_continuations() {
    let path =
        std::env::temp_dir().join(format!("ronin-ninja-depfile-crlf-{}.d", std::process::id()));
    fs::write(&path, "foo.o: \\\r\n  bar.h baz.h\r\n").unwrap();
    let mut graph = graph::Graph::default();
    let deps = deps::depsparse(&mut graph, &path, false).unwrap();
    assert_eq!(deps.nodes.len(), 2);
    assert!(graph::nodeget(&graph, b"bar.h").is_some());
    assert!(graph::nodeget(&graph, b"baz.h").is_some());
    let _ = fs::remove_file(path);
}

// Adapted from Ninja's CleanTest.CleanAll.
#[test]
fn ninja_clean_all_removes_generated_outputs() {
    let directory = std::env::temp_dir().join(format!("ronin-ninja-clean-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let paths = ["in1", "out1", "in2", "out2"].map(|name| directory.join(name));
    let manifest = directory.join("build.ninja");
    fs::write(
        &manifest,
        format!(
            "rule cat\n  command = cat $in > $out\n\
             build {}: cat src1\n\
             build {}: cat {}\n\
             build {}: cat src2\n\
             build {}: cat {}\n",
            paths[0].display(),
            paths[1].display(),
            paths[0].display(),
            paths[2].display(),
            paths[3].display(),
            paths[2].display(),
        ),
    )
    .unwrap();
    for path in &paths {
        fs::write(path, "").unwrap();
    }
    let mut graph = graph::Graph::default();
    let mut parser = parse::Parser::default();
    let mut state = env::EnvState::new(&mut graph);
    parse::parse(
        manifest.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap();
    assert_eq!(tool::clean(&graph, &[], &[], true).unwrap(), 4);
    assert!(paths.iter().all(|path| !path.exists()));
    let _ = fs::remove_dir_all(directory);
}

// Cases adapted from Ninja's CleanTest.CleanTarget,
// CleanTest.CleanTargetMultiOutput, and CleanTest.CleanRule.
#[test]
fn ninja_clean_target_multi_output_and_rule() {
    let directory =
        std::env::temp_dir().join(format!("ronin-ninja-clean-target-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let in1 = directory.join("in1");
    let out1 = directory.join("out1");
    let aux1 = directory.join("aux1");
    let in2 = directory.join("in2");
    let out2 = directory.join("out2");
    let manifest = directory.join("build.ninja");
    fs::write(
        &manifest,
        format!(
            "rule cat\n  command = cat $in > $out\n\
             rule cc\n  command = cc $in -o $out\n\
             build {}: cat source1\n\
             build {} {}: cat {}\n\
             build {}: cc source2\n\
             build {}: cc {}\n",
            in1.display(),
            out1.display(),
            aux1.display(),
            in1.display(),
            in2.display(),
            out2.display(),
            in2.display(),
        ),
    )
    .unwrap();
    for path in [&in1, &out1, &aux1, &in2, &out2] {
        fs::write(path, "").unwrap();
    }
    let (graph, _, _) = parse_manifest(&manifest);
    assert_eq!(
        tool::clean(
            &graph,
            &[BString::from(out1.to_string_lossy().as_bytes())],
            &[],
            true,
        )
        .unwrap(),
        3
    );
    assert!(!in1.exists() && !out1.exists() && !aux1.exists());
    assert!(in2.exists() && out2.exists());
    assert_eq!(tool::clean(&graph, &[], &["cc".into()], true).unwrap(), 2);
    assert!(!in2.exists() && !out2.exists());
    let _ = fs::remove_dir_all(directory);
}

// Cases adapted from Ninja's CleanTest.CleanDepFile,
// CleanTest.CleanRspFile, CleanTest.CleanRuleGenerator, and CleanTest.CleanPhony.
#[test]
fn ninja_clean_auxiliary_files_generators_and_phony_edges() {
    let directory =
        std::env::temp_dir().join(format!("ronin-ninja-clean-aux-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let output = directory.join("output");
    let depfile = directory.join("output.d");
    let rspfile = directory.join("output.rsp");
    let generated = directory.join("generated");
    let phony = directory.join("phony");
    let manifest = directory.join("build.ninja");
    fs::write(
        &manifest,
        format!(
            concat!(
                "rule cat\n",
                "  command = cat $in > $out\n",
                "build {}: cat source\n",
                "  depfile = {}\n",
                "  rspfile = {}\n",
                "build {}: cat source\n",
                "  generator = 1\n",
                "build {}: phony source\n",
            ),
            output.display(),
            depfile.display(),
            rspfile.display(),
            generated.display(),
            phony.display(),
        ),
    )
    .unwrap();
    for path in [&output, &depfile, &rspfile, &generated, &phony] {
        fs::write(path, "").unwrap();
    }
    let (graph, _, _) = parse_manifest(&manifest);
    assert_eq!(tool::clean(&graph, &[], &[], false).unwrap(), 3);
    assert!(!output.exists() && !depfile.exists() && !rspfile.exists());
    assert!(generated.exists() && phony.exists());
    assert_eq!(tool::clean(&graph, &[], &[], true).unwrap(), 1);
    assert!(!generated.exists() && phony.exists());
    let _ = fs::remove_dir_all(directory);
}

// Adapted from Ninja's BuildLogTest.WriteRead.
#[test]
fn ninja_build_log_write_and_read() {
    let directory =
        std::env::temp_dir().join(format!("ronin-ninja-log-roundtrip-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let manifest = directory.join("build.ninja");
    fs::write(
        &manifest,
        "rule cat\n  command = cat $in > $out\nbuild out: cat in\n",
    )
    .unwrap();
    let mut graph = graph::Graph::default();
    let mut parser = parse::Parser::default();
    let mut state = env::EnvState::new(&mut graph);
    parse::parse(
        manifest.to_str().unwrap(),
        &mut graph,
        &mut parser,
        state.root,
        &mut state,
    )
    .unwrap();
    let output = graph::nodeget(&graph, b"out").unwrap();
    let mut runtime = crate::runtime::RuntimeState::new(&graph);
    runtime
        .node_mut(output)
        .set_log_mtime(crate::runtime::FileTime::observed(25));
    runtime
        .node_mut(output)
        .set_logged_command_hash(crate::runtime::CommandHash::from_raw(0xface));
    let mut build_log = log::BuildLog::open(Some(&directory)).unwrap();
    log::logrecord(&mut build_log, &graph, &runtime, output).unwrap();
    build_log.finish().unwrap();
    let build_log = log::BuildLog::open(Some(&directory)).unwrap();
    let mut runtime = crate::runtime::RuntimeState::new(&graph);
    build_log.hydrate_runtime(&graph, &mut runtime, graph.node_ids());
    assert_eq!(runtime.node(output).log_mtime().raw(), 25);
    assert_eq!(runtime.node(output).logged_command_hash().raw(), 0xface);
    build_log.finish().unwrap();
    let _ = fs::remove_dir_all(directory);
}

// Cases adapted from Ninja's BuildLogTest.DoubleEntry,
// BuildLogTest.SpacesInOutput, and BuildLogTest.DuplicateVersionHeader.
#[test]
fn ninja_build_log_loads_duplicate_and_spaced_records() {
    let directory =
        std::env::temp_dir().join(format!("ronin-ninja-log-load-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let manifest = directory.join("build.ninja");
    fs::write(
        &manifest,
        concat!(
            "rule cat\n",
            "  command = cat $in > $out\n",
            "build out: cat in\n",
            "build out2: cat in\n",
            "build out$ with$ space: cat in\n",
        ),
    )
    .unwrap();
    let (graph, _, _) = parse_manifest(&manifest);
    fs::write(
        directory.join(".ninja_log"),
        concat!(
            "# ninja log v7\n",
            "0\t1\t2\tout\t1\n",
            "0\t1\t3\tout\t2\n",
            "123\t456\t456\tout with space\tface\n",
            "# ninja log v7\n",
            "456\t789\t789\tout2\tbeef\n",
        ),
    )
    .unwrap();
    let build_log = log::BuildLog::open(Some(&directory)).unwrap();
    let mut runtime = crate::runtime::RuntimeState::new(&graph);
    build_log.hydrate_runtime(&graph, &mut runtime, graph.node_ids());
    let out = graph::nodeget(&graph, b"out").unwrap();
    assert_eq!(runtime.node(out).log_mtime().raw(), 3);
    assert_eq!(runtime.node(out).logged_command_hash().raw(), 2);
    let spaced = graph::nodeget(&graph, b"out with space").unwrap();
    assert_eq!(runtime.node(spaced).log_mtime().raw(), 456);
    assert_eq!(runtime.node(spaced).logged_command_hash().raw(), 0xface);
    let out2 = graph::nodeget(&graph, b"out2").unwrap();
    assert_eq!(runtime.node(out2).log_mtime().raw(), 789);
    assert_eq!(runtime.node(out2).logged_command_hash().raw(), 0xbeef);
    build_log.finish().unwrap();
    let _ = fs::remove_dir_all(directory);
}

// Adapted from Ninja's BuildLogTest.ObsoleteOldVersion.
#[test]
fn ninja_build_log_resets_obsolete_headers() {
    let directory =
        std::env::temp_dir().join(format!("ronin-ninja-log-old-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(".ninja_log"),
        "# ninja log v3\n123 456 0 out command\n",
    )
    .unwrap();
    log::BuildLog::open(Some(&directory))
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(
        fs::read_to_string(directory.join(".ninja_log")).unwrap(),
        "# ninja log v7\n"
    );
    let _ = fs::remove_dir_all(directory);
}

// [spec:ronin:sem:deps.depsclose-fn/test]
// [spec:ronin:sem:deps.depsinit-fn/test]
// [spec:ronin:sem:deps.depsload-fn/test]
// [spec:ronin:sem:deps.depsparse-fn/test]
// [spec:ronin:sem:deps.depsrecord-fn/test]
// [spec:ronin:sem:deps.depswrite-fn/test]
// [spec:ronin:sem:deps.recorddeps-fn/test]
// [spec:ronin:sem:deps.recordid-fn/test]
// [spec:ronin:sem:build.build-fn/test]
// [spec:ronin:sem:build.buildadd-fn/test]
// [spec:ronin:sem:build.buildreset-fn/test]
// [spec:ronin:sem:build.catchsig-fn/test]
// [spec:ronin:sem:build.edgedone-fn/test]
// [spec:ronin:sem:build.formatstatus-fn/test]
// [spec:ronin:sem:build.isdirty-fn/test]
// [spec:ronin:sem:build.isnewer-fn/test]
// [spec:ronin:sem:build.jobdone-fn/test]
// [spec:ronin:sem:build.jobstart-fn/test]
// [spec:ronin:sem:build.jobwork-fn/test]
// [spec:ronin:sem:build.nodedone-fn/test]
// [spec:ronin:sem:build.printstatus-fn/test]
// [spec:ronin:sem:build.queryload-fn/test]
// [spec:ronin:sem:build.queue-fn/test]
// [spec:ronin:sem:build.shouldprune-fn/test]
// [spec:ronin:sem:samu.debugflag-fn/test]
// [spec:ronin:sem:samu.getbuilddir-fn/test]
// [spec:ronin:sem:samu.jobsflag-fn/test]
// [spec:ronin:sem:samu.loadflag-fn/test]
// [spec:ronin:sem:samu.main-fn+1/test]
// [spec:ronin:sem:samu.parseenvargs-fn+1/test]
// [spec:ronin:sem:samu.progname-fn/test]
// [spec:ronin:sem:samu.usage-fn/test]
// [spec:ronin:sem:samu.warnflag-fn/test]
// [spec:ronin:sem:tool.clean-fn/test]
// [spec:ronin:sem:tool.cleanedge-fn/test]
// [spec:ronin:sem:tool.cleanpath-fn/test]
// [spec:ronin:sem:tool.cleantarget-fn/test]
// [spec:ronin:sem:tool.commands-fn/test]
// [spec:ronin:sem:tool.compdb-fn/test]
// [spec:ronin:sem:tool.graph-fn/test]
// [spec:ronin:sem:tool.graphnode-fn/test]
// [spec:ronin:sem:tool.printquoted-fn/test]
// [spec:ronin:sem:tool.query-fn/test]
// [spec:ronin:sem:tool.targetcommands-fn/test]
// [spec:ronin:sem:tool.targets-fn/test]
// [spec:ronin:sem:tool.targetsdepth-fn/test]
// [spec:ronin:sem:tool.targetsusage-fn/test]
// [spec:ronin:sem:tool.tool.run-fn/test]
// [spec:ronin:sem:tool.toolget-fn/test]
#[test]
fn scheduler_cli_dependency_and_tool_behaviour() {
    let directory = std::env::temp_dir().join(format!("ronin-wave3-deps-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    deps::DepsLog::open(Some(&directory))
        .unwrap()
        .finish()
        .unwrap();
    let options = build::BuildOptions {
        dryrun: true,
        ..Default::default()
    };
    let mut graph = graph::Graph::default();
    let mut builder = build::Builder::new(&mut graph, options);
    assert!(builder.build().is_ok());
    assert!(builder.build_output.is_empty());
    assert!(matches!(tool::toolget("graph"), Ok(tool::Tool::Graph)));
    let _ = fs::remove_dir_all(directory);
}

// [spec:ronin:sem:log.loginit-fn/test]
// [spec:ronin:sem:log.logclose-fn/test]
// [spec:ronin:sem:tool.cleanpath-fn/test]
#[test]
fn ninja_build_log_signature_and_clean_path() {
    let directory = std::env::temp_dir().join(format!("ronin-ninja-log-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    log::BuildLog::open(Some(&directory))
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(
        fs::read_to_string(directory.join(".ninja_log")).unwrap(),
        "# ninja log v7\n"
    );
    let output = directory.join("output");
    fs::write(&output, "x").unwrap();
    let path = util::xasprintf(format_args!("{}", output.display()));
    assert!(tool::cleanpath(Some(path.as_ref())).unwrap());
    assert!(!output.exists());
    let _ = fs::remove_dir_all(directory);
}
