//! Tests for per-game / per-faction tenancy (FR-39).
//!
//! Isolation is asserted DIRECTLY here rather than inferred from the overlay
//! design, because "it falls out of the architecture" is an argument and a leak
//! between factions is a security failure that would be invisible in results.
#![allow(non_snake_case)]

use super::*;
use crate::state::graph::AttrValue;
use crate::state::ingest::{EntitySpec, RelationSpec, Visibility};
use crate::state::orders::OrderEffect;
use crate::state::policy::{Boundary, Effect, MatchType, Predicate, Selector, SelectorLang};

fn entity(name: &str, kind: &str) -> EntitySpec {
    EntitySpec {
        name: name.to_string(),
        kind: kind.to_string(),
        description: None,
        attrs: std::collections::BTreeMap::new(),
    }
}

fn ingest(
    registry: &mut StateRegistry,
    game: &str,
    faction: &str,
    visibility: Visibility,
    entities: Vec<EntitySpec>,
    edges: Vec<RelationSpec>,
) -> crate::state::ingest::IngestReport {
    registry
        .ingest(&IngestRequest {
            game_id: game.to_string(),
            faction_id: faction.to_string(),
            visibility,
            replace: false,
            provenance: crate::state::graph::Provenance {
                adapter: Some("test-adapter".to_string()),
                turn: Some(1),
                faction: None,
            },
            entities,
            edges,
        })
        .expect("within the tenant cap")
}

/// One game, shared map, two factions each holding a private unit.
fn two_faction_game() -> StateRegistry {
    let mut r = StateRegistry::new();
    ingest(
        &mut r,
        "g1",
        "gaians",
        Visibility::Shared,
        vec![entity("tile_1", "smac:Tile"), entity("tile_2", "smac:Tile")],
        vec![RelationSpec {
            source: "tile_1".to_string(),
            target: "tile_2".to_string(),
            relation: "adjacent_to".to_string(),
        }],
    );
    ingest(
        &mut r,
        "g1",
        "gaians",
        Visibility::Private,
        vec![entity("gaian_scout", "smac:UnitState")],
        Vec::new(),
    );
    ingest(
        &mut r,
        "g1",
        "morgan",
        Visibility::Private,
        vec![entity("morgan_probe", "smac:UnitState")],
        Vec::new(),
    );
    r
}

#[test]
fn a_faction_CANNOT_see_a_siblings_private_intel() {
    // The assertion the whole boundary exists for.
    let r = two_faction_game();
    let gaians = TenantKey::new("g1", "gaians");
    let morgan = TenantKey::new("g1", "morgan");

    assert!(r.view(&gaians).node("gaian_scout").is_some());
    assert!(
        r.view(&gaians).node("morgan_probe").is_none(),
        "gaians must not see morgan's probe"
    );
    assert!(r.view(&morgan).node("morgan_probe").is_some());
    assert!(
        r.view(&morgan).node("gaian_scout").is_none(),
        "and morgan must not see the gaian scout"
    );
}

#[test]
fn both_factions_DO_see_the_shared_base() {
    // The positive control. Without it, the isolation test above would pass just
    // as well over two empty views.
    let r = two_faction_game();
    for faction in ["gaians", "morgan"] {
        let view = r.view(&TenantKey::new("g1", faction));
        assert!(view.node("tile_1").is_some(), "{faction} sees the map");
        assert_eq!(view.edges().len(), 1, "{faction} sees the shared edge");
    }
}

#[test]
fn a_full_node_listing_leaks_nothing_either() {
    // `node()` is not the only read path. An enumeration that composed all
    // overlays would leak everything while the by-id lookup still looked clean.
    let r = two_faction_game();
    let ids: Vec<&str> = r
        .view(&TenantKey::new("g1", "gaians"))
        .nodes()
        .iter()
        .map(|n| n.id.as_str())
        .collect();
    assert_eq!(ids, vec!["gaian_scout", "tile_1", "tile_2"]);
}

#[test]
fn a_sibling_GAME_is_a_different_board_entirely() {
    // One base per process would make every fact in one game common knowledge in
    // the other — the same leak, one level up.
    let mut r = two_faction_game();
    ingest(
        &mut r,
        "g2",
        "gaians",
        Visibility::Shared,
        vec![entity("other_map", "smac:Tile")],
        Vec::new(),
    );
    assert!(r
        .view(&TenantKey::new("g1", "gaians"))
        .node("other_map")
        .is_none());
    assert!(r
        .view(&TenantKey::new("g2", "gaians"))
        .node("tile_1")
        .is_none());
    assert!(r
        .view(&TenantKey::new("g2", "gaians"))
        .node("other_map")
        .is_some());
}

