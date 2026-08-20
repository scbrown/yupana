//! The Yupana MCP server and its tools.
//!
//! Registration mirrors Bobbin: a `#[tool_router]` impl of `#[tool]`-annotated
//! async methods taking `Parameters<Req>`, a `#[tool_handler] ServerHandler`
//! providing `get_info`, and stdio + streamable-HTTP transports.

use std::path::PathBuf;

use anyhow::Result;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use serde::Serialize;

use std::collections::BTreeSet;

use super::goldenpath_tools::PathCheckRequest;
use super::state_tools::{StateGuardRequest, StateIngestRequest, StateWhatIfRequest};
use super::tools::PromoteRequest;
#[cfg(feature = "quipu")]
use super::tools::PromoteResponse;
use super::tools::{
    AnalyzeRequest, AnalyzeResponse, CommunitiesRequest, CommunitiesResponse, CommunityItem,
    CommunityMemberItem, DataflowRequest, DataflowResponse, DepEdgeItem, FlowStepItem,
    ImpactRequest, ImpactResponse, NeighborsRequest, NeighborsResponse, ReachItem,
    ReconciliationItem, RefItem, ReferencesRequest, ReferencesResponse, StatusResponse, SymbolItem,
    SymbolsRequest, SymbolsResponse, VerifyRequest, VerifyResponse, ViolationItem,
};
use crate::config::YupanaConfig;
use crate::dataflow::{Dataflow, FlowDir};
use crate::extract::{extract_symbols, rust_files};
use crate::graph::{CodeGraph, Dir, Reached};
use crate::reconcile::reconcile;
use crate::types::Tier;

/// The provenance tier of everything the call graph and dataflow serve. The
/// graph is built entirely from tree-sitter extraction (`CodeGraph::build`), so
/// every reachability/dataflow fact is `treesitter` today — one source of truth
/// for that string rather than a literal repeated per handler, and the place to
/// propagate a real per-node tier from when the LSP/CPG tiers start resolving
/// edges (FR-3).
fn graph_tier() -> String {
    Tier::TreeSitter.as_str().to_string()
}

/// Yupana's MCP server. Resolves requests against the analysis root for a tenant.
#[derive(Clone)]
pub struct YupanaMcpServer {
    root: PathBuf,
    tenant: Option<String>,
    /// The `--config` override the server was launched with, if any. Honoured
    /// on every config read so `yupana serve --config` is not silently ignored
    /// (aegis-ll3p).
    config: Option<PathBuf>,
    /// The FR-39 board layer this PROCESS holds (see
    /// [`super::state_handlers`]). Shared across clones of the server, so every
    /// tool call in one process sees one board — and, necessarily, a board
    /// ingested into a DIFFERENT process is not this one.
    #[cfg(feature = "game-state")]
    board: std::sync::Arc<std::sync::RwLock<crate::state::StateRegistry>>,
    tool_router: ToolRouter<Self>,
}

