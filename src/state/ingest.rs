//! The FR-35 ingestion seam: `{ entities[], edges[], tenant, provenance }`.
//!
//! The node/edge JSON mirrors `quipu_episode` (`name` / `type` / `description`
//! for a node, `source` / `target` / `relation` for an edge) so ONE adapter
//! output can feed both stores. Where yupana needs more — scalar attributes the
//! pattern engine can compare — it adds a field rather than overloading one, so
//! the shared subset stays byte-identical.
//!
//! ## `visibility` is the FR-39 routing decision, not a hint
//!
//! [`Visibility::Shared`] writes to the base graph every tenant in the game
//! reads; [`Visibility::Private`] writes to the calling tenant's overlay, which
//! no sibling can reach. There is no default: an adapter must SAY which, because
//! the failure mode of guessing is one faction's private intel becoming common
//! knowledge, and that failure is invisible in results — the run just looks
//! unusually well-informed.
//!
//! A shared write carrying a faction provenance is REFUSED, and counted. That is
//! the one path by which fog isolation could fail that the type system cannot
//! close on its own: overlays are structurally disjoint, but nothing about the
//! types stops an adapter posting private intel with `visibility: shared`.

use serde::{Deserialize, Serialize};

use super::graph::{AttrValue, Provenance, StateEdge, StateGraph, StateNode};
use super::overlay::{StateOverlay, StateView};
use crate::types::Tier;

/// Which layer a fact is written to.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// Common knowledge: the shared base every tenant in the game reads.
    Shared,
    /// This tenant's private intel: its own copy-on-write overlay.
    Private,
}

/// One entity in an ingest request. Mirrors the `quipu_episode` node shape.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct EntitySpec {
    /// The entity id (`quipu_episode`'s `name`).
    pub name: String,
    /// The entity type (`quipu_episode`'s `type`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Scalar attributes — yupana's addition, for the pattern engine to compare.
    #[serde(default)]
    pub attrs: std::collections::BTreeMap<String, AttrValue>,
}

/// One relationship in an ingest request. Mirrors the `quipu_episode` edge shape.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct RelationSpec {
    /// Source entity id.
    pub source: String,
    /// Target entity id.
    pub target: String,
    /// The relation.
    pub relation: String,
}

/// An FR-35 ingest request.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct IngestRequest {
    /// The game this board belongs to.
    pub game_id: String,
    /// The faction whose view is ingesting.
    pub faction_id: String,
    /// Which layer these facts belong to. No default — see the module docs.
    pub visibility: Visibility,
    /// Where the facts came from.
    #[serde(default)]
    pub provenance: Provenance,
    /// The entities.
    #[serde(default)]
    pub entities: Vec<EntitySpec>,
    /// The relationships.
    #[serde(default)]
    pub edges: Vec<RelationSpec>,
}

/// What an ingest did, and what it refused.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IngestReport {
    /// Entities newly created.
    pub nodes_added: usize,
    /// Entities that replaced an existing id.
    pub nodes_replaced: usize,
    /// Relationships newly created.
    pub edges_added: usize,
    /// Relationships already present (restating a board is idempotent).
    pub edges_unchanged: usize,
    /// What was refused, and why. NEVER silently dropped: an adapter whose facts
    /// are being discarded must be able to see it, or it will keep guarding
    /// against a board it thinks it wrote.
    pub rejected: Vec<String>,
    /// Refused shared writes that carried a faction — the FR-39 leak counter.
    pub fog_leaks_blocked: usize,
    /// Board size after the ingest, as `(nodes, edges)`.
    pub board: (usize, usize),
    /// The tier every ingested fact carries.
    pub tier: String,
}

