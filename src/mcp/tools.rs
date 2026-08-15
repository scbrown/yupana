//! Request/response DTOs for Yupana's MCP tools.
//!
//! Requests derive `Deserialize + schemars::JsonSchema` (the schema is served to
//! clients); responses derive `Serialize + schemars::JsonSchema`. Every response
//! that carries facts includes the `tier` tag (FR-3).

use serde::{Deserialize, Serialize};

/// Request for `yupana_symbols` — the symbol tree of one file.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SymbolsRequest {
    /// File path (relative to the analysis root) to list symbols for.
    #[schemars(description = "File path relative to the root, e.g. 'src/main.rs'")]
    pub file: String,
}

/// Request for `yupana_references` — definition sites of a symbol by name.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReferencesRequest {
    /// The symbol name to locate. Optional so a caller can name the symbol by
    /// POSITION instead (`at_file` + `at_line`, FR-4 / yupana #8).
    #[schemars(
        description = "Symbol name to locate, e.g. 'authenticate'. Give this OR at_file+at_line."
    )]
    pub symbol: Option<String>,

    /// Directory to search (relative to the root; defaults to the whole root).
    #[schemars(
        description = "Directory to search, relative to the root. Omit to search everything."
    )]
    pub path: Option<String>,

    /// File of a position-based lookup, relative to the search path.
    #[schemars(
        description = "With at_line: resolve the symbol AT this file/line instead of by name. \
                       Use when a name is ambiguous ('build', 'new') and you know where you are. \
                       Answers with the one symbol enclosing that line, not every symbol sharing \
                       its name."
    )]
    pub at_file: Option<String>,

    /// 1-based line of a position-based lookup.
    ///
    /// LINE, not (line, column): the tree-sitter extractor records lines, so a
    /// column would be accepted and silently ignored — the FR-3 shape where an
    /// approximation is served as the finer tier. Column precision arrives with
    /// the LSP tier (FR-2).
    #[schemars(description = "1-based line for a position-based lookup. Requires at_file.")]
    pub at_line: Option<usize>,
}

/// Request for `yupana_analyze` — a structural summary of a subtree.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AnalyzeRequest {
    /// Directory to analyze (relative to the root; defaults to the whole root).
    #[schemars(
        description = "Directory to analyze, relative to the root. Omit for the whole root."
    )]
    pub path: Option<String>,
}

/// One extracted symbol in a response.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SymbolItem {
    /// The symbol's name.
    pub name: String,
    /// The kind of symbol (`function`, `struct`, ...).
    pub kind: String,
    /// 1-based start line.
    pub start_line: usize,
    /// 1-based end line.
    pub end_line: usize,
    /// Provenance tier (`treesitter`, `lsp`, `cpg`).
    pub tier: String,
}

/// Response for `yupana_symbols`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SymbolsResponse {
    /// The file that was analyzed.
    pub file: String,
    /// Number of symbols found.
    pub count: usize,
    /// The extracted symbols.
    pub symbols: Vec<SymbolItem>,
}

/// One definition site in a `yupana_references` response.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RefItem {
    /// File the definition is in (relative to the root).
    pub file: String,
    /// The symbol's name.
    pub name: String,
    /// The kind of symbol.
    pub kind: String,
    /// 1-based start line.
    pub start_line: usize,
    /// Provenance tier.
    pub tier: String,
}

/// Response for `yupana_references`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ReferencesResponse {
    /// The symbol that was searched for.
    pub symbol: String,
    /// Number of definition sites found.
    pub count: usize,
    /// The definition sites.
    pub definitions: Vec<RefItem>,
    /// How many symbols the answer was searched against, when that is known. A
    /// `count` of 0 over `searched_symbols: 0` means NOTHING under the queried
    /// path was parseable — which is not the same fact as "the name is absent
    /// from a populated graph", and a caller must be able to tell them apart
    /// before reporting "this symbol does not exist" (yupana #76).
    ///
    /// OMITTED, not zeroed, when the answer came from the resident daemon: the
    /// `/references` reply carries `found` but no graph size, and a fabricated
    /// `0` there would assert "nothing was parseable" about a fully populated
    /// resident graph — the exact confident-wrong-answer this field exists to
    /// prevent. Same rule as the FR-3 freshness half: omit rather than fake.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searched_symbols: Option<usize>,
    /// Provenance tier of the answer (FR-3), carried at the top level so the
    /// EMPTY result is tagged too — a zero-definition reply has no `RefItem` to
    /// hang a tier on, which is the same hole `not_found` closed on the CLI side.
    pub tier: String,
}