impl YupanaMcpServer {
    /// Construct a server rooted at `root` for an optional `tenant`, honouring
    /// an optional `--config` override.
    #[must_use]
    pub fn new(root: PathBuf, tenant: Option<String>, config: Option<PathBuf>) -> Self {
        Self {
            root,
            tenant,
            config,
            #[cfg(feature = "game-state")]
            board: std::sync::Arc::new(std::sync::RwLock::new(crate::state::StateRegistry::new())),
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl YupanaMcpServer {
    #[tool(
        description = "Show Yupana's base ref, tenant, available extraction tiers, the languages this build can parse, and Quipu promotion settings. Check `languages` before trusting an empty impact/callers answer: a language absent there yields no symbols at all."
    )]
    async fn yupana_status(&self) -> Result<CallToolResult, McpError> {
        let config = YupanaConfig::resolve(self.config.as_deref(), &self.root).map_err(internal)?;
        let response = StatusResponse {
            base_ref: config.base_ref,
            tenant: self
                .tenant
                .clone()
                .unwrap_or_else(|| "(single-tenant)".to_string()),
            tiers: Tier::served(),
            languages: crate::extract::languages()
                .into_iter()
                .map(String::from)
                .collect(),
            quipu_enabled: config.quipu.enabled,
            branch_model: config.quipu.branch_model,
            tenant_layer: super::resident::tenant_layer(self.config.as_deref(), &self.root),
        };
        json_result(&response)
    }

    #[tool(
        description = "List the symbols (functions, structs, traits, ...) defined in one file. Each symbol carries a tier tag. Best for: 'what's defined in src/auth.rs?'."
    )]
    async fn yupana_symbols(
        &self,
        Parameters(req): Parameters<SymbolsRequest>,
    ) -> Result<CallToolResult, McpError> {
        let file = self.root.join(&req.file);
        let source = std::fs::read_to_string(&file).map_err(internal)?;
        let symbols = extract_symbols(&source, "rust").map_err(internal)?;
        let response = SymbolsResponse {
            file: req.file.clone(),
            count: symbols.len(),
            symbols: symbols
                .iter()
                .map(|symbol| SymbolItem {
                    name: symbol.name.clone(),
                    kind: symbol.kind.as_str().to_string(),
                    start_line: symbol.start_line,
                    end_line: symbol.end_line,
                    tier: symbol.tier.as_str().to_string(),
                })
                .collect(),
        };
        json_result(&response)
    }

    #[tool(
        description = "Find the definition site(s) of a symbol by name across a subtree. Best for: 'where is authenticate defined?'."
    )]
    async fn yupana_references(
        &self,
        Parameters(req): Parameters<ReferencesRequest>,
    ) -> Result<CallToolResult, McpError> {
        handlers::references(self, &req)
    }

    #[tool(
        description = "Summarize the structure of a subtree: how many files and symbols. Best for a quick health check of the base graph."
    )]
    async fn yupana_analyze(
        &self,
        Parameters(req): Parameters<AnalyzeRequest>,
    ) -> Result<CallToolResult, McpError> {
        let base = req
            .path
            .as_ref()
            .map_or_else(|| self.root.clone(), |p| self.root.join(p));
        let mut files = 0usize;
        let mut symbols = 0usize;
        for file in rust_files(&base) {
            let Ok(source) = std::fs::read_to_string(&file) else {
                continue;
            };
            if let Ok(found) = extract_symbols(&source, "rust") {
                files += 1;
                symbols += found.len();
            }
        }
        let response = AnalyzeResponse {
            files,
            symbols,
            tier: "treesitter".to_string(),
        };
        json_result(&response)
    }

    #[tool(
        description = "List the direct callers of a symbol (who calls it). Best for: 'who calls authenticate?'."
    )]
    async fn yupana_callers(
        &self,
        Parameters(req): Parameters<NeighborsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.neighbors(&req, Dir::Callers)
    }

    #[tool(
        description = "List the direct callees of a symbol (what it calls). Best for: 'what does authenticate call?'."
    )]
    async fn yupana_callees(
        &self,
        Parameters(req): Parameters<NeighborsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.neighbors(&req, Dir::Callees)
    }

    #[tool(
        description = "Blast radius: the symbols transitively affected by changing a symbol (its callers, up to N hops). Best for: 'what breaks if I change authenticate?'."
    )]
    async fn yupana_impact(
        &self,
        Parameters(req): Parameters<ImpactRequest>,
    ) -> Result<CallToolResult, McpError> {
        let hops = req.hops.unwrap_or(5);
        // Stage 3c: an expected, same-root resident daemon answers with no
        // per-call build — but never a `path`-scoped request (the resident
        // graph is whole-root; a subtree query is a different graph). Every
        // `None` falls through to the transient build, silently (query
        // surface, not the guard — loud-absence is the hook's contract).
        if req.path.is_none() {
            if let Some(response) = super::resident::impact(
                self.config.as_deref(),
                &self.root,
                &req.symbol,
                hops,
                req.cochange.as_deref(),
            ) {
                return json_result(&response);
            }
        }
        let base = req
            .path
            .as_ref()
            .map_or_else(|| self.root.clone(), |p| self.root.join(p));
        let graph = CodeGraph::build(&base).map_err(internal)?;
        let found = graph.has_symbol(&req.symbol);
        let reachable = graph.reachable(&req.symbol, Dir::Callers, hops);
        let structural_files: BTreeSet<String> = reachable.iter().map(|r| r.file.clone()).collect();
        let reconciliation = req.cochange.as_ref().map(|cochange| {
            let cochange_set: BTreeSet<String> = cochange.iter().cloned().collect();
            let recon = reconcile(&structural_files, &cochange_set);
            ReconciliationItem {
                corroborated: recon.corroborated,
                structural_only: recon.structural_only,
                cochange_only: recon.cochange_only,
            }
        });
        let response = ImpactResponse {
            symbol: req.symbol.clone(),
            found,
            hops,
            count: reachable.len(),
            reachable: reachable.iter().map(reach_item).collect(),
            structural_files: structural_files.into_iter().collect(),
            reconciliation,
            tier: graph_tier(),
        };
        json_result(&response)
    }

    #[tool(
        description = "Detect communities: densely-connected clusters of symbols in the call graph (deterministic Louvain). Best for: 'what are the natural modules/subsystems here?'."
    )]
    async fn yupana_communities(
        &self,
        Parameters(req): Parameters<CommunitiesRequest>,
    ) -> Result<CallToolResult, McpError> {
        let base = req
            .path
            .as_ref()
            .map_or_else(|| self.root.clone(), |p| self.root.join(p));
        let graph = CodeGraph::build(&base).map_err(internal)?;
        let comms = graph.communities();
        let communities = comms
            .iter()
            .map(|c| CommunityItem {
                id: c.id,
                size: c.members.len(),
                members: c
                    .members
                    .iter()
                    .map(|m| CommunityMemberItem {
                        name: m.name.clone(),
                        kind: m.kind.clone(),
                        file: m.file.clone(),
                        start_line: m.start_line,
                        tier: m.tier.as_str().to_string(),
                    })
                    .collect(),
            })
            .collect();
        let response = CommunitiesResponse {
            count: comms.len(),
            communities,
            tier: "treesitter".to_string(),
        };
        json_result(&response)
    }

    #[tool(
        description = "Verify a PROPOSED edit buffer before you write it: returns a boolean verdict plus violations (identifier-does-not-exist, wrong-arity, unresolved-import). Best for: 'will this edit break something?'. Note the `unchecked` list — a true verdict is not a compile."
    )]
    async fn yupana_verify(
        &self,
        Parameters(req): Parameters<VerifyRequest>,
    ) -> Result<CallToolResult, McpError> {
        let file = self.root.join(&req.file);
        // The file's current contents are the baseline, so violations that
        // already exist are not blamed on the proposed edit.
        let baseline = std::fs::read_to_string(&file).ok();
        let verdict =
            crate::verify::verify_buffer(&self.root, &file, &req.buffer, baseline.as_deref())
                .map_err(internal)?;

        let response = VerifyResponse {
            file: req.file,
            ok: verdict.ok,
            violations: verdict
                .violations
                .iter()
                .map(|v| ViolationItem {
                    kind: serde_json::to_value(v.kind)
                        .ok()
                        .and_then(|k| k.as_str().map(str::to_string))
                        .unwrap_or_default(),
                    symbol: v.symbol.clone(),
                    line: v.line,
                    message: v.message.clone(),
                })
                .collect(),
            unchecked: verdict.unchecked,
            tier: "treesitter".to_string(),
        };
        json_result(&response)
    }

    // Always REGISTERED (the `#[tool_router]` macro references every `#[tool]`
    // method unconditionally, so cfg-gating the whole method breaks an `mcp`-only
    // build). The BODY is feature-split: real promotion under `quipu`, an honest
    // refusal without it — the same shape the CLI's `promote` uses.
    #[tool(
        description = "Promote a subtree's structural code facts into Quipu: emits Turtle, SHACL-validates it IN-PROCESS, and writes it only if it conforms (all-or-nothing). Returns wrote + triple count on success, or violations on refusal. The write is guarded by serve.read_only. Best for: 'get this code's structure into the knowledge graph, validated'."
    )]
    async fn yupana_promote(
        &self,
        Parameters(req): Parameters<PromoteRequest>,
    ) -> Result<CallToolResult, McpError> {
        handlers::promote(self, &req)
    }

    #[tool(
        description = "Intra-procedural data dependence within a function. With `var`, trace what it depends on (or, with forward=true, what it flows into); without `var`, list all dependence edges. Best for: 'where does this value come from?'."
    )]
    async fn yupana_dataflow(
        &self,
        Parameters(req): Parameters<DataflowRequest>,
    ) -> Result<CallToolResult, McpError> {
        handlers::dataflow(self, &req)
    }

    // The three board tools (FR-35/37/38). Always REGISTERED for the same reason
    // `yupana_promote` is — the `#[tool_router]` macro references every `#[tool]`
    // method unconditionally — with the BODY feature-split on `game-state`.
    #[tool(
        description = "Ingest generic (non-code) facts into the hot board graph: {entities[], edges[], game_id, faction_id, visibility, provenance}. The node/edge JSON mirrors quipu_episode, so one adapter output feeds both stores. `visibility` has NO default and MUST be stated: `shared` writes the game's common-knowledge base that every faction reads, `private` writes only this faction's copy-on-write overlay. A shared write carrying a faction is refused and counted — that is a fog-of-war leak. Facts are tagged tier `engine-state`. Best for: 'load this turn's world view before guarding a move'."
    )]
    async fn yupana_ingest(
        &self,
        Parameters(req): Parameters<StateIngestRequest>,
    ) -> Result<CallToolResult, McpError> {
        state_handlers::ingest(self, &req)
    }

    #[tool(
        description = "Check proposed orders against game-state policies, over a copy-on-write overlay of THIS faction's board — the (game_state + proposed_orders) analog of yupana_verify. Returns violations (deny) and advisories (warn), each naming the offending order ids, plus `unevaluated` and `vacuous` for policies that could not run or whose selector matched nothing. REFUSES if no board was ingested into this process, rather than reporting zero violations over an empty board. COMPLEMENTS the game engine, which remains the sole authority on legality: this can only subtract from, or annotate, moves that are already legal, and it judges an APPROXIMATED post-order board built from each order's declared effects. Best for: 'would these orders break one of our standing rules?'."
    )]
    async fn yupana_guard(
        &self,
        Parameters(req): Parameters<StateGuardRequest>,
    ) -> Result<CallToolResult, McpError> {
        state_handlers::guard(self, &req)
    }

    #[tool(
        description = "Speculatively apply an order set to this faction's board and return what it changes and what those changes reach, ranked nearest-then-largest — yupana_impact generalized from the call graph to the board. Nothing is committed. Structural only: which entities a change reaches, how far, by which relations, over the adapter's own vocabulary — domain judgements like 'is this base exposed' are graph-pattern policies via yupana_guard, not hardcoded here. Contrast with quipu_impact, which is durable and cross-game; this is ephemeral, this-turn and tactical. Best for: 'what does this move expose?'."
    )]
    async fn yupana_whatif(
        &self,
        Parameters(req): Parameters<StateWhatIfRequest>,
    ) -> Result<CallToolResult, McpError> {
        state_handlers::whatif(self, &req)
    }

    // The golden-path conformance check (FR-41/FR-42). Always REGISTERED, body
    // feature-split on `golden-path`, same as the board tools above.
    #[tool(
        description = "Check work against a blessed golden path (a pruned, human-promoted trajectory governed in Quipu). Pass the declared follows_path, the steps as {action_kind, target_class} signatures, and the projected paths (per call — a stale resident copy would enforce yesterday's blessing). mode='plan' treats the steps as the whole intent and names the first deviation point; mode='progress' reports how far along the path the work is and which dead-end hazards it brushed, and never denies. Effects are capped by blessing level: advisory warns at most; blessed denies only with deny=true in plan mode. Refuses (never reports clean) an empty path set, an undeclared path, or a gp-grammar version this build does not implement."
    )]
    async fn yupana_path_check(
        &self,
        Parameters(req): Parameters<PathCheckRequest>,
    ) -> Result<CallToolResult, McpError> {
        goldenpath_handlers::path_check(self, &req)
    }
}

