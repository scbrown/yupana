//! The daemon's HTTP surface (aegis-1qze).
//!
//! - stage 1: `/health` (a bare 200 for the [`super::client`] probe) and
//!   `/status` (the resident graph's real counts).
//! - stage 2: the graph-backed query endpoints — `/callers`, `/callees`,
//!   `/impact` — answered from the RESIDENT graph with no per-call rebuild, which
//!   is the daemon's whole point. They mirror the `yupana_callers`/`callees`/`impact`
//!   MCP tools, so stage 3 can point the MCP surface at the same engine methods.
//! - stage 3a: `/measure` (POST) — the pre-edit guard's exact blast-radius
//!   question, sized against the resident graph. This is what the hook becomes a
//!   thin client of; the client + hook cutover (with loud-when-absent) is stage 3b.
//! - stage 4: the rest of the FR-27 query surface — `/references` and `/symbols`
//!   from the resident node index, and `/dataflow`, which is NOT resident (no
//!   resident dataflow model exists yet, yupana #22) but is served per-request so
//!   the HTTP API is complete rather than silently partial.

use std::path::PathBuf;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use super::wire::{graph_tier, DataflowReply, DepEdgeItem, FlowStepItem};
use super::ResidentEngine;
use super::{Definitions, EngineStatus, FileSymbols, Impact, MeasureReply, Neighbors};
use crate::dataflow::{Dataflow, FlowDir};
use crate::graph::Dir;

/// Build the daemon router over a resident engine.
///
/// The FR-35/37/38 board routes are merged in from [`super::state_http`] when
/// the `game-state` engine is compiled in — and are ABSENT otherwise, so a build
/// without the engine 404s on `/guard` rather than mounting a route that cannot
/// answer. The FR-41/FR-42 golden-path route is merged from
/// [`super::path_http`] under the same rule for `golden-path`.
pub fn router(engine: ResidentEngine) -> Router {
    #[allow(unused_mut)]
    let mut router = Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/callers", get(callers))
        .route("/callees", get(callees))
        .route("/impact", get(impact))
        .route("/references", get(references))
        .route("/symbols", get(symbols))
        .route("/dataflow", get(dataflow))
        .route("/measure", post(measure))
        .route("/edit", post(edit));
    #[cfg(feature = "game-state")]
    {
        router = router.merge(super::state_http::routes());
    }
    #[cfg(feature = "golden-path")]
    {
        router = router.merge(super::path_http::routes());
    }
    #[cfg(feature = "quipu")]
    {
        router = router
            .route("/projection", get(projection))
            .route("/exposure", get(exposure));
    }
    router.with_state(engine)
}

/// Liveness: a healthy daemon answers 200. The [`super::client::probe`] keys on
/// this and nothing else, so it stays a bare, dependency-free 200.
async fn health() -> &'static str {
    "ok"
}

