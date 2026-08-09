//! Tests for `yupana_whatif` (FR-38).
#![allow(non_snake_case)]

use super::*;
use crate::state::graph::{AttrValue, StateEdge, StateNode};
use crate::state::orders::OrderEffect;

/// A chain plus a hub: `outpost — alpha — beta — gamma`, with `alpha` also
/// holding a scout, so distance and degree can be told apart in the ranking.
fn board() -> StateGraph {
    let mut g = StateGraph::new();
    for id in ["outpost", "alpha", "beta", "gamma"] {
        g.upsert_node(StateNode::new(id, "BaseState").with("garrison", AttrValue::Num(1.0)));
    }
    g.upsert_node(StateNode::new("scout", "UnitState"));
    g.insert_edge(StateEdge::new("outpost", "adjacent_to", "alpha"));
    g.insert_edge(StateEdge::new("alpha", "adjacent_to", "beta"));
    g.insert_edge(StateEdge::new("beta", "adjacent_to", "gamma"));
    g.insert_edge(StateEdge::new("scout", "garrisoned_at", "alpha"));
    g
}

fn strip_alpha() -> Vec<Order> {
    vec![Order {
        id: "o1".to_string(),
        kind: Some("MOVE".to_string()),
        effects: vec![OrderEffect::SetAttr {
            id: "alpha".to_string(),
            key: "garrison".to_string(),
            value: AttrValue::Num(0.0),
        }],
    }]
}

fn evaluated(outcome: WhatIfOutcome) -> WhatIfReport {
    match outcome {
        WhatIfOutcome::Evaluated(report) => report,
        WhatIfOutcome::Refused { reason } => panic!("expected an evaluation: {reason}"),
    }
}

#[test]
fn an_EMPTY_board_is_REFUSED_not_reported_as_no_impact() {
    // Same failure shape as the guard: an empty impact set over a board that was
    // never ingested reads as "this move changes nothing".
    let outcome = whatif(&StateGraph::new(), &StateOverlay::new(), &strip_alpha(), 3);
    let WhatIfOutcome::Refused { reason } = outcome else {
        panic!("an empty board must refuse");
    };
    assert!(reason.contains("changes nothing"), "{reason}");
}

#[test]
fn speculation_does_not_commit_and_SAYS_so_on_the_wire() {
    // The property a caller most needs to be able to check. An absent field is
    // not a check, so it is stated.
    let (base, overlay) = (board(), StateOverlay::new());
    let report = evaluated(whatif(&base, &overlay, &strip_alpha(), 3));
    assert!(!report.committed);
    assert!(overlay.is_empty(), "and the tenant's overlay is untouched");
    assert_eq!(
        base.node("alpha").unwrap().attrs.get("garrison"),
        Some(&AttrValue::Num(1.0)),
        "as is the shared base"
    );
}

#[test]
fn the_direct_change_is_reported_as_a_before_and_after() {
    let report = evaluated(whatif(&board(), &StateOverlay::new(), &strip_alpha(), 3));
    assert_eq!(report.changes.len(), 1);
    assert_eq!(report.changes[0].node, "alpha");
    assert_eq!(report.changes[0].kind, "attr");
    assert_eq!(report.changes[0].detail, "garrison: 1 -> 0");
}

#[test]
fn impacts_are_ranked_nearest_first_then_by_degree() {
    let report = evaluated(whatif(&board(), &StateOverlay::new(), &strip_alpha(), 3));
    let ranked: Vec<(&str, u32)> = report
        .impacts
        .iter()
        .map(|i| (i.node.as_str(), i.distance))
        .collect();
    assert_eq!(
        ranked,
        vec![
            ("alpha", 0),
            ("beta", 1),
            ("outpost", 1),
            ("scout", 1),
            ("gamma", 2),
        ],
        "distance ascending; at distance 1, beta (degree 2) outranks the leaves"
    );
    let beta = report.impacts.iter().find(|i| i.node == "beta").unwrap();
    assert_eq!(beta.degree, 2, "the rank is explained, not just asserted");
    assert_eq!(beta.via, vec!["adjacent_to".to_string()]);
    assert_eq!(beta.node_kind, "BaseState");
}

