//! Tests for `pre_edit` — the fail-open contract, capability scoping, the rule
//! planes, and the audit record. Child module of `pre_edit` (`super::*` reaches
//! its private `guard_inner`/`Decision`); size-exempt (`_test.rs`).

//! Test names here shout the invariant they pin — `is_NEVER_observable`,
//! `daemon_EXPECTED_but_DOWN`, `is_DOWN_not_UP`. That capitalisation is the
//! same emphasis the prose uses throughout this repo, and it is load-bearing in
//! a test name: it says which word the assertion turns on. Allowed explicitly,
//! and scoped to tests, so the lint stays live everywhere else rather than
//! being switched off crate-wide (yupana #83).
#![allow(non_snake_case)]
use super::*;

/// A repo where `leaf` is called from three other files — HERMETIC BY
/// CONSTRUCTION (aegis-enbzz, completed by aegis-0upyu).
///
/// f1ca99a made [`write_policy`] hermetic, which covered every fixture that
/// declares a policy. It could not cover the fixtures that declare NO config at
/// all: those still fell through to the host-level `~/.config/bobbin/config.toml`,
/// which on a crew machine carries `[yupana.quipu] enabled = true` pointed at the
/// LIVE endpoint. Two tests were still reaching quipu over the network —
/// `allows_when_no_policy_is_configured` and `a_config_override_scopes_the_guard`
/// — and both FAIL on an unmodified tree whenever quipu is slow, for a reason
/// that has nothing to do with what they assert (measured 2026-08-04: the guard
/// timed out at 2s, failed open, and returned `Notify` where the test expected
/// `Allow`).
///
/// Writing the hermetic stanza HERE rather than in each fixture is the point: a
/// rule that says "remember to disable quipu in your fixture" is one every new
/// test can forget, and the failure it produces is a timeout in an unrelated
/// assertion. Every fixture built on this repo is now sealed by construction,
/// and `write_policy` overwrites the file when a test declares its own policy.
fn wide_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
    for (i, name) in ["one", "two", "three"].iter().enumerate() {
        std::fs::write(
            dir.path().join(format!("caller{i}.rs")),
            format!("fn {name}() {{ leaf(); }}\n"),
        )
        .unwrap();
    }
    // No policy — the ambient config still allows everything, which is what the
    // fixtures relying on an "absent" config actually depend on. The only thing
    // pinned is that this repo never inherits the host's live quipu.
    let bobbin = dir.path().join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    std::fs::write(
        bobbin.join("config.toml"),
        "[yupana.quipu]\nenabled = false\n",
    )
    .unwrap();
    dir
}

/// Write a fixture policy — HERMETIC BY DEFAULT (aegis-enbzz).
///
/// yupana's config discovery falls back to a HOST-level `~/.config/bobbin/config.toml`,
/// which on a crew machine carries a LIVE quipu endpoint. A fixture that said
/// nothing about quipu therefore inherited it and made real network calls. Under
/// cargo's parallelism those contend on quipu's effectively-serialised `/query`
/// and time out, the guard fails open, and the assertion under test — about blast
/// radius, path scope or rule matching, none of which involve quipu — fails for a
/// reason that has nothing to do with what it tests.
///
/// Measured 2026-08-04 on an unmodified tree: 36/36 pass with `--test-threads=1`
/// (twice), while parallel runs failed 8, 9, 12, 14 and 18 tests across runs, and
/// EVERY failure carried `could not project governed policy from quipu … timed out
/// reading response`. The flakiness was never in the guard logic.
///
/// A fixture that reaches the network is not a fixture: it imports the load of
/// every other agent on the host into a unit test. Tests that genuinely exercise
/// projection declare `[yupana.quipu]` themselves and are left untouched — see
/// `an_unreachable_quipu_projection_fails_open_loudly`, which pins its own
/// endpoint at `127.0.0.1:1` precisely so it does not need a live server.
fn write_policy(dir: &Path, body: &str) {
    let bobbin = dir.join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    let body = if body.contains("[yupana.quipu]") {
        body.to_string()
    } else {
        format!("{body}\n\n[yupana.quipu]\nenabled = false\n")
    };
    std::fs::write(bobbin.join("config.toml"), body).unwrap();
}

