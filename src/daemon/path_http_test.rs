//! Tests for the golden-path HTTP surface (FR-41/FR-42).
#![allow(non_snake_case)]

use std::time::Duration;

use super::*;
use crate::daemon::http::router;

fn tiny_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
    dir
}

async fn spawn() -> u16 {
    let dir = tiny_repo();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    std::mem::forget(dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(engine)).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    port
}

/// `(status_code, body_text)`.
async fn post(port: u16, path: &str, body: &str) -> (u16, String) {
    let (path, body) = (path.to_string(), body.to_string());
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
        let code: u16 = raw
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap();
        let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
        (code, body)
    })
    .await
    .unwrap()
}

const PATHS: &str = r#"[{
    "grammar": "gp-grammar/1",
    "path": "http://ex/gp-deploy",
    "level": "advisory",
    "pattern": [
        {"action_kind": "edit", "target_class": "literal"},
        {"action_kind": "verify", "target_class": "literal"}
    ],
    "dead_ends": [{"action_kind": "cache", "target_class": "literal",
                   "note": "did not help the exemplars"}],
    "exemplars": ["http://ex/traj"],
    "projected_at": "2026-08-20T00:00:00Z"
}]"#;

#[tokio::test]
async fn checking_with_no_projected_paths_is_409_not_200() {
    // The safety property of this surface: "nothing was projected" and "this
    // plan conforms" must never be the same status.
    let port = spawn().await;
    let (code, body) = post(
        port,
        "/path/check",
        r#"{"follows_path": "http://ex/gp-deploy"}"#,
    )
    .await;
    assert_eq!(code, 409, "{body}");
    assert!(body.contains("dead backend"), "{body}");
}

#[tokio::test]
async fn a_conforming_plan_answers_200_with_the_verdict_and_its_freshness() {
    let port = spawn().await;
    let req = format!(
        r#"{{"follows_path": "http://ex/gp-deploy", "paths": {PATHS},
            "steps": [{{"action_kind": "edit", "target_class": "literal"}},
                      {{"action_kind": "run", "target_class": "literal"}},
                      {{"action_kind": "verify", "target_class": "literal"}}]}}"#
    );
    let (code, body) = post(port, "/path/check", &req).await;
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"effect\":\"none\""), "{body}");
    assert!(body.contains("2026-08-20T00:00:00Z"), "{body}");
    assert!(body.contains("http://ex/traj"), "{body}");
}

#[tokio::test]
async fn a_hazardous_deviating_plan_warns_and_names_both() {
    let port = spawn().await;
    let req = format!(
        r#"{{"follows_path": "http://ex/gp-deploy", "paths": {PATHS},
            "steps": [{{"action_kind": "cache", "target_class": "literal"}},
                      {{"action_kind": "verify", "target_class": "literal"}}]}}"#
    );
    let (code, body) = post(port, "/path/check", &req).await;
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"effect\":\"warn\""), "{body}");
    assert!(body.contains("first_deviation"), "{body}");
    assert!(body.contains("did not help the exemplars"), "{body}");
}

#[tokio::test]
async fn a_grammar_this_build_does_not_implement_is_409() {
    let port = spawn().await;
    let paths = PATHS.replace("gp-grammar/1", "gp-grammar/9");
    let req =
        format!(r#"{{"follows_path": "http://ex/gp-deploy", "paths": {paths}, "steps": []}}"#);
    let (code, body) = post(port, "/path/check", &req).await;
    assert_eq!(code, 409, "{body}");
    assert!(body.contains("gp-grammar/9"), "{body}");
}
