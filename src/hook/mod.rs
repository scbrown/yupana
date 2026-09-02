//! Harness hook adapters — the edit-reactive interface (§5.9/FR-30).
//!
//! An agent harness (Claude Code) fires a hook on every edit; the edit tool call
//! *is* the `didChange` event, so Yupana's response is automatic — the agent never
//! has to remember to call a tool. Two adapters share the payload types here:
//!
//! - [`post_edit`] (`PostToolUse`) — after the edit lands, report the cross-file
//!   blast radius as injected context. **Advisory, always.**
//! - [`pre_edit`] (`PreToolUse`) — before the edit lands, check it against the
//!   tenant's capability scope and optionally **deny** it (§5.8/FR-25).
//!   Opt-in; off by default.
//!
//! Both are thin, harness-specific translation layers; the engine and its facts
//! stay harness-agnostic.
//!
//! ## The one rule
//!
//! **A hook must never fail the harness.** The full contract lives in
//! `docs/book/src/reference/policy-guard.md`; the parts this module enforces:
//! allow is *silence* (exit 0, empty stdout) and Yupana never exits `2`, which is
//! Claude Code's fail-*closed* channel. Reserving exit `2` means even a panic
//! (exit 101, a non-blocking error to the harness) lets the edit through.

#[cfg(feature = "quipu")]
mod disk_guard;
mod measure;
mod memory_guard;
pub mod paa;
mod post_edit;
mod pre_bash;
mod pre_bash_grounding;
mod pre_edit;
mod scope_notice;
#[cfg(feature = "quipu")]
mod session_start;

pub use post_edit::{advisory_for, run_post_edit};
pub use pre_bash::run_pre_bash;
pub use pre_edit::{run_pre_edit, Outcome};
#[cfg(feature = "quipu")]
pub use session_start::run_session_start;
// The resident-graph measurement path (FR-31): the daemon measures an edit against
// its resident graph via `measure_with_graph`, returning the same `Sizing` the
// transient path does. Crate-visible so `crate::daemon` can call it.
pub(crate) use measure::{measure_with_graph, Sizing};

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

/// The subset of a harness hook payload Yupana needs.
///
/// Every field is optional: a payload Yupana cannot fully parse must degrade to
/// "nothing to say", never to an error.
#[derive(Debug, Default, Deserialize)]
pub struct HookInput {
    /// The harness session, used to rate-limit fail-open notices.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The agent's working directory — the analysis root.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The tool being invoked (`Edit`, `Write`, `MultiEdit`).
    #[serde(default)]
    pub tool_name: Option<String>,
    /// The tool's arguments.
    #[serde(default)]
    pub tool_input: ToolInput,
    /// Turn-boundary grounding reference injected by the harness for a
    /// NeuralAmplifier-scoped action.
    #[serde(default)]
    pub grounding: Option<crate::turn_grounding::GroundingRef>,
}

/// The tool arguments Yupana reads, across `Edit` / `Write` / `MultiEdit`.
#[derive(Debug, Default, Deserialize)]
pub struct ToolInput {
    /// Target file (all three tools).
    #[serde(default)]
    pub file_path: Option<String>,
    /// Text being replaced (`Edit`).
    #[serde(default)]
    pub old_string: Option<String>,
    /// Replacement text (`Edit`).
    #[serde(default)]
    pub new_string: Option<String>,
    /// Full proposed file contents (`Write`).
    #[serde(default)]
    pub content: Option<String>,
    /// The individual edits (`MultiEdit`).
    #[serde(default)]
    pub edits: Vec<EditItem>,
}

/// One edit within a `MultiEdit` call.
#[derive(Debug, Default, Deserialize)]
pub struct EditItem {
    /// Text being replaced.
    #[serde(default)]
    pub old_string: Option<String>,
    /// Replacement text.
    #[serde(default)]
    pub new_string: Option<String>,
}

impl HookInput {
    /// Parse a payload, or `None` if it is not JSON Yupana understands.
    #[must_use]
    pub fn parse(input_json: &str) -> Option<Self> {
        serde_json::from_str(input_json).ok()
    }