#[test]
fn a_faction_stamped_SHARED_write_is_refused_and_the_registry_counts_it() {
    let mut r = StateRegistry::new();
    let report = r
        .ingest(&IngestRequest {
            game_id: "g1".to_string(),
            faction_id: "gaians".to_string(),
            visibility: Visibility::Shared,
            replace: false,
            provenance: crate::state::graph::Provenance {
                adapter: Some("test-adapter".to_string()),
                turn: Some(1),
                faction: Some("gaians".to_string()),
            },
            entities: vec![entity("gaian_scout", "smac:UnitState")],
            edges: Vec::new(),
        })
        .unwrap();
    assert_eq!(report.fog_leaks_blocked, 1);
    assert_eq!(
        r.status().fog_leaks_blocked,
        1,
        "and it is visible in status"
    );
    assert!(r
        .view(&TenantKey::new("g1", "gaians"))
        .node("gaian_scout")
        .is_none());
}

#[test]
fn the_isolation_report_MEASURES_rather_than_argues() {
    // Refusal at ingest and structural disjointness are both arguments. This is
    // the measurement — the only one of the three that catches a leak arriving
    // by a path nobody anticipated.
    let r = two_faction_game();
    let clean = r.isolation_report();
    assert!(!clean.leaked, "a correctly-routed game is clean");
    assert!(clean.faction_tagged_shared_entities.is_empty());
}

#[test]
fn the_tenant_cap_REFUSES_rather_than_evicting_private_intel() {
    // Evicting an overlay would silently widen that faction's view back to the
    // shared base — a correctness change disguised as a resource policy. The
    // code plane can evict because a developer's overlay costs a re-touch.
    let mut r = StateRegistry::with_max_tenants(1);
    ingest(
        &mut r,
        "g1",
        "gaians",
        Visibility::Private,
        vec![entity("gaian_scout", "smac:UnitState")],
        Vec::new(),
    );
    let refused = r.ingest(&IngestRequest {
        game_id: "g1".to_string(),
        faction_id: "morgan".to_string(),
        visibility: Visibility::Private,
        replace: false,
        provenance: crate::state::graph::Provenance::default(),
        entities: vec![entity("morgan_probe", "smac:UnitState")],
        edges: Vec::new(),
    });
    let err = refused.unwrap_err();
    assert!(err.contains("tenant cap reached"), "{err}");
    assert!(
        r.view(&TenantKey::new("g1", "gaians"))
            .node("gaian_scout")
            .is_some(),
        "and the incumbent kept its intel"
    );
}

#[test]
fn status_reports_the_shared_base_and_each_factions_delta_separately() {
    let r = two_faction_game();
    let status = r.status();
    assert_eq!(status.games.len(), 1);
    let game = &status.games[0];
    assert_eq!(game.game_id, "g1");
    assert_eq!((game.shared_nodes, game.shared_edges), (2, 1));
    assert_eq!(game.factions.len(), 2);
    for faction in &game.factions {
        assert_eq!(
            faction.overlay_nodes, 1,
            "{} holds O(delta), not a whole board",
            faction.faction_id
        );
    }
}

#[test]
fn closing_a_tenant_drops_its_intel_and_leaves_the_shared_base() {
    let mut r = two_faction_game();
    r.close_tenant(&TenantKey::new("g1", "morgan"));
    assert!(r
        .view(&TenantKey::new("g1", "morgan"))
        .node("morgan_probe")
        .is_none());
    assert!(
        r.view(&TenantKey::new("g1", "morgan"))
            .node("tile_1")
            .is_some(),
        "the shared map is not a tenant's to drop"
    );
}

#[test]
fn closing_a_game_takes_its_base_and_every_overlay_in_it() {
    let mut r = two_faction_game();
    r.close_game("g1");
    assert!(r.status().games.is_empty());
    assert!(r.view(&TenantKey::new("g1", "gaians")).is_empty());
}

#[test]
fn a_guard_for_an_unknown_tenant_REFUSES_rather_than_clearing_the_orders() {
    // The cross-surface trap: ingest to one process, guard against another. The
    // second sees an empty board, and an empty board must never read as clean.
    let r = StateRegistry::new();
    let policy = StatePolicy {
        label: "p".to_string(),
        targets: None,
        claim: "c".to_string(),
        boundary: Boundary::Order,
        effect: Effect::Deny,
        selector: Selector {
            selector_lang: SelectorLang::GraphPattern,
            evidence_source: "?u a smac:UnitState".to_string(),
        },
        predicate: Predicate {
            selector_lang: SelectorLang::GraphPattern,
            match_type: MatchType::MustMatch,
            evidence_source: "?u alive true".to_string(),
        },
    };
    let outcome = r.guard(&TenantKey::new("nope", "nobody"), &[policy], &[]);
    assert!(matches!(
        outcome,
        crate::state::GuardOutcome::Refused { .. }
    ));
}

