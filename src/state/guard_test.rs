//! Tests for `yupana_guard` (FR-37).
#![allow(non_snake_case)]

use super::*;
use crate::state::graph::{AttrValue, StateEdge, StateNode};
use crate::state::orders::OrderEffect;
use crate::state::policy::{Boundary, Effect, Predicate, Selector, SelectorLang};

/// Two border bases, one garrison unit each.
fn board() -> StateGraph {
    let mut g = StateGraph::new();
    for id in ["base_alpha", "base_beta"] {
        g.upsert_node(
            StateNode::new(id, "smac:BaseState")
                .with("smac:isBorderBase", AttrValue::Bool(true))
                .with("smac:garrisonCount", AttrValue::Num(1.0)),
        );
    }
    g.insert_edge(StateEdge::new("base_alpha", "adjacent_to", "base_beta"));
    g
}

fn garrison_policy(effect: Effect) -> StatePolicy {
    StatePolicy {
        label: "garrison-border-bases".to_string(),
        targets: Some("BaseState".to_string()),
        claim: "every border base retains >=1 garrison after the proposed orders apply".to_string(),
        boundary: Boundary::Order,
        effect,
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

/// Strip the last garrison out of `base`.
fn empty_the_garrison(id: &str, order_id: &str) -> Order {
    Order {
        id: order_id.to_string(),
        kind: Some("MOVE".to_string()),
        effects: vec![OrderEffect::SetAttr {
            id: id.to_string(),
            key: "smac:garrisonCount".to_string(),
            value: AttrValue::Num(0.0),
        }],
    }
}

fn evaluated(outcome: GuardOutcome) -> GuardReport {
    match outcome {
        GuardOutcome::Evaluated(report) => report,
        GuardOutcome::Refused { reason } => panic!("expected an evaluation, got refusal: {reason}"),
    }
}

#[test]
fn an_EMPTY_board_is_REFUSED_never_reported_as_zero_violations() {
    // The single most important behaviour here. Ingest and guard are separate
    // calls, possibly to separate processes; "nothing was ingested" and
    // "everything is fine" are otherwise the same JSON. A clean guard over a
    // board that was never loaded is a green light over a dead backend.
    let outcome = guard(
        &StateGraph::new(),
        &StateOverlay::new(),
        &[garrison_policy(Effect::Deny)],
        &[empty_the_garrison("base_alpha", "o1")],
    );
    let GuardOutcome::Refused { reason } = outcome else {
        panic!("an empty board must REFUSE, not clear the orders");
    };
    assert!(reason.contains("no board is loaded"), "{reason}");
}

#[test]
fn an_empty_POLICY_set_is_refused_too() {
    // Same shape one level along: a guard with nothing to check cannot clear an
    // order set, and returning `violations: []` would say it did.
    let outcome = guard(&board(), &StateOverlay::new(), &[], &[]);
    let GuardOutcome::Refused { reason } = outcome else {
        panic!("no policies must REFUSE");
    };
    assert!(reason.contains("no policies"), "{reason}");
}

#[test]
fn stripping_the_last_garrison_from_a_border_base_is_DENIED_and_names_the_order() {
    let report = evaluated(guard(
        &board(),
        &StateOverlay::new(),
        &[garrison_policy(Effect::Deny)],
        &[empty_the_garrison("base_alpha", "move-scout-out")],
    ));
    assert!(!report.allowed());
    assert_eq!(report.violations.len(), 1, "{:?}", report.violations);

    let finding = &report.violations[0];
    assert_eq!(finding.policy, "garrison-border-bases");
    assert_eq!(
        finding.tier, "engine-state",
        "FR-3: the tier is on the finding"
    );
    assert_eq!(finding.effect, "deny");
    assert!(!finding.pre_existing);
    assert_eq!(
        finding.offending_order_ids,
        vec!["move-scout-out".to_string()],
        "the caller has to know WHICH order to strip"
    );
    assert!(
        finding.detail.contains("base_alpha"),
        "and which entity: {}",
        finding.detail
    );
    assert!(report.advisories.is_empty());
}

#[test]
fn a_WARN_policy_routes_to_advisories_and_leaves_the_orders_allowed() {
    let report = evaluated(guard(
        &board(),
        &StateOverlay::new(),
        &[garrison_policy(Effect::Warn)],
        &[empty_the_garrison("base_alpha", "o1")],
    ));
    assert!(report.allowed(), "a warn never blocks");
    assert!(report.violations.is_empty());
    assert_eq!(report.advisories.len(), 1);
    assert_eq!(report.advisories[0].effect, "warn");
}

#[test]
fn a_compliant_order_set_produces_no_findings() {
    // The positive control. Without it, "no violations" proves nothing — the
    // guard could be failing to evaluate anything at all.
    let report = evaluated(guard(
        &board(),
        &StateOverlay::new(),
        &[garrison_policy(Effect::Deny)],
        &[Order {
            id: "reinforce".to_string(),
            kind: Some("BUILD".to_string()),
            effects: vec![OrderEffect::SetAttr {
                id: "base_alpha".to_string(),
                key: "smac:garrisonCount".to_string(),
                value: AttrValue::Num(3.0),
            }],
        }],
    ));
    assert!(report.allowed());
    assert!(report.violations.is_empty() && report.advisories.is_empty());
    assert!(
        report.vacuous.is_empty(),
        "and the policy WAS asked — not vacuously clean"
    );
}

#[test]
fn a_condition_that_ALREADY_held_blames_no_order() {
    // A false deny removes a legal, possibly correct move. Denying orders for a
    // breach they did not cause is the commonest way to produce one.
    let mut already_broken = board();
    already_broken.upsert_node(
        StateNode::new("base_alpha", "smac:BaseState")
            .with("smac:isBorderBase", AttrValue::Bool(true))
            .with("smac:garrisonCount", AttrValue::Num(0.0)),
    );

    let report = evaluated(guard(
        &already_broken,
        &StateOverlay::new(),
        &[garrison_policy(Effect::Deny)],
        &[Order {
            id: "unrelated-build".to_string(),
            kind: Some("BUILD".to_string()),
            effects: vec![OrderEffect::SetAttr {
                id: "base_beta".to_string(),
                key: "smac:garrisonCount".to_string(),
                value: AttrValue::Num(4.0),
            }],
        }],
    ));
    assert_eq!(report.violations.len(), 1);
    assert!(
        report.violations[0].pre_existing,
        "the breach predates the orders"
    );
    assert!(
        report.violations[0].offending_order_ids.is_empty(),
        "so no order is blamed for it"
    );
}

#[test]
fn a_selector_that_matches_NOTHING_is_VACUOUS_not_satisfied() {
    // A selector rotted away from the adapter's vocabulary matches zero nodes
    // and produces zero violations, which reads exactly like a clean board. This
    // is the FR-35/36 form of "a rule existing is not a rule that can fire".
    let mut policy = garrison_policy(Effect::Deny);
    policy.selector.evidence_source = "?b a smac:CityState".to_string();

    let report = evaluated(guard(
        &board(),
        &StateOverlay::new(),
        &[policy],
        &[empty_the_garrison("base_alpha", "o1")],
    ));
    assert!(report.violations.is_empty());
    assert_eq!(report.vacuous.len(), 1, "and it SAYS it was never asked");
    assert_eq!(report.vacuous[0].policy, "garrison-border-bases");
    assert!(report.vacuous[0].reason.contains("never asked"));
}

#[test]
fn a_policy_yupana_cannot_evaluate_is_LISTED_not_dropped() {
    let mut sparql = garrison_policy(Effect::Deny);
    sparql.label = "quipu-owned".to_string();
    sparql.predicate.selector_lang = SelectorLang::Sparql;

    let report = evaluated(guard(
        &board(),
        &StateOverlay::new(),
        &[sparql, garrison_policy(Effect::Deny)],
        &[empty_the_garrison("base_alpha", "o1")],
    ));
    assert_eq!(report.unevaluated.len(), 1);
    assert_eq!(report.unevaluated[0].policy, "quipu-owned");
    assert!(report.unevaluated[0].reason.contains("RESERVED for Quipu"));
    assert_eq!(
        report.violations.len(),
        1,
        "the evaluable policy still ran — one bad policy does not disarm the set"
    );
}

#[test]
fn allowed_is_NOT_the_whole_answer_when_policies_went_unevaluated() {
    // `allowed()` can be true while nothing meaningful was checked. Pinned so a
    // caller reading only that field is a documented risk, not an accident.
    let mut sparql = garrison_policy(Effect::Deny);
    sparql.predicate.selector_lang = SelectorLang::Sparql;
    let report = evaluated(guard(
        &board(),
        &StateOverlay::new(),
        &[sparql],
        &[empty_the_garrison("base_alpha", "o1")],
    ));
    assert!(report.allowed());
    assert!(
        !report.unevaluated.is_empty(),
        "and the report says why that means nothing"
    );
}

#[test]
fn a_must_not_match_policy_fires_on_the_presence_of_a_condition() {
    let mut policy = garrison_policy(Effect::Deny);
    policy.label = "no-overstacked-base".to_string();
    policy.predicate.match_type = MatchType::MustNotMatch;
    policy.predicate.evidence_source = "?b smac:garrisonCount ?n | ?n > 8".to_string();

    let report = evaluated(guard(
        &board(),
        &StateOverlay::new(),
        &[policy],
        &[Order {
            id: "overstack".to_string(),
            kind: None,
            effects: vec![OrderEffect::SetAttr {
                id: "base_alpha".to_string(),
                key: "smac:garrisonCount".to_string(),
                value: AttrValue::Num(12.0),
            }],
        }],
    ));
    assert_eq!(report.violations.len(), 1);
    assert_eq!(
        report.violations[0].offending_order_ids,
        vec!["overstack".to_string()]
    );
}

#[test]
fn a_must_exist_policy_is_ONE_board_level_finding_not_one_per_entity() {
    // Same as the code plane, where `must-exist` is a file-level question.
    let mut policy = garrison_policy(Effect::Deny);
    policy.label = "someone-must-hold-a-border".to_string();
    policy.predicate.match_type = MatchType::MustExist;

    let orders = vec![
        empty_the_garrison("base_alpha", "o1"),
        empty_the_garrison("base_beta", "o2"),
    ];
    let report = evaluated(guard(&board(), &StateOverlay::new(), &[policy], &orders));
    assert_eq!(
        report.violations.len(),
        1,
        "one finding for the board, though two bases were emptied"
    );
    assert!(report.violations[0].detail.contains("2 selected"));
}

#[test]
fn an_unapplied_effect_rides_the_report_rather_than_vanishing() {
    let report = evaluated(guard(
        &board(),
        &StateOverlay::new(),
        &[garrison_policy(Effect::Deny)],
        &[empty_the_garrison("base_ghost", "o1")],
    ));
    assert_eq!(report.unapplied.len(), 1);
    assert!(report.unapplied[0].contains("base_ghost"));
}

#[test]
fn the_report_states_the_board_it_actually_judged() {
    let report = evaluated(guard(
        &board(),
        &StateOverlay::new(),
        &[garrison_policy(Effect::Deny)],
        &[empty_the_garrison("base_alpha", "o1")],
    ));
    assert_eq!(report.board, (2, 1));
    assert_eq!(report.orders, 1);
    assert_eq!(report.tier, "engine-state");
}

#[test]
fn a_guard_reads_the_tenants_overlay_not_just_the_base() {
    // Private intel is where most of a faction's board lives; a guard that only
    // saw the shared base would judge a fraction of it.
    let mut overlay = StateOverlay::new();
    overlay.upsert_node(
        StateNode::new("base_secret", "smac:BaseState")
            .with("smac:isBorderBase", AttrValue::Bool(true))
            .with("smac:garrisonCount", AttrValue::Num(1.0)),
    );
    let report = evaluated(guard(
        &board(),
        &overlay,
        &[garrison_policy(Effect::Deny)],
        &[empty_the_garrison("base_secret", "o1")],
    ));
    assert_eq!(report.violations.len(), 1);
    assert!(report.violations[0].detail.contains("base_secret"));
}
