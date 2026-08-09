//! Tests for the `graph-pattern` selector language (FR-36).
#![allow(non_snake_case)]

use super::*;
use crate::state::graph::{StateEdge, StateGraph, StateNode};
use crate::state::overlay::StateOverlay;

/// The addendum's own vocabulary, prefixes included — Yupana does not expand them,
/// so the fixture ingests the tokens verbatim, exactly as an adapter would.
fn board() -> StateGraph {
    let mut g = StateGraph::new();
    g.upsert_node(
        StateNode::new("base_alpha", "smac:BaseState")
            .with("smac:isBorderBase", AttrValue::Bool(true))
            .with("smac:garrisonCount", AttrValue::Num(2.0)),
    );
    g.upsert_node(
        StateNode::new("base_beta", "smac:BaseState")
            .with("smac:isBorderBase", AttrValue::Bool(true))
            .with("smac:garrisonCount", AttrValue::Num(0.0)),
    );
    g.upsert_node(
        StateNode::new("base_gamma", "smac:BaseState")
            .with("smac:isBorderBase", AttrValue::Bool(false))
            .with("smac:garrisonCount", AttrValue::Num(0.0)),
    );
    g.upsert_node(StateNode::new("scout_1", "smac:UnitState"));
    g.insert_edge(StateEdge::new("scout_1", "garrisoned_at", "base_alpha"));
    g
}

fn matches(pattern: &str, base: &StateGraph, overlay: &StateOverlay) -> Matches {
    Pattern::parse(pattern)
        .expect("pattern parses")
        .eval(&StateView::new(base, Some(overlay)))
}

fn bound_ids(m: &Matches, var: &str) -> Vec<String> {
    let mut ids: Vec<String> = m
        .bindings
        .iter()
        .filter_map(|b| b.get(var).and_then(Bound::node_id).map(str::to_string))
        .collect();
    ids.sort();
    ids
}

#[test]
fn the_addendums_own_selector_selects_the_border_bases() {
    let (base, overlay) = (board(), StateOverlay::new());
    let m = matches(
        "?b a smac:BaseState ; smac:isBorderBase true",
        &base,
        &overlay,
    );
    assert_eq!(
        bound_ids(&m, "b"),
        vec!["base_alpha".to_string(), "base_beta".to_string()],
        "gamma is not a border base and the scout is not a base at all"
    );
    assert!(m.warnings.is_empty());
}

#[test]
fn a_numeric_filter_compares_as_a_number() {
    let (base, overlay) = (board(), StateOverlay::new());
    let m = matches(
        "?b a smac:BaseState ; smac:garrisonCount ?n | ?n >= 1",
        &base,
        &overlay,
    );
    assert_eq!(bound_ids(&m, "b"), vec!["base_alpha".to_string()]);
}

#[test]
fn a_name_predicate_traverses_an_EDGE_when_no_such_attribute_exists() {
    let (base, overlay) = (board(), StateOverlay::new());
    let m = matches("?u garrisoned_at ?b", &base, &overlay);
    assert_eq!(bound_ids(&m, "u"), vec!["scout_1".to_string()]);
    assert_eq!(bound_ids(&m, "b"), vec!["base_alpha".to_string()]);
}

#[test]
fn edge_traversal_is_OUTGOING_only() {
    // Both directions would bind every symmetric-looking relation twice, in
    // mirror, and silently double every finding built on one.
    //
    // Pinned against a NAMED endpoint, not by renaming the variables: variable
    // names carry no meaning, so `?b garrisoned_at ?u` is the same pattern as
    // `?u garrisoned_at ?b` and matching it proves nothing about direction.
    let (base, overlay) = (board(), StateOverlay::new());
    assert!(
        !matches("?u garrisoned_at base_alpha", &base, &overlay).is_empty(),
        "the edge runs unit -> base, so following it forwards must match"
    );
    assert!(
        matches("?b garrisoned_at scout_1", &base, &overlay).is_empty(),
        "and following it backwards must not"
    );
}

#[test]
fn a_prefix_is_NOT_expanded_and_a_mismatch_simply_does_not_match() {
    // Yupana has no prefix map by design. This test exists so the absence is
    // deliberate and visible: a pattern written against an unexpanded prefix
    // matches nothing, which looks exactly like a clean board — hence the
    // `vacuous` reporting in the guard.
    let (base, overlay) = (board(), StateOverlay::new());
    assert!(matches("?b a BaseState", &base, &overlay).is_empty());
    assert!(!matches("?b a smac:BaseState", &base, &overlay).is_empty());
}

#[test]
fn a_multi_clause_pattern_joins_on_the_shared_variable() {
    let (base, overlay) = (board(), StateOverlay::new());
    let m = matches(
        "?u a smac:UnitState ; garrisoned_at ?b . ?b smac:isBorderBase true",
        &base,
        &overlay,
    );
    assert_eq!(bound_ids(&m, "b"), vec!["base_alpha".to_string()]);
}

