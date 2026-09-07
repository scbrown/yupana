use super::*;

fn model(source: &str) -> Cpg {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), source).unwrap();
    Cpg::build(dir.path()).unwrap()
}
fn forward(model: &Cpg, function: &str, var: &str) -> Report {
    model.query(function, Some(var), FlowDir::FlowsInto, 64)
}
fn reached(report: &Report, function: &str, name: &str) -> bool {
    report
        .flow
        .iter()
        .any(|s| s.value.function.contains(&format!("::{function}@")) && s.value.name == name)
}

#[test]
fn branch_and_merge_use_postdominance_not_lexical_nesting() {
    let graph =
        model("fn f(a: bool) {\n if a {\n let x = 1;\n } else {\n let y = 2;\n }\n let z = 3;\n}");
    let report = graph.query("f", None, FlowDir::FlowsInto, 5);
    let pairs: Vec<_> = report
        .control_edges
        .iter()
        .map(|e| (e.line, e.condition_line))
        .collect();
    assert!(pairs.contains(&(3, 2)), "{pairs:?}");
    assert!(pairs.contains(&(5, 2)), "{pairs:?}");
    assert!(!pairs.iter().any(|(line, _)| *line == 7));
    assert!(report
        .control_edges
        .iter()
        .all(|e| e.tier == Tier::Cpg && e.relation == "bobbin:controlDependsOn"));
}

#[test]
fn early_return_controls_following_statement() {
    let graph = model("fn f(a: bool) {\n if a { return; }\n let x = 1;\n}");
    let report = graph.query("f", None, FlowDir::FlowsInto, 5);
    assert!(report
        .control_edges
        .iter()
        .any(|e| e.line == 3 && e.condition_line == 2));
}

#[test]
fn loops_break_continue_and_unreachable_code() {
    let graph = model("fn f(a: bool, b: bool) {\n while a {\n if b { break; }\n continue;\n let unreachable = 1;\n }\n let after = 2;\n}");
    let r = graph.query("f", None, FlowDir::FlowsInto, 5);
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    assert!(r.control_edges.iter().any(|e| e.line == 4));
    assert!(!r.control_edges.iter().any(|e| e.line == 5 || e.line == 7));
    let nonterminating = model("fn f() { loop {} }").query("f", None, FlowDir::FlowsInto, 5);
    assert!(nonterminating
        .diagnostics
        .iter()
        .any(|s| s.contains("nonterminating")));
}

#[test]
fn source_to_sink_through_parameter_and_return_has_witness() {
    let graph = model("fn source() -> i32 { 7 }\nfn relay(p: i32) -> i32 { let q = p; q }\nfn sink(s: i32) {}\nfn main() { let x = source(); let y = relay(x); sink(y); }");
    let r = forward(&graph, "source", "$return");
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    assert!(reached(&r, "relay", "p"));
    assert!(reached(&r, "main", "y"));
    assert!(reached(&r, "sink", "s"));
    let step = r.flow.iter().find(|s| s.value.name == "s").unwrap();
    assert_eq!(step.distance as usize, step.path.len() - 1);
    assert!(step.path.iter().any(|p| p.contains("$return")));
    assert!(r.flow.iter().all(|s| s.value.tier == Tier::Cpg));
}

#[test]
fn call_sites_do_not_cross_contaminate_returns_or_constant_functions() {
    let graph = model("fn id(p:i32)->i32 { p }\nfn constant(p:i32)->i32 { 0 }\nfn outer(a:i32,b:i32) { let yes = id(a); let no = id(b); let clean = constant(a); }");
    let r = forward(&graph, "outer", "a");
    assert!(reached(&r, "outer", "yes"));
    assert!(!reached(&r, "outer", "no"));
    assert!(!reached(&r, "outer", "clean"));
    let backward = graph.query("outer", Some("no"), FlowDir::DependsOn, 64);
    assert!(reached(&backward, "outer", "b"));
    assert!(!reached(&backward, "outer", "a"));
}

#[test]
fn lexical_shadowing_and_qualified_function_selection() {
    let graph = model("fn f(a:i32) { let x=a; { let x=0; let clean=x; } let yes=x; }\nmod other { fn f(a:i32) { let no=a; } }");
    let ambiguous = forward(&graph, "f", "a");
    assert!(!ambiguous.found);
    assert!(ambiguous.flow.is_empty());
    let id = graph
        .functions
        .values()
        .find(|f| f.scope.is_empty())
        .unwrap()
        .id
        .clone();
    let r = forward(&graph, &id, "a");
    assert!(reached(&r, "f", "yes"));
    assert!(!reached(&r, "f", "clean"));
    assert!(!reached(&r, "f", "no"));
}

#[test]
fn crate_qualified_call_crosses_files_without_name_fanout() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("helper.rs"),
        "pub fn pass(p:i32)->i32 { p }",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.rs"),
        "fn caller(a:i32) { let b=crate::helper::pass(a); }",
    )
    .unwrap();
    let graph = Cpg::build(dir.path()).unwrap();
    let r = forward(&graph, "caller", "a");
    assert!(reached(&r, "pass", "p"));
    assert!(reached(&r, "caller", "b"));
}

#[test]
fn bounded_recursion_and_unsupported_constructs_are_visible() {
    let graph =
        model("fn recur(p:i32)->i32 { recur(p) }\nfn f(a:i32) { let x=external(a); let y=|| a; }");
    let r = graph.query("recur", Some("p"), FlowDir::FlowsInto, 8);
    assert!(r.truncated);
    assert!(r.flow.len() < 20);
    assert!(r.diagnostics.iter().any(|s| s.contains("external")));
    assert!(r
        .diagnostics
        .iter()
        .any(|s| s.contains("closure_expression")));
    assert!(!reached(&forward(&graph, "f", "a"), "f", "x"));
    let broken = model("fn bad( {").query("bad", None, FlowDir::FlowsInto, 5);
    assert!(!broken.found);
    assert!(broken
        .diagnostics
        .iter()
        .any(|s| s.contains("syntax errors")));
}

#[test]
fn removing_call_argument_severs_source_to_sink() {
    let before = model("fn sink(s:i32) {} fn f(a:i32) { sink(a); }");
    let after = model("fn sink(s:i32) {} fn f(a:i32) { sink(0); }");
    assert!(reached(&forward(&before, "f", "a"), "sink", "s"));
    assert!(!reached(&forward(&after, "f", "a"), "sink", "s"));
}

#[test]
fn a_qualified_constant_is_not_a_same_named_local() {
    let graph = model("fn f(a:i32) { let no=other::a; }");
    let r = forward(&graph, "f", "a");
    assert!(!reached(&r, "f", "no"));
    assert!(r
        .diagnostics
        .iter()
        .any(|s| s.contains("qualified non-call value")));
}