/// A session id unique to this call, so the once-per-session fail-open
/// marker (a file in the temp dir, which outlives the test process) cannot
/// leak state between tests or between `cargo test` runs.
///
/// The nanosecond stamp is load-bearing, not decoration. This was `{pid}-{counter}`,
/// and the "or between `cargo test` runs" half of that promise was FALSE: PIDs
/// recycle, the marker files do not, and 1,335 `yupana-guard-failopen-test-*` markers
/// had accumulated in `/tmp` on this host (oldest three days old, nothing prunes
/// them). A run that drew a recycled PID found its marker already present, the
/// notice was suppressed as "already warned", and
/// `an_unreachable_quipu_projection_fails_open_loudly` — whose whole assertion is
/// that the notice FIRES — failed. Measured 2026-08-04 (aegis-w99qp): it failed
/// once and passed on the next invocation of the identical command.
///
/// That is the same defect class the hermetic-fixture work above cures — a
/// verdict that depends on state outside the test — but reached via `/tmp`
/// rather than via a socket, so NO amount of config pinning touches it. It is
/// also outside the `.cargo/config.toml` `[env]` seal, which pins
/// `YUPANA_METRICS_PATH` / `YUPANA_VERDICT_PATH` / `YUPANA_PROJECTION_CACHE_PATH`:
/// this marker resolves through `std::env::temp_dir()`, which has no override.
fn unique_session() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "test-{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn edit_payload(dir: &Path, file: &str, old: &str) -> String {
    serde_json::json!({
        "session_id": unique_session(),
        "cwd": dir.to_str().unwrap(),
        "tool_name": "Edit",
        "tool_input": {
            "file_path": dir.join(file).to_str().unwrap(),
            "old_string": old,
            "new_string": "fn leaf() { changed(); }",
        },
    })
    .to_string()
}

#[test]
fn allows_when_no_policy_is_configured() {
    let dir = wide_repo();
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    assert_eq!(guard(&payload, dir.path(), Some("t"), None), Outcome::Allow);
}

#[test]
fn allows_when_mode_is_off_despite_a_scope() {
    let dir = wide_repo();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"off\"\n[yupana.policy.scopes.t]\nmax_impacted_symbols = 0\n",
    );
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    assert_eq!(guard(&payload, dir.path(), Some("t"), None), Outcome::Allow);
}

#[test]
fn denies_an_out_of_scope_path() {
    let dir = wide_repo();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.t]\nallow_paths = [\"caller*.rs\"]\n",
    );
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    let Outcome::Deny(reason) = guard(&payload, dir.path(), Some("t"), None) else {
        panic!("expected a deny");
    };
    assert!(reason.contains("leaf.rs"));
    assert!(reason.contains("outside the writable capability scope"));
}

#[test]
fn denies_an_edit_that_exceeds_the_blast_radius() {
    let dir = wide_repo();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 30000\n\
         [yupana.policy.scopes.t]\nmax_impacted_files = 1\n",
    );
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    let Outcome::Deny(reason) = guard(&payload, dir.path(), Some("t"), None) else {
        panic!("expected a deny");
    };
    // leaf is called from three files; the ceiling is one.
    assert!(reason.contains("3 files (ceiling 1)"), "got: {reason}");
}

#[test]
fn allows_an_edit_within_the_blast_radius() {
    let dir = wide_repo();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 30000\n\
         [yupana.policy.scopes.t]\nmax_impacted_files = 10\n",
    );
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    assert_eq!(guard(&payload, dir.path(), Some("t"), None), Outcome::Allow);
}

#[test]
fn advise_mode_never_denies() {
    let dir = wide_repo();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"advise\"\ndeadline_ms = 30000\n\
         [yupana.policy.scopes.t]\nmax_impacted_files = 1\n",
    );
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    let Outcome::Notify(message) = guard(&payload, dir.path(), Some("t"), None) else {
        panic!("expected an advisory, not a block");
    };
    assert!(message.contains("not blocking"));
}

#[test]
fn an_untouched_tenant_is_unconstrained() {
    let dir = wide_repo();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.other]\nallow_paths = [\"nothing/**\"]\n",
    );
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    assert_eq!(guard(&payload, dir.path(), Some("t"), None), Outcome::Allow);
    // ...and so is an agent with no tenant at all.
    assert_eq!(guard(&payload, dir.path(), None, None), Outcome::Allow);
}

