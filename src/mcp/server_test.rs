//! FR-3 enforcement: every `yupana_*` MCP response carries its provenance tier
//! (aegis-8yrn). Child module of `server`, so it can drive the private tool
//! handlers directly; size-exempt (`_test.rs`).
//!
//! The bug this pins: `yupana_impact`, `yupana_callers`, `yupana_callees` and
//! `yupana_dataflow` served an unlabelled tree-sitter approximation — no `tier`
//! anywhere — which FR-3 exists to forbid, and which is worse on `yupana_impact`
//! precisely because it is the trust-boundary/capability-scoping surface. This
//! walk asserts the served WIRE JSON, so a future response type that omits the
//! tag fails here rather than shipping silent.

use super::YupanaMcpServer;
use crate::mcp::tools::{
    AnalyzeRequest, CommunitiesRequest, DataflowRequest, ImpactRequest, NeighborsRequest,
    ReferencesRequest, SymbolsRequest, VerifyRequest,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;

const KNOWN_TIERS: &[&str] = &["treesitter", "lsp", "cpg"];

/// A two-function project: `a` calls `b`, so `b` has a caller and the graph is
/// non-empty. Fresh temp dir per test.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.rs"), "fn a() { b(); }\nfn b() {}\n").unwrap();
    dir
}

fn server(dir: &tempfile::TempDir) -> YupanaMcpServer {
    YupanaMcpServer::new(dir.path().to_path_buf(), None, None)
}

/// The served JSON payload, parsed out of the MCP `CallToolResult` wire form
/// (`{ "content": [ { "type": "text", "text": "<json>" } ] }`). Asserting the
/// actual wire bytes is the point — a `tier` field that exists on the struct but
/// is dropped in serialization would still be caught.
fn served(result: Result<CallToolResult, rmcp::ErrorData>) -> serde_json::Value {
    let result = result.expect("handler returned Ok");
    let wire = serde_json::to_value(&result).expect("CallToolResult serializes");
    let text = wire
        .pointer("/content/0/text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("no text content in result: {wire}"));
    serde_json::from_str(text).expect("served payload is JSON")
}

/// Does any object in the tree carry a `tier` key? Covers both per-item tags
/// (symbols/references) and the top-level tag (graph responses).
fn carries_tier(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(m) => m.contains_key("tier") || m.values().any(carries_tier),
        serde_json::Value::Array(a) => a.iter().any(carries_tier),
        _ => false,
    }
}

/// A top-level `tier` that is one of the known tiers. Used for the graph
/// responses, where the top-level tag is what makes an EMPTY / not-found answer
/// still declare its provenance.
fn assert_top_level_tier(payload: &serde_json::Value, tool: &str) {
    let tier = payload
        .get("tier")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("{tool}: no top-level `tier` in {payload}"));
    assert!(
        KNOWN_TIERS.contains(&tier),
        "{tool}: tier {tier:?} is not one of {KNOWN_TIERS:?}"
    );
}

#[tokio::test]
async fn impact_carries_a_top_level_tier() {
    let dir = fixture();
    let payload = served(
        server(&dir)
            .yupana_impact(Parameters(ImpactRequest {
                symbol: "b".into(),
                path: None,
                hops: None,
                cochange: None,
            }))
            .await,
    );
    // The bug was that this — the trust-boundary surface — served no tier at all.
    assert_top_level_tier(&payload, "yupana_impact");
    // And the per-item reach facts carry it too.
    let first = &payload["reachable"][0];
    assert_eq!(
        first["tier"], "treesitter",
        "reach item missing tier: {first}"
    );
}

#[tokio::test]
async fn impact_on_a_missing_symbol_still_declares_its_tier() {
    // The empty-case hole: a not-found answer has no items to tag, so without the
    // top-level tag it would arrive unlabelled and read as authoritative.
    let dir = fixture();
    let payload = served(
        server(&dir)
            .yupana_impact(Parameters(ImpactRequest {
                symbol: "does_not_exist".into(),
                path: None,
                hops: None,
                cochange: None,
            }))
            .await,
    );
    assert_eq!(payload["found"], false);
    assert_top_level_tier(&payload, "yupana_impact(not-found)");
}

