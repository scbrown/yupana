//! The resident engine — Phase 3, stage 1 (FR-31, yupana #1 / aegis-1qze).
//!
//! Today the hook and one-shot commands build the whole `CodeGraph` transiently,
//! per invocation. FR-31 makes a resident process that holds the base graph in
//! memory the foundation for the sub-100ms guard budget and for per-tenant
//! overlays. This module is that process's core: it builds the graph ONCE, holds
//! it, and exposes a liveness/status surface (stage 1) plus graph-backed query
//! endpoints (stage 2). The hook/MCP thin-client cutover is stage 3. Landing in stages is
//! deliberate — a half-built resident guard is a footgun (see below).
//!
//! ## Two invariants this stage exists to establish before any query lands
//!
//! 1. **Daemon-absent must be a DISTINCT, LOUD signal — never a silent allow.**
//!    Once the guard is a thin client (stage 3), a down daemon is the cheapest
//!    possible bypass: kill one process and every edit sails through. So the
//!    client seam ([`client`]) reports "not reachable" as its own variant that a
//!    caller cannot fold into a default — the compiler makes you handle it. This
//!    is built now, with the process, so the cutover cannot forget it.
//!
//! 2. **The resident policy state is loaded ONCE, at a single trust point.** The
//!    engine holds a config snapshot ([`ResidentEngine::policy`]) taken at build
//!    time, not re-read per request. That single load site is where the
//!    aegis-hac0 signed rule cache will verify-and-trust: sign/verify wraps this
//!    one boundary rather than being scattered across per-invocation disk reads.
//!    The seam is here; the signing is that issue's job, not this stage's.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use crate::config::YupanaConfig;
use crate::graph::{Base, CodeGraph, Dir, TenantRegistry};
use crate::hook::Sizing;
use crate::policy::PolicyConfig;

pub mod client;
#[cfg(feature = "quipu")]
pub mod client_policy;
#[cfg(feature = "quipu")]
pub mod exposure;
#[cfg(feature = "mcp")]
pub(crate) mod http;
pub mod keywords;
#[cfg(all(feature = "mcp", feature = "golden-path"))]
pub(crate) mod path_http;
#[cfg(feature = "quipu")]
pub mod projection;
#[cfg(all(feature = "mcp", feature = "game-state"))]
pub(crate) mod state_http;
mod tenanted;
pub mod wire;

use wire::{def_item, graph_tier, reached_item};
pub use wire::{
    AdvisedSymbol, DefItem, Definitions, EditReply, EngineStatus, FileSymbolItem, FileSymbols,
    Impact, MeasureReply, Neighbors, ReachedItem,
};

/// The base graph plus its policy snapshot, built once and held for the process
/// lifetime. Cheap to clone (`Arc`), so the HTTP layer shares one instance.
#[derive(Clone)]
pub struct ResidentEngine {
    inner: Arc<Engine>,
}

