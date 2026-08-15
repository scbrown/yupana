//! The daemon's wire types — what the HTTP surface serializes and a thin client
//! parses. Owned types, independent of the `mcp` feature's response DTOs, and
//! `Deserialize` where a thin client reads them back, so client and server share
//! ONE type per reply and there is no shadow DTO to drift (the stage-3 rule).
//!
//! Split out of `mod.rs` in stage 4 purely for file-size discipline; the engine
//! logic stays there, the wire shapes live here.

use serde::{Deserialize, Serialize};

use crate::graph::{Reached, SymbolNode};
use crate::hook::Sizing;
use crate::types::Tier;

/// The status payload served at `/status` and returned by a successful probe.
/// `status: "ok"` is a constant liveness marker a client greps for; the counts
/// let an operator see the daemon is holding a real graph, not an empty one.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EngineStatus {
    /// Constant `"ok"` — presence of a parseable status body with this field is
    /// the liveness signal.
    pub status: &'static str,
    /// The analysis root the resident graph was built from.
    pub root: String,
    /// Nodes (symbols) in the resident graph.
    pub nodes: usize,
    /// Edges (relations) in the resident graph.
    pub edges: usize,
    /// Seconds since the graph was built.
    pub uptime_secs: u64,
    /// Precision tiers this build actually serves.
    pub tier: Vec<String>,
    /// The tenant layer (yupana #2): base commit + active overlays. `None` means
    /// the layer is ABSENT (the root is not a git repo, so there is no commit
    /// to anchor a shared base to) — distinct from present-with-no-overlays.
    pub tenant_layer: Option<crate::graph::RegistryStatus>,
    /// The FR-39 board layer: games resident, their shared bases, and each
    /// faction's overlay size. `None` when this build has no `game-state`
    /// engine — which is a different fact from an engine holding no games, and
    /// the two must not read alike: the first means `/guard` does not exist
    /// here, the second means it exists and would refuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_layer: Option<BoardLayerStatus>,
}

/// The board layer in an [`EngineStatus`]. A plain mirror of
/// `crate::state::StateStatus` so the wire type stays compilable without the
/// `game-state` feature — the field must EXIST on every build for the "absent
/// engine vs. empty engine" distinction above to be expressible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardLayerStatus {
    /// Games resident, with their factions.
    pub games: Vec<BoardGameStatus>,
    /// Shared writes refused for carrying a faction (the fog-leak counter).
    pub fog_leaks_blocked: usize,
}

/// One game in a [`BoardLayerStatus`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardGameStatus {
    /// The game.
    pub game_id: String,
    /// Entities in its shared, common-knowledge base.
    pub shared_nodes: usize,
    /// Relationships in that base.
    pub shared_edges: usize,
    /// `faction_id` → `(overlay entities, overlay relationships)`.
    pub factions: Vec<(String, usize, usize)>,
}

/// One advised symbol in an [`EditReply`]: a symbol of the edited file that
/// has callers elsewhere.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdvisedSymbol {
    /// The edited file's symbol.
    pub symbol: String,
    /// How many callers outside the edited file.
    pub external_callers: usize,
}

/// Reply for `POST /edit` — the FR-30 feed-and-advise cycle: the edit is
/// recorded in the tenant's overlay, and the advisory is computed from the
/// FRESH composed view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditReply {
    /// The tenant whose overlay absorbed the edit.
    pub tenant: String,
    /// The touched file, root-relative.
    pub file: String,
    /// Symbols the overlay now holds for that file.
    pub symbols: usize,
    /// Symbols with callers OUTSIDE the edited file, with counts.
    pub advised: Vec<AdvisedSymbol>,
    /// Distinct files those external callers live in.
    pub files: Vec<String>,
    /// Symbols the FR-16 frontier recompute reached from this edit — evidence
    /// the recompute ran on the graph this daemon serves, not a fact about the
    /// edit itself. Defaulted so a reply from a daemon predating the field
    /// still parses (as 0, honestly: that daemon recomputed nothing).
    #[serde(default)]
    pub frontier: usize,
    /// Provenance tier of these facts.
    pub tier: String,
}

