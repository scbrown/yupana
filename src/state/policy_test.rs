//! Tests for the game-state policy model (FR-36).
#![allow(non_snake_case)]

use super::*;

fn garrison_policy() -> StatePolicy {
    // The addendum's worked example, transcribed field for field.
    StatePolicy {
        label: "garrison-border-bases".to_string(),
        targets: Some("BaseState".to_string()),
        claim: "every border base retains >=1 garrison after the proposed orders apply".to_string(),
        boundary: Boundary::Order,
        effect: Effect::Deny,
        selector: Selector {
            selector_lang: SelectorLang::GraphPattern,
            evidence_source: "?b a smac:BaseState ; smac:isBorderBase true".to_string(),
        },
        predicate: Predicate {
            selector_lang: SelectorLang::GraphPattern,
            match_type: MatchType::MustMatch,
            evidence_source: "?b smac:garrisonCount ?n | ?n >= 1".to_string(),
        },
    }
}

#[test]
fn the_addendums_example_policy_compiles() {
    assert!(garrison_policy().compile().is_ok());
    assert!(errors(&[garrison_policy()]).is_empty());
}

#[test]
fn a_SPARQL_policy_is_REFUSED_not_skipped() {
    // The distinction that matters: a skipped policy is indistinguishable from a
    // satisfied one, so `errors` must list it rather than filter it out as "not
    // ours". Yupana is not an RDF store, and approximating SPARQL with the
    // graph-pattern engine would silently answer a different question.
    let mut policy = garrison_policy();
    policy.predicate.selector_lang = SelectorLang::Sparql;

    let reason = policy.evaluable().unwrap_err();
    assert!(reason.contains("RESERVED for Quipu"), "{reason}");
    assert!(
        reason.contains("predicate"),
        "it names WHICH half: {reason}"
    );

    let listed = errors(&[policy]);
    assert_eq!(listed.len(), 1, "it appears in errors(), not nowhere");
    assert_eq!(listed[0].0, "garrison-border-bases");
}

#[test]
fn a_tree_sitter_policy_is_refused_on_this_plane() {
    // The mirror of the above: a code policy pointed at a board. There is no
    // AST here, and a `.scm` query cannot be run against a fact graph.
    let mut policy = garrison_policy();
    policy.selector.selector_lang = SelectorLang::TreeSitter;
    let reason = policy.evaluable().unwrap_err();
    assert!(reason.contains("CODE plane"), "{reason}");
}

#[test]
fn an_ACTION_boundary_policy_is_not_the_order_guards_to_evaluate() {
    let mut policy = garrison_policy();
    policy.boundary = Boundary::Action;
    let reason = policy.evaluable().unwrap_err();
    assert!(reason.contains("`action`"), "{reason}");
    assert!(reason.contains("`order`"), "{reason}");
}

#[test]
fn a_malformed_selector_is_reported_with_which_half_broke() {
    let mut policy = garrison_policy();
    policy.selector.evidence_source = "a smac:BaseState".to_string();
    let reason = policy.compile().unwrap_err();
    assert!(reason.starts_with("selector:"), "{reason}");

    let mut policy = garrison_policy();
    policy.predicate.evidence_source = "?b".to_string();
    let reason = policy.compile().unwrap_err();
    assert!(reason.starts_with("predicate:"), "{reason}");
}

#[test]
fn the_match_type_enum_IS_the_code_planes_one() {
    // Not a look-alike. If these ever diverge, `must-match` comes to mean one
    // thing over an AST and another over a board, and the second meaning is
    // invisible to anyone auditing the first.
    let shared: crate::rules::MatchType = MatchType::MustMatch;
    assert_eq!(shared, crate::rules::MatchType::MustMatch);
}

#[test]
fn the_wire_spellings_match_the_ontology() {
    // These strings are `aegis:selectorLang` / `aegis:boundary` / `aegis:effect`
    // values; a projected policy round-trips through them.
    assert_eq!(SelectorLang::GraphPattern.as_str(), "graph-pattern");
    assert_eq!(SelectorLang::TreeSitter.as_str(), "tree-sitter");
    assert_eq!(SelectorLang::Sparql.as_str(), "sparql");
    assert_eq!(Boundary::Order.as_str(), "order");
    assert_eq!(Boundary::Action.as_str(), "action");
    assert_eq!(Effect::Deny.as_str(), "deny");
    assert_eq!(Effect::Warn.as_str(), "warn");

    let json = serde_json::to_string(&SelectorLang::GraphPattern).unwrap();
    assert_eq!(json, "\"graph-pattern\"");
    let back: SelectorLang = serde_json::from_str("\"graph-pattern\"").unwrap();
    assert_eq!(back, SelectorLang::GraphPattern);
}

#[test]
fn a_policy_deserializes_from_the_projected_JSON_shape() {
    // What a Quipu projection hands over. `match_type` is the code plane's
    // kebab-case spelling, unchanged.
    let json = r#"{
        "label": "garrison-border-bases",
        "targets": "BaseState",
        "claim": "every border base retains >=1 garrison",
        "boundary": "order",
        "effect": "deny",
        "selector": {
            "selector_lang": "graph-pattern",
            "evidence_source": "?b a smac:BaseState ; smac:isBorderBase true"
        },
        "predicate": {
            "selector_lang": "graph-pattern",
            "match_type": "must-match",
            "evidence_source": "?b smac:garrisonCount ?n | ?n >= 1"
        }
    }"#;
    let policy: StatePolicy = serde_json::from_str(json).unwrap();
    assert_eq!(policy.predicate.match_type, MatchType::MustMatch);
    assert!(policy.compile().is_ok());
}
