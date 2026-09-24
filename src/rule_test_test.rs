use super::*;
use crate::textrules::TextTier;

fn host_rule() -> TextRule {
    TextRule {
        name: "pattern_host".into(),
        label: None,
        pattern: r"\b[a-z0-9-]+\.example-internal\b".into(),
        tier: TextTier::Warn,
        class: Some("hostname".into()),
        exempt_path_regex: Some(r"(^|/)fixtures/".into()),
        exempt_repos: vec!["infra".into()],
        exempt_line_marker: None,
        rationale: None,
    }
}

fn case(rule: &str, text: &str, expect: Expect) -> RuleCase {
    RuleCase {
        rule: rule.into(),
        text: text.into(),
        expect,
        path: None,
        repo: None,
    }
}

#[test]
fn a_must_match_case_that_fires_passes_and_names_the_token() {
    let r = run_cases(
        &[host_rule()],
        &[case("pattern_host", "db.example-internal", Expect::Match)],
    );
    assert_eq!(r[0].outcome, Outcome::Pass);
    assert_eq!(r[0].fired, Some(true));
    assert_eq!(r[0].matched, vec!["db.example-internal".to_string()]);
}

#[test]
fn a_must_not_match_case_that_fires_fails() {
    let r = run_cases(
        &[host_rule()],
        &[case("pattern_host", "db.example-internal", Expect::NoMatch)],
    );
    assert_eq!(r[0].outcome, Outcome::Fail);
    assert_eq!(exit_code(&r), 1);
}

#[test]
fn a_must_match_case_that_stays_silent_fails() {
    let r = run_cases(
        &[host_rule()],
        &[case("pattern_host", "db.example.com", Expect::Match)],
    );
    assert_eq!(r[0].outcome, Outcome::Fail);
    assert_eq!(r[0].fired, Some(false));
}

#[test]
fn exemptions_apply_exactly_as_in_the_hook() {
    // Path exemption and repo exemption both silence the rule; the case can
    // therefore prove an exemption holds, not only that the pattern matches.
    let mut by_path = case("pattern_host", "db.example-internal", Expect::NoMatch);
    by_path.path = Some("tests/fixtures/a.txt".into());
    let mut by_repo = case("pattern_host", "db.example-internal", Expect::NoMatch);
    by_repo.repo = Some("INFRA".into());
    let r = run_cases(&[host_rule()], &[by_path, by_repo]);
    assert!(r.iter().all(|c| c.outcome == Outcome::Pass), "{r:?}");
}

#[test]
fn an_absent_rule_is_unknown_never_pass() {
    // A NoMatch case against a missing rule would "pass" if absence were read
    // as silence. That is the vacuous proof this verb must not give.
    let r = run_cases(
        &[host_rule()],
        &[case("pattern_gone", "anything", Expect::NoMatch)],
    );
    assert_eq!(r[0].outcome, Outcome::Unknown);
    assert_eq!(r[0].fired, None);
    assert_eq!(exit_code(&r), 2);
}

#[test]
fn a_rule_that_does_not_compile_is_unknown() {
    let mut bad = host_rule();
    bad.pattern = "([unclosed".into();
    let r = run_cases(&[bad], &[case("pattern_host", "x", Expect::NoMatch)]);
    assert_eq!(r[0].outcome, Outcome::Unknown);
    assert!(r[0].reason.as_deref().unwrap_or("").contains("fails open"));
}

#[test]
fn exit_code_orders_fail_over_unknown_and_empty_is_unknown() {
    assert_eq!(exit_code(&[]), 2);
    let r = run_cases(
        &[host_rule()],
        &[
            case("pattern_gone", "x", Expect::Match),
            case("pattern_host", "db.example-internal", Expect::NoMatch),
        ],
    );
    assert_eq!(exit_code(&r), 1);
    let ok = run_cases(
        &[host_rule()],
        &[case("pattern_host", "clean", Expect::NoMatch)],
    );
    assert_eq!(exit_code(&ok), 0);
}

#[test]
fn cases_parse_from_the_documented_json_shape() {
    let cases: Vec<RuleCase> = serde_json::from_str(
        r#"[{"rule":"r","text":"t","expect":"no-match","path":"a/b.md","repo":"x"},
            {"rule":"r","text":"t","expect":"match"}]"#,
    )
    .unwrap();
    assert_eq!(cases[0].expect, Expect::NoMatch);
    assert_eq!(cases[1].path, None);
}