#[test]
fn guard_and_whatif_run_against_the_TENANTS_OWN_board() {
    let mut r = two_faction_game();
    ingest(
        &mut r,
        "g1",
        "gaians",
        Visibility::Private,
        vec![EntitySpec {
            name: "gaian_base".to_string(),
            kind: "smac:BaseState".to_string(),
            description: None,
            attrs: [("garrison".to_string(), AttrValue::Num(1.0))]
                .into_iter()
                .collect(),
        }],
        Vec::new(),
    );
    let orders = vec![crate::state::Order {
        id: "strip".to_string(),
        kind: None,
        effects: vec![OrderEffect::SetAttr {
            id: "gaian_base".to_string(),
            key: "garrison".to_string(),
            value: AttrValue::Num(0.0),
        }],
    }];

    let crate::state::WhatIfOutcome::Evaluated(report) =
        r.whatif(&TenantKey::new("g1", "gaians"), &orders, 2)
    else {
        panic!("the gaians have a board");
    };
    assert_eq!(report.changes[0].detail, "garrison: 1 -> 0");

    // Morgan's board does not contain that base, so the same order is unapplied
    // there — the two factions genuinely evaluate different boards.
    let crate::state::WhatIfOutcome::Evaluated(theirs) =
        r.whatif(&TenantKey::new("g1", "morgan"), &orders, 2)
    else {
        panic!("morgan has a board too — the shared map");
    };
    assert_eq!(theirs.unapplied.len(), 1);
}

/// `replace` makes an ingest the whole of the private layer, not a patch on it.
///
/// The failure it exists for: an adapter whose world view IS the board posts a complete set
/// each turn. Without `replace`, a base razed twenty turns ago survives every later ingest that
/// simply does not mention it, and goes on matching policy selectors forever — a stale second
/// source of board state sitting behind a caller who believes it just stated the current one.
#[test]
fn replace_drops_private_nodes_the_new_ingest_does_not_mention() {
    let mut r = StateRegistry::new();
    ingest(
        &mut r,
        "g1",
        "gaians",
        Visibility::Private,
        vec![
            entity("base_1", "smac:BaseState"),
            entity("base_2", "smac:BaseState"),
        ],
        vec![],
    );

    // base_2 has been lost. The new world view lists only base_1.
    r.ingest(&IngestRequest {
        game_id: "g1".to_string(),
        faction_id: "gaians".to_string(),
        visibility: Visibility::Private,
        replace: true,
        provenance: crate::state::graph::Provenance::default(),
        entities: vec![entity("base_1", "smac:BaseState")],
        edges: vec![],
    })
    .expect("within the tenant cap");

    let view = r.view(&TenantKey::new("g1", "gaians"));
    assert!(
        view.node("base_1").is_some(),
        "the base still held must survive"
    );
    assert!(
        view.node("base_2").is_none(),
        "the base no longer held must be gone"
    );
}

/// The default stays additive, because every existing caller depends on it.
#[test]
fn without_replace_an_earlier_private_node_survives() {
    let mut r = StateRegistry::new();
    ingest(
        &mut r,
        "g1",
        "gaians",
        Visibility::Private,
        vec![
            entity("base_1", "smac:BaseState"),
            entity("base_2", "smac:BaseState"),
        ],
        vec![],
    );
    ingest(
        &mut r,
        "g1",
        "gaians",
        Visibility::Private,
        vec![entity("base_1", "smac:BaseState")],
        vec![],
    );

    let view = r.view(&TenantKey::new("g1", "gaians"));
    assert!(
        view.node("base_2").is_some(),
        "an additive ingest must not drop anything"
    );
}

/// Private layer only: clearing one tenant's overlay must not touch common knowledge.
#[test]
fn replace_leaves_the_shared_base_alone() {
    let mut r = StateRegistry::new();
    ingest(
        &mut r,
        "g1",
        "gaians",
        Visibility::Shared,
        vec![entity("tile_1", "smac:Tile")],
        vec![],
    );
    r.ingest(&IngestRequest {
        game_id: "g1".to_string(),
        faction_id: "gaians".to_string(),
        visibility: Visibility::Private,
        replace: true,
        provenance: crate::state::graph::Provenance::default(),
        entities: vec![entity("base_1", "smac:BaseState")],
        edges: vec![],
    })
    .expect("within the tenant cap");

    let view = r.view(&TenantKey::new("g1", "gaians"));
    assert!(
        view.node("tile_1").is_some(),
        "the shared map is not one tenant's to clear"
    );
}