struct Engine {
    keywords: keywords::ResidentKeywords,
    root: PathBuf,
    graph: CodeGraph,
    /// The config resolved at startup. NOT re-read per request — this is the
    /// single trust point the aegis-hac0 signed cache will guard (see module docs).
    config: YupanaConfig,
    built_at: SystemTime,
    nodes: usize,
    edges: usize,
    /// The tenant layer (yupana #2 wiring): a shared [`Base`] at the startup
    /// HEAD plus per-tenant overlays, fed by `POST /edit`. `None` outside a
    /// git repo — a `Base` needs a commit to anchor to; the working-tree
    /// `graph` above keeps serving the un-tenanted surface either way.
    /// Tenant views compose over the COMMITTED base by design (FR-22 keeps
    /// overlay churn distinct from committed truth), so an uncommitted
    /// working-tree delta at startup is visible to the legacy surface and
    /// absent from tenant views until a tenant touches those files.
    registry: Option<RwLock<TenantRegistry>>,
    /// The FR-39 board layer: one shared base per game, one overlay per
    /// `(game, faction)`. Unlike `registry` this is never `None` — a board is
    /// built by ingestion, not by a repo, so there is nothing for it to be
    /// absent FOR. It starts empty, and an empty board is REFUSED by the guard
    /// rather than reported as clean (see [`crate::state`]).
    #[cfg(feature = "game-state")]
    board: RwLock<crate::state::StateRegistry>,
    /// Per-`(tenant, file)` code-fact freshness, tracked on the edit path the
    /// way the watch path tracks it (`Recomputing` while the frontier is behind
    /// the touch, `Fresh` once recomputed). This is the bobbin-bnq wire: the
    /// FR-16 recompute and the serving surface finally meet in one process, so
    /// the recomputed overlay is queryable and its freshness is a fact this
    /// engine actually knows. Tracked, not yet stamped onto query DTOs — the
    /// serve half is Phase 3 (FR-3).
    freshness:
        std::sync::Mutex<std::collections::HashMap<(String, String), crate::types::Freshness>>,
    /// The RESIDENT PROJECTED POLICY (aegis-x894x2). `None` when quipu is not
    /// configured, so `/projection` 503s. Details in [`projection`].
    #[cfg(feature = "quipu")]
    projection: Option<Arc<projection::ResidentProjection>>,
}

impl ResidentEngine {
    /// Shared entity keyword data for all resident feedback clients.
    pub fn keywords(&self) -> &keywords::ResidentKeywords {
        &self.inner.keywords
    }
    /// Build the base graph for `root` and hold it resident. Runs once, at
    /// startup; a failure here means the daemon refuses to start rather than
    /// serving a graph it could not build.
    ///
    /// `config_override` mirrors the `--config` flag so the daemon honours the
    /// same config resolution as every other entry point.
    pub fn build(root: &Path, config_override: Option<&Path>) -> anyhow::Result<Self> {
        let config = YupanaConfig::resolve(config_override, root)?;
        let graph = CodeGraph::build(root)?;
        let (nodes, edges) = graph.stats();
        // The tenant layer needs a commit to anchor its shared base to.
        // Outside a repo there is none — the engine still serves, un-tenanted,
        // and /status says the tenant layer is absent rather than empty.
        let registry = Base::build_at(root, "HEAD")
            .ok()
            .map(|base| RwLock::new(TenantRegistry::with_tenancy(base, config.tenancy.clone())));
        #[cfg(feature = "quipu")]
        let projection = projection::for_config(&config);
        Ok(Self {
            inner: Arc::new(Engine {
                keywords: keywords::ResidentKeywords::default(),
                root: root.to_path_buf(),
                graph,
                config,
                built_at: SystemTime::now(),
                nodes,
                edges,
                registry,
                #[cfg(feature = "game-state")]
                board: RwLock::new(crate::state::StateRegistry::new()),
                freshness: std::sync::Mutex::new(std::collections::HashMap::new()),
                #[cfg(feature = "quipu")]
                projection,
            }),
        })
    }

    /// The resident projection, if this daemon has one.
    #[cfg(feature = "quipu")]
    #[must_use]
    pub fn projection(&self) -> Option<&Arc<projection::ResidentProjection>> {
        self.inner.projection.as_ref()
    }
    /// The build-time config snapshot — the single trust point, never re-read.
    #[must_use]
    pub fn config(&self) -> &YupanaConfig {
        &self.inner.config
    }

    /// The code-fact freshness of `rel` for `tenant`, or `None` if this engine
    /// never absorbed an edit for it. The query half of the tracking the edit
    /// path maintains — the FR-3 serve wiring reads from here at Phase 3.
    #[must_use]
    pub fn freshness_of(&self, tenant: &str, rel: &str) -> Option<crate::types::Freshness> {
        self.inner
            .freshness
            .lock()
            .ok()?
            .get(&(tenant.to_string(), rel.to_string()))
            .copied()
    }