#[test]
fn a_blown_deadline_allows_the_edit_and_says_so() {
    let dir = wide_repo();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 0\n\
         [yupana.policy.scopes.t]\nmax_impacted_files = 1\n",
    );
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    // The same edit is denied with a real budget (see the test above), so
    // this proves the deadline — not the policy — is what let it through.
    // CONTRACT CHANGE: it still ALLOWS (fail-open is deliberate), but it no
    // longer allows in SILENCE. This test previously asserted Outcome::Allow,
    // which is the same value a clean edit produces — so the suite could not
    // tell "we did not look" from "we looked and it was fine", and neither
    // could an operator.
    match guard(&payload, dir.path(), Some("t"), None) {
        Outcome::Notify(message) => {
            assert!(message.contains("NOT EVALUATED"), "{message}");
            assert!(message.contains("deadline_ms"), "{message}");
        }
        other => panic!("a blown deadline must be reported, got {other:?}"),
    }
}

#[test]
fn a_path_check_still_applies_under_a_zero_deadline() {
    let dir = wide_repo();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 0\n\
         [yupana.policy.scopes.t]\nallow_paths = [\"caller*.rs\"]\n",
    );
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    assert!(matches!(
        guard(&payload, dir.path(), Some("t"), None),
        Outcome::Deny(_)
    ));
}

#[test]
fn garbage_and_unknown_payloads_allow() {
    let dir = wide_repo();
    assert_eq!(
        guard("not json", dir.path(), Some("t"), None),
        Outcome::Allow
    );
    let no_file = serde_json::json!({ "tool_input": {} }).to_string();
    assert_eq!(guard(&no_file, dir.path(), Some("t"), None), Outcome::Allow);
}

#[test]
fn a_malformed_glob_fails_open_loudly() {
    let dir = wide_repo();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.t]\nallow_paths = [\"src/[\"]\n",
    );
    let session = unique_session();
    let payload = serde_json::json!({
        "session_id": session,
        "cwd": dir.path().to_str().unwrap(),
        "tool_name": "Edit",
        "tool_input": {
            "file_path": dir.path().join("leaf.rs").to_str().unwrap(),
            "old_string": "fn leaf() {}",
        },
    })
    .to_string();

    let Outcome::Notify(message) = guard(&payload, dir.path(), Some("t"), None) else {
        panic!("expected a fail-open notice, not a block");
    };
    assert!(message.contains("UNGUARDED"));
    assert!(message.contains("malformed path globs"));

    // Second edit in the same session: still allowed, but no longer noisy.
    assert_eq!(guard(&payload, dir.path(), Some("t"), None), Outcome::Allow);
    let _ =
        std::fs::remove_file(std::env::temp_dir().join(format!("yupana-guard-failopen-{session}")));
}

