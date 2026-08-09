//! Hosting-layer (I6) tests. Size-exempt (`*_test.rs`).

use super::*;

#[test]
fn a_stronger_claim_than_the_placement_is_reported() {
    // The failure the check exists for: `tool` is the layer an agent cannot
    // route around, and yupana's hook is not it.
    let why = overclaims("secrets-guard", Some(HostingLayer::Tool), YUPANA_HOSTS_AT)
        .expect("a tool claim enforced at orchestration must be reported");
    assert!(why.contains("secrets-guard"), "{why}");
    assert!(why.contains("\"tool\""), "names the claim: {why}");
    assert!(
        why.contains("orchestration layer"),
        "names the truth: {why}"
    );
    assert!(
        why.contains("still runs"),
        "says it is not an outage: {why}"
    );
    assert!(why.contains("or declare"), "names both remedies: {why}");
}

#[test]
fn the_policy_layer_claim_is_reported_too() {
    assert!(overclaims("p", Some(HostingLayer::Policy), YUPANA_HOSTS_AT).is_some());
}

#[test]
fn an_honest_claim_is_silent() {
    // The control. Without it the check could be flagging every declaration.
    assert!(overclaims("p", Some(HostingLayer::Orchestration), YUPANA_HOSTS_AT).is_none());
}

#[test]
fn an_understated_claim_is_silent() {
    // The asymmetry that makes the check one-directional: a tool-enforced
    // constraint described as orchestration understates its own robustness, and
    // misleads nobody in a direction that costs them.
    assert!(overclaims("p", Some(HostingLayer::Orchestration), HostingLayer::Tool).is_none());
    assert!(overclaims("p", Some(HostingLayer::Tool), HostingLayer::Policy).is_none());
}

#[test]
fn an_absent_claim_is_silent() {
    // An absent claim overclaims nothing. Treating "undeclared" as a defect
    // would flag every pre-Phase-1 policy in the catalog on every edit, and the
    // notice would stop being read.
    assert!(overclaims("p", None, YUPANA_HOSTS_AT).is_none());
}

#[test]
fn yupana_hosts_at_orchestration_and_that_is_not_configurable() {
    // A hook in the agent's loop IS the orchestration layer. If this ever reads
    // as anything else, the check above is measuring against a fiction.
    assert_eq!(YUPANA_HOSTS_AT, HostingLayer::Orchestration);
}

#[test]
fn the_layers_are_ordered_by_how_hard_they_are_to_route_around() {
    // The whole check is a comparison, so the ordering is the check.
    assert!(HostingLayer::Orchestration < HostingLayer::Tool);
    assert!(HostingLayer::Tool < HostingLayer::Policy);
}

#[test]
fn prompt_is_not_a_layer_this_vocabulary_has() {
    // Not an omission — a constraint that lives in the prompt is one the model
    // can be talked out of, which is what I6 forbids.
    assert!(HostingLayer::parse("prompt").is_none());
    assert!(HostingLayer::parse("").is_none());
    assert!(
        HostingLayer::parse("Tool").is_none(),
        "case is not normalised"
    );
}

#[test]
fn parse_and_render_round_trip() {
    for layer in [
        HostingLayer::Orchestration,
        HostingLayer::Tool,
        HostingLayer::Policy,
    ] {
        assert_eq!(HostingLayer::parse(layer.as_str()), Some(layer));
    }
}

#[test]
fn a_catalog_audit_reports_only_the_overclaims_and_keeps_order() {
    let catalog = vec![
        ("honest".to_string(), Some(HostingLayer::Orchestration)),
        ("undeclared".to_string(), None),
        ("overclaims-tool".to_string(), Some(HostingLayer::Tool)),
        ("overclaims-policy".to_string(), Some(HostingLayer::Policy)),
    ];
    let notices = audit_projection(&catalog, YUPANA_HOSTS_AT);
    assert_eq!(notices.len(), 2, "{notices:#?}");
    assert!(notices[0].contains("overclaims-tool"));
    assert!(notices[1].contains("overclaims-policy"));
}

#[test]
fn a_clean_catalog_produces_no_notices() {
    let catalog = vec![
        ("a".to_string(), Some(HostingLayer::Orchestration)),
        ("b".to_string(), None),
    ];
    assert!(audit_projection(&catalog, YUPANA_HOSTS_AT).is_empty());
}