    /// Record `rel`'s freshness for `tenant`. Fail-silent like the watch path's
    /// tracker: a poisoned map loses a freshness note, never an edit.
    pub(crate) fn set_freshness(&self, tenant: &str, rel: &str, f: crate::types::Freshness) {
        if let Ok(mut map) = self.inner.freshness.lock() {
            map.insert((tenant.to_string(), rel.to_string()), f);
        }
    }

    /// The frontier hop budget for the edit path — the same `policy.max_hops`
    /// the watch path's `OverlayRefresh` is built with.
    #[must_use]
    pub(crate) fn frontier_hops(&self) -> u32 {
        self.inner.config.policy.max_hops
    }

    /// The FR-39 board layer, for the `/ingest`, `/guard` and `/whatif`
    /// endpoints. Behind an `RwLock` because ingestion mutates while guard and
    /// what-if only read — and because what-if speculates on a CLONE of the
    /// overlay, a long speculation never holds a write lock.
    #[cfg(feature = "game-state")]
    #[must_use]
    pub fn board(&self) -> &RwLock<crate::state::StateRegistry> {
        &self.inner.board
    }

    /// The resident graph. Query endpoints (stage 2) borrow this; nothing mutates
    /// it — a rebuild replaces the whole engine, it does not patch in place.
    #[must_use]
    pub fn graph(&self) -> &CodeGraph {
        &self.inner.graph
    }

    /// The analysis root the resident graph was built from. Used to confine the
    /// `/measure` endpoint to files under this root, and to check a client is
    /// talking to a daemon serving the repo it means to measure.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// The resident policy, from the config snapshot taken at build time. The
    /// aegis-hac0 signed cache will verify the source of this at load; callers
    /// read it from here rather than re-reading config from disk per request.
    #[must_use]
    pub fn policy(&self) -> &PolicyConfig {
        &self.inner.config.policy
    }

    /// Direct callers or callees of `symbol`, from the RESIDENT graph — no
    /// per-call rebuild, which is the daemon's whole point. This is the shared
    /// query layer: the HTTP surface (stage 2) calls it now, and the hook/MCP thin
    /// clients (stage 3) will call the same method instead of building transiently.
    #[must_use]
    pub fn neighbors(&self, symbol: &str, dir: Dir) -> Neighbors {
        let graph = self.graph();
        Neighbors {
            symbol: symbol.to_string(),
            found: graph.has_symbol(symbol),
            neighbors: graph.direct(symbol, dir).iter().map(reached_item).collect(),
            tier: graph_tier(),
        }
    }

    /// Blast radius: symbols transitively affected by changing `symbol`, up to
    /// `hops`. Resident-graph, no rebuild.
    #[must_use]
    pub fn impact(&self, symbol: &str, hops: u32) -> Impact {
        let graph = self.graph();
        let reachable = graph.reachable(symbol, Dir::Callers, hops);
        let files: std::collections::BTreeSet<String> =
            reachable.iter().map(|r| r.file.clone()).collect();
        Impact {
            symbol: symbol.to_string(),
            found: graph.has_symbol(symbol),
            hops,
            count: reachable.len(),
            reachable: reachable.iter().map(reached_item).collect(),
            files: files.into_iter().collect(),
            tier: graph_tier(),
        }
    }

    /// Definition sites of `symbol`, from the resident node index — the answer
    /// `yupana_references` walks every file to compute, with no re-extraction.
    #[must_use]
    pub fn references(&self, symbol: &str) -> Definitions {
        let defs = self.graph().definitions(symbol);
        Definitions {
            symbol: symbol.to_string(),
            found: !defs.is_empty(),
            count: defs.len(),
            definitions: defs.into_iter().map(def_item).collect(),
            tier: graph_tier(),
        }
    }

