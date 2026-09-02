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
        ("openai", r"(?:sk-[A-Za-z0-9_-]{20,})"),
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
        let session = format!("credential-output-same-{}", std::process::id());
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
        let session = format!("credential-output-change-{}", std::process::id());
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
}
