//! `yupana_whatif` — speculative impact over the board (FR-38).
//!
//! [`crate::graph::reachable_over`] generalized from the call graph to the fact
//! graph: apply an order set to a copy-on-write overlay, then walk outward from
//! the nodes it touched and rank what it reaches — **without committing**.
//!
//! ## Why the ranked set is domain-neutral
//!
//! The addendum names the impacts it wants: bases exposed, own units entering
//! enemy threat range, reachability / zone-of-control / supply shifts. None of
//! those are computed here by name, and that is deliberate. Yupana does not know
//! what a supply line is; hardcoding a `supply` traversal would put a slice of
//! *Alpha Centauri*'s rules inside a general fact-graph engine, where nobody
//! playing a different game could see it and nobody would maintain it.
//!
//! What is computed is the structural half — which entities a change reaches,
//! how far, and by which relations — over the adapter's own vocabulary. "Bases
//! exposed" is then a `graph-pattern` policy ([`super::policy`]) over the same
//! speculative board, which is the half that belongs in the graph. The two
//! surfaces share one overlay and one apply path, so what the guard denies and
//! what the what-if shows can never be computed from different boards.
//!
//! ## The contrast to hold on to
//!
//! | | `yupana_whatif` | Quipu `quipu_impact remove=true` |
//! |---|---|---|
//! | Subject | ephemeral live board | persisted knowledge |
//! | Speed | fast, this-turn | durable, cross-game |
//! | Use | tactical | strategic / archival |
//!
//! Blurring them is how the hot path acquires a database dependency it cannot
//! afford.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use super::graph::StateGraph;
use super::orders::{speculate, Order};
use super::overlay::{StateOverlay, StateView};
use crate::types::Tier;

/// One board change the order set makes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    /// The node id.
    pub node: String,
    /// `added`, `removed`, or `attr`.
    pub kind: String,
    /// What changed, rendered for a human.
    pub detail: String,
}

/// One entity the change reaches.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Impact {
    /// The node id.
    pub node: String,
    /// The node's kind, as the adapter typed it.
    pub node_kind: String,
    /// Hops from the nearest directly-changed entity. `0` is a direct change.
    pub distance: u32,
    /// The relations the walk crossed to reach it, in first-seen order.
    pub via: Vec<String>,
    /// How many visible edges the entity has on the post-order board. The
    /// secondary rank: at equal distance, a hub is the more consequential
    /// finding, and this says WHY it ranked where it did rather than leaving the
    /// order unexplained.
    pub degree: usize,
}

/// A completed what-if.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WhatIfReport {
    /// How many orders were speculated.
    pub orders: usize,
    /// What the orders changed directly.
    pub changes: Vec<Change>,
    /// What those changes reach, nearest-and-largest first.
    pub impacts: Vec<Impact>,
    /// Declared effects that referenced something absent from the board.
    pub unapplied: Vec<String>,
    /// Hops the walk followed.
    pub hops: u32,
    /// Post-order board size, as `(nodes, edges)`.
    pub board: (usize, usize),
    /// Always `false`. Stated on the wire rather than left implicit: "without
    /// committing" is the property a caller most needs to be able to CHECK, and
    /// an absent field is not a check.
    pub committed: bool,
    /// The tier of everything above.
    pub tier: String,
}

/// The result of a what-if call. Refuses an empty board for the same reason
/// [`super::guard::GuardOutcome`] does — an impact set of zero over a board that
/// was never ingested reads as "this move changes nothing".
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WhatIfOutcome {
    /// The board could not be speculated over.
    Refused {
        /// Why.
        reason: String,
    },
    /// A completed speculation.
    Evaluated(WhatIfReport),
}

