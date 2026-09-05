//! Tests for the hook module's shared envelope and fail-open session markers.
//! Size-exempt (`_test.rs`), the same split `pre_edit`/`pre_edit_test` already uses.

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn parses_an_edit_payload() {
        let payload = serde_json::json!({
            "session_id": "s1",
            "cwd": "/repo",
            "tool_name": "Edit",
            "tool_input": { "file_path": "/repo/a.rs", "old_string": "fn a", "new_string": "fn b" },
        })
        .to_string();
        let input = HookInput::parse(&payload).unwrap();
        assert_eq!(input.tool_name.as_deref(), Some("Edit"));
        assert_eq!(input.replaced_texts(), vec!["fn a"]);
    }

    #[test]
    fn parses_a_multiedit_payload() {
        let payload = serde_json::json!({
            "tool_name": "MultiEdit",
            "tool_input": { "file_path": "/repo/a.rs", "edits": [
                { "old_string": "one", "new_string": "1" },
                { "old_string": "two", "new_string": "2" },
            ]},
        })
        .to_string();
        let input = HookInput::parse(&payload).unwrap();
        assert_eq!(input.replaced_texts(), vec!["one", "two"]);
    }

    #[test]
    fn a_write_has_no_replaced_text() {
        let payload = serde_json::json!({
            "tool_name": "Write",
            "tool_input": { "file_path": "/repo/a.rs", "content": "fn a() {}" },
        })
        .to_string();
        let input = HookInput::parse(&payload).unwrap();
        assert!(input.replaced_texts().is_empty());
        assert_eq!(input.tool_input.content.as_deref(), Some("fn a() {}"));
    }

    #[test]
    fn unknown_fields_and_missing_fields_are_tolerated() {
        // Forward compatibility: a harness that grows a field must not break us.
        let payload = serde_json::json!({ "brand_new_field": 42, "tool_input": {} }).to_string();
        let input = HookInput::parse(&payload).unwrap();
        assert!(input.tool_input.file_path.is_none());
        assert!(HookInput::parse("not json").is_none());
    }

    #[test]
    fn deny_envelope_matches_the_documented_protocol() {
        let value: serde_json::Value = serde_json::from_str(&deny_envelope("too big")).unwrap();
        let out = &value["hookSpecificOutput"];
        assert_eq!(out["hookEventName"], "PreToolUse");
        assert_eq!(out["permissionDecision"], "deny");
        assert_eq!(out["permissionDecisionReason"], "too big");
    }

    #[test]
    fn system_message_carries_no_permission_decision() {
        // Critical: a notice must not disturb the harness's permission flow.
        let value: serde_json::Value = serde_json::from_str(&system_message("heads up")).unwrap();
        assert_eq!(value["systemMessage"], "heads up");
        assert!(value.get("hookSpecificOutput").is_none());
    }

    #[test]
    fn advisory_delivery_dedupes_stable_causes_but_not_config_errors() {
        let session = unique_test_session("advisory-delivery");
        let payload = serde_json::json!({"session_id": session}).to_string();

        assert_eq!(
            advisory_for_session(&payload, "same advisory".into()).as_deref(),
            Some("same advisory")
        );
        assert_eq!(advisory_for_session(&payload, "same advisory".into()), None);
        assert_eq!(
            advisory_for_session(&payload, "changed cause".into()).as_deref(),
            Some("changed cause")
        );

        let config = format!("{CONFIG_ERROR_PREFIX} missing binary");
        assert_eq!(
            advisory_for_session(&payload, config.clone()),
            Some(config.clone())
        );
        assert_eq!(advisory_for_session(&payload, config.clone()), Some(config));
    }

    #[test]
    fn fail_open_notice_fires_once_per_session() {
        let session = unique_test_session("test");
        assert!(first_notice_for_session(Some(&session), "config"));
        assert!(!first_notice_for_session(Some(&session), "config"));
        // A DIFFERENT kind of gap in the same session must still warn — the whole
        // point of keying on kind. Before, this returned false and the second gap
        // went silent.
        assert!(first_notice_for_session(
            Some(&session),
            "deadline-src/a.rs"
        ));
        assert!(!first_notice_for_session(
            Some(&session),
            "deadline-src/a.rs"
        ));
        // ... and a deadline in a DIFFERENT file is a different gap again.
        assert!(first_notice_for_session(
            Some(&session),
            "deadline-src/b.rs"
        ));
        for kind in ["config", "deadline-src/a.rs", "deadline-src/b.rs"] {
            let safe_kind: String = kind
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(80)
                .collect();
            let _ = std::fs::remove_file(
                fail_open_marker_dir().join(format!("{MARKER_PREFIX}{session}-{safe_kind}")),
            );
        }
        // Without a session id we cannot rate-limit, so we always warn.
        assert!(first_notice_for_session(None, "config"));
    }

    #[test]
    fn stale_fail_open_markers_are_pruned_but_fresh_markers_remain() {
        use std::fs::FileTimes;

        let dir = fail_open_marker_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let nonce = format!("{}-{:?}", std::process::id(), SystemTime::now());
        let stale = dir.join(format!("{MARKER_PREFIX}stale-{nonce}"));
        let fresh = dir.join(format!("{MARKER_PREFIX}fresh-{nonce}"));
        // A marker left behind by the pre-rename `hank` binary must be pruned too,
        // otherwise the rename quietly makes this state unbounded again.
        let legacy = dir.join(format!("{HANK_MARKER_PREFIX}stale-{nonce}"));
        let stale_file = std::fs::File::create(&stale).unwrap();
        let legacy_file = std::fs::File::create(&legacy).unwrap();
        std::fs::File::create(&fresh).unwrap();
        let old =
            FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(25 * 60 * 60));
        stale_file.set_times(old).unwrap();
        legacy_file.set_times(old).unwrap();

        prune_fail_open_markers(&dir, SystemTime::now());

        assert!(!stale.exists());
        assert!(
            !legacy.exists(),
            "a pre-rename hank marker survived the prune"
        );
        assert!(fresh.exists());
        let _ = std::fs::remove_file(fresh);
    }

    #[test]
    fn cargo_tests_use_the_sealed_fail_open_marker_directory() {
        let dir = fail_open_marker_dir();
        assert!(
            dir.ends_with("target/test-state/failopen"),
            "{}",
            dir.display()
        );
    }
}
