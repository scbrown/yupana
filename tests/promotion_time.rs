//! Committed promotion keeps author time, committer identity and writer separate.
#![cfg(feature = "quipu")]

use assert_cmd::prelude::*;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

const AUTHORED: &str = "2026-03-04T00:15:00+01:00";
const STORED: &str = "2026-03-03T23:15:00Z";

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "Author")
            .env("GIT_AUTHOR_EMAIL", "author@example.com")
            .env("GIT_AUTHOR_DATE", AUTHORED)
            .env("GIT_COMMITTER_NAME", "Integrator")
            .env("GIT_COMMITTER_EMAIL", "integrator@example.com")
            .env("GIT_COMMITTER_DATE", "2026-09-01T12:00:00Z")
            .assert()
            .success();
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("x.rs"), "pub fn x() {}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "base"]);
    std::fs::write(dir.path().join("y.rs"), "pub fn y() {}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "add y"]);
    std::fs::write(
        dir.path().join("config.toml"),
        "[yupana.quipu]\nenabled = false\n",
    )
    .unwrap();
    dir
}

fn receiver(count: usize, echo: bool) -> (String, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..count {
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0; 8192];
            loop {
                let n = sock.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&buf[..n]);
                let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..split]);
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if bytes.len() >= split + 4 + length {
                    requests.push(
                        serde_json::from_slice(&bytes[split + 4..split + 4 + length]).unwrap(),
                    );
                    break;
                }
            }
            let response = if echo {
                serde_json::json!({"count":10,"tx_id":77,"valid_from":STORED})
            } else {
                serde_json::json!({"count":10,"tx_id":77})
            }
            .to_string();
            write!(
                sock,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
        }
        requests
    });
    (url, thread)
}

fn promote(args: &[&str], count: usize) {
    let dir = fixture();
    let (url, server) = receiver(count, true);
    let output = Command::cargo_bin("yupana")
        .unwrap()
        .args([
            "promote",
            "--repo",
            "fixture",
            "--config",
            "config.toml",
            "--to",
            &url,
        ])
        .args(args)
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env_remove("QUIPU_AUTH_TOKEN")
        .env("QUIPU_AUTH_TOKEN_FILE", dir.path().join("absent-token"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(
        output.contains(&format!("valid-from: {STORED}")),
        "{output}"
    );
    assert!(
        !output.contains(&format!("valid-from: {AUTHORED}")),
        "must report server key"
    );
    let requests = server.join().unwrap();
    let sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let sha = String::from_utf8(sha.stdout).unwrap();
    let mut turtle = String::new();
    for req in requests {
        assert_eq!(req["valid_from"], AUTHORED);
        assert_eq!(req["actor"], "yupana");
        assert!(
            req.get("timestamp").is_none(),
            "transaction time must stay server assigned"
        );
        assert!(req["source"].as_str().unwrap().contains(sha.trim()));
        turtle.push_str(req["turtle"].as_str().unwrap());
    }
    let identity = |predicate: &str, value: &str| {
        turtle.lines().any(|line| {
            (line.contains(&format!("bobbin:{predicate}"))
                || line.contains(&format!("/{predicate}>")))
                && line.contains(&format!("\"{value}\""))
        })
    };
    assert!(
        identity("author", "Author <author@example.com>"),
        "{turtle}"
    );
    assert!(
        identity("committer", "Integrator <integrator@example.com>"),
        "{turtle}"
    );
    let modifies = |file: &str| {
        turtle.lines().any(|line| {
            (line.contains("bobbin:modifies") || line.contains("/modifies>"))
                && line.ends_with(&format!("/fixture/{file}> ."))
        })
    };
    assert!(modifies("y.rs"));
    assert!(!modifies("x.rs"));
}

#[test]
fn snapshot_uses_author_time_and_reports_normalized_key() {
    promote(&["--replace-snapshot"], 1);
}

#[test]
fn append_uses_author_time_and_reports_normalized_key() {
    promote(&["--append"], 1);
}

#[test]
fn subset_carries_author_time_on_every_partition() {
    promote(&["--subset", "--base", "HEAD~1"], 2);
}

#[test]
fn a_server_without_valid_time_confirmation_cannot_report_success() {
    let dir = fixture();
    let (url, server) = receiver(1, false);
    Command::cargo_bin("yupana")
        .unwrap()
        .args([
            "promote",
            "--replace-snapshot",
            "--repo",
            "fixture",
            "--config",
            "config.toml",
            "--to",
            &url,
        ])
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env_remove("QUIPU_AUTH_TOKEN")
        .env("QUIPU_AUTH_TOKEN_FILE", dir.path().join("absent-token"))
        .assert()
        .failure()
        .stderr(predicates::str::contains("did not confirm valid_from"));
    assert_eq!(
        server.join().unwrap().len(),
        1,
        "an uncertain write must not be retried"
    );
}
