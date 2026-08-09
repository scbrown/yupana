//! yupana #77 — a guard record must be auditable IN PARTICULAR, not just in
//! aggregate.
//!
//! The reported failure: a host logged 18 denies in a few minutes, and the spool
//! could not say which files they were. `agent`, `tenant`, `ext`, `mode`,
//! `result`, `ts` — and no subject. So the log could neither implicate the guard
//! in a concurrent incident nor clear it, which is the difference between a
//! guard earning its keep and a guard being noise.
//!
//! These tests drive the real binary, so each case gets its OWN process and its
//! own `YUPANA_METRICS_PATH`. That is deliberate: the spool location is resolved
//! from the environment, and asserting it in-process would make these tests race
//! each other over a shared env var.

use assert_cmd::Command;

/// A project with a guard policy and a spool path, returning both temp dirs so
/// neither is dropped (and deleted) while the test still needs it.
fn guarded_project(policy: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
    for i in 0..3 {
        std::fs::write(
            dir.path().join(format!("caller{i}.rs")),
            format!("fn c{i}() {{ leaf(); }}\n"),
        )
        .unwrap();
    }
    let bobbin = dir.path().join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    // HERMETIC (aegis-0upyu): pin the graph plane OFF unless the fixture
    // declares its own. Without this the fixture inherits the developer's
    // `~/.config/bobbin/config.toml`, which on this fleet enables the graph
    // against a live endpoint — so every assertion here about the AUDIT RECORD
    // silently depended on a shared service answering within 2s. When it did
    // not, the guard failed open and `result` was `notify` instead of the
    // `deny` under test, in five tests at once.
    let policy = if policy.contains("[yupana.quipu]") {
        policy.to_string()
    } else {
        format!("{policy}\n\n[yupana.quipu]\nenabled = false\n")
    };
    std::fs::write(bobbin.join("config.toml"), policy).unwrap();
    let spool = dir.path().join("metrics.jsonl");
    (dir, spool)
}

/// The session id is unique PER CALL: the fail-open notice records "already
/// warned" in a temp-dir file keyed by (session, kind) that outlives the
/// process, so a shared id makes tests suppress each other's notices and turns
/// which-test-fails into a race (aegis-w99qp).
fn pre_edit_payload(dir: &std::path::Path, file: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // The nanosecond stamp defeats PID recycling — the markers outlive the
    // process and nothing prunes them. See `unique_session` in
    // `src/hook/pre_edit_test.rs` for the measurement.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let session = format!(
        "audit-{}-{nanos}-{}-{file}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    serde_json::json!({
        "session_id": session,
        "cwd": dir.to_str().unwrap(),
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": dir.join(file).to_str().unwrap(),
            "old_string": "fn leaf() {}",
            "new_string": "fn leaf() { changed(); }",
        },
    })
    .to_string()
}

/// Run the guard against `file` and return the `guard` line it spooled.
fn guard_record(dir: &std::path::Path, spool: &std::path::Path, file: &str) -> serde_json::Value {
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "pre-edit", "--tenant", "polecat"])
        .env("YUPANA_METRICS_PATH", spool)
        .write_stdin(pre_edit_payload(dir, file))
        .assert()
        .success();

    let spooled = std::fs::read_to_string(spool).expect("the guard wrote a spool");
    spooled
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("each line is JSON"))
        .find(|v| v["kind"] == "guard")
        .expect("a guard line was spooled")
}

const DENY_PATH_POLICY: &str = "[yupana.policy]\nmode = \"enforce\"\n\
     [yupana.policy.scopes.polecat]\nallow_paths = [\"caller*.rs\"]\n";

#[test]
fn a_deny_records_the_target_path_when_recording_is_enabled() {
    // The whole issue in one assertion: the operator asked "what was denied?",
    // and this is the field that answers.
    let (dir, spool) = guarded_project(&format!(
        "{DENY_PATH_POLICY}[yupana.metrics]\nrecord_paths = \"relative\"\n"
    ));
    let record = guard_record(dir.path(), &spool, "leaf.rs");

    assert_eq!(record["result"], "deny", "fixture must deny: {record}");
    assert_eq!(
        record["path"], "leaf.rs",
        "a deny with no subject cannot be reviewed: {record}"
    );
}