/// The resident projected policy (aegis-x894x2) — the endpoint that makes a
/// hook's projection cost a localhost read of resident memory instead of a
/// contended quipu `/query`.
///
/// Answers from memory: no network call happens on this path, which is the
/// whole point. The reply carries the catalogue's AGE and never a servability
/// verdict — the caller applies its own `projection_cache_ttl_secs`, so the
/// deliberate past-TTL refusal (`a_cache_past_the_ttl_fails_open_and_the_reason
/// _names_both_halves`) keeps being made in exactly one place.
///
/// **503, never an empty catalogue**, when this daemon has no projection at all:
/// a `200` with zero rules is indistinguishable from "no rules apply", and those
/// need opposite responses. The client turns this into a fall-back-to-live, not
/// into a clean allow.
#[cfg(feature = "quipu")]
async fn projection(
    State(engine): State<ResidentEngine>,
) -> Result<Json<super::projection::ProjectionReply>, StatusCode> {
    match engine.projection() {
        Some(resident) => Ok(Json(resident.snapshot(crate::projection_cache::now_secs()))),
        None => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

/// One repo's exposure, from the resident cache when it can be (aegis-q4tt56).
///
/// The endpoint that removes the CONSTANT half of the guard's quipu load:
/// `POST /policy/check` was measured at 2.4-7.2s and ran once per governed edit,
/// uncached, from every agent.
///
/// A miss still resolves live here — one process doing that is the point, rather
/// than every hook doing it. 503 when this daemon has no projection, because the
/// verdict is bound to a rule set and without one there is nothing to bind to;
/// the client turns that into a fall-back-to-live, never a clean answer.
#[cfg(feature = "quipu")]
async fn exposure(
    State(engine): State<ResidentEngine>,
    Query(q): Query<ExposureQuery>,
) -> Result<Json<super::exposure::ExposureReply>, StatusCode> {
    let Some(resident) = engine.projection() else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    let ttl = engine.config().quipu.projection_cache_ttl_secs;
    Ok(Json(resident.exposure(
        &q.repo,
        ttl,
        crate::projection_cache::now_secs(),
    )))
}

/// Query for [`exposure`].
#[cfg(feature = "quipu")]
#[derive(Deserialize)]
struct ExposureQuery {
    /// The repo LABEL (e.g. `yupana`), as `git::origin_repo_name` resolves it —
    /// not an IRI. The daemon builds the entity IRI, so both paths build it the
    /// same way and cannot disagree about which entity was asked about.
    repo: String,
}

/// The resident graph's real facts — what an operator (or `yupana daemon status`,
/// later) reads to confirm the daemon is holding a non-empty graph.
async fn status(State(engine): State<ResidentEngine>) -> Json<EngineStatus> {
    Json(engine.status())
}

/// `?symbol=NAME[&tenant=T]` for the neighbor/reference endpoints. With
/// `tenant`, the answer resolves against that tenant's `base + overlay` view;
/// without it, against the un-tenanted working-tree snapshot (unchanged).
#[derive(Debug, Deserialize)]
struct SymbolQuery {
    symbol: String,
    tenant: Option<String>,
}

/// `?symbol=NAME&hops=N[&tenant=T]`; `hops` defaults to 5, matching the CLI.
#[derive(Debug, Deserialize)]
struct ImpactQuery {
    symbol: String,
    hops: Option<u32>,
    tenant: Option<String>,
}

/// A tenant was named but the tenant layer is absent (the root is not a git
/// repo, so no commit anchors a shared base). An explicit refusal — never an
/// empty answer wearing a normal one.
const NO_TENANT_LAYER: StatusCode = StatusCode::PRECONDITION_FAILED;

/// Direct callers of a symbol.
async fn callers(
    State(engine): State<ResidentEngine>,
    Query(q): Query<SymbolQuery>,
) -> Result<Json<Neighbors>, StatusCode> {
    match &q.tenant {
        Some(t) => engine
            .neighbors_for(t, &q.symbol, Dir::Callers)
            .map(Json)
            .ok_or(NO_TENANT_LAYER),
        None => Ok(Json(engine.neighbors(&q.symbol, Dir::Callers))),
    }
}

/// Direct callees of a symbol.
async fn callees(
    State(engine): State<ResidentEngine>,
    Query(q): Query<SymbolQuery>,
) -> Result<Json<Neighbors>, StatusCode> {
    match &q.tenant {
        Some(t) => engine
            .neighbors_for(t, &q.symbol, Dir::Callees)
            .map(Json)
            .ok_or(NO_TENANT_LAYER),
        None => Ok(Json(engine.neighbors(&q.symbol, Dir::Callees))),
    }
}

/// Blast radius of changing a symbol.
async fn impact(
    State(engine): State<ResidentEngine>,
    Query(q): Query<ImpactQuery>,
) -> Result<Json<Impact>, StatusCode> {
    let hops = q.hops.unwrap_or(5);
    match &q.tenant {
        Some(t) => engine
            .impact_for(t, &q.symbol, hops)
            .map(Json)
            .ok_or(NO_TENANT_LAYER),
        None => Ok(Json(engine.impact(&q.symbol, hops))),
    }
}

/// Definition sites of a symbol by name.
async fn references(
    State(engine): State<ResidentEngine>,
    Query(q): Query<SymbolQuery>,
) -> Result<Json<Definitions>, StatusCode> {
    match &q.tenant {
        Some(t) => engine
            .references_for(t, &q.symbol)
            .map(Json)
            .ok_or(NO_TENANT_LAYER),
        None => Ok(Json(engine.references(&q.symbol))),
    }
}

/// `?file=REL[&tenant=T]` for `/symbols`.
#[derive(Debug, Deserialize)]
struct FileQuery {
    file: String,
    tenant: Option<String>,
}

/// The symbols one file contributes, at the relevant snapshot. See
/// [`FileSymbols`] for the `known` semantics.
async fn symbols(
    State(engine): State<ResidentEngine>,
    Query(q): Query<FileQuery>,
) -> Result<Json<FileSymbols>, StatusCode> {
    match &q.tenant {
        Some(t) => engine
            .symbols_for(t, &q.file)
            .map(Json)
            .ok_or(NO_TENANT_LAYER),
        None => Ok(Json(engine.symbols(&q.file))),
    }
}

/// `POST /edit` body — the FR-30 feed: record the edit in the tenant's
/// overlay, answer the advisory from the fresh view. `content` is optional;
/// when omitted the daemon reads `rel` from disk (the post-edit hook's case —
/// the file was just saved), CONFINED TO THE ROOT like `/measure`.
#[derive(Debug, Deserialize)]
struct EditRequest {
    tenant: String,
    /// The edited file, relative to the root.
    rel: String,
    /// The file's new content; omit to read it from disk.
    #[serde(default)]
    content: Option<String>,
}

/// Feed a tenant's overlay and advise (FR-30). 412 when the tenant layer is
/// absent (not a repo); 400 for a path escaping the root.
async fn edit(
    State(engine): State<ResidentEngine>,
    Json(req): Json<EditRequest>,
) -> Result<Json<super::EditReply>, StatusCode> {
    let content = match req.content {
        Some(content) => content,
        None => {
            let root = engine.root();
            let joined = root.join(&req.rel);
            let (Ok(canon), Ok(canon_root)) = (joined.canonicalize(), root.canonicalize()) else {
                return Err(StatusCode::BAD_REQUEST);
            };
            if !canon.starts_with(&canon_root) {
                return Err(StatusCode::BAD_REQUEST);
            }
            std::fs::read_to_string(&canon).map_err(|_| StatusCode::BAD_REQUEST)?
        }
    };
    engine
        .edit(&req.tenant, &req.rel, &content)
        .map(Json)
        .ok_or(NO_TENANT_LAYER)
}

/// Query for `/dataflow`, mirroring the `yupana_dataflow` request.
#[derive(Debug, Deserialize)]
struct DataflowQuery {
    /// The function to analyze.
    function: String,
    /// Subtree to build over, relative to the root; omit for the whole root.
    path: Option<String>,
    /// Variable to trace; omit to return all dependence edges.
    var: Option<String>,
    /// Trace what the variable flows into (default: what it depends on).
    forward: Option<bool>,
    /// Maximum hops to follow (default 5).
    hops: Option<u32>,
}

/// Intra-procedural data dependence, mirroring `yupana_dataflow`. NOT resident —
/// built per request (see the module doc) — and CONFINED TO THE ROOT like
/// `/measure`: a `path` resolving outside the resident root is refused (400),
/// so the localhost daemon cannot be pointed at arbitrary trees.
async fn dataflow(
    State(engine): State<ResidentEngine>,
    Query(q): Query<DataflowQuery>,
) -> Result<Json<DataflowReply>, StatusCode> {
    let root = engine.root();
    let base = match &q.path {
        Some(p) => {
            let joined = root.join(p);
            // Canonicalize BOTH sides so `..` cannot escape the root.
            let (Ok(canon), Ok(canon_root)) = (joined.canonicalize(), root.canonicalize()) else {
                return Err(StatusCode::BAD_REQUEST);
            };
            if !canon.starts_with(&canon_root) {
                return Err(StatusCode::BAD_REQUEST);
            }
            canon
        }
        None => root.to_path_buf(),
    };
    let flow = Dataflow::build(&base).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let found = flow.has_function(&q.function);
    let (direction, steps, edges) = match &q.var {
        Some(var) => {
            let dir = if q.forward.unwrap_or(false) {
                FlowDir::FlowsInto
            } else {
                FlowDir::DependsOn
            };
            let steps = flow
                .flow(&q.function, var, dir, q.hops.unwrap_or(5))
                .into_iter()
                .map(|s| FlowStepItem {
                    name: s.name,
                    distance: s.distance,
                })
                .collect();
            (Some(dir.as_str().to_string()), steps, Vec::new())
        }
        None => {
            let edges = flow
                .edges(&q.function)
                .iter()
                .map(|e| DepEdgeItem {
                    dependent: e.dependent.clone(),
                    depends_on: e.depends_on.clone(),
                    line: e.line,
                })
                .collect();
            (None, Vec::new(), edges)
        }
    };
    Ok(Json(DataflowReply {
        function: q.function,
        found,
        direction,
        var: q.var,
        flow: steps,
        edges,
        tier: graph_tier(),
    }))
}

/// The pre-edit guard's exact question, as a POST body: size editing `file` (whose
/// anchors are the replaced texts) against the resident graph. This is what the
/// hook thin client calls in the cutover instead of building the graph itself.
#[derive(Debug, Deserialize)]
struct MeasureRequest {
    /// The edited file, absolute or root-relative.
    file: String,
    /// The edited file's path relative to the root (excluded from its own radius).
    rel: String,
    /// The replaced texts the edit lands inside; empty = whole-file.
    #[serde(default)]
    anchors: Vec<String>,
    /// Hops to follow; defaults to the resident policy's `max_hops`.
    #[serde(default)]
    max_hops: Option<u32>,
}

/// Size an edit against the resident graph. CONFINED TO THE ROOT: the request names
/// a file to read, and a localhost daemon must not become an arbitrary-file reader,
/// so a path resolving outside the resident root is refused (400) rather than read.
async fn measure(
    State(engine): State<ResidentEngine>,
    Json(req): Json<MeasureRequest>,
) -> Result<Json<MeasureReply>, StatusCode> {
    let root = engine.root();
    let raw = PathBuf::from(&req.file);
    let file = if raw.is_absolute() {
        raw
    } else {
        root.join(raw)
    };
    // Canonicalize BOTH sides so `..` cannot escape the root; a file that does not
    // exist cannot be canonicalized, but then there is nothing to read either.
    let (Ok(canon_file), Ok(canon_root)) = (file.canonicalize(), root.canonicalize()) else {
        return Err(StatusCode::BAD_REQUEST);
    };
    if !canon_file.starts_with(&canon_root) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let hops = req.max_hops.unwrap_or(engine.policy().max_hops);
    let sizing = engine.measure_edit(&canon_file, &req.rel, &req.anchors, hops);
    Ok(Json(MeasureReply::from_sizing(&sizing)))
}

/// Bind `host:port` and serve the router until SIGINT/SIGTERM, then drain
/// in-flight requests and exit 0 — a supervisor's `stop` is a clean stop, not
/// a kill. (The thin clients' loud-when-absent contract covers the window
/// after: a stopped daemon is announced by its callers, not by this process.)
pub async fn serve(engine: ResidentEngine, host: &str, port: u16) -> anyhow::Result<()> {
    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("yupana daemon: liveness surface on http://{addr}/health");
    axum::serve(listener, router(engine))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    eprintln!("yupana daemon: shut down cleanly");
    Ok(())
}

/// Resolves on SIGINT (Ctrl-C) or, on unix, SIGTERM — the signals a terminal
/// or a supervisor actually sends.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    eprintln!("yupana daemon: shutdown signal received — draining");
}

#[cfg(test)]
#[path = "http_test.rs"]
mod http_test;
