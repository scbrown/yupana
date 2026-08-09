//! Proposed orders, and applying them to a copy-on-write overlay.
//!
//! ## The divergence risk, and how this bounds it
//!
//! FR-37's standing caveat is that the guard sees an **approximated**
//! post-order board: applying orders to an overlay re-implements a slice of the
//! engine's order semantics outside the engine, and the two can drift.
//!
//! This module bounds that risk by refusing to do the thing that creates it. An
//! [`Order`] carries **declared effects** — yupana does not know that `MOVE`
//! implies a supply change, and does not try to. It applies exactly the deltas
//! the adapter states, so the approximation gap is not "yupana's model of the
//! rules vs. the engine's", it is "what the adapter declared vs. what the engine
//! will do". That is a gap the adapter's author can see, test, and close;
//! an inference engine's gap is one nobody can enumerate.
//!
//! It also means yupana never has an opinion about LEGALITY. The engine remains
//! the sole authority; an order reaching here is one the engine already accepts,
//! and the guard can only subtract from, or annotate, moves that are legal.

use serde::{Deserialize, Serialize};

use super::graph::{AttrValue, StateEdge, StateGraph, StateNode};
use super::overlay::StateOverlay;

/// One declared change an order makes to the board.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OrderEffect {
    /// Create or replace a node.
    UpsertNode {
        /// The node as it will be after the order.
        node: StateNode,
    },
    /// Remove a node and its incident edges.
    RemoveNode {
        /// Node id.
        id: String,
    },
    /// Set one attribute on an existing node.
    SetAttr {
        /// Node id.
        id: String,
        /// Attribute key.
        key: String,
        /// New value.
        value: AttrValue,
    },
    /// Add an edge.
    AddEdge {
        /// The edge to add.
        edge: StateEdge,
    },
    /// Remove an edge.
    RemoveEdge {
        /// Source node id.
        source: String,
        /// Relation.
        relation: String,
        /// Target node id.
        target: String,
    },
}

impl OrderEffect {
    /// Node ids this effect touches — the attribution seam. A policy violation
    /// is blamed on the orders that touched a node in its binding, so "which
    /// orders caused this" is answered from declared effects rather than
    /// guessed.
    #[must_use]
    pub fn touched(&self) -> Vec<&str> {
        match self {
            OrderEffect::UpsertNode { node } => vec![node.id.as_str()],
            OrderEffect::RemoveNode { id } | OrderEffect::SetAttr { id, .. } => vec![id.as_str()],
            OrderEffect::AddEdge { edge } => vec![edge.source.as_str(), edge.target.as_str()],
            OrderEffect::RemoveEdge { source, target, .. } => {
                vec![source.as_str(), target.as_str()]
            }
        }
    }
}

/// One proposed order.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    /// The adapter's id for this order — what a violation names so the caller
    /// can strip exactly this one.
    pub id: String,
    /// What the order is (`MOVE`, `BUILD`, …). Descriptive; yupana attaches no
    /// semantics to it (see the module docs).
    #[serde(default)]
    pub kind: Option<String>,
    /// The declared board deltas.
    #[serde(default)]
    pub effects: Vec<OrderEffect>,
}

impl Order {
    /// Node ids this order touches, deduplicated.
    #[must_use]
    pub fn touched(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for id in self.effects.iter().flat_map(OrderEffect::touched) {
            if !out.iter().any(|seen| seen == id) {
                out.push(id.to_string());
            }
        }
        out
    }
}

/// What applying an order set changed, and what it could not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Applied {
    /// Node ids touched by at least one applied effect.
    pub touched: Vec<String>,
    /// Effects that referenced something absent from the view, described for a
    /// human. NOT silently dropped: an order whose subject does not exist means
    /// the caller and yupana disagree about the board, and a guard run over that
    /// disagreement is answering about a different game.
    pub unapplied: Vec<String>,
}

/// Apply `orders` to a CLONE of `overlay` against `base`, returning the
/// speculative overlay and what changed.
///
/// The clone is the whole "without committing" (FR-38) guarantee, and it is a
/// property of this signature rather than a discipline at the call site: there
/// is no way to pass the tenant's own overlay in and get it mutated.
#[must_use]
pub fn speculate(
    base: &StateGraph,
    overlay: &StateOverlay,
    orders: &[Order],
) -> (StateOverlay, Applied) {
    let mut speculative = overlay.clone();
    let mut applied = Applied::default();
    for order in orders {
        for effect in &order.effects {
            apply_one(base, &mut speculative, order, effect, &mut applied);
        }
    }
    applied.touched.sort();
    applied.touched.dedup();
    (speculative, applied)
}

fn apply_one(
    base: &StateGraph,
    overlay: &mut StateOverlay,
    order: &Order,
    effect: &OrderEffect,
    applied: &mut Applied,
) {
    let view_node = |overlay: &StateOverlay, id: &str| -> Option<StateNode> {
        super::overlay::StateView::new(base, Some(overlay))
            .node(id)
            .cloned()
    };
    match effect {
        OrderEffect::UpsertNode { node } => overlay.upsert_node(node.clone()),
        OrderEffect::RemoveNode { id } => {
            if view_node(overlay, id).is_none() {
                applied.unapplied.push(format!(
                    "order `{}`: cannot remove `{id}` — no such node on this board",
                    order.id
                ));
                return;
            }
            overlay.remove_node(id, base);
        }
        OrderEffect::SetAttr { id, key, value } => {
            let Some(mut node) = view_node(overlay, id) else {
                applied.unapplied.push(format!(
                    "order `{}`: cannot set `{key}` on `{id}` — no such node on this board",
                    order.id
                ));
                return;
            };
            node.attrs.insert(key.clone(), value.clone());
            overlay.upsert_node(node);
        }
        OrderEffect::AddEdge { edge } => {
            // Endpoints are checked for the same reason a dangling edge is
            // refused at ingest: an edge to an entity that is not on the board
            // is a disagreement between the caller and yupana about what the board
            // IS, and a guard run over that disagreement answers about a
            // different game. Reported, never quietly stored.
            let missing: Vec<&str> = [edge.source.as_str(), edge.target.as_str()]
                .into_iter()
                .filter(|id| view_node(overlay, id).is_none())
                .collect();
            if !missing.is_empty() {
                applied.unapplied.push(format!(
                    "order `{}`: cannot add `{} -{}-> {}` — no such node on this board: {}",
                    order.id,
                    edge.source,
                    edge.relation,
                    edge.target,
                    missing.join(", ")
                ));
                return;
            }
            overlay.insert_edge(edge.clone());
        }
        OrderEffect::RemoveEdge {
            source,
            relation,
            target,
        } => {
            let present = super::overlay::StateView::new(base, Some(overlay))
                .edges()
                .iter()
                .any(|e| &e.source == source && &e.relation == relation && &e.target == target);
            if !present {
                applied.unapplied.push(format!(
                    "order `{}`: cannot remove `{source} -{relation}-> {target}` — no such edge \
                     on this board",
                    order.id
                ));
                return;
            }
            overlay.remove_edge(&(source.clone(), relation.clone(), target.clone()));
        }
    }
    for id in effect.touched() {
        applied.touched.push(id.to_string());
    }
}

#[cfg(test)]
#[path = "orders_test.rs"]
mod orders_test;