#[test]
fn two_different_gaps_in_one_session_both_notify() {
    // Regression (aegis-nz2x): the once-per-session notice was keyed on the
    // session alone, so the FIRST fail-open of any kind consumed the marker and
    // every later, DIFFERENT gap in that session went silent. A config error in
    // one repo would mute a blown blast-radius deadline in another. Demonstrated
    // on the shipped binary before this fix: edit 1 (bad config) notified, edit 2
    // (blown deadline) emitted nothing at all.
    let session = unique_session();
    let mkpayload = |dir: &Path, file: &str| {
        serde_json::json!({
            "session_id": session,
            "cwd": dir.to_str().unwrap(),
            "tool_name": "Edit",
            "tool_input": {
                "file_path": dir.join(file).to_str().unwrap(),
                "old_string": "fn leaf() {}",
                "new_string": "fn leaf() { x(); }",
            },
        })
        .to_string()
    };

    // Gap 1: an unreadable config.
    let a = tempfile::tempdir().unwrap();
    std::fs::write(a.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
    write_policy(a.path(), "this is not [[[ valid toml");
    let Outcome::Notify(m1) = guard(&mkpayload(a.path(), "leaf.rs"), a.path(), Some("t"), None)
    else {
        panic!("gap 1 (config) must notify");
    };
    assert!(m1.contains("failed open"), "{m1}");

    // Gap 2: SAME session, a blown blast-radius deadline — a different kind.
    let b = wide_repo();
    write_policy(
        b.path(),
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 0\n\
         [yupana.policy.scopes.t]\nmax_impacted_files = 1\n",
    );
    match guard(&mkpayload(b.path(), "leaf.rs"), b.path(), Some("t"), None) {
        Outcome::Notify(m2) => assert!(m2.contains("NOT EVALUATED"), "{m2}"),
        other => {
            panic!("gap 2 (deadline) was SILENCED by gap 1's marker — the exact bug: {other:?}")
        }
    }

    for kind in ["config", "unmeasured-deadline-leaf.rs"] {
        let _ = std::fs::remove_file(
            std::env::temp_dir().join(format!("yupana-guard-failopen-{session}-{kind}")),
        );
    }
}

#[test]
fn an_unparseable_language_is_reported_unmeasured_not_silently_allowed() {
    let dir = wide_repo();
    std::fs::write(dir.path().join("notes.md"), "# hi\n").unwrap();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 30000\n\
         [yupana.policy.scopes.t]\nmax_impacted_files = 0\n",
    );
    let payload = edit_payload(dir.path(), "notes.md", "# hi");
    // A zero ceiling denies anything measurable, so an Allow here means the
    // rule did not apply. Declining to measure is still right — but the
    // decline must be VISIBLE, or a rule that cannot be evaluated is
    // indistinguishable from one that passed. That is the whole bug: a fleet
    // was days from turning blocking on over ceilings that silently did not
    // apply to .py/.ts/.go.
    match guard(&payload, dir.path(), Some("t"), None) {
        Outcome::Notify(message) => {
            assert!(message.contains("NOT EVALUATED"), "{message}");
            assert!(message.contains("no grammar for `.md`"), "{message}");
            assert!(message.contains("UNGUARDED"), "{message}");
        }
        other => panic!("an unparseable language must be reported, got {other:?}"),
    }
}

/// THE regression test, at the guard level: a ceiling that denies a Rust edit
/// must deny the identical edit in Python and TypeScript. Measured on the
/// shipped v0.2.0 binary, both ALLOWED with empty stdout.
#[cfg(feature = "langs-extra")]
#[test]
fn a_ceiling_that_denies_rust_denies_python_and_typescript_too() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "py",
            "def leaf():\n    return 1\n",
            "from leaf import leaf\ndef one():\n    return leaf()\n",
            "def leaf():",
        ),
        (
            "ts",
            "export function leaf(): number { return 1; }\n",
            "import { leaf } from \"./leaf\";\nexport function one() { return leaf(); }\n",
            "export function leaf(): number { return 1; }",
        ),
    ];
    for (ext, leaf_src, caller_src, anchor) in cases {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(format!("leaf.{ext}")), leaf_src).unwrap();
        std::fs::write(dir.path().join(format!("one.{ext}")), caller_src).unwrap();
        write_policy(
            dir.path(),
            "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 30000\n\
             [yupana.policy.scopes.t]\nmax_impacted_files = 0\n",
        );
        let payload = edit_payload(dir.path(), &format!("leaf.{ext}"), anchor);
        match guard(&payload, dir.path(), Some("t"), None) {
            Outcome::Deny(message) => assert!(
                message.contains("reaches"),
                ".{ext}: denied, but not for reach: {message}"
            ),
            other => panic!(
                ".{ext}: a zero ceiling did NOT deny an edit reaching another \
                 file — got {other:?}. The rule silently does not apply."
            ),
        }
    }
}

/// The load-bearing test for aegis-ll3p: a scope supplied ONLY via
/// `--config` must actually govern the edit. The ambient config allows
/// (no policy), so a deny here can only come from the override being read.
#[test]
fn a_config_override_scopes_the_guard() {
    let dir = wide_repo(); // ambient config declares no policy — allows everything
    let scope_file = dir.path().join("elsewhere").join("scope.toml");
    std::fs::create_dir_all(scope_file.parent().unwrap()).unwrap();
    std::fs::write(
        &scope_file,
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.t]\nallow_paths = [\"src/**\"]\n",
    )
    .unwrap();
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");

    // Negative control: without the override, the ambient (absent) config
    // allows the edit.
    assert_eq!(guard(&payload, dir.path(), Some("t"), None), Outcome::Allow);

    // With the override, `leaf.rs` is outside `src/**` and is denied.
    let Outcome::Deny(reason) = guard(&payload, dir.path(), Some("t"), Some(&scope_file)) else {
        panic!("the --config scope must govern the edit");
    };
    assert!(reason.contains("leaf.rs"));
    assert!(reason.contains("outside the writable capability scope"));
}

