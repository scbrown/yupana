//! Integration: the REAL `yupana daemon` binary over REAL HTTP (yupana #1 close-out).
//!
//! Everything else tests the router in-process; this drives the shipped binary:
//! start → poll `/health` → tier-tagged query replies → §6.1 latency targets on
//! the warm path → SIGTERM → clean exit. The daemon surface needs the `mcp`
//! feature, so the whole file is gated on it (and on unix, for the signal half).
#![cfg(all(feature = "mcp", unix))]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A repo big enough that a per-query rebuild would be visible next to a
/// resident answer, and with a real 2-hop chain for /impact.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
    std::fs::write(dir.path().join("mid.rs"), "fn caller() { leaf(); }\n").unwrap();
    std::fs::write(dir.path().join("top.rs"), "fn top() { caller(); }\n").unwrap();
    for i in 0..30 {
        std::fs::write(
            dir.path().join(format!("pad{i}.rs")),
            format!("fn pad{i}() {{ leaf(); }}\n"),
        )
        .unwrap();
    }
    dir
}

/// GET `path`; return (status, body). Raw HTTP/1.1, no client dep.
fn get(port: u16, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

/// Spawn `yupana daemon` on an OS-assigned free port and wait for `/health`.
/// Returns the child and the port. Panics (with the daemon's stderr) if it
/// does not come up — a daemon that cannot build the graph must refuse loudly.
fn start_daemon(root: &std::path::Path) -> (Child, u16) {
    // Bind-then-drop to pick a free port; the tiny race with another process
    // is acceptable in CI (the test fails loudly, not silently).
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let child = Command::new(env!("CARGO_BIN_EXE_yupana"))
        .args(["daemon", "--port", &port.to_string()])
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = child;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = s.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
            let mut raw = String::new();
            if s.read_to_string(&mut raw).is_ok() && raw.contains("200") {
                return (child, port);
            }
        }
        if Instant::now() >= deadline {
            // REAP BEFORE PANICKING. The timeout path used to drop the child
            // implicitly, which neither kills nor waits on it: a daemon that came
            // up slowly (or came up but failed /health) survived the test run,
            // holding its port and its graph. In CI that is a stray process per
            // failed run; locally it is a background `yupana daemon` nobody
            // remembers starting. Take its stderr with us, too — the panic that
            // follows is the only place it will ever be read.
            let _ = child.kill();
            let stderr = child.stderr.take().map_or_else(String::new, |mut e| {
                let mut s = String::new();
                let _ = e.read_to_string(&mut s);
                s
            });
            let _ = child.wait();
            panic!("daemon did not serve /health within 30s; its stderr:\n{stderr}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_real_daemon_serves_tier_tagged_answers_within_the_slo_and_stops_cleanly() {
    let dir = repo();
    let (mut child, port) = start_daemon(dir.path());

    // Tier-tagged replies from every query family (FR-3: nothing unlabelled).
    let (code, status) = get(port, "/status");
    assert_eq!(code, 200);
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["status"], "ok");
    assert!(status["nodes"].as_u64().unwrap() >= 33, "{status}");

    for path in [
        "/callers?symbol=leaf",
        "/references?symbol=leaf",
        "/impact?symbol=leaf&hops=5",
        "/symbols?file=mid.rs",
    ] {
        let (code, body) = get(port, path);
        assert_eq!(code, 200, "{path}");
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["tier"], "treesitter", "{path} must be tier-tagged");
    }

    // §6.1 on the warm wire path: ref lookup < 50ms p95, 5-hop impact < 300ms
    // p95. Every request pays full connection setup here, so this is an upper
    // bound on what a keep-alive client would see.
    let mut ref_times = Vec::new();
    let mut impact_times = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        let (code, _) = get(port, "/references?symbol=leaf");
        ref_times.push(t.elapsed());
        assert_eq!(code, 200);
        let t = Instant::now();
        let (code, _) = get(port, "/impact?symbol=leaf&hops=5");
        impact_times.push(t.elapsed());
        assert_eq!(code, 200);
    }
    ref_times.sort();
    impact_times.sort();
    let p95 = |v: &[Duration]| v[(v.len() * 95).div_euclid(100).min(v.len() - 1)];
    assert!(
        p95(&ref_times) < Duration::from_millis(50),
        "ref lookup p95 {:?} breaches the 50ms target",
        p95(&ref_times)
    );
    assert!(
        p95(&impact_times) < Duration::from_millis(300),
        "impact p95 {:?} breaches the 300ms target",
        p95(&impact_times)
    );

    // SIGTERM = graceful: drain and exit 0, not a kill.
    let pid = child.id().to_string();
    let killed = Command::new("kill").arg(&pid).status().unwrap();
    assert!(killed.success(), "kill(1) must reach the daemon");
    let deadline = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(code) = child.try_wait().unwrap() {
            break code;
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not exit within 10s of SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(code.success(), "SIGTERM must be a clean stop, got {code}");

    // And its stderr says so — the drain is announced, not silent.
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(err.contains("shut down cleanly"), "stderr was: {err}");
}
