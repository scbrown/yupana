//! Tests for the daemon HTTP surface. Child module of [`super`] (`http`), split
//! into a `_test.rs` file for the file-size discipline (tests exempt).

//! Test names here shout the invariant they pin — `is_NEVER_observable`,
//! `daemon_EXPECTED_but_DOWN`, `is_DOWN_not_UP`. That capitalisation is the
//! same emphasis the prose uses throughout this repo, and it is load-bearing in
//! a test name: it says which word the assertion turns on. Allowed explicitly,
//! and scoped to tests, so the lint stays live everywhere else rather than
//! being switched off crate-wide (yupana #83).
#![allow(non_snake_case)]
use super::*;
use crate::daemon::client::{probe, Reachability};
use std::time::Duration;
use tempfile::tempdir;

// Build a tiny real repo so the resident graph is non-empty.
fn tiny_repo() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
    std::fs::write(dir.path().join("caller.rs"), "fn caller() { leaf(); }\n").unwrap();
    dir
}

// Serve `engine` on an ephemeral port; return the port.
async fn spawn(engine: ResidentEngine) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(engine)).await;
    });
    // Let the listener accept.
    tokio::time::sleep(Duration::from_millis(50)).await;
    port
}

#[tokio::test]
async fn a_running_daemon_answers_the_probe_UP_and_serves_real_counts() {
    let dir = tiny_repo();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let served = engine.status();
    assert!(served.nodes > 0, "the resident graph must be non-empty");
    let port = spawn(engine).await;

    // The client seam sees it as UP.
    let host = "127.0.0.1".to_string();
    let r = tokio::task::spawn_blocking(move || probe(&host, port, Duration::from_millis(500)))
        .await
        .unwrap();
    assert_eq!(r, Reachability::Up, "a live daemon must probe UP");

    // And /status reports the same real counts the engine holds.
    let body = get_json(port, "/status").await;
    assert_eq!(body["nodes"].as_u64().unwrap() as usize, served.nodes);
    assert_eq!(body["edges"].as_u64().unwrap() as usize, served.edges);
    assert_eq!(body["status"].as_str().unwrap(), "ok");
}

#[tokio::test]
async fn a_DEAD_daemon_probes_DOWN_never_up() {
    // No server bound. The seam must say Down, loudly, with a reason — this is
    // the "killing it is the cheapest bypass" case the whole seam exists for.
    let r = tokio::task::spawn_blocking(|| probe("127.0.0.1", 1, Duration::from_millis(200)))
        .await
        .unwrap();
    assert!(!r.is_up());
    assert!(r.down_reason().unwrap().contains("no daemon"));
}