/// A `--config` path that does not exist must fail OPEN loudly, never
/// silently revert to the ambient config — reverting is the disarm this
/// override exists to prevent. Fail-open (allow) is still correct for a
/// guard, but it must be the loud, once-per-session kind.
#[test]
fn a_missing_config_override_fails_open_loudly() {
    let dir = wide_repo();
    // An ambient policy that WOULD deny, to prove the fail-open is the
    // override erroring — not the ambient config quietly taking over.
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.t]\nallow_paths = [\"src/**\"]\n",
    );
    let missing = dir.path().join("does-not-exist.toml");
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");

    let Outcome::Notify(message) = guard(&payload, dir.path(), Some("t"), Some(&missing)) else {
        panic!("a bad --config must fail open loudly, not deny and not silently revert");
    };
    assert!(message.contains("UNGUARDED"));
    assert!(message.contains("does not exist"));
}

// --- Structural rules (tree-sitter tier) at the guard level -----------------

/// A `[[yupana.policy.rules]]` set banning ticket ids in comments. TOML literal
/// (single-quote) strings keep the regex and query free of escape doubling.
const NO_TICKET_RULE: &str = "[yupana.policy]\nmode = \"enforce\"\n\n\
     [[yupana.policy.rules]]\nname = \"no-ticket-in-comment\"\nlanguage = \"rust\"\n\
     query = '(line_comment) @c'\nmatch_type = \"must-not-match\"\n\
     pattern = '\\b[A-Z]+-[0-9]+\\b'\n";

fn rule_edit_payload(dir: &Path, new_string: &str) -> String {
    serde_json::json!({
        "session_id": unique_session(),
        "cwd": dir.to_str().unwrap(),
        "tool_name": "Edit",
        "tool_input": {
            "file_path": dir.join("leaf.rs").to_str().unwrap(),
            "old_string": "fn leaf() {}",
            "new_string": new_string,
        },
    })
    .to_string()
}

#[test]
fn a_rule_denies_an_edit_introducing_a_forbidden_comment() {
    let dir = wide_repo();
    write_policy(dir.path(), NO_TICKET_RULE);
    let payload = rule_edit_payload(dir.path(), "fn leaf() {} // see ABC-123");
    let Outcome::Deny(reason) = guard(&payload, dir.path(), Some("t"), None) else {
        panic!("a forbidden comment must be denied");
    };
    assert!(reason.contains("no-ticket-in-comment"), "{reason}");
    assert!(reason.contains("ABC-123"), "{reason}");
    // Honest about provenance AND freshness (FR-3): a local-config verdict is
    // computed against the exact proposed edit, so it declares itself fresh —
    // it never silently omits or fakes the tag.
    assert!(reason.contains("treesitter tier"), "{reason}");
    assert!(reason.contains("verdict freshness: fresh"), "{reason}");
}

#[test]
fn a_rule_allows_a_clean_edit() {
    let dir = wide_repo();
    write_policy(dir.path(), NO_TICKET_RULE);
    let payload = rule_edit_payload(dir.path(), "fn leaf() {} // no ticket here");
    assert_eq!(guard(&payload, dir.path(), Some("t"), None), Outcome::Allow);
}

#[test]
fn a_rule_applies_even_without_a_tenant_scope() {
    // Rules are global: no scopes table and no tenant, still enforced. This is
    // the whole reason they run BEFORE the tenant-scope gate.
    let dir = wide_repo();
    write_policy(dir.path(), NO_TICKET_RULE);
    let payload = rule_edit_payload(dir.path(), "fn leaf() {} // ABC-123");
    assert!(matches!(
        guard(&payload, dir.path(), None, None),
        Outcome::Deny(_)
    ));
}

#[test]
fn advise_mode_reports_a_rule_without_blocking() {
    let dir = wide_repo();
    write_policy(dir.path(), &NO_TICKET_RULE.replace("enforce", "advise"));
    let payload = rule_edit_payload(dir.path(), "fn leaf() {} // ABC-123");
    let Outcome::Notify(msg) = guard(&payload, dir.path(), Some("t"), None) else {
        panic!("advise must not block");
    };
    assert!(msg.contains("not blocking"), "{msg}");
}

