//! Tests for proposed orders and their application to a COW overlay (FR-37).
#![allow(non_snake_case)]

use super::*;
use crate::state::graph::AttrValue;
use crate::state::overlay::StateView;

fn board() -> StateGraph {
    let mut g = StateGraph::new();
    g.upsert_node(StateNode::new("alpha", "BaseState").with("garrison", AttrValue::Num(2.0)));
    g.upsert_node(StateNode::new("scout", "UnitState"));
    g.insert_edge(StateEdge::new("scout", "garrisoned_at", "alpha"));
    g
}

fn order(id: &str, effects: Vec<OrderEffect>) -> Order {
    Order {
        id: id.to_string(),
        kind: Some("MOVE".to_string()),
        effects,
    }
}

#[test]
fn speculation_does_NOT_touch_the_tenants_own_overlay() {
    // "Without committing" (FR-38) is a property of this signature, not a
    // discipline at the call site — there is no way to pass the tenant's own
    // overlay in and get it mutated.
    let base = board();
    let mine = StateOverlay::new();
    let orders = vec![order(
        "o1",
        vec![OrderEffect::SetAttr {
            id: "alpha".to_string(),
            key: "garrison".to_string(),
            value: AttrValue::Num(0.0),
        }],
    )];

    let (speculative, _) = speculate(&base, &mine, &orders);
    assert!(mine.is_empty(), "my overlay is untouched");
    assert_eq!(
        StateView::new(&base, Some(&mine))
            .node("alpha")
            .unwrap()
            .attrs
            .get("garrison"),
        Some(&AttrValue::Num(2.0)),
        "and my own view still shows the pre-order board"
    );
    assert_eq!(
        StateView::new(&base, Some(&speculative))
            .node("alpha")
            .unwrap()
            .attrs
            .get("garrison"),
        Some(&AttrValue::Num(0.0)),
        "the speculative view shows the post-order board"
    );
}

#[test]
fn an_effect_on_an_ABSENT_entity_is_reported_not_silently_dropped() {
    // The caller and yupana disagreeing about the board is the condition under
    // which every later answer is about a different game. It must surface.
    let base = board();
    let overlay = StateOverlay::new();
    let orders = vec![order(
        "o9",
        vec![OrderEffect::SetAttr {
            id: "no_such_base".to_string(),
            key: "garrison".to_string(),
            value: AttrValue::Num(1.0),
        }],
    )];
    let (_, applied) = speculate(&base, &overlay, &orders);
    assert_eq!(applied.unapplied.len(), 1);
    assert!(applied.unapplied[0].contains("o9"), "it names the order");
    assert!(applied.unapplied[0].contains("no_such_base"));
}

#[test]
fn removing_an_entity_removes_what_reached_it() {
    let base = board();
    let overlay = StateOverlay::new();
    let orders = vec![order(
        "o2",
        vec![OrderEffect::RemoveNode {
            id: "alpha".to_string(),
        }],
    )];
    let (speculative, _) = speculate(&base, &overlay, &orders);
    let view = StateView::new(&base, Some(&speculative));
    assert!(view.node("alpha").is_none());
    assert!(view.edges().is_empty(), "the garrison edge went with it");
}

#[test]
fn an_added_entity_and_edge_appear_in_the_speculative_view() {
    let base = board();
    let overlay = StateOverlay::new();
    let orders = vec![order(
        "o3",
        vec![
            OrderEffect::UpsertNode {
                node: StateNode::new("bravo", "BaseState"),
            },
            OrderEffect::AddEdge {
                edge: StateEdge::new("alpha", "adjacent_to", "bravo"),
            },
        ],
    )];
    let (speculative, applied) = speculate(&base, &overlay, &orders);
    let view = StateView::new(&base, Some(&speculative));
    assert!(view.node("bravo").is_some());
    assert_eq!(view.edges().len(), 2);
    assert_eq!(
        applied.touched,
        vec!["alpha".to_string(), "bravo".to_string()]
    );
}

#[test]
fn attribution_runs_off_DECLARED_effects() {
    // Blame is answered from what an order says it changes, not inferred from
    // proximity — which is what keeps a violation's `offending_order_ids`
    // actionable rather than a list of everything in the turn.
    let one = order(
        "move-scout",
        vec![OrderEffect::RemoveEdge {
            source: "scout".to_string(),
            relation: "garrisoned_at".to_string(),
            target: "alpha".to_string(),
        }],
    );
    assert_eq!(
        one.touched(),
        vec!["scout".to_string(), "alpha".to_string()]
    );

    let none = order("noop", Vec::new());
    assert!(none.touched().is_empty());
}

#[test]
fn orders_apply_in_sequence_and_the_last_write_wins() {
    let base = board();
    let overlay = StateOverlay::new();
    let orders = vec![
        order(
            "first",
            vec![OrderEffect::SetAttr {
                id: "alpha".to_string(),
                key: "garrison".to_string(),
                value: AttrValue::Num(1.0),
            }],
        ),
        order(
            "second",
            vec![OrderEffect::SetAttr {
                id: "alpha".to_string(),
                key: "garrison".to_string(),
                value: AttrValue::Num(0.0),
            }],
        ),
    ];
    let (speculative, _) = speculate(&base, &overlay, &orders);
    assert_eq!(
        StateView::new(&base, Some(&speculative))
            .node("alpha")
            .unwrap()
            .attrs
            .get("garrison"),
        Some(&AttrValue::Num(0.0))
    );
}

#[test]
fn an_order_deserializes_from_its_tagged_JSON_shape() {
    let json = r#"{
        "id": "o1",
        "kind": "MOVE",
        "effects": [
            {"op": "set_attr", "id": "alpha", "key": "garrison", "value": 0},
            {"op": "remove_edge", "source": "scout", "relation": "garrisoned_at", "target": "alpha"}
        ]
    }"#;
    let parsed: Order = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.effects.len(), 2);
    assert_eq!(
        parsed.touched(),
        vec!["alpha".to_string(), "scout".to_string()]
    );
}