#[test]
fn seeding_pins_the_predicate_to_ONE_selected_entity() {
    // Without seeding, `?b smac:garrisonCount ?n | ?n >= 1` is satisfied for
    // every base as soon as ONE base anywhere has a garrison — a `must-match`
    // policy would then pass over a board where all but one base is empty.
    let (base, overlay) = (board(), StateOverlay::new());
    let view = StateView::new(&base, Some(&overlay));
    let predicate = Pattern::parse("?b smac:garrisonCount ?n | ?n >= 1").unwrap();

    let mut seed = Binding::new();
    seed.insert("b".to_string(), Bound::Node("base_beta".to_string()));
    assert!(
        predicate.eval_seeded(&view, &seed).is_empty(),
        "beta has no garrison, so seeded on beta the predicate must fail"
    );

    assert!(
        !predicate.eval(&view).is_empty(),
        "unseeded it succeeds — which is exactly the wrong answer for beta"
    );
}

#[test]
fn an_ordering_comparison_with_no_answer_WARNS_and_does_not_hold() {
    // The silent-pass this prevents: a `must-match` predicate whose comparison
    // is unevaluable would otherwise report the policy as satisfied.
    let mut base = StateGraph::new();
    base.upsert_node(
        StateNode::new("odd", "BaseState").with("garrison", AttrValue::Str("two".to_string())),
    );
    let overlay = StateOverlay::new();
    let m = matches("?b garrison ?n | ?n >= 1", &base, &overlay);
    assert!(m.is_empty(), "it did NOT hold");
    assert_eq!(m.warnings.len(), 1, "and it said so: {:?}", m.warnings);
    assert!(m.warnings[0].contains("no answer"));
}

#[test]
fn comparing_a_NODE_binding_with_a_literal_warns() {
    let (base, overlay) = (board(), StateOverlay::new());
    let m = matches("?u garrisoned_at ?b | ?b >= 1", &base, &overlay);
    assert!(m.is_empty());
    assert!(m.warnings.iter().any(|w| w.contains("bound to a node")));
}

#[test]
fn a_filter_over_an_unbound_variable_is_reported_not_ignored() {
    let (base, overlay) = (board(), StateOverlay::new());
    let m = matches("?b a smac:BaseState | ?z >= 1", &base, &overlay);
    assert!(m.is_empty());
    assert!(m.warnings.iter().any(|w| w.contains("no clause binds")));
}

#[test]
fn a_malformed_pattern_is_an_ERROR_never_an_empty_match_set() {
    // The `rules::errors` discipline: a selector that silently matched nothing
    // would disarm its policy while reporting a clean board.
    for bad in [
        "a smac:BaseState",          // no subject variable
        "?b a",                      // predicate with no object
        "?b a smac:BaseState | ?n",  // filter with no operator
        "?b a \"unterminated",       // bad literal
        "?b a smac:BaseState ; ; x", // empty section
        "",                          // nothing at all
    ] {
        assert!(
            Pattern::parse(bad).is_err(),
            "`{bad}` must fail to parse rather than match nothing"
        );
    }
}

#[test]
fn every_comparison_operator_parses_and_means_what_it_says() {
    let mut base = StateGraph::new();
    base.upsert_node(StateNode::new("b", "B").with("n", AttrValue::Num(2.0)));
    let overlay = StateOverlay::new();
    for (expr, expected) in [
        ("?b n ?n | ?n = 2", true),
        ("?b n ?n | ?n != 2", false),
        ("?b n ?n | ?n < 3", true),
        ("?b n ?n | ?n <= 2", true),
        ("?b n ?n | ?n > 2", false),
        ("?b n ?n | ?n >= 2", true),
    ] {
        assert_eq!(
            !matches(expr, &base, &overlay).is_empty(),
            expected,
            "`{expr}`"
        );
    }
}

#[test]
fn a_negative_and_a_fractional_literal_both_parse() {
    let mut base = StateGraph::new();
    base.upsert_node(StateNode::new("b", "B").with("n", AttrValue::Num(-1.5)));
    let overlay = StateOverlay::new();
    assert!(!matches("?b n ?n | ?n < -1", &base, &overlay).is_empty());
    assert!(!matches("?b n ?n | ?n = -1.5", &base, &overlay).is_empty());
}

#[test]
fn a_quoted_string_matches_a_string_attribute() {
    let mut base = StateGraph::new();
    base.upsert_node(
        StateNode::new("b", "B").with("faction", AttrValue::Str("the gaians".to_string())),
    );
    let overlay = StateOverlay::new();
    assert!(!matches("?b faction \"the gaians\"", &base, &overlay).is_empty());
    assert!(matches("?b faction \"morgan\"", &base, &overlay).is_empty());
}

#[test]
fn a_pattern_reads_through_the_overlay_not_around_it() {
    let base = board();
    let mut overlay = StateOverlay::new();
    overlay.upsert_node(
        StateNode::new("base_beta", "smac:BaseState")
            .with("smac:isBorderBase", AttrValue::Bool(true))
            .with("smac:garrisonCount", AttrValue::Num(5.0)),
    );
    let m = matches(
        "?b a smac:BaseState ; smac:garrisonCount ?n | ?n >= 1",
        &base,
        &overlay,
    );
    assert_eq!(
        bound_ids(&m, "b"),
        vec!["base_alpha".to_string(), "base_beta".to_string()]
    );
}

#[test]
fn variables_are_reported_in_first_appearance_order() {
    let p = Pattern::parse("?u garrisoned_at ?b . ?b smac:garrisonCount ?n").unwrap();
    assert_eq!(p.variables(), vec!["u", "b", "n"]);
}