impl YupanaMcpServer {
    /// Shared body for `yupana_callers` / `yupana_callees`.
    fn neighbors(&self, req: &NeighborsRequest, dir: Dir) -> Result<CallToolResult, McpError> {
        // Stage 3c: same cutover shape as `yupana_impact` — resident daemon when
        // usable and unscoped, transient fallback otherwise (see there).
        if req.path.is_none() {
            if let Some(response) =
                super::resident::neighbors(self.config.as_deref(), &self.root, &req.symbol, dir)
            {
                return json_result(&response);
            }
        }
        let base = req
            .path
            .as_ref()
            .map_or_else(|| self.root.clone(), |p| self.root.join(p));
        let graph = CodeGraph::build(&base).map_err(internal)?;
        let found = graph.has_symbol(&req.symbol);
        let neighbors = graph.direct(&req.symbol, dir);
        let response = NeighborsResponse {
            symbol: req.symbol.clone(),
            found,
            count: neighbors.len(),
            neighbors: neighbors.iter().map(reach_item).collect(),
            tier: graph_tier(),
        };
        json_result(&response)
    }
}

#[tool_handler]
impl ServerHandler for YupanaMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "yupana".to_string(),
                title: Some("Yupana Code Structure".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "Yupana serves live, per-tenant code structure. Use yupana_symbols to list a \
                 file's symbols, yupana_references to find where a symbol is defined, \
                 yupana_analyze for a subtree summary, and yupana_status for base ref and tiers. \
                 Every fact is tagged with its tier (treesitter/lsp/cpg)."
                    .to_string(),
            ),
        }
    }
}

