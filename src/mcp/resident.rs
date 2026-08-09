//! Stage 3c (aegis-1qze): the MCP graph tools as thin clients of the resident
//! daemon.
//!
//! When a daemon is EXPECTED (`serve.use_daemon`), reachable, and serving THIS
//! root, `yupana_callers` / `yupana_callees` / `yupana_impact` are answered from the
//! RESIDENT graph — no per-call `CodeGraph::build`, which is the daemon's whole
//! point. In every other case these functions return `None` and the tool falls
//! back to the transient build it has always done.
//!
//! Unlike the hook cutover (stage 3b), fallback here is SILENT to the model:
//! MCP is a query surface, not the guard. A transient answer is equally
//! correct, just slower, so there is no enforcement gap to be loud about —
//! loud-absence is the guard's contract. Absence still goes to stderr for the
//! operator.
//!
//! Two refusals that keep the answers honest:
//! - A `path`-scoped request is NEVER routed here (the caller checks): the
//!   resident graph covers the whole root, and answering a subtree query from
//!   it would silently substitute a different, larger graph.
//! - A daemon serving a DIFFERENT root is not used: `/status` names the root it
//!   was built from, and it must match ours (canonicalized) — a daemon for repo
//!   B answering repo A would not error, it would confidently lie.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use super::tools::{
    ImpactResponse, NeighborsResponse, ReachItem, ReconciliationItem, RefItem, ReferencesResponse,
};
use crate::config::YupanaConfig;
use crate::daemon::client::{
    expected_same_root_daemon, fetch_impact, fetch_neighbors, fetch_references,
};
use crate::daemon::ReachedItem;
use crate::graph::Dir;
use crate::reconcile::reconcile;

/// Budget per localhost round-trip. Generous against a resident graph
/// (microseconds), and small enough that a wedged daemon costs one slow query,
/// not a hang — the transient fallback then answers.
const DAEMON_TIMEOUT: Duration = Duration::from_millis(500);

/// The daemon address to query, IF one is expected, reachable, and serving this
/// root. `None` in every other case — including config that fails to resolve
/// (the transient path will surface that error itself, once, rather than this
/// probe pre-empting it).
fn usable_daemon(config_override: Option<&Path>, root: &Path) -> Option<(String, u16)> {
    let config = YupanaConfig::resolve(config_override, root).ok()?;
    // The expected/same-root logic is the shared client helper's (stage 5) —
    // one implementation of "would this daemon confidently lie to us".
    match expected_same_root_daemon(&config, root, DAEMON_TIMEOUT)? {
        Ok(addr) => Some(addr),
        Err(reason) => {
            eprintln!("yupana mcp: daemon expected but unusable, transient fallback: {reason}");
            None
        }
    }
}

/// Convert a daemon reach item to the MCP wire DTO. The daemon tags provenance
/// once at the top of its reply; each item inherits that tier (FR-3 — no item
/// leaves here unlabelled).
fn reach_item(r: &ReachedItem, tier: &str) -> ReachItem {
    ReachItem {
        name: r.name.clone(),
        file: r.file.clone(),
        start_line: r.start_line,
        distance: r.distance,
        via: r.via.clone(),
        tier: tier.to_string(),
    }
}

/// `yupana_callers` / `yupana_callees` from the resident daemon, or `None` to fall
/// back to the transient build.
pub(super) fn neighbors(
    config_override: Option<&Path>,
    root: &Path,
    symbol: &str,
    dir: Dir,
) -> Option<NeighborsResponse> {
    let (host, port) = usable_daemon(config_override, root)?;
    match fetch_neighbors(&host, port, symbol, dir, DAEMON_TIMEOUT) {
        Ok(reply) => Some(NeighborsResponse {
            symbol: reply.symbol.clone(),
            found: reply.found,
            count: reply.neighbors.len(),
            neighbors: reply
                .neighbors
                .iter()
                .map(|r| reach_item(r, &reply.tier))
                .collect(),
            tier: reply.tier,
        }),
        Err(reason) => {
            eprintln!("yupana mcp: daemon neighbors query failed, transient fallback: {reason}");
            None
        }
    }
}