/// Apply `request` to a base graph and one tenant overlay.
///
/// Both layers are passed because [`Visibility`] picks between them; the caller
/// ([`super::registry`]) is what guarantees the overlay is the REQUESTING
/// tenant's, and it is the only thing that can — this function is given one
/// overlay and has no way to name another.
#[must_use]
pub fn apply(
    request: &IngestRequest,
    base: &mut StateGraph,
    overlay: &mut StateOverlay,
) -> IngestReport {
    let mut report = IngestReport {
        tier: Tier::EngineState.as_str().to_string(),
        ..IngestReport::default()
    };

    let stamped = stamp(&request.provenance, request);
    if request.visibility == Visibility::Shared && stamped.faction.is_some() {
        report.fog_leaks_blocked = request.entities.len() + request.edges.len();
        report.rejected.push(format!(
            "refused {} shared fact(s) stamped with faction `{}`: the shared base is COMMON \
             KNOWLEDGE, and a faction-private fact written there is readable by every other \
             faction in the game. Re-post it with `visibility: private`, or drop the faction \
             from its provenance if it really is public.",
            report.fog_leaks_blocked,
            stamped.faction.as_deref().unwrap_or_default()
        ));
        report.board = base.stats();
        return report;
    }

    for entity in &request.entities {
        if entity.name.trim().is_empty() || entity.kind.trim().is_empty() {
            report.rejected.push(format!(
                "entity with name `{}` and type `{}`: both must be non-empty",
                entity.name, entity.kind
            ));
            continue;
        }
        let node = StateNode {
            id: entity.name.clone(),
            kind: entity.kind.clone(),
            description: entity.description.clone(),
            attrs: entity.attrs.clone(),
            provenance: stamped.clone(),
        };
        let replaced = match request.visibility {
            Visibility::Shared => base.upsert_node(node).is_some(),
            Visibility::Private => {
                let existed = StateView::new(base, Some(overlay))
                    .node(&entity.name)
                    .is_some();
                overlay.upsert_node(node);
                existed
            }
        };
        if replaced {
            report.nodes_replaced += 1;
        } else {
            report.nodes_added += 1;
        }
    }

    for spec in &request.edges {
        let view = StateView::new(base, Some(overlay));
        let missing: Vec<&str> = [spec.source.as_str(), spec.target.as_str()]
            .into_iter()
            .filter(|id| view.node(id).is_none())
            .collect();
        if !missing.is_empty() {
            // A dangling edge is refused rather than stored: a pattern that
            // traversed it would bind a variable to an id with no entity behind
            // it — a match against something that is not there.
            report.rejected.push(format!(
                "edge `{} -{}-> {}`: no such entity: {}",
                spec.source,
                spec.relation,
                spec.target,
                missing.join(", ")
            ));
            continue;
        }
        let edge = StateEdge {
            source: spec.source.clone(),
            relation: spec.relation.clone(),
            target: spec.target.clone(),
            provenance: stamped.clone(),
        };
        let added = match request.visibility {
            Visibility::Shared => base.insert_edge(edge),
            Visibility::Private => {
                let existed = view.edges().iter().any(|e| {
                    e.source == spec.source
                        && e.relation == spec.relation
                        && e.target == spec.target
                });
                overlay.insert_edge(edge);
                !existed
            }
        };
        if added {
            report.edges_added += 1;
        } else {
            report.edges_unchanged += 1;
        }
    }

    report.board = StateView::new(base, Some(overlay)).stats();
    report
}

/// The provenance actually stamped on each fact: the request's, with the
/// faction filled in from the tenant when the adapter did not state one.
///
/// Filling it in from the TENANT is not the invented-provenance this repo warns
/// about — the tenant id is a fact about the call, not a guess about the world.
/// It matters because it is what makes a private fact's faction non-`None`, and
/// therefore what makes a mis-routed one detectable.
fn stamp(provenance: &Provenance, request: &IngestRequest) -> Provenance {
    let faction = match request.visibility {
        Visibility::Private => provenance
            .faction
            .clone()
            .or_else(|| Some(request.faction_id.clone())),
        Visibility::Shared => provenance.faction.clone(),
    };
    Provenance {
        adapter: provenance.adapter.clone(),
        turn: provenance.turn,
        faction,
    }
}

#[cfg(test)]
#[path = "ingest_test.rs"]
mod ingest_test;
