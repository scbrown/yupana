//! The generic fact graph: nodes and edges NOT tied to a source span (FR-35).
//!
//! Yupana's [`CodeGraph`](crate::graph::CodeGraph) node is a `SymbolNode` — a
//! name anchored to `file:start_line..end_line`. Everything about it assumes a
//! parsed source file. A board fact ("base Alpha holds 2 garrison units") has no
//! file and no line; forcing it into a span-anchored node would mean inventing
//! coordinates, which is the FR-3 failure one level down: a fabricated anchor
//! reads exactly like a real one.
//!
//! So this is a second, parallel node type rather than a widening of the first.
//! A [`StateNode`] is identified by an opaque `id`, typed by a free-form `kind`,
//! and carries an attribute map. Its provenance is an adapter id + turn +
//! faction ([`Provenance`]), and its tier is always
//! [`Tier::EngineState`](crate::types::Tier::EngineState) — Yupana did not derive
//! this fact, an adapter stated it.
//!
//! ## What is deliberately NOT here
//!
//! No inference, no schema, no closed vocabulary of kinds or relations. Yupana is
//! not an RDF store and this is not a triple store with the serial numbers filed
//! off (see [`super::pattern`] for where that line is drawn). The graph indexes
//! what it was told and answers pattern queries over it; meaning lives in the
//! adapter and in the policies.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::types::Tier;

/// An attribute value on a [`StateNode`].
///
/// A closed set of three scalars, not `serde_json::Value`. The pattern engine
/// compares these with `<`/`>=`/`=`, and a comparison is only well-defined over
/// values whose ordering is agreed in advance — an arbitrary JSON value admits
/// `{"a":1} >= 3`, which has no answer, so the type would have to invent one.
/// Nested structure belongs in edges, which is what a graph is for.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttrValue {
    /// A boolean flag. Listed first so `true` does not deserialize as a number.
    Bool(bool),
    /// A number. One numeric type, `f64`: an adapter sending `2` and one sending
    /// `2.0` must compare equal against the same threshold, and two numeric
    /// variants would make that depend on the sender's JSON formatting.
    Num(f64),
    /// A string.
    Str(String),
}

impl AttrValue {
    /// The value as a number, when it is one. `None` for strings and booleans —
    /// an ordering comparison against a non-number has no answer, and returning
    /// a coerced `0.0` would silently make every such comparison succeed or fail
    /// on a fiction.
    #[must_use]
    pub fn as_num(&self) -> Option<f64> {
        match self {
            AttrValue::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// A short human-readable rendering, for a finding's detail text.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            AttrValue::Bool(b) => b.to_string(),
            AttrValue::Num(n) => {
                if n.fract() == 0.0 && n.is_finite() {
                    format!("{n:.0}")
                } else {
                    n.to_string()
                }
            }
            AttrValue::Str(s) => s.clone(),
        }
    }
}

/// Where an engine-state fact came from: the adapter that stated it, and the
/// turn and faction it was stated for.
///
/// This is FR-35's replacement for `file:line`. It is not decoration: the turn
/// is what makes a stale board detectable, and the faction is what makes an
/// FR-39 fog leak *countable* (see [`super::registry`]). All three are optional
/// on the wire because an adapter that cannot state one must be able to say so
/// rather than fill it in — an invented turn is worse than a missing one.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The adapter that produced the fact (e.g. `smac-worldview`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// The game turn the fact was observed on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    /// The faction whose view produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faction: Option<String>,
}

impl Provenance {
    /// A one-line rendering for a finding or a log line.
    #[must_use]
    pub fn render(&self) -> String {
        let adapter = self.adapter.as_deref().unwrap_or("unknown-adapter");
        match (&self.faction, self.turn) {
            (Some(f), Some(t)) => format!("{adapter}@turn{t}/{f}"),
            (Some(f), None) => format!("{adapter}/{f}"),
            (None, Some(t)) => format!("{adapter}@turn{t}"),
            (None, None) => adapter.to_string(),
        }
    }
}

/// One generic fact-graph node.
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateNode {
    /// Opaque stable identifier — the adapter's entity id. Mirrors the `name`
    /// field of a `quipu_episode` node so one adapter output feeds both stores.
    pub id: String,
    /// The node's type (`BaseState`, `UnitState`, …). Free-form: Yupana enforces
    /// no vocabulary, because the vocabulary that matters is SHACL-validated in
    /// Quipu, and duplicating half of it here would be a second, drifting copy.
    pub kind: String,
    /// Human-readable description, if the adapter supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Scalar attributes the pattern engine can test.
    #[serde(default)]
    pub attrs: BTreeMap<String, AttrValue>,
    /// Where the fact came from.
    #[serde(default)]
    pub provenance: Provenance,
}

impl StateNode {
    /// A bare node of `kind` with no attributes — the constructor tests and
    /// adapters reach for before filling anything in.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            description: None,
            attrs: BTreeMap::new(),
            provenance: Provenance::default(),
        }
    }

    /// Builder: set an attribute.
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: AttrValue) -> Self {
        self.attrs.insert(key.into(), value);
        self
    }

    /// The tier of every fact this node carries. Fixed, not stored: a
    /// [`StateNode`] exists only because an adapter stated it, so there is no
    /// second value this could take, and a settable field would only create the
    /// possibility of a board node claiming to be tree-sitter-derived.
    #[must_use]
    pub fn tier(&self) -> Tier {
        Tier::EngineState
    }
}