#[tokio::test]
async fn callers_and_callees_carry_a_top_level_tier() {
    let dir = fixture();
    let callers = served(
        server(&dir)
            .yupana_callers(Parameters(NeighborsRequest {
                symbol: "b".into(),
                path: None,
            }))
            .await,
    );
    assert_top_level_tier(&callers, "yupana_callers");

    let callees = served(
        server(&dir)
            .yupana_callees(Parameters(NeighborsRequest {
                symbol: "a".into(),
                path: None,
            }))
            .await,
    );
    assert_top_level_tier(&callees, "yupana_callees");
}

// --- Stage 3c wiring (aegis-1qze): resident daemon vs. transient fallback ----
//
// Multi-thread runtime on purpose: the tool handlers call the SYNC daemon
// client in-line, so the daemon must be able to accept on another worker while
// the handler's thread blocks on the socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unscoped_query_uses_the_daemon_and_a_path_scoped_one_never_does() {
    let dir = fixture(); // x.rs: a calls b
    let engine = crate::daemon::ResidentEngine::build(dir.path(), None).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, crate::daemon::http::router(engine)).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Config override EXPECTING that daemon (override path, so no user-config
    // layering can leak in).
    let config = dir.path().join("daemon-config.toml");
    std::fs::write(
        &config,
        format!(
            "[yupana.serve]\nuse_daemon = true\nbind_address = \"127.0.0.1\"\n\
             mcp_http_port = {port}\n"
        ),
    )
    .unwrap();
    let server = YupanaMcpServer::new(dir.path().to_path_buf(), None, Some(config));

    // Grow the tree AFTER the resident graph was built: a transient build sees
    // `late`, the daemon cannot. Which graph answered is therefore observable.
    std::fs::write(dir.path().join("late.rs"), "fn late() { b(); }\n").unwrap();

    // Unscoped -> the RESIDENT graph answers (no `late`).
    let resident = served(
        server
            .yupana_callers(Parameters(NeighborsRequest {
                symbol: "b".into(),
                path: None,
            }))
            .await,
    );
    let names: Vec<&str> = resident["neighbors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"a"), "got {names:?}");
    assert!(
        !names.contains(&"late"),
        "`late` postdates the resident graph; its presence means the transient \
         path answered an unscoped query despite a usable daemon: {names:?}"
    );
    assert_top_level_tier(&resident, "yupana_callers(resident)");

    // Path-scoped -> NEVER the daemon (whole-root graph ≠ subtree graph): the
    // transient build answers and sees `late`.
    let scoped = served(
        server
            .yupana_callers(Parameters(NeighborsRequest {
                symbol: "b".into(),
                path: Some(".".into()),
            }))
            .await,
    );
    let names: Vec<&str> = scoped["neighbors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"late"),
        "a path-scoped query must be answered transiently, not by the daemon: {names:?}"
    );
}

#[tokio::test]
async fn dataflow_carries_a_top_level_tier() {
    let dir = fixture();
    let payload = served(
        server(&dir)
            .yupana_dataflow(Parameters(DataflowRequest {
                function: "a".into(),
                path: None,
                var: None,
                forward: None,
                hops: None,
            }))
            .await,
    );
    assert_top_level_tier(&payload, "yupana_dataflow");
}

#[tokio::test]
async fn every_fact_serving_response_carries_a_tier() {
    // The walk: each fact-serving tool's WIRE response must carry a tier somewhere
    // (top-level for graph/summary responses, per-item for symbol lists). If a new
    // response type omits it, its line here fails rather than shipping unlabelled.
    let dir = fixture();
    let s = server(&dir);

    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "yupana_symbols",
            served(
                s.yupana_symbols(Parameters(SymbolsRequest {
                    file: "x.rs".into(),
                }))
                .await,
            ),
        ),
        (
            "yupana_references",
            served(
                s.yupana_references(Parameters(ReferencesRequest {
                    symbol: Some("a".into()),
                    path: None,
                    at_file: None,
                    at_line: None,
                }))
                .await,
            ),
        ),
        (
            "yupana_analyze",
            served(
                s.yupana_analyze(Parameters(AnalyzeRequest { path: None }))
                    .await,
            ),
        ),
        (
            "yupana_communities",
            served(
                s.yupana_communities(Parameters(CommunitiesRequest { path: None }))
                    .await,
            ),
        ),
        (
            "yupana_verify",
            served(
                s.yupana_verify(Parameters(VerifyRequest {
                    file: "x.rs".into(),
                    buffer: "fn a() { b(); }\nfn b() {}\n".into(),
                }))
                .await,
            ),
        ),
        (
            "yupana_callers",
            served(
                s.yupana_callers(Parameters(NeighborsRequest {
                    symbol: "b".into(),
                    path: None,
                }))
                .await,
            ),
        ),
        (
            "yupana_impact",
            served(
                s.yupana_impact(Parameters(ImpactRequest {
                    symbol: "b".into(),
                    path: None,
                    hops: None,
                    cochange: None,
                }))
                .await,
            ),
        ),
        (
            "yupana_dataflow",
            served(
                s.yupana_dataflow(Parameters(DataflowRequest {
                    function: "a".into(),
                    path: None,
                    var: None,
                    forward: None,
                    hops: None,
                }))
                .await,
            ),
        ),
    ];

    for (tool, payload) in &cases {
        assert!(
            carries_tier(payload),
            "{tool}: served response carries NO tier anywhere: {payload}"
        );
    }
}

