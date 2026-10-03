use super::*;

fn request(source: &str) -> Request {
    serde_json::from_value(serde_json::json!({
        "rule": {
            "name": "todo-needs-ticket", "language": "rust",
            "query": "(line_comment) @c", "gate": "TODO",
            "match_type": "must-match", "pattern": "[A-Z]+-[0-9]+"
        },
        "path": "src/example.rs", "source": source
    }))
    .unwrap()
}

#[test]
fn todo_predicate_is_evaluated_on_comments_not_strings() {
    let bad = evaluate(&request("// TODO finish this\nfn f() {}"));
    assert_eq!(bad.verdict, Verdict::Unsatisfied);
    assert_eq!(bad.exit_code(), 1);
    assert_eq!(bad.violations.len(), 1);
    for source in [
        "// TODO APP-12 finish this\nfn f() {}",
        "fn f() { let _s = \"TODO finish this\"; }",
        "fn f() {}",
    ] {
        let good = evaluate(&request(source));
        assert_eq!(good.verdict, Verdict::Satisfied, "{good:?}");
        assert_eq!(good.exit_code(), 0);
    }
}

#[test]
fn malformed_rules_and_sources_are_unknown_not_satisfied() {
    let base = request("fn f() {}");
    let mut cases = Vec::new();
    for (field, value) in [
        ("language", "unsupported"),
        ("query", "(no_such_node) @c"),
        ("pattern", "["),
        ("gate", "["),
        ("name", ""),
    ] {
        let mut encoded =
            serde_json::json!({"rule": base.rule, "source": base.source, "path": base.path});
        encoded["rule"][field] = value.into();
        cases.push(serde_json::from_value::<Request>(encoded).unwrap());
    }
    cases.push(request("fn broken( {"));
    for path in ["", "/src/x.rs", "../x.rs", "src/./x.rs", "src//x.rs"] {
        let mut case = request("fn f() {}");
        case.path = path.into();
        cases.push(case);
    }
    for globs in [vec!["[".into()], vec!["other/**".into()]] {
        let mut case = request("fn f() {}");
        case.rule.applies_to = globs;
        cases.push(case);
    }
    for case in cases {
        let result = evaluate(&case);
        assert_eq!(result.verdict, Verdict::Unknown, "{result:?}");
        assert_eq!(result.exit_code(), 2);
        assert!(!result.errors.is_empty());
    }
}

#[test]
fn must_exist_and_must_not_match_reuse_existing_rule_semantics() {
    let mut case = request("fn f() {}");
    case.rule.match_type = rules::MatchType::MustExist;
    assert_eq!(evaluate(&case).verdict, Verdict::Unsatisfied);
    case.source = "// TODO APP-12\nfn f() {}".into();
    assert_eq!(evaluate(&case).verdict, Verdict::Satisfied);
    case.rule.match_type = rules::MatchType::MustNotMatch;
    assert_eq!(evaluate(&case).verdict, Verdict::Unsatisfied);
}

#[test]
fn language_scope_is_decided_by_the_existing_path_classifier() {
    let mut case = request("this is not Rust source");
    case.path = "README.md".into();
    assert_eq!(evaluate(&case).verdict, Verdict::NotApplicable);
    case.rule.language = "unsupported".into();
    assert_eq!(evaluate(&case).verdict, Verdict::Unknown);
}
