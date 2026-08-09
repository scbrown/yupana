//! The game-state harness (FR-35..FR-39) — Yupana as a general in-memory fact
//! graph and policy harness, not only a code-analysis engine.
//!
//! Gated behind the `game-state` Cargo feature, which joins the CI matrix in the
//! same change: a feature that can be enabled without its code being exercised
//! is how a build starts advertising a capability it does not have (AGENTS.md,
//! "don't let a feature ship dark" — the same rule the removed `lsp`/`cpg`
//! flags broke, one level up).
//!
//! ## The widening, stated plainly
//!
//! [`crate::graph`] holds a node per parsed symbol, anchored to `file:line`.
//! This module holds a node per *stated fact*, anchored to an adapter id, a turn
//! and a faction. Nothing here parses anything. The two graphs share no node
//! type and no index — deliberately, because the alternative is a `SymbolNode`
//! with an optional file, and an optional anchor is one a consumer will read as
//! present.
//!
//! What IS shared is the machinery around them: copy-on-write overlays, the
//! multi-tenant base/overlay split, the breadth-first impact walk, and the
//! Selector/Predicate/`matchType` policy shape (this plane imports
//! [`crate::rules::MatchType`] rather than redeclaring it). One governance
//! model, two subjects.
//!
//! ## The layers
//!
//! | module | requirement | what it is |
//! |---|---|---|
//! | [`graph`] | FR-35 | the span-free node/edge type and the shared base |
//! | [`overlay`] | FR-35/39 | the copy-on-write delta and the composed read view |
//! | [`ingest`] | FR-35 | the `{entities, edges, tenant, provenance}` seam |
//! | [`pattern`] | FR-36 | the `graph-pattern` selector language |
//! | [`policy`] | FR-36 | Selector/Predicate/boundary/effect over a board |
//! | [`orders`] | FR-37 | proposed orders and their declared effects |
//! | [`guard`] | FR-37 | evaluate policies over the post-order board |
//! | [`whatif`] | FR-38 | ranked speculative impact, uncommitted |
//! | [`registry`] | FR-39 | `(game, faction)` tenancy and its isolation |
//!
//! ## Three standing caveats, all load-bearing
//!
//! 1. **Yupana is not an RDF store.** `graph-pattern` is a small fixed subset;
//!    `sparql` is reserved for Quipu and REFUSED here, never approximated. If a
//!    predicate starts wanting property paths or aggregation, the policy belongs
//!    in Quipu.
//! 2. **The guard sees an APPROXIMATED post-order board.** Orders carry declared
//!    effects and yupana applies exactly those, so the gap is "what the adapter
//!    declared vs. what the engine does" — visible and closable — rather than
//!    yupana's own reimplementation of the rules. The **game engine remains the
//!    sole authority** on legality and effects; this can only subtract from, or
//!    annotate, moves that are already legal, and `deny` policies should be
//!    conservative because a false deny removes a legal, possibly correct move.
//! 3. **A board lives in the process it was ingested into.** There is no shared
//!    store behind these surfaces — that is the point of a hot graph. Ingesting
//!    over HTTP and then guarding over a *different* MCP process would guard an
//!    empty board, so [`guard::guard`] and [`whatif::whatif`] REFUSE an empty
//!    board rather than returning zero findings. Zero findings over a board that
//!    was never loaded is a green light over a dead backend.

pub mod graph;
pub mod guard;
pub mod ingest;
pub mod orders;
pub mod overlay;
pub mod pattern;
pub mod policy;
pub mod registry;
pub mod whatif;

pub use graph::{AttrValue, Provenance, StateEdge, StateGraph, StateNode};
pub use guard::{guard, Finding, GuardOutcome, GuardReport, PolicyNote};
pub use ingest::{EntitySpec, IngestReport, IngestRequest, RelationSpec, Visibility};
pub use orders::{Order, OrderEffect};
pub use overlay::{StateOverlay, StateView};
pub use pattern::{Bound, Pattern};
pub use policy::{Boundary, Effect, MatchType, Predicate, Selector, SelectorLang, StatePolicy};
pub use registry::{IsolationReport, StateRegistry, StateStatus, TenantKey};
pub use whatif::{whatif, WhatIfOutcome, WhatIfReport};