/// The provenance tier of every reachability fact the resident graph serves.
/// One source of truth for the string, mirroring `mcp::server::graph_tier`; the
/// place to propagate a real per-node tier from when the LSP/CPG tiers land.
pub(super) fn graph_tier() -> String {
    Tier::TreeSitter.as_str().to_string()
}

/// One reached symbol in a neighbors/impact reply. Owned + `Serialize` so the
/// daemon layer does not depend on the `mcp` feature's response types, and
/// `Deserialize` so a thin client (the MCP cutover, stage 3c) parses the same
/// type off the wire that the daemon serialized — no shadow DTO to drift.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReachedItem {
    /// Symbol name.
    pub name: String,
    /// File, relative to the analysis root.
    pub file: String,
    /// 1-based definition line.
    pub start_line: usize,
    /// Hop distance from the seed (1 = direct).
    pub distance: u32,
    /// Relationship to the seed (`calls` or `called_by`). Owned (not
    /// `&'static str`) so the type round-trips through a client's parse.
    pub via: String,
}

pub(super) fn reached_item(r: &Reached) -> ReachedItem {
    ReachedItem {
        name: r.name.clone(),
        file: r.file.clone(),
        start_line: r.start_line,
        distance: r.distance,
        via: r.via.to_string(),
    }
}

/// Reply for `/callers` and `/callees`. `found` is separate from an empty
/// `neighbors` on purpose: "the symbol is not in the graph" and "the symbol has
/// no callers" are different answers, and collapsing them is the fact-vs-absence
/// bug this project keeps paying for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Neighbors {
    /// The queried symbol.
    pub symbol: String,
    /// Whether the symbol exists in the resident graph at all.
    pub found: bool,
    /// Direct neighbors in the requested direction.
    pub neighbors: Vec<ReachedItem>,
    /// Provenance tier of these facts.
    pub tier: String,
}

/// Reply for `/impact` — the transitive blast radius of changing a symbol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Impact {
    /// The queried symbol.
    pub symbol: String,
    /// Whether the symbol exists in the resident graph at all.
    pub found: bool,
    /// Hops followed.
    pub hops: u32,
    /// Number of transitively affected symbols.
    pub count: usize,
    /// The affected symbols (callers).
    pub reachable: Vec<ReachedItem>,
    /// Distinct files in the impact set.
    pub files: Vec<String>,
    /// Provenance tier of these facts.
    pub tier: String,
}

/// One definition site in a `/references` reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DefItem {
    /// File the definition is in, relative to the analysis root.
    pub file: String,
    /// The kind of symbol (lowercase form).
    pub kind: String,
    /// 1-based definition line.
    pub start_line: usize,
}

pub(super) fn def_item(n: &SymbolNode) -> DefItem {
    DefItem {
        file: n.file.clone(),
        kind: n.kind.clone(),
        start_line: n.start_line,
    }
}

/// Reply for `/references` — definition sites of a symbol by name, from the
/// resident graph. Mirrors `yupana_references` (which walks every file per call);
/// here the node index answers with no re-extraction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Definitions {
    /// The queried symbol.
    pub symbol: String,
    /// Whether the symbol exists in the resident graph at all.
    pub found: bool,
    /// Number of definition sites.
    pub count: usize,
    /// The definition sites.
    pub definitions: Vec<DefItem>,
    /// Provenance tier of these facts.
    pub tier: String,
}

/// One symbol in a `/symbols` reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSymbolItem {
    /// Symbol name.
    pub name: String,
    /// The kind of symbol (lowercase form).
    pub kind: String,
    /// 1-based definition line.
    pub start_line: usize,
}