#[tokio::test]
async fn status_advertises_only_implemented_tiers() {
    // yupana_status must claim a tier only when it is real. The extractor
    // assigns TreeSitter, so that is always advertised — never lsp/cpg, which have
    // no implementation and are no longer even Cargo features.
    //
    // `engine-state` (FR-35) is the one tier that varies, and it varies WITH ITS
    // ENGINE, not with a bare flag: `game-state` gates `crate::state`, which is
    // the ingestion path itself. Asserted in both directions so neither
    // "advertised without the engine" nor "engine built but not advertised" can
    // pass — the first is the empty-feature lie this test was written for, the
    // second sends a consumer looking for a tier the build really serves.
    let dir = fixture();
    let payload = served(server(&dir).yupana_status().await);
    let tiers = payload["tiers"].as_array().unwrap().clone();
    assert!(tiers.contains(&serde_json::json!("treesitter")));
    assert!(!tiers.contains(&serde_json::json!("lsp")));
    assert!(!tiers.contains(&serde_json::json!("cpg")));
    assert_eq!(
        tiers.contains(&serde_json::json!("engine-state")),
        cfg!(feature = "game-state"),
        "status advertised a tier out of step with its engine: {payload}"
    );
}

/// The MCP surface must report the parseable language set, in step with the
/// grammars compiled in (aegis-ah0q1).
///
/// This is the tier test's sibling and it exists for the mirror-image reason.
/// The tier lie was advertising a capability that was absent; this one is
/// STAYING SILENT about an absence — an agent that asks `yupana_impact` about a
/// Python symbol on a Rust-only build is told "no callers", which reads as
/// "safe to change" rather than "this build cannot see Python at all". Asserted
/// in both directions so a Rust-only build cannot claim the extra languages and
/// a complete one cannot hide them.
#[tokio::test]
async fn status_advertises_the_languages_it_can_parse() {
    let dir = fixture();
    let payload = served(server(&dir).yupana_status().await);
    let languages = payload["languages"]
        .as_array()
        .expect("status must report a language set")
        .clone();
    assert!(
        languages.contains(&serde_json::json!("rust")),
        "rust is unconditional: {payload}"
    );
    for language in ["typescript", "tsx", "python", "go", "java", "cpp"] {
        assert_eq!(
            languages.contains(&serde_json::json!(language)),
            cfg!(feature = "langs-extra"),
            "`{language}` advertisement is out of step with langs-extra: {payload}"
        );
    }
}

