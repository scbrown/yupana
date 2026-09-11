//! Exposure must travel with the session-attributed guard decision, not a timestamp join.
#![cfg(feature = "quipu")]
// Test names shout the invariant they turn on — the emphasis this repo already
// uses in `daemon::exposure_test`. Scoped to tests.
#![allow(non_snake_case)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

struct ExposureServer {
    endpoint: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Vec<String>>>,
}

impl ExposureServer {
    fn new(outcome: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            while !stopped.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 4096];
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                        let len: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if body.len() >= len {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                let body = serde_json::json!({"outcome":outcome}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        Self {
            endpoint,
            stop,
            thread: Some(thread),
        }
    }

    fn finish(&mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap()
    }
}

impl Drop for ExposureServer {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.finish();
        }
    }
}

fn probe(outcome: &'static str, expected: &str, mode: &str, matching: bool, origin: bool) {
    probe_with(outcome, expected, mode, matching, origin, None);
}

/// `endpoint_override` points the guard at a REFUSED port, so "we never got an
/// answer" is a real event rather than a mock of one. `expected_source` is the
/// fact aegis-8tumi4 exists for: `unknown` alone cannot tell a repo quipu does
/// not know from a quipu we never reached, and both fail open.
fn probe_with(
    outcome: &'static str,
    expected: &str,
    mode: &str,
    matching: bool,
    origin: bool,
    endpoint_override: Option<&str>,
) {
    let mut server = ExposureServer::new(outcome);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let target = root.join("artifact");
    std::fs::create_dir(&target).unwrap();
    assert!(Command::new("git")
        .args(["init", "--quiet"])
        .arg(&target)
        .status()
        .unwrap()
        .success());
    if origin {
        assert!(Command::new("git")
            .current_dir(&target)
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/example/artifact.git"
            ])
            .status()
            .unwrap()
            .success());
    }
    // cwd deliberately differs from the edited file's repository.
    std::fs::create_dir_all(root.join(".bobbin")).unwrap();
    let endpoint = endpoint_override.unwrap_or(&server.endpoint).to_string();
    std::fs::write(root.join(".bobbin/config.toml"), format!(
        "[yupana.policy]\nmode = '{mode}'\n[yupana.quipu]\nenabled = true\nendpoint = '{endpoint}'\nprojection_cache_ttl_secs = 3600\n"
    )).unwrap();
    let cache = serde_json::json!({
        "version":2, "written_at":yupana::projection_cache::now_secs(),
        "endpoint":endpoint,"policies":[],
        "text_rules":[{"name":"test-boundary","pattern":"FORBIDDEN_TEST_TOKEN","tier":"block"}]
    });
    std::fs::write(root.join("projection.json"), cache.to_string()).unwrap();
    let session = "exposure-eval-probe";
    let payload = serde_json::json!({"session_id":session,"cwd":root,"tool_name":"Edit",
        "tool_input":{"file_path":target.join("note.md"),"old_string":"a",
        "new_string":if matching {"FORBIDDEN_TEST_TOKEN"} else {"ordinary prose"}}});
    let binary = std::env::var_os("YUPANA_EXPOSURE_TEST_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_yupana").into());
    let mut child = Command::new(binary)
        .args(["hook", "pre-edit"])
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("YUPANA_PROJECTION_CACHE_PATH", root.join("projection.json"))
        .env("YUPANA_METRICS_PATH", root.join("metrics.jsonl"))
        .env("YUPANA_FAILOPEN_MARKER_DIR", root.join("markers"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = server.finish();
    let records: Vec<serde_json::Value> = std::fs::read_to_string(root.join("metrics.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let guard = records.iter().find(|r| r["kind"] == "guard").unwrap();
    assert_eq!(guard["session"], session);
    assert!(
        guard.get("path").is_none(),
        "path recording is still opt-in"
    );
    if matching {
        let governed = records.iter().find(|r| r["kind"] == "governed").unwrap();
        assert_eq!(guard["rule"], "test-boundary");
        assert_eq!(guard["exposure"], expected);
        let want_source = if !origin {
            // Decided WITHOUT asking anything: not a lookup, so neither a
            // success nor a fail-open. It must be countable as neither.
            "local"
        } else if endpoint_override.is_some() {
            "unreachable"
        } else {
            "answered"
        };
        assert_eq!(
            guard["exposure_source"], want_source,
            "the record must say HOW the exposure was obtained, not only what it was"
        );
        assert_eq!(guard["exposure_source"], governed["exposure_source"]);
        assert_eq!(
            guard["repo"],
            if origin { "artifact" } else { "unresolved" }
        );
        assert_eq!(guard["exposure"], governed["exposure"]);
        assert_eq!(guard["repo"], governed["repo"]);
        // BLOCKING IS NOW TWO CASES, NOT ONE (aegis-8tumi4 item 3, ruled 2026-09-11).
        // A public repo blocks because we MEASURED that it is exposed. An
        // unreachable lookup with nothing cached blocks because we measured
        // NOTHING — "not public" is then the absence of a finding, and a check
        // that cannot measure must not report safe. A repo quipu ANSWERED it
        // does not know still only warns: that one is a genuine guess.
        let unmeasured = want_source == "unreachable";
        let blocks = (expected == "public" || unmeasured) && mode == "enforce";
        assert_eq!(guard["result"], if blocks { "deny" } else { "notify" });
        assert_eq!(
            requests.len(),
            usize::from(origin && endpoint_override.is_none()),
            "no second exposure lookup"
        );
        if origin && endpoint_override.is_none() {
            assert!(requests[0].starts_with("POST /policy/check "));
            assert!(requests[0].contains("repo_artifact"));
        }
    } else {
        assert!(guard.get("exposure").is_none());
        assert!(guard.get("exposure_source").is_none());
        assert!(guard.get("repo").is_none());
        assert!(requests.is_empty());
    }
}

#[test]
fn exposure_and_provenance_share_the_decision_record() {
    for (outcome, exposure) in [
        ("satisfied", "public"),
        ("unsatisfied", "internal"),
        ("unknown", "unknown"),
    ] {
        for mode in ["advise", "enforce"] {
            probe(outcome, exposure, mode, true, true);
        }
    }
}

#[test]
fn unresolved_repo_is_unknown_and_unmatched_edits_omit_exposure() {
    probe("satisfied", "unknown", "enforce", true, false);
    probe("satisfied", "n/a", "advise", false, true);
}

/// THE FAIL-OPEN, CLOSED. This test used to assert that a quipu we never reached
/// and a repo quipu does not know both decide `unknown` and both let the edit
/// through, "by design". The record half of that was right and stays. The
/// OUTCOME half was the defect: 6 of 14 network-needing lookups timed out, 43%,
/// every one indistinguishable from a correct pass, and every one letting an
/// identifier into a repo whose exposure was never established.
///
/// Since aegis-8tumi4 item 3 the two cases are separated in BOTH halves. Quipu
/// answered "no such repo" -> we measured, ignorance about an unpinned repo is a
/// guess, still a warning. The lookup failed with nothing cached -> we did not
/// measure at all, and at enforce tier that now REFUSES.
///
/// 127.0.0.1:1 is refused immediately, so "quipu is down" is fast and real
/// rather than mocked — the same shape `daemon::exposure_test` uses.
#[test]
fn an_UNREACHABLE_quipu_is_recorded_as_unreachable_not_as_a_plain_unknown() {
    probe_with(
        "satisfied",
        "unknown",
        "enforce",
        true,
        true,
        Some("http://127.0.0.1:1"),
    );
}
