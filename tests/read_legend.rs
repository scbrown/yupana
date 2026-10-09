//! Exercise the real hook adapter, including transport failures and byte caps.
#![cfg(feature = "quipu")]
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn hook(payload: &serde_json::Value, config: &std::path::Path, enabled: bool) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yupana"))
        .args(["hook", "post-read", "--config"])
        .arg(config)
        .env("YUPANA_READ_LEGEND", if enabled { "1" } else { "0" })
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
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

fn payload() -> serde_json::Value {
    serde_json::json!({"session_id":"read-test", "cwd":".", "hook_event_name":"PostToolUse",
        "remaining_context_bytes":100, "tool_name":"Read", "tool_input":{"file_path":"notes.md"},
        "tool_response":{"file":{"content":"demo-service"}}})
}

#[test]
fn the_real_adapter_sends_only_to_resident_data_and_preserves_budget() {
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[yupana.serve]\nuse_daemon=true\nbind_address=\"127.0.0.1\"\nmcp_http_port={port}\n"
        ),
    )
    .unwrap();
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            if let Ok(pair) = listener.accept() {
                break pair;
            }
            assert!(
                Instant::now() < deadline,
                "hook never requested resident keywords"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut raw = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            raw.extend_from_slice(&buffer[..n]);
            if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&raw[..pos]);
                let length = header
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|l| l.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                if raw.len() >= pos + 4 + length {
                    assert!(header.starts_with("POST /keywords "));
                    let request: serde_json::Value =
                        serde_json::from_slice(&raw[pos + 4..]).unwrap();
                    assert_eq!(request["text"], "demo-service");
                    assert_eq!(request["remaining_bytes"], 100);
                    break;
                }
            }
        }
        let body = serde_json::json!({"context":"Quipu entities here: demo-service (Service)",
            "shown":["demo-service"], "raw_hits":1,"generated_at":100})
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    assert!(hook(&payload(), &config, false).is_empty());
    let mut no_budget = payload();
    no_budget
        .as_object_mut()
        .unwrap()
        .remove("remaining_context_bytes");
    assert!(hook(&no_budget, &config, true).is_empty());
    let output: serde_json::Value = serde_json::from_str(&hook(&payload(), &config, true)).unwrap();
    assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    assert_eq!(
        output["hookSpecificOutput"]["additionalContext"],
        "Quipu entities here: demo-service (Service)"
    );
    server.join().unwrap();
    assert!(
        hook(&payload(), &config, true).is_empty(),
        "down resident is advisory silence"
    );
}

#[test]
fn a_configured_remote_bind_never_receives_read_text() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        "[yupana.serve]\nuse_daemon=true\nbind_address=\"remote.example\"\n",
    )
    .unwrap();
    assert!(hook(&payload(), &config, true).is_empty());
}