#[tokio::test]
async fn the_query_endpoints_answer_from_the_resident_graph() {
    let dir = tiny_repo();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let port = spawn(engine).await;

    // /callers?symbol=leaf — `caller` calls `leaf`.
    let callers = get_json(port, "/callers?symbol=leaf").await;
    assert!(callers["found"].as_bool().unwrap());
    let names: Vec<&str> = callers["neighbors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"caller"), "got {names:?}");
    assert_eq!(callers["tier"].as_str().unwrap(), "treesitter");

    // /impact?symbol=leaf — the transitive blast radius.
    let impact = get_json(port, "/impact?symbol=leaf&hops=5").await;
    assert!(impact["found"].as_bool().unwrap());
    assert!(impact["count"].as_u64().unwrap() >= 1);

    // An unknown symbol is found=false, not an error.
    let missing = get_json(port, "/callers?symbol=nope").await;
    assert!(!missing["found"].as_bool().unwrap());
}

#[tokio::test]
async fn references_serves_every_definition_site_from_the_resident_index() {
    // `shared` is defined in TWO files; /references must return both sites,
    // tier-tagged, with found distinct from empty.
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn shared() {}\n").unwrap();
    std::fs::write(dir.path().join("b.rs"), "fn shared() {}\n").unwrap();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let port = spawn(engine).await;

    let refs = get_json(port, "/references?symbol=shared").await;
    assert!(refs["found"].as_bool().unwrap());
    assert_eq!(refs["count"].as_u64().unwrap(), 2, "both definition sites");
    let mut files: Vec<&str> = refs["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["file"].as_str().unwrap())
        .collect();
    files.sort_unstable();
    assert_eq!(files, ["a.rs", "b.rs"]);
    assert_eq!(refs["tier"].as_str().unwrap(), "treesitter");

    // Absent symbol: found=false, zero sites — an answer, not an error.
    let missing = get_json(port, "/references?symbol=absent").await;
    assert!(!missing["found"].as_bool().unwrap());
    assert_eq!(missing["count"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn symbols_serves_one_files_symbols_in_line_order() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("two.rs"), "fn first() {}\nfn second() {}\n").unwrap();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let port = spawn(engine).await;

    let syms = get_json(port, "/symbols?file=two.rs").await;
    assert!(syms["known"].as_bool().unwrap());
    let names: Vec<&str> = syms["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["first", "second"], "line order");
    assert_eq!(syms["tier"].as_str().unwrap(), "treesitter");

    // A path the graph holds nothing for: known=false, and the reply still
    // carries its tier — absence is tagged like any other fact.
    let missing = get_json(port, "/symbols?file=missing.rs").await;
    assert!(!missing["known"].as_bool().unwrap());
    assert_eq!(missing["count"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn dataflow_mirrors_the_mcp_tool_and_is_confined_to_the_root() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("flow.rs"),
        "fn f() { let a = 1; let b = a + 1; let c = b * 2; }\n",
    )
    .unwrap();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let port = spawn(engine).await;

    // All edges of f: b depends on a, c depends on b.
    let all = get_json(port, "/dataflow?function=f").await;
    assert!(all["found"].as_bool().unwrap());
    assert!(
        all["edges"].as_array().unwrap().len() >= 2,
        "got {}",
        all["edges"]
    );
    assert_eq!(all["tier"].as_str().unwrap(), "treesitter");

    // Tracing c backwards reaches a (via b).
    let trace = get_json(port, "/dataflow?function=f&var=c").await;
    assert_eq!(trace["direction"].as_str().unwrap(), "depends_on");
    let reached: Vec<&str> = trace["flow"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(reached.contains(&"a"), "got {reached:?}");

    // A path escaping the root is refused, not built — same confinement as
    // /measure: the localhost daemon must not analyze arbitrary trees.
    let (code, _) = get_raw(port, "/dataflow?function=f&path=../").await;
    assert_eq!(code, 400, "an escaping path must be refused");
}

#[tokio::test]
async fn fetch_measure_client_gets_a_reply_from_a_LIVE_daemon() {
    // The "daemon expected AND up" quadrant of the hook cutover, at the client
    // level: fetch_measure against a real daemon returns a measured reply.
    use crate::daemon::client::fetch_measure;
    let dir = tiny_repo(); // leaf.rs <- caller.rs
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let leaf = dir.path().join("leaf.rs").to_string_lossy().to_string();
    let port = spawn(engine).await;

    let reply = tokio::task::spawn_blocking(move || {
        fetch_measure(
            "127.0.0.1",
            port,
            &leaf,
            "leaf.rs",
            &["fn leaf".to_string()],
            5,
            Duration::from_millis(500),
        )
    })
    .await
    .unwrap();
    let reply = reply.expect("a live daemon must answer fetch_measure");
    assert!(reply.measured);
    assert!(reply.symbols >= 1);

    // And against a CLOSED port it is an Err with a reason — never a silent ok.
    let err = tokio::task::spawn_blocking(|| {
        fetch_measure(
            "127.0.0.1",
            1,
            "/x",
            "x",
            &[],
            5,
            Duration::from_millis(200),
        )
    })
    .await
    .unwrap();
    assert!(
        err.is_err(),
        "a closed port must be an Err, never a silent reply"
    );
}

#[tokio::test]
async fn measure_sizes_an_edit_and_confines_to_the_root() {
    let dir = tiny_repo(); // leaf.rs <- caller.rs
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let leaf = dir.path().join("leaf.rs").to_string_lossy().to_string();
    let port = spawn(engine).await;

    // Editing leaf reaches caller — measured, radius >= 1.
    let (code, body) = post_json(
        port,
        "/measure",
        &serde_json::json!({ "file": leaf, "rel": "leaf.rs", "anchors": ["fn leaf"] }).to_string(),
    )
    .await;
    assert_eq!(code, 200);
    let body = body.unwrap();
    assert!(body["measured"].as_bool().unwrap());
    assert!(body["symbols"].as_u64().unwrap() >= 1);

    // A path OUTSIDE the root is refused, not read — the localhost daemon must
    // not be an arbitrary-file reader.
    let (code, _) = post_json(
        port,
        "/measure",
        &serde_json::json!({ "file": "/etc/passwd", "rel": "x" }).to_string(),
    )
    .await;
    assert_eq!(code, 400, "a file outside the root must be refused");
}

// A committed repo, so the engine grows its tenant layer (base@HEAD).
fn committed_repo() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "t@t"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
    std::fs::write(dir.path().join("mid.rs"), "fn mid() { leaf(); }\n").unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-qm", "base"]);
    dir
}

#[tokio::test]
async fn edit_feeds_the_tenant_overlay_and_queries_scope_by_tenant() {
    let dir = committed_repo();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let port = spawn(engine).await;

    // Feed tenant a: mid.rs rewritten, `mid2` now calls leaf.
    let (code, reply) = post_json(
        port,
        "/edit",
        &serde_json::json!({
            "tenant": "a", "rel": "mid.rs", "content": "fn mid2() { leaf(); }\n"
        })
        .to_string(),
    )
    .await;
    assert_eq!(code, 200);
    let reply = reply.unwrap();
    assert_eq!(reply["symbols"].as_u64().unwrap(), 1);
    assert_eq!(reply["tier"].as_str().unwrap(), "treesitter");

    // Tenant a's view: mid2 calls leaf; mid is masked.
    let a = get_json(port, "/callers?symbol=leaf&tenant=a").await;
    let names: Vec<&str> = a["neighbors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["mid2"], "a sees its overlay truth");

    // Tenant b — and the un-tenanted surface — are ISOLATED from it.
    let b = get_json(port, "/callers?symbol=leaf&tenant=b").await;
    let names: Vec<&str> = b["neighbors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["mid"], "b composes the bare base");
    let legacy = get_json(port, "/callers?symbol=leaf").await;
    let names: Vec<&str> = legacy["neighbors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["mid"], "the un-tenanted surface is unchanged");

    // /status reports the layer: base commit + a's overlay, O(touched) sized.
    let status = get_json(port, "/status").await;
    let layer = &status["tenant_layer"];
    assert_eq!(layer["base_commit"].as_str().unwrap().len(), 40);
    assert_eq!(layer["active_overlays"][0]["tenant"].as_str().unwrap(), "a");
    assert_eq!(layer["active_overlays"][0]["touched_files"], 1);
}

#[tokio::test]
async fn edit_matching_base_content_still_advises_and_does_not_412() {
    // FR-15 base hit: an edit identical to the baseline creates no overlay
    // entry, but /edit must still answer 200 with the base advisory — the file
    // still has symbols with external callers. (Regression: deriving names from
    // the absent overlay parse made this a spurious 412.)
    let dir = committed_repo(); // leaf.rs, mid.rs calls leaf
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let port = spawn(engine).await;

    let (code, reply) = post_json(
        port,
        "/edit",
        &serde_json::json!({
            "tenant": "a", "rel": "leaf.rs", "content": "fn leaf() {}\n"
        })
        .to_string(),
    )
    .await;
    assert_eq!(code, 200, "base-identical edit must not 412");
    let reply = reply.unwrap();
    assert_eq!(
        reply["symbols"].as_u64().unwrap(),
        1,
        "base symbols still seen"
    );
    let advised: Vec<&str> = reply["advised"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["symbol"].as_str().unwrap())
        .collect();
    assert_eq!(advised, ["leaf"], "leaf has an external caller (mid.rs)");

    // And it created NO overlay: /status shows no active overlay for a.
    let status = get_json(port, "/status").await;
    assert!(
        status["tenant_layer"]["active_overlays"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a base-identical edit must not mask anything"
    );
}

#[tokio::test]
async fn a_tenant_query_without_a_tenant_layer_is_refused_not_empty() {
    // A non-repo root has no commit to anchor a base to: naming a tenant must
    // be an explicit 412, never an empty answer wearing a normal one. The
    // un-tenanted surface keeps serving.
    let dir = tiny_repo();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let port = spawn(engine).await;

    let (code, _) = get_raw(port, "/callers?symbol=leaf&tenant=a").await;
    assert_eq!(code, 412, "tenant named, layer absent ⇒ explicit refusal");
    let (code, _) = post_json(
        port,
        "/edit",
        &serde_json::json!({"tenant": "a", "rel": "leaf.rs", "content": ""}).to_string(),
    )
    .await;
    assert_eq!(code, 412);

    let status = get_json(port, "/status").await;
    assert!(
        status["tenant_layer"].is_null(),
        "absent layer is null, not an empty registry"
    );
    let legacy = get_json(port, "/callers?symbol=leaf").await;
    assert!(legacy["found"].as_bool().unwrap());
}

// Minimal HTTP GET without pulling a client dep into the crate: reuse the
// probe's raw-socket approach and read the whole body as JSON.
async fn get_json(port: u16, path: &str) -> serde_json::Value {
    let (code, body) = get_raw(port, path).await;
    assert!((200..300).contains(&code), "GET {path} -> {code}");
    body.unwrap()
}

// GET returning (status_code, parsed_body_if_2xx).
async fn get_raw(port: u16, path: &str) -> (u16, Option<serde_json::Value>) {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).unwrap();
        let mut raw = String::new();
        s.read_to_string(&mut raw).unwrap();
        parse_response(&raw)
    })
    .await
    .unwrap()
}

// POST a JSON body; return (status_code, parsed_body_if_2xx).
async fn post_json(port: u16, path: &str, body: &str) -> (u16, Option<serde_json::Value>) {
    let path = path.to_string();
    let body = body.to_string();
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut raw = String::new();
        s.read_to_string(&mut raw).unwrap();
        parse_response(&raw)
    })
    .await
    .unwrap()
}

// Split a raw HTTP/1.1 response into (status, parsed JSON body when 2xx).
fn parse_response(raw: &str) -> (u16, Option<serde_json::Value>) {
    let status: u16 = raw
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let payload = raw.split("\r\n\r\n").nth(1).unwrap_or("");
    let parsed = if (200..300).contains(&status) {
        serde_json::from_str(payload).ok()
    } else {
        None
    };
    (status, parsed)
}

#[tokio::test]
async fn edit_drives_the_frontier_recompute_on_the_QUERYABLE_graph() {
    // bobbin-bnq: `update_frontier_bounded` used to have one production caller
    // — the watch path, over an inline registry no server held — so the
    // recomputed overlay was unqueryable. This pins the wire: an edit fed to
    // the DAEMON recomputes the frontier on the daemon's own registry, tracks
    // the watch path's freshness transitions, and the same process serves the
    // recomputed truth.
    let dir = committed_repo(); // leaf.rs; mid.rs calls leaf
    let engine = ResidentEngine::build(dir.path(), None).unwrap();

    // Never edited: nothing to claim freshness about.
    assert_eq!(engine.freshness_of("a", "leaf.rs"), None);

    let port = spawn(engine.clone()).await;
    let (code, reply) = post_json(
        port,
        "/edit",
        &serde_json::json!({
            "tenant": "a", "rel": "leaf.rs", "content": "fn leaf() {}\nfn leaf2() {}\n"
        })
        .to_string(),
    )
    .await;
    assert_eq!(code, 200);
    let reply = reply.unwrap();

    // The frontier reached leaf's caller (`mid`) — the recompute ran, and on
    // this engine's registry, not a watch-path registry nobody can query.
    assert!(
        reply["frontier"].as_u64().unwrap() >= 1,
        "the edit's frontier must include leaf's caller: {reply}"
    );

    // The watch path's freshness discipline, on the edit path: the frontier is
    // current once /edit answers.
    assert_eq!(
        engine.freshness_of("a", "leaf.rs"),
        Some(crate::types::Freshness::Fresh),
        "an answered edit's frontier is current"
    );
    // ... and per-tenant: tenant b never edited anything.
    assert_eq!(engine.freshness_of("b", "leaf.rs"), None);

    // The recomputed overlay is queryable in the same process (the whole
    // point): tenant a sees its new symbol.
    let refs = get_json(port, "/references?symbol=leaf2&tenant=a").await;
    assert_eq!(refs["found"], true, "recomputed overlay must be queryable");
}