#[test]
fn a_pre_existing_comment_is_not_re_flagged_only_the_introduced_text_is() {
    // The edit introduces a CLEAN comment; a bad comment already on disk is not
    // the agent's doing this turn, so the rule (which reads only introduced
    // text) allows.
    let dir = wide_repo();
    std::fs::write(
        dir.path().join("leaf.rs"),
        "// legacy XYZ-999\nfn leaf() {}\n",
    )
    .unwrap();
    write_policy(dir.path(), NO_TICKET_RULE);
    let payload = rule_edit_payload(dir.path(), "fn leaf() {} // clean now");
    assert_eq!(guard(&payload, dir.path(), Some("t"), None), Outcome::Allow);
}

#[test]
fn a_malformed_rule_fails_open_loudly() {
    let dir = wide_repo();
    let bad = "[yupana.policy]\nmode = \"enforce\"\n\n\
         [[yupana.policy.rules]]\nname = \"broken\"\nlanguage = \"rust\"\n\
         query = '(nonexistent_node) @x'\nmatch_type = \"must-not-match\"\npattern = 'x'\n";
    write_policy(dir.path(), bad);
    let payload = rule_edit_payload(dir.path(), "fn leaf() {} // whatever");
    let Outcome::Notify(msg) = guard(&payload, dir.path(), Some("t"), None) else {
        panic!("a malformed rule must fail open loudly, not block and not pass silently");
    };
    assert!(msg.contains("UNGUARDED"), "{msg}");
    assert!(msg.contains("do not compile"), "{msg}");
}

/// Projection is opt-in and behind the `quipu` feature: when quipu cannot be
/// reached, governed policy that could not be projected must be VISIBLE — a
/// loud fail-open, never a silent pass. (An unreachable port stands in for a
/// down quipu without a live server.)
#[cfg(feature = "quipu")]
#[test]
fn an_unreachable_quipu_projection_fails_open_loudly() {
    let dir = wide_repo();
    write_policy(
        dir.path(),
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.quipu]\nenabled = true\nendpoint = \"http://127.0.0.1:1\"\n",
    );
    let payload = rule_edit_payload(dir.path(), "fn leaf() {} // whatever");
    let Outcome::Notify(msg) = guard(&payload, dir.path(), Some("t"), None) else {
        panic!("an unreachable quipu projection must fail open loudly, not pass silently");
    };
    assert!(msg.contains("UNGUARDED"), "{msg}");
    assert!(msg.contains("could not project governed policy"), "{msg}");
}

// --- FR-31 thin-client cutover: daemon expected vs. absent (aegis-1qze) ------
//
// Four quadrants of (daemon expected?) x (daemon reachable?). A closed port
// (mcp_http_port = 1, never listening) stands in for "down" without a tokio
// server. The "up and used" quadrant is covered at the client level by the
// fetch_measure test against a live daemon in `daemon::http`.

const ALLOWS: &str = "[yupana.policy.scopes.t]\nmax_impacted_files = 50\n";
const FORBIDS: &str = "[yupana.policy.scopes.t]\nmax_impacted_files = 0\n";

fn policy_with_daemon(scope: &str, use_daemon: bool) -> String {
    format!(
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 30000\n{scope}\n\
         [yupana.serve]\nuse_daemon = {use_daemon}\nbind_address = \"127.0.0.1\"\n\
         mcp_http_port = 1\n"
    )
}

#[test]
fn no_daemon_expected_is_silent_transient_the_normal_case() {
    // use_daemon = false (the default, and every case today). The guard builds
    // transiently and says NOTHING about a daemon — absence is normal.
    let dir = wide_repo();
    write_policy(dir.path(), &policy_with_daemon(ALLOWS, false));
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    assert_eq!(
        guard(&payload, dir.path(), Some("t"), None),
        Outcome::Allow,
        "no daemon expected: transient build, allowed, silent"
    );
}

#[test]
fn daemon_EXPECTED_but_DOWN_falls_back_and_is_LOUD_when_allowed() {
    // The cheapest-bypass case. A daemon is expected (use_daemon = true) but the
    // port is closed. The guard must STILL run (fail-open, via transient) and,
    // because the edit is allowed, say LOUDLY that the resident guard is down.
    let dir = wide_repo();
    write_policy(dir.path(), &policy_with_daemon(ALLOWS, true));
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    let Outcome::Notify(msg) = guard(&payload, dir.path(), Some("t"), None) else {
        panic!("an allowed edit while the expected daemon is down must be LOUD, not silent");
    };
    assert!(msg.contains("daemon is DOWN"), "{msg}");
}