/// Serialize a response into a successful tool result.
fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value).map_err(internal)?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

/// Map any error into an MCP internal error.
fn internal<E: std::fmt::Display>(err: E) -> McpError {
    McpError::internal_error(err.to_string(), None)
}

/// Convert a graph `Reached` into the wire DTO.
fn reach_item(reached: &Reached) -> ReachItem {
    ReachItem {
        name: reached.name.clone(),
        file: reached.file.clone(),
        start_line: reached.start_line,
        distance: reached.distance,
        via: reached.via.to_string(),
        tier: graph_tier(),
    }
}

// The bodies of the two heaviest tool handlers (yupana #83). Not test-gated —
// `yupana_promote` and `yupana_dataflow` call into it at runtime.
#[path = "handlers.rs"]
mod handlers;

// The board tool bodies (FR-35/37/38), feature-split on `game-state`.
#[path = "state_handlers.rs"]
mod state_handlers;

// The golden-path check body (FR-41/FR-42), feature-split on `golden-path`.
#[path = "goldenpath_handlers.rs"]
mod goldenpath_handlers;

// The FR-3 enforcement walk (aegis-8yrn) lives in a size-exempt sibling file so
// it can call the private tool handlers as a child module without pushing
// server.rs past the 500-line limit.
#[cfg(all(test, feature = "mcp"))]
#[path = "server_test.rs"]
mod server_test;
