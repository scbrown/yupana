//! Actual CLI child environment and rollback contract, without live services.
#![cfg(all(feature = "quipu", unix))]
use assert_cmd::Command;
use std::os::unix::fs::PermissionsExt;

#[test]
fn pipeline_preserves_signpost_and_switches_only_child_legend() {
    let scratch = tempfile::tempdir().unwrap();
    let stub = scratch.path().join("dp");
    std::fs::write(
        &stub,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s' \"$DP_LEGEND\" > legend-flag\nprintf '%s' '{\"continue\":true,\"hookSpecificOutput\":{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"signpost\"}}'\n",
    ).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
    let expected = br#"{"continue":true,"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"signpost"}}"#;
    for enabled in ["0", "1"] {
        let mut command = Command::cargo_bin("yupana").unwrap();
        command.current_dir(scratch.path())
            .args(["hook", "post-read-pipeline"])
            .env("YUPANA_DP_BIN", &stub)
            .env("DP_LEGEND", "1")
            .env("YUPANA_READ_LEGEND", enabled)
            // Full allocation: no resident request or second injector.
            .env("DP_LEGEND_MAX_BYTES", "8")
            .write_stdin(r#"{"hook_event_name":"PostToolUse","session_id":"isolated","tool_name":"Read","tool_input":{"file_path":"notes.md"},"tool_response":"example"}"#)
            .assert().success().stdout(expected.to_vec());
        assert_eq!(
            std::fs::read_to_string(scratch.path().join("legend-flag")).unwrap(),
            if enabled == "1" { "0" } else { "1" }
        );
    }
}