/// Speculatively apply `orders` and rank what they reach, up to `hops`.
#[must_use]
pub fn whatif(
    base: &StateGraph,
    overlay: &StateOverlay,
    orders: &[Order],
    hops: u32,
) -> WhatIfOutcome {
    let before = StateView::new(base, Some(overlay));
    if before.is_empty() {
        return WhatIfOutcome::Refused {
            reason: "no board is loaded for this tenant — ingest game state before asking what \
                     an order set would do. Refusing rather than reporting an empty impact set, \
                     which would read as `this move changes nothing`."
                .to_string(),
        };
    }

    let (speculative, applied) = speculate(base, overlay, orders);
    let after = StateView::new(base, Some(&speculative));
    let changes = diff(&before, &after, &applied.touched);
    let impacts = walk(&after, &applied.touched, hops);

    WhatIfOutcome::Evaluated(WhatIfReport {
        orders: orders.len(),
        changes,
        impacts,
        unapplied: applied.unapplied,
        hops,
        board: after.stats(),
        committed: false,
        tier: Tier::EngineState.as_str().to_string(),
    })
}

/// What the orders changed about each touched entity.
fn diff(before: &StateView<'_>, after: &StateView<'_>, touched: &[String]) -> Vec<Change> {
    let mut out = Vec::new();
    for id in touched {
        match (before.node(id), after.node(id)) {
            (None, Some(node)) => out.push(Change {
                node: id.clone(),
                kind: "added".to_string(),
                detail: format!("new `{}` entity", node.kind),
            }),
            (Some(node), None) => out.push(Change {
                node: id.clone(),
                kind: "removed".to_string(),
                detail: format!("`{}` entity removed", node.kind),
            }),
            (Some(old), Some(new)) => {
                let mut keys: BTreeSet<&str> = old.attrs.keys().map(String::as_str).collect();
                keys.extend(new.attrs.keys().map(String::as_str));
                for key in keys {
                    let (was, now) = (old.attrs.get(key), new.attrs.get(key));
                    if was != now {
                        out.push(Change {
                            node: id.clone(),
                            kind: "attr".to_string(),
                            detail: format!(
                                "{key}: {} -> {}",
                                was.map_or_else(
                                    || "unset".to_string(),
                                    super::graph::AttrValue::render
                                ),
                                now.map_or_else(
                                    || "unset".to_string(),
                                    super::graph::AttrValue::render
                                ),
                            ),
                        });
                    }
                }
            }
            // Defensive. No effect kind reaches this today — every one of them
            // validates its subjects and returns via `unapplied` instead of
            // touching a stranger. It is kept so that a NEW effect kind which
            // forgets to validate shows up as an explicit `unknown` change
            // rather than being ranked at hop 0 as if it were on the board.
            (None, None) => out.push(Change {
                node: id.clone(),
                kind: "unknown".to_string(),
                detail: "referenced by an order but absent from the board".to_string(),
            }),
        }
    }
    out
}

/// Breadth-first from the touched entities over the post-order board.
fn walk(view: &StateView<'_>, seeds: &[String], hops: u32) -> Vec<Impact> {
    let mut distance: BTreeMap<String, u32> = BTreeMap::new();
    let mut via: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut queue: VecDeque<(String, u32)> = VecDeque::new();

    for seed in seeds {
        if view.node(seed).is_some() && !distance.contains_key(seed) {
            distance.insert(seed.clone(), 0);
            queue.push_back((seed.clone(), 0));
        }
    }
    while let Some((id, depth)) = queue.pop_front() {
        if depth >= hops {
            continue;
        }
        for (neighbor, relation) in view.neighbors(&id) {
            let relations = via.entry(neighbor.clone()).or_default();
            if !relations.iter().any(|r| r == relation) {
                relations.push(relation.to_string());
            }
            if !distance.contains_key(&neighbor) {
                distance.insert(neighbor.clone(), depth + 1);
                queue.push_back((neighbor, depth + 1));
            }
        }
    }

    let mut impacts: Vec<Impact> = distance
        .into_iter()
        .filter_map(|(id, dist)| {
            let node = view.node(&id)?;
            Some(Impact {
                degree: view.neighbors(&id).len(),
                node_kind: node.kind.clone(),
                via: via.remove(&id).unwrap_or_default(),
                node: id,
                distance: dist,
            })
        })
        .collect();
    // Nearest first, then the biggest hub, then id — a total order, so the
    // ranking is reproducible rather than depending on map iteration.
    impacts.sort_by(|a, b| {
        a.distance
            .cmp(&b.distance)
            .then(b.degree.cmp(&a.degree))
            .then(a.node.cmp(&b.node))
    });
    impacts
}

#[cfg(test)]
#[path = "whatif_test.rs"]
mod whatif_test;
