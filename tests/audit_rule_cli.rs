//! Actual offline command: explicit input, exit status, and no HOME writes.
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn offline_command_reports_all_three_outcomes_without_writing_state() {
    let home = tempfile::tempdir().unwrap();
    for (source, expected) in [
        ("// TODO fix this\nfn f() {}", 1),
        ("// TODO APP-12 fix this\nfn f() {}", 0),
        ("fn broken( {", 2),
    ] {
        let input = serde_json::json!({
            "rule": {"name":"todo", "language":"rust", "query":"(line_comment) @c",
                "match_type":"must-match", "pattern":"[A-Z]+-[0-9]+", "gate":"TODO"},
            "path":"src/example.rs", "source":source
        });
        let mut child = Command::new(env!("CARGO_BIN_EXE_yupana"))
            .arg("audit-rule")
            .env("HOME", home.path())
            .env("XDG_STATE_HOME", home.path())
            .env("XDG_CACHE_HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(expected), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["rule"], "todo");
    }
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn invalid_input_is_an_unknown_json_response() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yupana"))
        .arg("audit-rule")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"not json").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["verdict"], "unknown");
}