    /// The symbols `rel` contributes to the resident graph, in line order. See
    /// [`FileSymbols`] for the `known` semantics (no-symbols vs no-such-file are
    /// one state here) and the snapshot-freshness caveat.
    #[must_use]
    pub fn symbols(&self, rel: &str) -> FileSymbols {
        let symbols = self.graph().file_symbols(rel);
        FileSymbols {
            file: rel.to_string(),
            known: !symbols.is_empty(),
            count: symbols.len(),
            symbols: symbols
                .into_iter()
                .map(|n| FileSymbolItem {
                    name: n.name.clone(),
                    kind: n.kind.clone(),
                    start_line: n.start_line,
                })
                .collect(),
            tier: graph_tier(),
            // The untenanted path has no tenant to key the freshness map by,
            // so nothing is known here. Omitted rather than guessed.
            freshness: None,
        }
    }

    /// Size an edit against the RESIDENT graph — the exact question the pre-edit
    /// guard asks, answered without the per-invocation `CodeGraph::build`. The
    /// edited file is still read fresh (its content is what changed), so the answer
    /// matches the transient path on the same tree; only the graph build is saved.
    /// This is what the hook becomes a thin client of in the cutover (stage 3b).
    #[must_use]
    pub fn measure_edit(
        &self,
        file: &Path,
        rel: &str,
        anchors: &[String],
        max_hops: u32,
    ) -> Sizing {
        crate::hook::measure_with_graph(self.graph(), file, rel, anchors, max_hops)
    }

    /// A machine-readable liveness/status snapshot — real facts about what is
    /// resident, so a probe distinguishes "up and holding a graph" from "up but
    /// empty" as well as from "not reachable at all" (the last is the client's
    /// job, in [`client`]).
    #[must_use]
    pub fn status(&self) -> EngineStatus {
        let uptime_secs = self.inner.built_at.elapsed().map_or(0, |d| d.as_secs());
        EngineStatus {
            status: "ok",
            root: self.inner.root.display().to_string(),
            nodes: self.inner.nodes,
            edges: self.inner.edges,
            uptime_secs,
            tier: crate::types::Tier::served(),
            tenant_layer: self
                .inner
                .registry
                .as_ref()
                .and_then(|lock| lock.read().ok())
                .map(|reg| reg.status()),
            board_layer: self.board_status(),
        }
    }

    /// The board layer for [`EngineStatus`]. `None` on a build without the
    /// `game-state` engine — see the field's docs for why that must not read the
    /// same as an engine holding no games.
    #[cfg(feature = "game-state")]
    fn board_status(&self) -> Option<wire::BoardLayerStatus> {
        let status = self.inner.board.read().ok()?.status();
        Some(wire::BoardLayerStatus {
            fog_leaks_blocked: status.fog_leaks_blocked,
            games: status
                .games
                .into_iter()
                .map(|g| wire::BoardGameStatus {
                    game_id: g.game_id,
                    shared_nodes: g.shared_nodes,
                    shared_edges: g.shared_edges,
                    factions: g
                        .factions
                        .into_iter()
                        .map(|f| (f.faction_id, f.overlay_nodes, f.overlay_edges))
                        .collect(),
                })
                .collect(),
        })
    }

    #[cfg(not(feature = "game-state"))]
    #[allow(clippy::unused_self)]
    fn board_status(&self) -> Option<wire::BoardLayerStatus> {
        None
    }
}

/// Build the resident engine and serve its liveness surface on `bind`.
///
/// Serves `/health`, `/status` (stage 1) and the graph-backed query endpoints
/// `/callers`, `/callees`, `/impact` (stage 2). Runs until the process is signalled.
#[cfg(feature = "mcp")]
pub async fn serve(
    root: &Path,
    config_override: Option<&Path>,
    bind: &str,
    port: u16,
) -> anyhow::Result<()> {
    let engine = ResidentEngine::build(root, config_override)?;
    let status = engine.status();
    eprintln!(
        "yupana daemon: resident graph built — {} nodes, {} edges from {}",
        status.nodes, status.edges, status.root
    );
    #[cfg(feature = "quipu")]
    let _refresher = projection::spawn_for(&engine);
    #[cfg(feature = "quipu")]
    let _keywords = keywords::spawn_for(&engine);
    http::serve(engine, bind, port).await
}

#[cfg(test)]
#[allow(non_snake_case)]
#[path = "daemon_test.rs"]
mod tests;