#[test]
fn daemon_EXPECTED_but_DOWN_still_DENIES_a_violation_block_wins() {
    // Fail-open does not mean fail-quiet on a real violation: a down daemon must
    // not turn a Deny into an allow. The transient fallback still enforces, and a
    // Deny wins over the daemon-absent notice.
    let dir = wide_repo(); // leaf is called from three files
    write_policy(dir.path(), &policy_with_daemon(FORBIDS, true));
    let payload = edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");
    assert!(
        matches!(
            guard(&payload, dir.path(), Some("t"), None),
            Outcome::Deny(_)
        ),
        "a violation must still be denied even with the expected daemon down"
    );
}

#[test]
fn daemon_EXPECTED_but_DOWN_keeps_the_UNMEASURED_contract() {
    // An unparseable file is still UNMEASURED (not a silent zero), and that notice
    // takes precedence over the daemon-absent one — the transient fallback reports
    // it exactly as it would with no daemon configured.
    let dir = wide_repo();
    std::fs::write(dir.path().join("notes.md"), "# hi\n").unwrap();
    write_policy(dir.path(), &policy_with_daemon(ALLOWS, true));
    let payload = edit_payload(dir.path(), "notes.md", "# hi");
    let Outcome::Notify(msg) = guard(&payload, dir.path(), Some("t"), None) else {
        panic!("an unmeasurable file must stay UNMEASURED-loud");
    };
    assert!(msg.contains("NOT EVALUATED"), "{msg}");
}
// --- the governed TEXT plane's blocking matrix (aegis-m9ln / aegis-mqnl) ---
//
// The pure half of governed_check: which tier x exposure combinations
// BLOCK. This is the part of the circuit that must never be wrong in the
// blocking direction, so it is tested without a network. The projection
// and evaluation halves have their own suites (project.rs, textrules.rs).

#[cfg(feature = "quipu")]
fn text_violation(tier: crate::textrules::TextTier) -> crate::textrules::TextViolation {
    crate::textrules::TextViolation {
        rule: "pattern_internal-lan-host".into(),
        tier,
        message: "governed text rule `pattern_internal-lan-host`: the edit \
                  introduces `db.lan` (hostname) — internal .lan hostname."
            .into(),
    }
}

#[cfg(feature = "quipu")]
#[test]
fn a_block_tier_hit_in_a_public_repo_blocks() {
    use crate::project::RepoExposure;
    let (messages, blocks) = text_plane(
        &[text_violation(crate::textrules::TextTier::Block)],
        &RepoExposure::Public,
    );
    assert!(blocks, "this is the exact leak the rule exists to stop");
    assert!(messages.iter().any(|m| m.contains("PUBLIC remote")));
}

#[cfg(feature = "quipu")]
#[test]
fn a_block_tier_hit_in_an_internal_repo_downgrades_and_says_why() {
    use crate::project::RepoExposure;
    let (messages, blocks) = text_plane(
        &[text_violation(crate::textrules::TextTier::Block)],
        &RepoExposure::Internal,
    );
    assert!(!blocks, "internal-only exposure must not block");
    assert!(messages.iter().any(|m| m.contains("downgraded")));
    assert!(messages
        .iter()
        .any(|m| m.contains("would BLOCK in a public repo")));
}

#[cfg(feature = "quipu")]
#[test]
fn an_unknown_repo_never_blocks_and_says_it_is_unknown() {
    // mqnl's constraint verbatim: warn AND SAY SO — never block on a guess.
    use crate::project::RepoExposure;
    let (messages, blocks) = text_plane(
        &[text_violation(crate::textrules::TextTier::Block)],
        &RepoExposure::Unknown("repo `yupana` is not in the graph".into()),
    );
    assert!(!blocks);
    assert!(messages
        .iter()
        .any(|m| m.contains("never blocks on a guess")));
    assert!(messages.iter().any(|m| m.contains("not in the graph")));
    // The remedy is named: exposure is DATA, so the fix is a graph write.
    assert!(messages.iter().any(|m| m.contains("repo_<name>")));
}

