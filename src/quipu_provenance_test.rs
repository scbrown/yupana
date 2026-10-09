use super::*;
use std::collections::BTreeMap;

fn build(env: &[(&str, &str)]) -> BTreeMap<&'static str, String> {
    headers(
        |key| {
            env.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        },
        Some("test-host".into()),
        "yupana-promote",
    )
    .into_iter()
    .collect()
}

#[test]
fn producers_ignore_inherited_agent_and_session() {
    let h = build(&[
        ("SHANTY_AGENT", "stale"),
        ("CLAUDE_CODE_SESSION_ID", "stale"),
        ("CODEX_THREAD_ID", "stale"),
        ("SHANTY_MODEL", "stale"),
    ]);
    assert_eq!(h["X-Quipu-Agent"], "yupana-promote");
    assert_eq!(h["X-Quipu-Harness"], "cli");
    assert_eq!(h["X-Quipu-Host"], "test-host");
    assert!(!h.contains_key("X-Quipu-Session"));
    assert!(!h.contains_key("X-Quipu-Model"));
}

#[test]
fn active_codex_prefers_session_id_then_thread_and_never_claude_id() {
    for (extra, expected) in [
        (vec![("CODEX_SESSION_ID", "session")], Some("session")),
        (vec![], Some("thread")),
    ] {
        let mut env = vec![
            ("CODEX_HOME", "/managed"),
            ("SHANTY_AGENT", "worker"),
            ("SHANTY_MODEL", "model"),
            ("CODEX_THREAD_ID", "thread"),
            ("CLAUDE_CODE_SESSION_ID", "wrong"),
        ];
        env.extend(extra);
        let h = build(&env);
        assert_eq!(h["X-Quipu-Harness"], "codex");
        assert_eq!(h["X-Quipu-Agent"], "worker");
        assert_eq!(h["X-Quipu-Model"], "model");
        assert_eq!(h.get("X-Quipu-Session").map(String::as_str), expected);
    }
    let h = build(&[
        ("CODEX_HOME", "/managed"),
        ("CLAUDE_CODE_SESSION_ID", "wrong"),
    ]);
    assert!(!h.contains_key("X-Quipu-Session"));
    assert!(!h.contains_key("X-Quipu-Model"));
    assert!(!h.contains_key("X-Quipu-Agent"));
}

#[test]
fn claude_selection_and_explicit_overrides() {
    let h = build(&[
        ("CLAUDECODE", "1"),
        ("CODEX_HOME", "/managed"),
        ("CLAUDE_CODE_SESSION_ID", "claude-session"),
        ("CODEX_SESSION_ID", "wrong"),
    ]);
    assert_eq!(h["X-Quipu-Harness"], "claude");
    assert_eq!(h["X-Quipu-Session"], "claude-session");
    let h = build(&[
        ("CODEX_HOME", "/managed"),
        ("SHANTY_AGENT", "wrong"),
        ("QUIPU_AGENT", "producer"),
        ("QUIPU_HARNESS", "cron"),
        ("QUIPU_MODEL", "explicit-model"),
        ("QUIPU_SESSION", "explicit-session"),
        ("QUIPU_HOST", "explicit-host"),
    ]);
    for (key, value) in [
        ("Agent", "producer"),
        ("Harness", "cron"),
        ("Model", "explicit-model"),
        ("Session", "explicit-session"),
        ("Host", "explicit-host"),
    ] {
        assert_eq!(h[format!("X-Quipu-{key}").as_str()], value);
    }
}

#[test]
fn sanitization_omission_and_length_bound() {
    let long = "x".repeat(200);
    let h = build(&[
        ("QUIPU_AGENT", " \r\n\0aé\tgent "),
        ("QUIPU_MODEL", "\r\né"),
        ("QUIPU_SESSION", &long),
    ]);
    assert_eq!(h["X-Quipu-Agent"], "agent");
    assert!(!h.contains_key("X-Quipu-Model"));
    assert_eq!(h["X-Quipu-Session"].len(), 128);
    assert!(h
        .values()
        .all(|v| v.bytes().all(|b| (32..=126).contains(&b))));
    let h = headers(|_| None, None, crate::quipu_label::DAEMON);
    assert!(h.contains(&("X-Quipu-Harness", "service".into())));
    assert!(!h.iter().any(|(k, _)| *k == "X-Quipu-Host"));
}

#[test]
fn request_headers_preserve_caller_label_and_content_type() {
    let req = crate::quipu_label::json_post("http://127.0.0.1:9/knot", crate::quipu_label::PROMOTE);
    assert_eq!(
        req.header("X-Quipu-Client"),
        Some(crate::quipu_label::PROMOTE)
    );
    assert_eq!(req.header("Content-Type"), Some("application/json"));
    assert!(req.header("X-Quipu-Harness").is_some());
}