    /// The analysis root: the payload's `cwd`, else `default_root`.
    #[must_use]
    pub fn root(&self, default_root: &std::path::Path) -> PathBuf {
        self.cwd
            .as_ref()
            .map_or_else(|| default_root.to_path_buf(), PathBuf::from)
    }

    /// The anchor texts this edit replaces — used to locate the change within
    /// the current file. Empty for a `Write` (which replaces everything).
    #[must_use]
    pub fn replaced_texts(&self) -> Vec<&str> {
        let mut texts: Vec<&str> = Vec::new();
        if let Some(old) = self.tool_input.old_string.as_deref() {
            texts.push(old);
        }
        for edit in &self.tool_input.edits {
            if let Some(old) = edit.old_string.as_deref() {
                texts.push(old);
            }
        }
        texts
    }
}

/// The `PreToolUse` deny envelope: exit 0 and print this to block the edit.
#[must_use]
pub fn deny_envelope(reason: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

/// A user-visible notice that leaves the tool call untouched.
///
/// Carries no `hookSpecificOutput`, so the harness's normal permission flow runs
/// exactly as it would have. This is how the guard is *loud* when it fails open:
/// a hook's stderr is surfaced only on exit `2`, so stderr alone would be
/// invisible in practice.
#[must_use]
pub fn system_message(message: &str) -> String {
    serde_json::json!({ "systemMessage": message }).to_string()
}

/// Whether this process has already emitted a fail-open notice for `session`.
///
/// Records the notice as a marker file in `YUPANA_FAILOPEN_MARKER_DIR`, or the
/// system temp directory by default. The file is created atomically
/// (`create_new`), so the warning fires once per session instead of
/// on every edit — a per-edit warning about a down daemon just trains everyone
/// to ignore it. Markers older than one day are pruned before a new marker is
/// written, bounding production state and preventing stale session-id collisions.
/// When no session id is available, or the marker cannot be written, the notice
/// is allowed through: over-warning beats silence.
#[must_use]
pub fn first_notice_for_session(session: Option<&str>, kind: &str) -> bool {
    let Some(session) = session else {
        return true;
    };
    // The id comes from the harness; keep only characters safe in a file name.
    let safe: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    if safe.is_empty() {
        return true;
    }
    // KEYED ON THE KIND OF GAP, not the session alone. With one marker per session
    // the FIRST fail-open of any kind silenced every later, DIFFERENT one: an
    // unreadable config in one repo would consume the marker, and a blown blast-radius
    // deadline in another repo in the same session then said nothing — the mechanism
    // whose whole job is making gaps visible, suppressing a gap it had never reported.
    let kind_safe: String = kind
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(80)
        .collect();
    let marker_dir = fail_open_marker_dir();
    if std::fs::create_dir_all(&marker_dir).is_err() {
        return true;
    }
    prune_fail_open_markers(&marker_dir, SystemTime::now());
    let marker = marker_dir.join(format!("{MARKER_PREFIX}{safe}-{kind_safe}"));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(_) => true,
        // Already exists => already warned. Any other error => warn anyway.
        Err(e) => e.kind() != std::io::ErrorKind::AlreadyExists,
    }
}

/// Marker file-name prefix. `HANK_MARKER_PREFIX` is the pre-rename spelling: markers
/// written by an installed `hank` outlive the rename, and a prune that only matched
/// the new prefix would leave them in the temp directory forever — the unbounded
/// state this function exists to bound.
const MARKER_PREFIX: &str = "yupana-guard-failopen-";
const HANK_MARKER_PREFIX: &str = "hank-guard-failopen-";

fn fail_open_marker_dir() -> PathBuf {
    std::env::var_os("YUPANA_FAILOPEN_MARKER_DIR").map_or_else(std::env::temp_dir, PathBuf::from)
}

fn prune_fail_open_markers(dir: &Path, now: SystemTime) {
    const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_marker = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(MARKER_PREFIX) || name.starts_with(HANK_MARKER_PREFIX)
            });
        if !is_marker {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > MAX_AGE);
        if stale {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn fail_open_notice_fires_once_per_session() {
        let session = format!("test-{}", std::process::id());
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