/// Response for `yupana_analyze`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AnalyzeResponse {
    /// Number of files analyzed.
    pub files: usize,
    /// Total symbols found.
    pub symbols: usize,
    /// Provenance tier of the summary.
    pub tier: String,
}

/// Request for `yupana_callers` / `yupana_callees` — call-graph neighbors.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NeighborsRequest {
    /// The symbol name.
    #[schemars(description = "Symbol name, e.g. 'authenticate'")]
    pub symbol: String,

    /// Directory to build the call graph over (relative to the root).
    #[schemars(description = "Directory relative to the root. Omit for the whole root.")]
    pub path: Option<String>,
}

/// Request for `yupana_impact` — the blast radius of changing a symbol.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ImpactRequest {
    /// The seed symbol.
    #[schemars(description = "Seed symbol name")]
    pub symbol: String,

    /// Directory to build the call graph over (relative to the root).
    #[schemars(description = "Directory relative to the root. Omit for the whole root.")]
    pub path: Option<String>,

    /// Maximum hops to follow (default 5).
    #[schemars(description = "Maximum hops to follow (default 5)")]
    pub hops: Option<u32>,

    /// Historical co-change file set to reconcile against (FR-11). Supplied by
    /// Bobbin; when present the response includes a `reconciliation`.
    #[schemars(
        description = "Co-changed file paths (from Bobbin) to reconcile against the structural impact"
    )]
    pub cochange: Option<Vec<String>>,
}

/// One reached symbol in a call-graph response.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ReachItem {
    /// Symbol name.
    pub name: String,
    /// File (relative to the root).
    pub file: String,
    /// 1-based definition line.
    pub start_line: usize,
    /// Hop distance from the seed.
    pub distance: u32,
    /// Relationship to the seed (`calls` / `called_by`).
    pub via: String,
    /// Provenance tier (FR-3). The call graph is tree-sitter today, so an agent
    /// never reads a `treesitter` reachability edge as if it were LSP-precise.
    pub tier: String,
}

/// Response for `yupana_callers` / `yupana_callees`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct NeighborsResponse {
    /// The seed symbol.
    pub symbol: String,
    /// Whether the symbol exists in the graph.
    pub found: bool,
    /// Number of neighbors.
    pub count: usize,
    /// The direct neighbors.
    pub neighbors: Vec<ReachItem>,
    /// Provenance tier of the whole answer (FR-3). Top-level so a `found: false`
    /// or empty result — which has no items to tag — still declares its tier
    /// rather than arriving unlabelled and reading as authoritative.
    pub tier: String,
}

/// The three-way reconciliation of structural vs. historical coupling (FR-11).
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ReconciliationItem {
    /// In both — corroborated, real coupling.
    pub corroborated: Vec<String>,
    /// Structural but never co-changed — new/unexercised coupling.
    pub structural_only: Vec<String>,
    /// Co-changed but structurally unexplained — possible refactoring smell.
    pub cochange_only: Vec<String>,
}

/// Response for `yupana_impact`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ImpactResponse {
    /// The seed symbol.
    pub symbol: String,
    /// Whether the symbol exists in the graph.
    pub found: bool,
    /// Maximum hops followed.
    pub hops: u32,
    /// Number of affected symbols.
    pub count: usize,
    /// The transitively affected symbols (callers).
    pub reachable: Vec<ReachItem>,
    /// Distinct files in the structural impact set.
    pub structural_files: Vec<String>,
    /// Reconciliation against the supplied co-change set, if any.
    pub reconciliation: Option<ReconciliationItem>,
    /// Provenance tier of the blast radius (FR-3). This is the surface capability
    /// scoping (FR-25) and the trust boundary read from, so an unlabelled
    /// tree-sitter approximation here is exactly what FR-3 forbids. Top-level so a
    /// not-found seed still declares its tier.
    pub tier: String,
}

