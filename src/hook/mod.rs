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
mod config_drift;
mod credential_output;
/// Asking the resident daemon for the projected policy before going live
/// (aegis-x894x2). Gated with the projection it serves.
#[cfg(feature = "quipu")]
mod daemon_projection;
mod delegate_line;
#[cfg(feature = "quipu")]
mod disk_guard;
/// The governed landing policy's decision procedure — pure, every arm testable
/// without a graph. Gated with the projection that resolves its authority.
#[cfg(feature = "quipu")]
pub mod landing_decision;
/// The governed landing policy's I/O half — evidence gathering and the hook
/// outcome. UNGATED like `memory_guard`: it carries its own no-op for a build
/// without the projection, so the pre-bash chain has one shape in every build.
mod landing_guard;
mod measure;
mod memory_guard;
pub mod paa;
mod post_bash;
mod post_edit;
mod pre_bash;
mod pre_bash_grounding;
mod pre_edit;
mod reread;
mod scope_notice;
#[cfg(feature = "quipu")]
mod session_start;

pub use post_bash::run_post_bash;
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
use sha2::Digest;

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
    /// The completed tool's result (`PostToolUse` only). Kept as JSON so the
    /// advisory can inspect strings without depending on each tool's response
    /// schema; it must never echo this value.
    #[serde(default)]
    pub tool_response: serde_json::Value,
    /// The harness's id for THIS tool call, present on both `PreToolUse` and
    /// `PostToolUse` and identical across the pair (aegis-368cu.10).
    ///
    /// This is the correlation id that makes an action's OUTCOME joinable to the
    /// action itself. It is supplied by the harness, so the two short-lived hook
    /// processes need no shared state and no minted id — and critically not a
    /// timestamp-plus-agent correlation, which is what failed in aegis-0jv06
    /// with ~20 agents running concurrently.
    ///
    /// Measured on this harness: a matched pre/post pair shares one value
    /// (e.g. `toolu_01P22bAxbYw46r3SRRpjY8DE`).
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// Which hook event this payload is for — `PostToolUse` vs
    /// `PostToolUseFailure` is how the harness REPORTS an action's outcome, so
    /// reading it is observation. Inferring success by parsing `stderr` would be
    /// the fiction this bead exists to avoid.
    #[serde(default)]
    pub hook_event_name: Option<String>,
    /// Wall time the HARNESS measured for the tool call (`PostToolUse` only).
    /// Authoritative in a way a hook's own timing cannot be: the hook runs after
    /// the call it would be timing.
    #[serde(default)]
    pub duration_ms: Option<u64>,
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

/// A configuration error is actionable on every invocation and must never be
/// hidden by advisory rate limiting.
pub(super) const CONFIG_ERROR_PREFIX: &str = "yupana: configuration error:";

/// Return an advisory only when this stable cause has not yet spoken in the
/// harness session. Configuration errors deliberately bypass the gate.
#[must_use]
pub(super) fn advisory_for_session(input_json: &str, message: String) -> Option<String> {
    if message.starts_with(CONFIG_ERROR_PREFIX) {
        return Some(message);
    }
    let session = HookInput::parse(input_json).and_then(|input| input.session_id);
    let digest = sha2::Sha256::digest(message.as_bytes());
    let cause = format!("advisory-{}", hex::encode(&digest[..12]));
    first_notice_for_session(session.as_deref(), &cause).then_some(message)
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
    let marker = marker_dir.join(marker_name(&safe, &kind_safe));
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

/// The marker file name for one (session, kind) pair.
///
/// ONE definition, because [`first_notice_for_session`] writes it and
/// [`session_event_recorded`] reads it. Two spellings of this name would make
/// the reader silently answer "no" forever while the writer kept recording —
/// a guard that is never wrong out loud.
fn marker_name(safe_session: &str, safe_kind: &str) -> String {
    format!("{MARKER_PREFIX}{safe_session}-{safe_kind}")
}

/// Sanitise a session id and a kind the same way [`first_notice_for_session`]
/// does, or `None` when the session cannot be keyed on.
fn safe_marker_parts(session: Option<&str>, kind: &str) -> Option<(String, String)> {
    let session = session?;
    let safe: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    if safe.is_empty() {
        return None;
    }
    let kind_safe: String = kind
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(80)
        .collect();
    Some((safe, kind_safe))
}

/// Whether this session has already recorded `kind`, WITHOUT recording it.
///
/// [`first_notice_for_session`] is a test-and-set: asking it whether something
/// happened makes it have happened. This is the read-only half, for a guard
/// that needs to consult an event it did not cause.
///
/// Absence answers `false`. That is deliberate and it is the safe direction
/// here: the caller advises when the event IS present, so an unreadable or
/// pruned marker degrades to silence rather than to a spurious advisory.
#[must_use]
pub fn session_event_recorded(session: Option<&str>, kind: &str) -> bool {
    let Some((safe, kind_safe)) = safe_marker_parts(session, kind) else {
        return false;
    };
    fail_open_marker_dir()
        .join(marker_name(&safe, &kind_safe))
        .exists()
}

/// Record `kind` for this session, ignoring whether it was already there.
pub fn record_session_event(session: Option<&str>, kind: &str) {
    let _ = first_notice_for_session(session, kind);
}

/// Marker file-name prefix. `HANK_MARKER_PREFIX` is the pre-rename spelling: markers
/// written by an installed `hank` outlive the rename, and a prune that only matched
/// the new prefix would leave them in the temp directory forever — the unbounded
/// state this function exists to bound.
pub(crate) const MARKER_PREFIX: &str = "yupana-guard-failopen-";
const HANK_MARKER_PREFIX: &str = "hank-guard-failopen-";

pub(crate) fn fail_open_marker_dir() -> PathBuf {
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

/// A marker-safe session id that cannot collide with a previous test RUN.
///
/// WHY THIS EXISTS (aegis-beavto). Fail-open markers live in
/// `YUPANA_FAILOPEN_MARKER_DIR`, which `.cargo/config.toml` seals to
/// `target/test-state/failopen` so tests never touch the real fleet spool
/// (aegis-0upyu). That seal is correct — and it puts test state INSIDE `target`,
/// which CI caches (`actions/cache` with `path: ... target`). So markers survive
/// from one CI run to the next inside the build cache.
///
/// A session keyed on `std::process::id()` alone is therefore not unique: CI
/// runner PIDs are low and repeat, `prune_fail_open_markers` only removes
/// markers older than 24h, and a restored cache within that window hands the
/// test binary a marker for its own session id. `first_notice_for_session` then
/// answers "already warned", the advisory returns `None`, and the test fails
/// asserting that a first cause must speak.
///
/// MEASURED: yupana PR #26, run 33964885630 — ALL TEN `Test (*)` arms failed on
/// three `credential_output` tests, and all ten passed on a re-run of the same
/// commit. The failing job's log shows `Cache restored from key:
/// Linux-cargo-test-default-...` before the suite ran.
///
/// The nanosecond stamp makes a session unique per RUN; the counter makes it
/// unique per CALL, so two sessions minted in the same tick from parallel test
/// threads cannot collide either. Both are needed: the stamp alone is not
/// guaranteed distinct across threads on a coarse clock.
#[cfg(test)]
pub(crate) fn unique_test_session(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{prefix}-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    )
}

#[cfg(test)]
#[path = "hook_test.rs"]
mod hook_test;