#[test]
fn the_hop_ceiling_bounds_the_walk() {
    let report = evaluated(whatif(&board(), &StateOverlay::new(), &strip_alpha(), 1));
    assert!(
        !report.impacts.iter().any(|i| i.node == "gamma"),
        "gamma is two hops out"
    );
    assert_eq!(report.hops, 1);
}

#[test]
fn an_added_entity_shows_as_added_and_seeds_the_walk() {
    let orders = vec![Order {
        id: "build".to_string(),
        kind: Some("BUILD".to_string()),
        effects: vec![
            OrderEffect::UpsertNode {
                node: StateNode::new("delta", "BaseState"),
            },
            OrderEffect::AddEdge {
                edge: StateEdge::new("gamma", "adjacent_to", "delta"),
            },
        ],
    }];
    let report = evaluated(whatif(&board(), &StateOverlay::new(), &orders, 2));
    assert!(report
        .changes
        .iter()
        .any(|c| c.node == "delta" && c.kind == "added"));
    assert!(report.impacts.iter().any(|i| i.node == "delta"));
    assert!(
        report.impacts.iter().any(|i| i.node == "beta"),
        "and the walk reaches back through gamma"
    );
}

#[test]
fn a_removed_entity_shows_as_removed_and_is_not_ranked() {
    let orders = vec![Order {
        id: "raze".to_string(),
        kind: None,
        effects: vec![OrderEffect::RemoveNode {
            id: "gamma".to_string(),
        }],
    }];
    let report = evaluated(whatif(&board(), &StateOverlay::new(), &orders, 3));
    assert!(report
        .changes
        .iter()
        .any(|c| c.node == "gamma" && c.kind == "removed"));
    assert!(
        !report.impacts.iter().any(|i| i.node == "gamma"),
        "a razed base is not an entity the change reaches"
    );
}

#[test]
fn EVERY_effect_kind_reports_a_subject_that_is_off_the_board() {
    // The caller and yupana disagreeing about what is on the board is the
    // condition under which every later answer is about a different game. It
    // must surface for every effect kind, not just the ones that happen to look
    // up their subject — and a stranger must never be RANKED, which would put a
    // non-existent entity at the top of an impact list.
    for effect in [
        OrderEffect::SetAttr {
            id: "nowhere".to_string(),
            key: "garrison".to_string(),
            value: AttrValue::Num(0.0),
        },
        OrderEffect::RemoveNode {
            id: "nowhere".to_string(),
        },
        OrderEffect::AddEdge {
            edge: StateEdge::new("alpha", "adjacent_to", "nowhere"),
        },
        OrderEffect::RemoveEdge {
            source: "alpha".to_string(),
            relation: "adjacent_to".to_string(),
            target: "nowhere".to_string(),
        },
    ] {
        let orders = vec![Order {
            id: "ghost".to_string(),
            kind: None,
            effects: vec![effect.clone()],
        }];
        let report = evaluated(whatif(&board(), &StateOverlay::new(), &orders, 2));
        assert_eq!(report.unapplied.len(), 1, "{effect:?} was applied silently");
        assert!(report.unapplied[0].contains("nowhere"), "{effect:?}");
        assert!(
            !report.impacts.iter().any(|i| i.node == "nowhere"),
            "{effect:?} ranked an entity that is not on the board"
        );
    }
}

#[test]
fn an_empty_order_set_over_a_real_board_reaches_nothing() {
    // Not a refusal — the board IS loaded, and "no orders change nothing" is a
    // true and useful answer, distinct from "no board".
    let report = evaluated(whatif(&board(), &StateOverlay::new(), &[], 3));
    assert!(report.impacts.is_empty() && report.changes.is_empty());
    assert_eq!(report.orders, 0);
    assert_eq!(report.board, (5, 4));
    assert_eq!(report.tier, "engine-state");
}