/// Request for `yupana_communities` — detected symbol clusters over a subtree.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CommunitiesRequest {
    /// Directory to build the call graph over (relative to the root).
    #[schemars(description = "Directory relative to the root. Omit for the whole root.")]
    pub path: Option<String>,
}

/// One member symbol of a community in a `yupana_communities` response.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CommunityMemberItem {
    /// Symbol name.
    pub name: String,
    /// Symbol kind (`function`, `struct`, ...).
    pub kind: String,
    /// File the symbol is defined in (relative to the root).
    pub file: String,
    /// 1-based definition line.
    pub start_line: usize,
    /// Provenance tier.
    pub tier: String,
}

/// One detected community.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CommunityItem {
    /// Stable community id (0-based, largest-cluster-first).
    pub id: usize,
    /// Number of member symbols.
    pub size: usize,
    /// The member symbols, sorted by location.
    pub members: Vec<CommunityMemberItem>,
}

/// Response for `yupana_communities`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CommunitiesResponse {
    /// Number of communities detected.
    pub count: usize,
    /// The detected communities, largest-first.
    pub communities: Vec<CommunityItem>,
    /// Provenance tier of the detection (derived from the tree-sitter graph).
    pub tier: String,
}

/// Request for `yupana_dataflow` — intra-procedural data dependence.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DataflowRequest {
    /// The function to analyze.
    #[schemars(description = "Function name to analyze")]
    pub function: String,

    /// Directory to build the dataflow over (relative to the root).
    #[schemars(description = "Directory relative to the root. Omit for the whole root.")]
    pub path: Option<String>,

    /// Trace flow for a specific variable (omit to return all edges).
    #[schemars(description = "Variable to trace; omit to return all dependence edges")]
    pub var: Option<String>,

    /// Trace what the variable flows into, rather than what it depends on.
    #[schemars(
        description = "If true, trace what the variable flows into (default: what it depends on)"
    )]
    pub forward: Option<bool>,

    /// Maximum hops to follow (default 5).
    #[schemars(description = "Maximum hops to follow (default 5)")]
    pub hops: Option<u32>,
}

/// One reached variable in a flow query.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct FlowStepItem {
    /// Variable name.
    pub name: String,
    /// Hop distance from the queried variable.
    pub distance: u32,
}

/// One data-dependence edge.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DepEdgeItem {
    /// The assigned/bound variable.
    pub dependent: String,
    /// A local used in its initializer.
    pub depends_on: String,
    /// 1-based line.
    pub line: usize,
}

/// Response for `yupana_dataflow`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DataflowResponse {
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
    /// Provenance tier of the dataflow (FR-3). Intra-procedural dependence is
    /// derived from the tree-sitter model; tagged so it is never mistaken for a
    /// CPG/LSP-precise result. Top-level so a not-found function still declares it.
    pub tier: String,
}

/// Response for `yupana_status`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct StatusResponse {
    /// Baseline ref the graph is built at.
    pub base_ref: String,
    /// Tenant/session id.
    pub tenant: String,
    /// Extraction tiers this build can serve.
    pub tiers: Vec<String>,
    /// The languages this build can actually PARSE (aegis-ah0q1). An agent
    /// asking `yupana_impact` about a Python symbol on a Rust-only build gets a
    /// confident empty answer, not an error — so the served set has to be
    /// visible here, next to the tiers, or "no callers" cannot be told apart
    /// from "this build cannot read that language".
    pub languages: Vec<String>,
    /// Whether Quipu promotion is enabled.
    pub quipu_enabled: bool,
    /// The configured branch model.
    pub branch_model: String,
    /// The resident daemon's tenant layer (base commit + active overlays,
    /// yupana #2), when a usable daemon holds one. `None` when no daemon is
    /// expected/usable OR its tenant layer is absent — the daemon's own
    /// `/status` distinguishes those.
    pub tenant_layer: Option<serde_json::Value>,
}