#[test]
fn a_deny_records_which_rule_fired() {
    // The field that makes a FALSE POSITIVE diagnosable. Without it a
    // wrongly-scoped rule and a correctly-scoped one produce identical records,
    // so the log cannot show that a rule is mis-scoped — only that it fired.
    let (dir, spool) = guarded_project(&format!(
        "{DENY_PATH_POLICY}[yupana.metrics]\nrecord_paths = \"relative\"\n"
    ));
    let record = guard_record(dir.path(), &spool, "leaf.rs");

    assert_eq!(
        record["rule"], "allow_paths",
        "the deny does not name the rule that produced it: {record}"
    );
}

#[test]
fn the_matching_deny_glob_is_named_not_just_the_rule_class() {
    // `deny_paths` denies name the PATTERN that matched, so an operator reading
    // the log can tell which of several globs is over-broad — the actionable
    // half of "which rule fired".
    let (dir, spool) = guarded_project(
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.polecat]\ndeny_paths = [\"leaf*.rs\"]\n\
         [yupana.metrics]\nrecord_paths = \"relative\"\n",
    );
    let record = guard_record(dir.path(), &spool, "leaf.rs");

    assert_eq!(record["result"], "deny", "{record}");
    assert_eq!(record["rule"], "deny_paths:leaf*.rs", "{record}");
}

#[test]
fn recording_is_off_by_default_and_the_record_is_unchanged() {
    // Paths are more sensitive than extensions, so this is opt-IN. A deployment
    // that never sets the knob must get exactly the pre-#77 record — the field
    // ABSENT, not empty, so a reader never has to tell "recorded as blank" from
    // "not recorded".
    let (dir, spool) = guarded_project(DENY_PATH_POLICY);
    let record = guard_record(dir.path(), &spool, "leaf.rs");

    assert_eq!(record["result"], "deny", "{record}");
    assert!(
        record.get("path").is_none(),
        "path recording must be opt-in, never on beneath a deployment: {record}"
    );
    // The rule id is NOT gated with the path: it is a name the operator wrote,
    // not user content, and it carries none of the sensitivity that argues for
    // gating paths.
    assert_eq!(record["rule"], "allow_paths", "{record}");
}

#[test]
fn absolute_recording_yields_a_path_that_locates_the_file_on_the_host() {
    let (dir, spool) = guarded_project(&format!(
        "{DENY_PATH_POLICY}[yupana.metrics]\nrecord_paths = \"absolute\"\n"
    ));
    let record = guard_record(dir.path(), &spool, "leaf.rs");

    let path = record["path"].as_str().unwrap_or_default();
    assert!(
        std::path::Path::new(path).is_absolute() && path.ends_with("leaf.rs"),
        "absolute recording did not yield an absolute path: {record}"
    );
}

#[test]
fn an_allow_records_its_path_too_so_scope_can_be_verified_not_inferred() {
    // Symmetry, on purpose (the issue's fourth ask). Scope that can only be
    // inferred from the ABSENCE of denies cannot be verified at all: an operator
    // confirming a rule is scoped correctly needs to see what it let through.
    let (dir, spool) = guarded_project(
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 30000\n\
         [yupana.policy.scopes.polecat]\nmax_impacted_files = 10\n\
         [yupana.metrics]\nrecord_paths = \"relative\"\n",
    );
    let record = guard_record(dir.path(), &spool, "leaf.rs");

    assert_eq!(record["result"], "allow", "fixture must allow: {record}");
    assert_eq!(record["path"], "leaf.rs", "{record}");
    // Nothing fired, so there is no rule to name — absent, not empty-string.
    assert!(
        record.get("rule").is_none(),
        "a clean allow must not invent a deciding rule: {record}"
    );
}

#[test]
fn a_blast_radius_deny_names_the_ceiling_it_exceeded() {
    let (dir, spool) = guarded_project(
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 30000\n\
         [yupana.policy.scopes.polecat]\nmax_impacted_files = 1\n\
         [yupana.metrics]\nrecord_paths = \"relative\"\n",
    );
    let record = guard_record(dir.path(), &spool, "leaf.rs");

    assert_eq!(record["result"], "deny", "{record}");
    assert_eq!(
        record["rule"], "max_impacted_files",
        "a ceiling deny must name the ceiling, so the operator knows which to \
         reconsider: {record}"
    );
}
