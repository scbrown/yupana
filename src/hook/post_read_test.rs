use super::*;

fn payload() -> Value {
    serde_json::json!({"session_id":"s", "hook_event_name":"PostToolUse", "tool_name":"Read",
        "remaining_context_bytes":600, "tool_input":{"file_path":"notes.md"},
        "tool_response":{"file":{"content":"build-01.example"}}})
}

#[test]
fn budget_is_required_and_failures_never_annotate() {
    let mut p = payload();
    assert_eq!(request(&p).unwrap().text, "build-01.example");
    p.as_object_mut().unwrap().remove("remaining_context_bytes");
    assert!(request(&p).is_none());
    p["remaining_context_bytes"] = 0.into();
    assert!(request(&p).is_none());
    p["remaining_context_bytes"] = 9_999.into();
    assert_eq!(request(&p).unwrap().remaining_bytes, 600);
    p["hook_event_name"] = "PostToolUseFailure".into();
    assert!(request(&p).is_none());
    p = payload();
    p["tool_response"]["interrupted"] = true.into();
    assert!(request(&p).is_none());
    p = payload();
    p["tool_response"]["isError"] = true.into();
    assert!(request(&p).is_none());
}

#[test]
fn bash_reads_handle_flags_and_quoted_files_but_not_searches() {
    for (command, reference) in [
        ("br --db /x/beads.db show issue-42", "issue-42"),
        ("br show issue-42", "issue-42"),
        ("cat 'docs/one two.md'", "docs/one two.md"),
        ("cd /tmp && head -5 a.md", "a.md"),
        ("sed -n 1,40p notes.md", "notes.md"),
    ] {
        assert_eq!(bash_reference(command).as_deref(), Some(reference));
    }
    for command in [
        "rg build-01 .",
        "go test ./...",
        "cat src.rs",
        "echo 'cat file.md'",
    ] {
        assert!(bash_reference(command).is_none(), "{command}");
    }
    let mut p = payload();
    p["tool_name"] = "Bash".into();
    p["tool_input"] = serde_json::json!({"command":"br show issue-42"});
    p["tool_response"] = serde_json::json!({"stdout":"build-01.example"});
    assert_eq!(request(&p).unwrap().reference, "issue-42");
}

#[test]
fn missing_session_and_large_response_are_silent() {
    let mut p = payload();
    p["session_id"] = "".into();
    assert!(request(&p).is_none());
    p = payload();
    p["tool_response"] = "x".repeat(128 * 1024 + 1).into();
    assert!(request(&p).is_none());
}