/// One generic fact-graph edge. Mirrors the `quipu_episode` edge shape
/// (`source`/`target`/`relation`).
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateEdge {
    /// Source node id.
    pub source: String,
    /// The relation (`adjacent_to`, `garrisoned_at`, …). Free-form, as `kind` is.
    pub relation: String,
    /// Target node id.
    pub target: String,
    /// Where the fact came from.
    #[serde(default)]
    pub provenance: Provenance,
}

impl StateEdge {
    /// An edge with no provenance — the constructor tests and adapters start from.
    #[must_use]
    pub fn new(
        source: impl Into<String>,
        relation: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            relation: relation.into(),
            target: target.into(),
            provenance: Provenance::default(),
        }
    }

    /// The identity three-tuple. Edges are a SET, not a list: an adapter
    /// restating the same edge on the next turn must not grow the graph without
    /// bound, and an order removing "the" edge must have exactly one thing to
    /// remove.
    #[must_use]
    pub fn key(&self) -> EdgeKey {
        (
            self.source.clone(),
            self.relation.clone(),
            self.target.clone(),
        )
    }
}

/// The identity of an edge: `(source, relation, target)`.
pub type EdgeKey = (String, String, String);

/// A generic fact graph — the shared, read-only base of the FR-39 tenancy model.
///
/// Node ids are unique: re-inserting an id REPLACES, it does not duplicate. That
/// is the right default for a board rebuilt every turn from a world view, where
/// the same base appears each time with new attribute values.
#[derive(Debug, Clone, Default)]
pub struct StateGraph {
    nodes: BTreeMap<String, StateNode>,
    edges: BTreeMap<EdgeKey, StateEdge>,
}

impl StateGraph {
    /// An empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a node by id. Returns the node it displaced, if any —
    /// so a caller can report "replaced" separately from "added" rather than
    /// reporting an overwrite as an insert.
    pub fn upsert_node(&mut self, node: StateNode) -> Option<StateNode> {
        self.nodes.insert(node.id.clone(), node)
    }

    /// Insert an edge. Returns `true` if it was not already present.
    pub fn insert_edge(&mut self, edge: StateEdge) -> bool {
        self.edges.insert(edge.key(), edge).is_none()
    }

    /// Remove a node and every edge incident to it. Returns whether it existed.
    ///
    /// Incident edges go with it deliberately: an edge to a removed node is a
    /// dangling reference, and a pattern that traverses it would bind a
    /// variable to an id with no node behind it — a match against something
    /// that is not there.
    pub fn remove_node(&mut self, id: &str) -> bool {
        let existed = self.nodes.remove(id).is_some();
        self.edges.retain(|(s, _, t), _| s != id && t != id);
        existed
    }

    /// Remove an edge by identity. Returns whether it existed.
    pub fn remove_edge(&mut self, key: &EdgeKey) -> bool {
        self.edges.remove(key).is_some()
    }

    /// A node by id.
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&StateNode> {
        self.nodes.get(id)
    }

    /// Every node, in id order.
    pub fn nodes(&self) -> impl Iterator<Item = &StateNode> {
        self.nodes.values()
    }

    /// Every edge, in identity order.
    pub fn edges(&self) -> impl Iterator<Item = &StateEdge> {
        self.edges.values()
    }

    /// The edge set keyed by identity — what [`super::overlay::StateView`]
    /// composes an overlay's adds and masks against. Crate-internal: the keyed
    /// form is a composition detail, and [`Self::edges`] is the public read.
    pub(crate) fn edge_index(&self) -> &BTreeMap<EdgeKey, StateEdge> {
        &self.edges
    }

    /// Node and edge counts.
    #[must_use]
    pub fn stats(&self) -> (usize, usize) {
        (self.nodes.len(), self.edges.len())
    }

    /// Whether the graph holds nothing at all. Load-bearing for the guard: an
    /// empty board cannot be guarded, and reporting "no violations" over one is
    /// the silent-allow this whole harness exists to avoid (see
    /// [`super::guard`]).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }

    /// Node ids whose provenance names a faction — the FR-39 leak detector.
    ///
    /// The shared base is COMMON KNOWLEDGE by definition. A base node stamped
    /// with a faction is private intel that reached the shared layer, which is
    /// the one way fog isolation can fail that the type system cannot prevent:
    /// overlays are structurally disjoint, but nothing stops an adapter posting
    /// a private fact with `visibility: shared`. Counting it is the difference
    /// between a leak that is detectable after the fact and one that is never
    /// detectable at all.
    #[must_use]
    pub fn faction_tagged_ids(&self) -> BTreeSet<String> {
        self.nodes
            .values()
            .filter(|n| n.provenance.faction.is_some())
            .map(|n| n.id.clone())
            .collect()
    }
}

#[cfg(test)]
#[path = "graph_test.rs"]
mod graph_test;
