//! Credential-shaped `PostToolUse` output advisory.
//!
//! This is separate from `post_edit`: output hygiene applies to every tool,
//! while blast-radius analysis applies only to edit-shaped calls. The module
//! never returns matched bytes, only stable shape-class names.

use regex::Regex;

use super::HookInput;

/// Warn when a completed tool returned credential-shaped material.
///
/// The stable cause is the sorted set of shape classes, never the matched
/// bytes. That makes identical risk speak once per harness session while a new
/// class speaks again. No-session and marker-write failures warn rather than
/// silently losing a security advisory.
pub(super) fn advisory(input_json: &str) -> Option<String> {
    let input = HookInput::parse(input_json)?;
    if input.tool_response.is_null() {
        return None;
    }
    let response = serde_json::to_string(&input.tool_response).ok()?;
    let patterns = [
        ("bearer", r"(?i)bearer[[:space:]]+[A-Za-z0-9._~+/=-]{16,}"),
        ("github", r"(?:gh[op]_[A-Za-z0-9]{20,})"),
        // Require a token boundary. Without it, the `sk-` spanning the end of
        // an ordinary word such as `disk-impact-<digest>` is misclassified as
        // an OpenAI key and every reader of governed disk observations warns.
        ("openai", r"(?:^|[^A-Za-z0-9])sk-[A-Za-z0-9_-]{20,}"),
        ("aws-access-key", r"(?:AKIA[0-9A-Z]{16})"),
    ];
    let mut classes = Vec::new();
    for (class, pattern) in patterns {
        if Regex::new(pattern).ok()?.is_match(&response) {
            classes.push(class);
        }
    }
    if classes.is_empty() {
        return None;
    }
    let cause = format!("credential-output-{}", classes.join("-"));
    if !super::first_notice_for_session(input.session_id.as_deref(), &cause) {
        return None;
    }
    Some(format!(
        "yupana security advisory (once per session for this stable cause): tool output contained credential-shaped material ({classes}). Do not quote, paste, index, or share the raw output; use a scrubbed derivative and rotate any credential confirmed as yours.",
        classes = classes.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(session: &str, output: &str) -> String {
        serde_json::json!({
            "session_id": session,
            "tool_name": "Bash",
            "tool_input": {"command": "synthetic"},
            "tool_response": {"stdout": output},
        })
        .to_string()
    }

    #[test]
    fn advises_once_for_an_unchanged_cause() {
        let session = crate::hook::unique_test_session("credential-output-same");
        let input = payload(
            &session,
            "Authorization: Bearer synthetic_token_0123456789abcdef",
        );
        let first = advisory(&input).expect("first cause must advise");
        assert!(first.contains("bearer"));
        assert!(!first.contains("synthetic_token"), "must never echo match");
        assert_eq!(advisory(&input), None);
    }

    #[test]
    fn re_advises_when_the_stable_cause_changes() {
        let session = crate::hook::unique_test_session("credential-output-change");
        let bearer = payload(
            &session,
            "Authorization: Bearer synthetic_token_0123456789abcdef",
        );
        let github = payload(&session, "ghp_0123456789abcdefghijklmnop");
        assert!(advisory(&bearer).is_some());
        let changed = advisory(&github).expect("new class must advise again");
        assert!(changed.contains("github"));
        assert!(!changed.contains("ghp_"), "must never echo match");
    }

    #[test]
    fn ordinary_output_and_malformed_payload_stay_silent() {
        assert_eq!(advisory("not json"), None);
        assert_eq!(advisory(&payload("credential-output-clean", "done")), None);
    }

    #[test]
    fn governed_disk_identifiers_are_not_openai_keys() {
        let digest = "a23ca75c58a36372b73bbbd15937be7c77c814fc61d2e274339f8ed7ac0c4506";
        let output = format!(
            "http://example.invalid/ontology/disk-impact-{digest} filesystemIdentity root:20cdc54d46e1706afbe62f91"
        );
        assert_eq!(
            advisory(&payload("credential-output-disk-identifiers", &output)),
            None
        );
    }

    #[test]
    fn openai_key_shape_at_a_boundary_still_advises() {
        let session = crate::hook::unique_test_session("credential-output-openai-boundary");
        let input = payload(&session, "value=sk-synthetic0123456789abcdef");
        let warning = advisory(&input).expect("boundary-delimited key shape must advise");
        assert!(warning.contains("openai"));
        assert!(!warning.contains("sk-synthetic"), "must never echo match");
    }

    /// The exact CI failure this file suffered (aegis-beavto), reproduced.
    ///
    /// A marker for the OLD session shape — prefix plus bare `std::process::id()`
    /// — is planted first, standing in for one restored from the CI build cache
    /// by a previous run that happened to get this PID. The advisory must still
    /// fire, because the session a test mints now carries a per-run nonce.
    ///
    /// This fails on the pre-fix code and passes after, which is the only reason
    /// it is worth having: it asserts the ISOLATION property, not the advisory's
    /// behaviour, which the three tests above already cover.
    #[test]
    fn a_marker_cached_from_a_previous_run_does_not_silence_the_advisory() {
        // Plant it exactly as first_notice_for_session would have written it.
        let stale_session = format!("credential-output-same-{}", std::process::id());
        let dir = crate::hook::fail_open_marker_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let planted = dir.join(format!(
            "{}{stale_session}-credential-output-bearer",
            crate::hook::MARKER_PREFIX
        ));
        std::fs::write(&planted, b"").unwrap();

        let input = payload(
            &crate::hook::unique_test_session("credential-output-same"),
            "Authorization: Bearer synthetic_token_0123456789abcdef",
        );
        let spoke = advisory(&input);

        let _ = std::fs::remove_file(&planted);
        assert!(
            spoke.is_some(),
            "a marker left by a previous run silenced a first-cause advisory — \
             the session id is not unique per run (aegis-beavto)"
        );
    }
}