/// `yupana_impact` from the resident daemon, or `None` to fall back. The
/// co-change reconciliation (FR-11) is computed HERE, over the daemon's file
/// set — the daemon serves structure; reconciliation stays a client concern so
/// both sources produce it identically.
pub(super) fn impact(
    config_override: Option<&Path>,
    root: &Path,
    symbol: &str,
    hops: u32,
    cochange: Option<&[String]>,
) -> Option<ImpactResponse> {
    let (host, port) = usable_daemon(config_override, root)?;
    let reply = match fetch_impact(&host, port, symbol, hops, DAEMON_TIMEOUT) {
        Ok(reply) => reply,
        Err(reason) => {
            eprintln!("yupana mcp: daemon impact query failed, transient fallback: {reason}");
            return None;
        }
    };
    let structural: BTreeSet<String> = reply.files.iter().cloned().collect();
    let reconciliation = cochange.map(|cochange| {
        let cochange_set: BTreeSet<String> = cochange.iter().cloned().collect();
        let recon = reconcile(&structural, &cochange_set);
        ReconciliationItem {
            corroborated: recon.corroborated,
            structural_only: recon.structural_only,
            cochange_only: recon.cochange_only,
        }
    });
    Some(ImpactResponse {
        symbol: reply.symbol.clone(),
        found: reply.found,
        hops: reply.hops,
        count: reply.count,
        reachable: reply
            .reachable
            .iter()
            .map(|r| reach_item(r, &reply.tier))
            .collect(),
        structural_files: reply.files,
        reconciliation,
        tier: reply.tier,
    })
}

/// `yupana_references` from the resident node index, or `None` to fall back to
/// the transient every-file walk — the biggest per-call saving of the three
/// cutovers, since the transient path re-extracts the whole subtree per query.
pub(super) fn references(
    config_override: Option<&Path>,
    root: &Path,
    symbol: &str,
) -> Option<ReferencesResponse> {
    let (host, port) = usable_daemon(config_override, root)?;
    match fetch_references(&host, port, symbol, DAEMON_TIMEOUT) {
        Ok(reply) => Some(ReferencesResponse {
            symbol: reply.symbol,
            count: reply.count,
            definitions: reply
                .definitions
                .iter()
                .map(|d| RefItem {
                    file: d.file.clone(),
                    name: symbol.to_string(),
                    kind: d.kind.clone(),
                    start_line: d.start_line,
                    tier: reply.tier.clone(),
                })
                .collect(),
            // The daemon reply has no graph size to pass through; see
            // `ReferencesResponse::searched_symbols` for why that is omitted
            // rather than zeroed.
            searched_symbols: None,
            tier: reply.tier.clone(),
        }),
        Err(reason) => {
            eprintln!("yupana mcp: daemon references query failed, transient fallback: {reason}");
            None
        }
    }
}

/// The daemon's tenant layer (base commit + active overlays) for
/// `yupana_status`, when a usable daemon holds one. `None` collapses "no daemon
/// expected", "daemon unusable", and "layer absent (not a repo)" — the
/// daemon's own `/status` distinguishes them; here absence just means the
/// status has no tenant facts to add.
pub(super) fn tenant_layer(
    config_override: Option<&Path>,
    root: &Path,
) -> Option<serde_json::Value> {
    let (host, port) = usable_daemon(config_override, root)?;
    let body = crate::daemon::client::fetch_status_json(&host, port, DAEMON_TIMEOUT).ok()?;
    let layer = body.get("tenant_layer")?.clone();
    (!layer.is_null()).then_some(layer)
}