#[tokio::test]
async fn references_declares_its_tier_at_the_top_level() {
    // The empty-answer hole, on the references surface. `ReferencesResponse`
    // tagged each `RefItem` and nothing else, so the one reply with no items —
    // "this symbol has no definitions" — was served with no tier at all. That is
    // the answer most likely to be acted on ("it does not exist"), and it was the
    // only one FR-3 did not cover.
    let dir = fixture();
    let payload = served(
        server(&dir)
            .yupana_references(Parameters(ReferencesRequest {
                symbol: Some("definitely_not_here".into()),
                path: None,
                at_file: None,
                at_line: None,
            }))
            .await,
    );
    assert_eq!(payload["count"], 0, "fixture has no such symbol: {payload}");
    assert_top_level_tier(&payload, "yupana_references");
}

#[tokio::test]
#[cfg(feature = "langs-extra")] // needs the python grammar compiled in
async fn references_resolves_a_non_rust_definition_on_the_transient_path() {
    // yupana #76 on the MCP surface. The transient fallback walked `rust_files()`
    // and parsed every hit as "rust" — so this path answered "no definitions" for
    // every Python/Go/TypeScript symbol in the tree. A `path`-scoped request ALWAYS
    // lands here (it never consults the daemon, by design), so this was not merely
    // the no-daemon case: it was every scoped reference query.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("quipu.py"),
        "def derive_agents(cfg):\n    return cfg\n",
    )
    .unwrap();
    let s = YupanaMcpServer::new(dir.path().to_path_buf(), None, None);

    let payload = served(
        s.yupana_references(Parameters(ReferencesRequest {
            symbol: Some("derive_agents".into()),
            // Scoped on purpose: pins the TRANSIENT path, not the resident one.
            at_file: None,
            at_line: None,
            path: Some(".".into()),
        }))
        .await,
    );

    assert_eq!(payload["count"], 1, "python definition missed: {payload}");
    assert_eq!(payload["definitions"][0]["file"], "quipu.py");
    assert_eq!(payload["definitions"][0]["start_line"], 1);
    // The searched-set size is knowable on this path, so it is served — the
    // discriminator between "absent name" and "nothing was parseable".
    assert_eq!(payload["searched_symbols"], 1, "{payload}");
}

#[tokio::test]
async fn references_by_position_answers_with_the_one_symbol_pointed_at() {
    // yupana #8 / FR-4 on the agent-facing surface. `x.rs` in `fixture()` defines
    // `a` and `b`; a real tree has twelve `build`s, and an agent reading code
    // knows WHERE it is, not which one. Position must answer with that symbol —
    // resolving it to a name and looking the name up would hand back the whole
    // name class, which is the ambiguity the position was given to remove.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("x.rs"),
        "struct Alpha;\nimpl Alpha {\n    fn build() {}\n}\nstruct Beta;\nimpl Beta {\n    fn build() {}\n}\n",
    )
    .unwrap();
    let s = YupanaMcpServer::new(dir.path().to_path_buf(), None, None);

    // By name: ambiguous, both sites.
    let by_name = served(
        s.yupana_references(Parameters(ReferencesRequest {
            symbol: Some("build".into()),
            path: Some(".".into()),
            at_file: None,
            at_line: None,
        }))
        .await,
    );
    assert_eq!(by_name["count"], 2, "name is ambiguous here: {by_name}");

    // By position: exactly the one enclosing that line.
    let by_pos = served(
        s.yupana_references(Parameters(ReferencesRequest {
            symbol: None,
            path: Some(".".into()),
            at_file: Some("x.rs".into()),
            at_line: Some(7),
        }))
        .await,
    );
    assert_eq!(by_pos["count"], 1, "position must disambiguate: {by_pos}");
    assert_eq!(by_pos["definitions"][0]["start_line"], 7, "{by_pos}");
    assert_top_level_tier(&by_pos, "yupana_references");
}

#[tokio::test]
async fn references_refuses_half_a_position_rather_than_downgrading_to_a_name() {
    // `at_file` without `at_line` is not a position. Falling back to a name
    // lookup would answer "which one is here?" with "all of them" — a silent
    // downgrade to the very over-connection the parameter exists to cut.
    let dir = fixture();
    let err = server(&dir)
        .yupana_references(Parameters(ReferencesRequest {
            symbol: Some("a".into()),
            path: Some(".".into()),
            at_file: Some("x.rs".into()),
            at_line: None,
        }))
        .await;
    assert!(err.is_err(), "half a position must be refused, not guessed");
}