/// Request for `yupana_verify` — a verdict on a proposed edit buffer (FR-23).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VerifyRequest {
    /// The file the buffer is proposed as.
    #[schemars(description = "File path relative to the root, e.g. 'src/auth.rs'")]
    pub file: String,

    /// The full proposed contents of that file.
    #[schemars(description = "The complete proposed contents of the file, as text")]
    pub buffer: String,
}

/// One violation in a `yupana_verify` verdict.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ViolationItem {
    /// `identifier-does-not-exist`, `wrong-arity`, `unresolved-import`, ...
    pub kind: String,
    /// The offending name.
    pub symbol: String,
    /// 1-based line in the proposed buffer (0 when not line-specific).
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

/// Response for `yupana_verify`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct VerifyResponse {
    /// The file that was verified.
    pub file: String,
    /// The boolean verdict: no violations found (FR-24).
    pub ok: bool,
    /// What was found.
    pub violations: Vec<ViolationItem>,
    /// What this tier could NOT check — do not read `ok` as "this compiles".
    pub unchecked: Vec<String>,
    /// Provenance tier (FR-3).
    pub tier: String,
}

/// Request for `yupana_promote` — promote a tree's structural facts into Quipu.
/// Fields are read only by the `quipu`-gated tool body; the type itself is always
/// present because the tool method (and thus its signature) always is.
#[cfg_attr(not(feature = "quipu"), allow(dead_code))]
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PromoteRequest {
    /// Directory to promote, relative to the root. Omit for the root itself.
    #[schemars(
        description = "Directory relative to the root, e.g. 'crates/core'. Omit for the whole root."
    )]
    pub path: Option<String>,

    /// Quipu base URL override (e.g. `http://localhost:8080`). Omit to use the
    /// deployment's configured `[yupana.quipu] endpoint`.
    #[schemars(description = "Quipu base URL override; omit to use the configured endpoint")]
    pub endpoint: Option<String>,

    /// Repository name to attribute promoted entities to. Omit to use the
    /// `origin` remote's repo name; with no origin either, the promotion refuses
    /// rather than deriving identity from the directory name.
    #[schemars(
        description = "Repository name for promoted IRIs; omit to derive from the origin remote"
    )]
    pub repo: Option<String>,
}

/// Response for `yupana_promote`. Constructed only by the `quipu`-gated tool body.
#[cfg_attr(not(feature = "quipu"), allow(dead_code))]
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct PromoteResponse {
    /// Did the promotion validate AND write? False means refused; see `violations`.
    pub wrote: bool,
    /// Triples present for these facts after the write (idempotence signal). None
    /// on refusal.
    pub count: Option<u64>,
    /// Quipu transaction id of the last chunk written, when written. Large
    /// promotions land in multiple `/knot` chunks (see `chunks`); every chunk's
    /// write is idempotent, so the last tx is the graph's settled state.
    pub tx_id: Option<u64>,
    /// How many `/knot` posts the write took (1 = single-post; >1 = the payload
    /// exceeded Quipu's body limit and was split on entity-block boundaries).
    pub chunks: Option<usize>,
    /// SHACL violations when refused — empty iff `wrote`. A refusal always carries
    /// at least one reason.
    pub violations: Vec<String>,
    /// Provenance tier of the promoted facts (FR-3). The export is tree-sitter
    /// derived, so this names the tier of what was (or would have been) written —
    /// a write outcome still says what precision of fact it moved.
    pub tier: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FR-3's tier-on-every-envelope invariant: `PromoteResponse` was the one
    /// served response without a `tier`. A write outcome still names the
    /// precision of the facts it moved.
    #[test]
    fn promote_response_carries_tier() {
        let v = serde_json::to_value(PromoteResponse {
            wrote: true,
            count: Some(1),
            tx_id: Some(1),
            chunks: Some(1),
            violations: Vec::new(),
            tier: crate::types::Tier::TreeSitter.as_str().to_string(),
        })
        .unwrap();
        assert_eq!(v["tier"], "treesitter", "PromoteResponse lost its tier: {v}");
    }
}