#[cfg(test)]
// Test names here shout the invariant they pin — `is_NEVER_observable`,
// `daemon_EXPECTED_but_DOWN`, `is_DOWN_not_UP`. That capitalisation is the same
// emphasis the prose and comments use throughout this repo, and it is load-
// bearing in a test name: it says which word the assertion turns on. Allowed
// explicitly, and scoped to tests, so the lint stays live everywhere else
// rather than being switched off crate-wide (yupana #83).
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::daemon::{http, ResidentEngine};

    // leaf <- caller: a non-empty graph with a real edge.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("caller.rs"), "fn caller() { leaf(); }\n").unwrap();
        dir
    }

    // A config file EXPECTING a daemon at 127.0.0.1:port. Written outside the
    // repo layering (passed as an override) so a developer's user config can
    // never leak into these tests.
    fn daemon_config(dir: &Path, port: u16) -> std::path::PathBuf {
        let path = dir.join("yupana-test-config.toml");
        std::fs::write(
            &path,
            format!(
                "[yupana.serve]\nuse_daemon = true\nbind_address = \"127.0.0.1\"\n\
                 mcp_http_port = {port}\n"
            ),
        )
        .unwrap();
        path
    }

    // Serve `engine` on an ephemeral port; return the port.
    async fn spawn_daemon(engine: ResidentEngine) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, http::router(engine)).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        port
    }

    #[test]
    fn no_daemon_expected_is_none_the_normal_silent_case() {
        // use_daemon defaults to false: no daemon is consulted, the tools build
        // transiently exactly as before the cutover.
        let dir = repo();
        assert!(neighbors(None, dir.path(), "leaf", Dir::Callers).is_none());
        assert!(impact(None, dir.path(), "leaf", 5, None).is_none());
    }

    #[test]
    fn daemon_expected_but_DOWN_is_none_so_the_tool_falls_back() {
        // Port 1 never listens. Down must be a fallback, never an empty answer.
        let dir = repo();
        let config = daemon_config(dir.path(), 1);
        assert!(neighbors(Some(&config), dir.path(), "leaf", Dir::Callers).is_none());
        assert!(impact(Some(&config), dir.path(), "leaf", 5, None).is_none());
    }

    #[tokio::test]
    async fn daemon_up_and_same_root_answers_from_the_RESIDENT_graph() {
        let dir = repo();
        let engine = ResidentEngine::build(dir.path(), None).unwrap();
        let port = spawn_daemon(engine).await;
        let config = daemon_config(dir.path(), port);

        // Grow the tree AFTER the engine was built: a transient build would see
        // `late`, the resident graph cannot. Its absence below proves the
        // answer came from the daemon, not a fresh build.
        std::fs::write(dir.path().join("late.rs"), "fn late() { leaf(); }\n").unwrap();

        let root = dir.path().to_path_buf();
        let response = tokio::task::spawn_blocking(move || {
            neighbors(
                Some(&daemon_config(&root, port)),
                &root,
                "leaf",
                Dir::Callers,
            )
        })
        .await
        .unwrap()
        .expect("an up, same-root daemon must answer");
        assert!(response.found);
        let names: Vec<&str> = response.neighbors.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"caller"), "got {names:?}");
        assert!(
            !names.contains(&"late"),
            "`late` postdates the resident graph — its presence would mean a \
             transient build answered, not the daemon: {names:?}"
        );
        assert_eq!(response.tier, "treesitter");

        // Impact from the daemon, with client-side reconciliation (FR-11).
        let root = dir.path().to_path_buf();
        let cfg = config.clone();
        let imp = tokio::task::spawn_blocking(move || {
            impact(
                Some(&cfg),
                &root,
                "leaf",
                5,
                Some(&["caller.rs".to_string(), "ghost.rs".to_string()]),
            )
        })
        .await
        .unwrap()
        .expect("impact must come from the daemon too");
        assert!(imp.found);
        assert!(imp.structural_files.contains(&"caller.rs".to_string()));
        let recon = imp
            .reconciliation
            .expect("cochange given -> reconciliation");
        assert_eq!(recon.corroborated, vec!["caller.rs".to_string()]);
        assert_eq!(recon.cochange_only, vec!["ghost.rs".to_string()]);

        // References from the daemon too (stage 4): `late` postdates the
        // resident graph, so a RESIDENT answer has zero sites — a transient
        // walk would find late.rs. Zero here proves who answered.
        let root = dir.path().to_path_buf();
        let cfg = config.clone();
        let refs = tokio::task::spawn_blocking(move || references(Some(&cfg), &root, "late"))
            .await
            .unwrap()
            .expect("references must come from the daemon");
        assert_eq!(refs.count, 0, "resident graph predates late.rs");

        // And a symbol the resident graph does hold arrives tier-tagged per item.
        let root = dir.path().to_path_buf();
        let cfg = config.clone();
        let refs = tokio::task::spawn_blocking(move || references(Some(&cfg), &root, "leaf"))
            .await
            .unwrap()
            .expect("references must come from the daemon");
        assert_eq!(refs.count, 1);
        assert_eq!(refs.definitions[0].name, "leaf");
        assert_eq!(refs.definitions[0].tier, "treesitter");
    }

    #[tokio::test]
    async fn a_daemon_serving_a_DIFFERENT_root_is_refused_not_believed() {
        // Repo A asks; the daemon holds repo B. The root check must refuse —
        // falling back to a transient build of A — rather than serve B's graph
        // as if it were A's.
        let repo_a = repo();
        let repo_b = repo();
        let engine_b = ResidentEngine::build(repo_b.path(), None).unwrap();
        let port = spawn_daemon(engine_b).await;

        let root_a = repo_a.path().to_path_buf();
        let response = tokio::task::spawn_blocking(move || {
            neighbors(
                Some(&daemon_config(&root_a, port)),
                &root_a,
                "leaf",
                Dir::Callers,
            )
        })
        .await
        .unwrap();
        assert!(
            response.is_none(),
            "a wrong-root daemon must be refused so the tool falls back"
        );
    }
}