#[cfg(feature = "quipu")]
#[test]
fn a_warn_tier_hit_never_blocks_even_in_a_public_repo() {
    use crate::project::RepoExposure;
    let (_, blocks) = text_plane(
        &[text_violation(crate::textrules::TextTier::Warn)],
        &RepoExposure::Public,
    );
    assert!(
        !blocks,
        "warn tier is advisory everywhere — per-pattern tier is data"
    );
}

// --- The Σ-derived trace record, through the real guard path ----------------

/// Run the guard and return the record it spools, as JSON.
///
/// Goes through `guard_recorded` — the real payload, the real config resolution
/// and the real decision path — so only the single `metrics::emit` call in
/// `guard` is outside it. Driving the actual spool would need
/// `std::env::set_var`, which this crate cannot do: `unsafe_code = "deny"`.
fn guard_line(policy: &str, new_string: &str) -> serde_json::Value {
    let dir = wide_repo();
    write_policy(dir.path(), policy);
    let payload = rule_edit_payload(dir.path(), new_string);
    let (_, fields) = guard_recorded(&payload, dir.path(), Some("t"), None);
    serde_json::Value::Object(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<serde_json::Map<_, _>>(),
    )
}

#[test]
fn the_record_carries_the_constraint_set_not_just_a_joined_name() {
    // SARC I3. The audit checker needs, per constraint: which one, where it was
    // evaluated, what it concluded and what was done — and none of the four
    // survive a `+`-joined string of names.
    let line = guard_line(NO_TICKET_RULE, "fn leaf() {} // see ABC-123");
    let constraints = line["constraints"]
        .as_array()
        .expect("the record carries a constraints array");
    assert_eq!(constraints.len(), 1);
    assert_eq!(constraints[0]["id"], "no-ticket-in-comment");
    assert_eq!(constraints[0]["outcome"], "unsatisfied");
    assert_eq!(
        constraints[0]["response"], "blocked",
        "an enforce-mode deny records as blocked"
    );
}

#[test]
fn the_legacy_rule_field_survives_the_change() {
    // Live dashboards group on `rule`. Dropping it would silently empty every
    // panel built on it, so it is DERIVED from the constraint set rather than
    // removed — and this test is what stops a later cleanup deleting it.
    let line = guard_line(NO_TICKET_RULE, "fn leaf() {} // see ABC-123");
    assert_eq!(line["rule"], "no-ticket-in-comment");
}

#[test]
fn an_advise_mode_violation_records_warned_not_blocked() {
    // Outcome and response are separate fields for exactly this: the constraint
    // was equally unsatisfied under both modes, and only the RESPONSE differed.
    // A record collapsing them could not tell an advise-mode fleet from an
    // enforcing one after the fact.
    let advise = NO_TICKET_RULE.replace("mode = \"enforce\"", "mode = \"advise\"");
    let line = guard_line(&advise, "fn leaf() {} // see ABC-123");
    let constraints = line["constraints"].as_array().unwrap();
    assert_eq!(constraints[0]["outcome"], "unsatisfied");
    assert_eq!(constraints[0]["response"], "warned");
    assert_eq!(line["result"], "notify");
}

#[test]
fn a_clean_edit_records_no_constraints_field_at_all() {
    // Absent rather than an empty array: the spool's discipline is that an
    // omitted field is honestly silent, and an empty array would read as "the
    // constraints were evaluated and all passed" when in fact none applied.
    let line = guard_line(NO_TICKET_RULE, "fn leaf() {} // nothing to see");
    assert_eq!(line["result"], "allow");
    assert!(
        line.get("constraints").is_none(),
        "a clean edit records no constraint set: {line}"
    );
    assert!(line.get("rule").is_none());
}

#[test]
fn the_record_declares_the_currency_of_the_policy_set() {
    // A confidence input, and the field that stops a soak window counting
    // verdicts computed against a stale projection as evidence about the
    // current policy. Local config is authoritative, so it is genuinely fresh.
    let line = guard_line(NO_TICKET_RULE, "fn leaf() {} // see ABC-123");
    assert_eq!(line["policy_freshness"], "fresh");
}

#[test]
fn a_clean_edit_declares_no_policy_freshness_either() {
    // It rides with the constraint set: no evaluations, no policy set whose
    // currency could be in question.
    let line = guard_line(NO_TICKET_RULE, "fn leaf() {} // nothing to see");
    assert!(line.get("policy_freshness").is_none());
}
