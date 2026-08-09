//! Tests for the Post-Action Auditor. Size-exempt (`_test.rs`).

use super::*;
use crate::constraint::ConstraintClass;
use crate::rules::MatchType;

/// A repo root. Throttles are scoped by it, so tests must not share one with
/// each other or with a real checkout.
const SCOPE: &str = "/repo/paa-test";

/// A rule declared at the PAA. `must-exist` over a whole file is the shape the
/// gate genuinely cannot answer: over an edit FRAGMENT the question is about the
/// wrong subject.
fn paa_rule(name: &str, backoff: Option<&str>) -> Rule {
    Rule {
        name: name.to_string(),
        language: "rust".to_string(),
        query: "(function_item) @f".to_string(),
        gate: None,
        match_type: MatchType::MustExist,
        pattern: "test".to_string(),
        applies_to: Vec::new(),
        message: None,
        class: Some(ConstraintClass::Soft),
        verification_point: Some(VerificationPoint::Paa),
        backoff_formula: backoff.map(str::to_string),
    }
}

fn config_with(rules: Vec<Rule>, mode: Mode) -> YupanaConfig {
    let mut config = YupanaConfig::default();
    config.policy.mode = mode;
    config.policy.rules = rules;
    config
}

#[test]
fn a_paa_rule_is_evaluated_against_the_completed_file() {
    // The gate sees a fragment; the auditor sees the file. That difference is
    // the reason this point exists.
    let config = config_with(vec![paa_rule("needs-a-test", None)], Mode::Enforce);
    let audit = audit(&config, "fn helper() {}\n", "src/a.rs", "rust", SCOPE);
    assert_eq!(audit.constraints.len(), 1);
    assert_eq!(
        audit.constraints[0].outcome,
        crate::trace::Outcome::Unsatisfied
    );
    assert!(!audit.messages.is_empty());
}

#[test]
fn a_satisfied_constraint_is_recorded_not_skipped() {
    // "The rule ran and held" and "the rule never ran" are different facts. The
    // audit checker's coverage pass needs to tell them apart, and an absent
    // evaluation reads as a constraint nobody applied.
    let config = config_with(vec![paa_rule("needs-a-test", None)], Mode::Enforce);
    let audit = audit(&config, "fn test_it() {}\n", "src/a.rs", "rust", SCOPE);
    assert_eq!(audit.constraints.len(), 1);
    assert_eq!(
        audit.constraints[0].outcome,
        crate::trace::Outcome::Satisfied
    );
    assert!(
        audit.messages.is_empty(),
        "a satisfied constraint tells the model nothing"
    );
}

#[test]
fn a_paa_constraint_never_blocks_under_any_mode() {
    // The invariant that makes the soft class mean something. Enforce and Advise
    // behave identically here; only Off disarms.
    for mode in [Mode::Enforce, Mode::Advise] {
        let config = config_with(vec![paa_rule("needs-a-test", None)], mode);
        let audit = audit(&config, "fn helper() {}\n", "src/a.rs", "rust", SCOPE);
        assert!(
            audit
                .constraints
                .iter()
                .all(|c| c.response != crate::trace::Response::Blocked),
            "a PAA constraint must never record a block ({mode:?})"
        );
    }
}

#[test]
fn mode_off_disarms_the_auditor() {
    let config = config_with(vec![paa_rule("needs-a-test", None)], Mode::Off);
    assert!(audit(&config, "fn helper() {}\n", "src/a.rs", "rust", SCOPE).is_empty());
}

#[test]
fn a_pag_rule_is_not_evaluated_here() {
    // The two points PARTITION the rule set rather than overlapping — a
    // constraint is judged once, and its verdict says where.
    let mut rule = paa_rule("gate-rule", None);
    rule.verification_point = Some(VerificationPoint::Pag);
    let config = config_with(vec![rule], Mode::Enforce);
    assert!(audit(&config, "fn helper() {}\n", "src/a.rs", "rust", SCOPE).is_empty());
}

#[test]
fn an_undeclared_point_is_not_evaluated_here_either() {
    // Absent means "wherever yupana ran it before the field existed", which is the
    // gate. Auditing it here too would evaluate it twice and record two verdicts
    // for one action.
    let mut rule = paa_rule("legacy", None);
    rule.verification_point = None;
    let config = config_with(vec![rule], Mode::Enforce);
    assert!(audit(&config, "fn helper() {}\n", "src/a.rs", "rust", SCOPE).is_empty());
}

#[test]
fn a_declared_backoff_records_a_throttle_and_says_so() {
    let config = config_with(
        vec![paa_rule("windowed", Some("exp(min(overage / 1, 5))"))],
        Mode::Enforce,
    );
    let audit = audit(&config, "fn helper() {}\n", "src/a.rs", "rust", SCOPE);
    assert_eq!(
        audit.constraints[0].response,
        crate::trace::Response::Warned
    );
    let message = audit.messages.join(" ");
    assert!(message.contains("throttling subsequent edits"), "{message}");
    assert!(
        message.contains("nothing is blocked"),
        "the advisory must not read like a refusal: {message}"
    );
}

#[test]
fn an_unparsed_backoff_formula_applies_no_throttle_and_says_so() {
    // The gap this makes visible: the constraint fired, and its declared
    // response did NOT happen. Silently warning instead would report the
    // crossing as handled.
    let config = config_with(
        vec![paa_rule("windowed", Some("linear(overage)"))],
        Mode::Enforce,
    );
    let audit = audit(&config, "fn helper() {}\n", "src/a.rs", "rust", SCOPE);
    assert_eq!(
        audit.constraints[0].response,
        crate::trace::Response::NoAction,
        "the response did not happen, and the record must say NoAction"
    );
    let message = audit.messages.join(" ");
    assert!(message.contains("NO throttle was applied"), "{message}");
    assert!(message.contains("linear(overage)"), "it names the formula");
}

#[test]
fn a_rule_for_another_language_is_left_alone() {
    let mut rule = paa_rule("py-only", None);
    rule.language = "python".to_string();
    let config = config_with(vec![rule], Mode::Enforce);
    assert!(audit(&config, "fn helper() {}\n", "src/a.rs", "rust", SCOPE).is_empty());
}

#[test]
fn applies_to_scopes_the_auditor_like_the_gate() {
    let mut rule = paa_rule("src-only", None);
    rule.applies_to = vec!["src/**".to_string()];
    let config = config_with(vec![rule], Mode::Enforce);
    assert!(!audit(&config, "fn helper() {}\n", "src/a.rs", "rust", SCOPE).is_empty());
    assert!(audit(&config, "fn helper() {}\n", "tests/a.rs", "rust", SCOPE).is_empty());
}