/// Reply for `/symbols` — the symbols one file contributes to the RESIDENT
/// graph, at its build snapshot. `known: false` means the graph holds no
/// symbols for that path — absent, unparseable, and symbol-less files are
/// indistinguishable here (files enter the graph only through their symbols),
/// so a consumer must render it as "no symbols in the resident graph", never
/// "the file is empty". The freshness caveat is the resident surface's usual
/// one: `/status.uptime_secs` says how old the snapshot is.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSymbols {
    /// The queried root-relative path.
    pub file: String,
    /// Whether the resident graph holds any symbols for this path.
    pub known: bool,
    /// Number of symbols.
    pub count: usize,
    /// The symbols, in line order.
    pub symbols: Vec<FileSymbolItem>,
    /// Provenance tier of these facts.
    pub tier: String,
}

/// One reached variable in a `/dataflow` flow trace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlowStepItem {
    /// Variable name.
    pub name: String,
    /// Hop distance from the queried variable.
    pub distance: u32,
}

/// One data-dependence edge in a `/dataflow` reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DepEdgeItem {
    /// The assigned/bound variable.
    pub dependent: String,
    /// A local used in its initializer.
    pub depends_on: String,
    /// 1-based line.
    pub line: usize,
}

/// Reply for `/dataflow`, mirroring `yupana_dataflow`. Unlike every other query
/// endpoint this is NOT resident — dataflow is a separate subsystem with no
/// resident model yet (yupana #22), so the daemon computes it per request over
/// the requested subtree. Served here anyway so the HTTP surface is complete
/// (FR-27); the reply shape will not change when a resident model arrives.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DataflowReply {
    /// The analyzed function.
    pub function: String,
    /// Whether the function exists in the model.
    pub found: bool,
    /// Direction when a variable was traced (`depends_on` / `flows_into`).
    pub direction: Option<String>,
    /// The traced variable, if any.
    pub var: Option<String>,
    /// Flow steps when a variable was traced.
    pub flow: Vec<FlowStepItem>,
    /// All dependence edges when no variable was given.
    pub edges: Vec<DepEdgeItem>,
    /// Provenance tier (FR-3): derived from the tree-sitter model, never to be
    /// mistaken for CPG/LSP precision. Top-level so not-found still declares it.
    pub tier: String,
}

/// The wire form of a [`Sizing`] — what `/measure` returns and a thin-client hook
/// parses. `measured` is separate from a zero radius on purpose: an UNMEASURED edit
/// (no grammar, unreadable, deadline) is NOT a radius of zero, and the client must
/// treat it as "not evaluated", never "within limits" (the fail-open/loud contract).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeasureReply {
    /// Whether a blast radius was actually computed.
    pub measured: bool,
    /// Symbols transitively affected (0 when not measured).
    pub symbols: usize,
    /// Files transitively affected (0 when not measured).
    pub files: usize,
    /// The `Sizing` variant tag: `measured`, `deadline`, `no-grammar`, …. Lets the
    /// client key its once-per-session loud notice by kind, exactly as the in-process
    /// guard does.
    pub kind: String,
    /// The operator-facing reason it was not measured; `None` when measured.
    pub reason: Option<String>,
}

impl MeasureReply {
    /// Map a `Sizing` to its wire form. A measured radius carries its counts; every
    /// unmeasured variant carries its kind tag and reason and a zero radius that the
    /// `measured: false` flag forbids reading as "within limits".
    #[must_use]
    pub fn from_sizing(sizing: &Sizing) -> Self {
        match sizing {
            Sizing::Measured(radius) => Self {
                measured: true,
                symbols: radius.symbols,
                files: radius.files,
                kind: "measured".to_string(),
                reason: None,
            },
            other => Self {
                measured: false,
                symbols: 0,
                files: 0,
                kind: other.kind_tag().to_string(),
                reason: other.unmeasured_reason(),
            },
        }
    }
}
