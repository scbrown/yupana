//! Integration tests driving the `yupana` binary.

use assert_cmd::Command;
use predicates::prelude::*;

/// Write a throwaway Rust file into a fresh temp dir and return the dir.
fn project_with(file: &str, contents: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(file), contents).unwrap();
    dir
}

/// A config that pins the graph plane OFF, so a test asserts on yupana and not on
/// the machine it runs on.
///
/// Without this, `status` falls through to layered discovery and reads the
/// DEVELOPER'S `~/.config/bobbin/config.toml` — which on every agent host in
/// this fleet enables quipu against a live endpoint. That was always a test
/// reading ambient state, and it became a correctness problem when the rule
/// plane got an exit code (aegis-hac0): the assertion below would then pass or
/// fail depending on whether a network service happened to answer.
fn pinned_config(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let path = dir.path().join("pinned.toml");
    std::fs::write(&path, "[yupana.quipu]\nenabled = false\n").unwrap();
    path
}

/// Seal a fixture's `[yupana]` body against the host's live graph the same way
/// [`pinned_config`] seals `status` — appending the OFF stanza unless the body
/// declares its own `[yupana.quipu]`, so tests that genuinely exercise projection
/// (which pin their own endpoint, usually `127.0.0.1:*`) are untouched.
///
/// `pinned_config` existed and was applied only where it was remembered. Every
/// fixture that wrote a config WITHOUT it still inherited the developer's
/// `~/.config/bobbin/config.toml` and made real network calls, so five guard
/// tests here asserted on whether a shared service happened to answer within
/// 2s. Measured 2026-08-04: they fail together whenever it does not, each
/// reporting the guard "failed open" in place of the deny/allow under test.
///
/// This is the same seal `hook::pre_edit::pre_edit_test::write_policy` applies
/// to the unit fixtures. Routing every writer through one helper is the point —
/// the previous arrangement depended on each author remembering, and the
/// failure it produces is a timeout in an unrelated assertion.
fn hermetic(body: &str) -> String {
    if body.contains("[yupana.quipu]") {
        body.to_string()
    } else {
        format!("{body}\n\n[yupana.quipu]\nenabled = false\n")
    }
}

#[test]
fn status_json_reports_base_ref() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["status", "--json", "--config"])
        .arg(pinned_config(&dir))
        .assert()
        .success()
        .stdout(predicate::str::contains("\"base_ref\""))
        // The resolved baseline commit is reported (this repo is a git repo, so
        // it resolves to a 40-char SHA; the key is present regardless).
        .stdout(predicate::str::contains("\"base_commit\""));
}

#[test]
fn analyze_counts_symbols() {
    let dir = project_with("a.rs", "fn foo() {}\nstruct Bar;\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["analyze", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 symbol"));
}

#[test]
fn refs_finds_definition() {
    let dir = project_with("a.rs", "fn target() {}\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["refs", "target", dir.path().to_str().unwrap()])
        .assert()
        .success()
        // Assert on what ONLY a RESOLVED hit prints (src/cli.rs refs(): the hits
        // branch renders `<file>:<line> <name> (<kind>) [<tier>]`). The old
        // assertion was `contains("target")` — and the EMPTY branch prints
        // "no definition found for target", which also contains "target". So the
        // test passed whether or not refs resolved anything: gutting refs() to push
        // no hits left it green (aegis-fo30). `a.rs:1` (the resolved location) and
        // "(function)" (the resolved kind) appear ONLY when a definition is found,
        // and the explicit not() pins that the not-found path is NOT what satisfied
        // the test.
        //
        // The kind reads "(function)" rather than "(Function)" since refs began
        // answering from the graph (yupana #76): the node stores the LOWERCASE kind
        // form — the one the daemon and MCP already serve — instead of the Debug
        // rendering of the extractor enum. One spelling across every surface.
        .stdout(
            predicate::str::contains("a.rs:1")
                .and(predicate::str::contains("(function)"))
                .and(predicate::str::contains("no definition found").not()),
        );
}

#[test]
fn refs_json_contains_the_resolved_definition() {
    // The programmatic FR-4/FR-5 surface (Bobbin + agents consume --json). Mirrors
    // refs_json_is_empty_array_when_absent, but for the POSITIVE case, and asserts
    // on fields the empty result `[]` cannot carry: a resolved hit emits "kind" and
    // "start_line": 1. This is the clean discriminator the empty branch prints
    // nothing of.
    let dir = project_with("a.rs", "fn target() {}\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["refs", "target", dir.path().to_str().unwrap(), "--json"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("\"kind\"")
                .and(predicate::str::contains("\"start_line\": 1"))
                .and(predicate::str::contains("\"name\": \"target\"")),
        );
}

#[test]
fn refs_json_absent_answer_is_tagged_and_says_what_it_searched() {
    // Was `refs_json_is_empty_array_when_absent`, asserting a bare `[]`. That
    // shape is exactly the FR-3 empty-case hole `not_found` closed for
    // callers/impact/dataflow: a top-level array has nowhere to hang a tier, so
    // yupana's most common answer — "nothing" — was the one answer it served
    // untagged. The absent result is still a served fact and carries its tier.
    let dir = project_with("a.rs", "fn other() {}\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["refs", "missing", dir.path().to_str().unwrap(), "--json"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("\"count\": 0")
                .and(predicate::str::contains("\"tier\": \"treesitter\""))
                // The discriminator yupana #76 turned on: a zero count over a
                // NON-zero searched set means "the name is absent". Over a zero
                // searched set it would mean "nothing here was parseable", and
                // reporting those as the same answer is what made refs confidently
                // wrong on every non-Rust tree.
                .and(predicate::str::contains("\"searched_symbols\": 1")),
        );
}

#[test]
#[cfg(feature = "langs-extra")] // needs the python grammar compiled in
fn refs_resolves_a_python_definition_the_way_callers_does() {
    // yupana #76, the regression that matters. `refs` walked `rust_files()` and
    // parsed every hit as "rust", so on a Python tree it searched ZERO files and
    // printed "no definition found" — while `callers`, over the same tree and the
    // same symbol, answered from the multi-language graph and listed call sites.
    // The reported failure was that pair of contradictory answers, so the test
    // asserts the pair: both commands, one fixture, both resolving.
    let dir = project_with(
        "quipu.py",
        "def derive_agents(cfg):\n    return cfg\n\ndef consume():\n    return derive_agents({})\n",
    );
    let path = dir.path().to_str().unwrap();

    Command::cargo_bin("yupana")
        .unwrap()
        .args(["refs", "derive_agents", path])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("quipu.py:1")
                .and(predicate::str::contains("no definition found").not()),
        );

    // The command that always worked, pinned alongside it: if these two ever
    // disagree about whether a symbol exists again, this test fails.
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["callers", "derive_agents", path])
        .assert()
        .success()
        .stdout(predicate::str::contains("consume"));
}

#[test]
fn refs_does_not_report_an_unparseable_tree_as_an_absent_symbol() {
    // The other half of yupana #76: "no definition found" over an EMPTY graph is not
    // evidence the symbol is absent — it is evidence yupana read nothing. A tree with
    // no source files yupana can parse must say so, or an agent routed here reads
    // "this symbol does not exist" from what is really "I could not look".
    let dir = project_with("notes.md", "# nothing parseable here\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["refs", "anything", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("the graph is empty")
                .and(predicate::str::contains("not evidence")),
        );
}

#[test]
fn callers_lists_direct_callers() {
    let dir = project_with("a.rs", "fn leaf() {}\nfn mid() { leaf(); }\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["callers", "leaf", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("mid"));
}

#[test]
fn impact_reconciles_with_cochange() {
    let dir = project_with(
        "a.rs",
        "fn leaf() {}\nfn mid() { leaf(); }\nfn top() { mid(); }\n",
    );
    // Co-change set: a.rs is corroborated (also structural); other.rs is not.
    std::fs::write(dir.path().join("cochange.json"), "[\"a.rs\", \"other.rs\"]").unwrap();

    Command::cargo_bin("yupana")
        .unwrap()
        .args([
            "impact",
            "leaf",
            dir.path().to_str().unwrap(),
            "--cochange",
            dir.path().join("cochange.json").to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"reconciliation\""))
        .stdout(predicate::str::contains("\"corroborated\""))
        .stdout(predicate::str::contains("other.rs"));
}

#[test]
fn dataflow_traces_dependence() {
    let dir = project_with(
        "a.rs",
        "fn f(a: i32) -> i32 { let b = a + 1; let c = b * 2; c }\n",
    );
    Command::cargo_bin("yupana")
        .unwrap()
        .args([
            "dataflow",
            "f",
            dir.path().to_str().unwrap(),
            "--var",
            "c",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"b\""))
        .stdout(predicate::str::contains("\"a\""));
}

#[test]
fn impact_reports_transitive_callers() {
    let dir = project_with(
        "a.rs",
        "fn leaf() {}\nfn mid() { leaf(); }\nfn top() { mid(); }\n",
    );
    Command::cargo_bin("yupana")
        .unwrap()
        .args([
            "impact",
            "leaf",
            dir.path().to_str().unwrap(),
            "--json",
            "--hops",
            "5",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"top\""))
        .stdout(predicate::str::contains("\"count\": 2"));
}

// ── The pre-edit policy guard (§5.8/FR-25, FR-30) ────────────────────────
//
// These drive the real binary end to end, because the guard's contract is about
// process behaviour — exit code and stdout — not just its return value. The one
// rule the harness depends on: **exit 0, always**. Exit 2 is Claude Code's
// fail-*closed* channel, so a guard that ever emitted it could hard-block an
// agent.

/// A repo where `leaf` is called from three other files, with a policy applied.
fn guarded_project(policy: &str) -> tempfile::TempDir {
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
    std::fs::write(bobbin.join("config.toml"), hermetic(policy)).unwrap();
    dir
}

/// A `PreToolUse` payload editing `file` in `dir`.
///
/// The session id is unique PER CALL, not per (process, file). The fail-open
/// notice fires once per (session, kind) and records that in a file under
/// `std::env::temp_dir()` which outlives the process, so tests sharing a session
/// id couple through it: with a stale marker present the notice is suppressed,
/// and only the first test to run sees it. That made WHICH test failed a race —
/// the aegis-w99qp report saw 5 failures in one run and 2 in another from an
/// unchanged tree, and one `leaf.rs` test always "passed" merely by losing it.
/// `pre_edit_test.rs` already solved this with `unique_session`; this is that fix
/// on the integration side.
fn pre_edit_payload(dir: &std::path::Path, file: &str, old: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // The nanosecond stamp defeats PID recycling: the marker files outlive the
    // process and nothing prunes them, so pid+counter alone collides across runs.
    // See `unique_session` in `src/hook/pre_edit_test.rs` for the measurement.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let session = format!(
        "it-{}-{nanos}-{}-{file}",
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
            "old_string": old,
            "new_string": "fn leaf() { changed(); }",
        },
    })
    .to_string()
}

#[test]
fn pre_edit_denies_an_edit_beyond_the_blast_radius() {
    let dir = guarded_project(
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 30000\n\
         [yupana.policy.scopes.polecat]\nmax_impacted_files = 1\n",
    );
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "pre-edit", "--tenant", "polecat"])
        .write_stdin(pre_edit_payload(dir.path(), "leaf.rs", "fn leaf() {}"))
        .assert()
        // Deny is exit 0 + JSON; the harness never sees a failing process.
        .success()
        .stdout(predicate::str::contains("\"permissionDecision\":\"deny\""))
        .stdout(predicate::str::contains("\"hookEventName\":\"PreToolUse\""))
        // The reason must be actionable: what was exceeded, and by how much.
        .stdout(predicate::str::contains("3 files (ceiling 1)"));
}

#[test]
fn pre_edit_denies_a_path_outside_the_capability_scope() {
    let dir = guarded_project(
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.polecat]\nallow_paths = [\"caller*.rs\"]\n",
    );
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "pre-edit", "--tenant", "polecat"])
        .write_stdin(pre_edit_payload(dir.path(), "leaf.rs", "fn leaf() {}"))
        .assert()
        .success()
        .stdout(predicate::str::contains("\"permissionDecision\":\"deny\""))
        .stdout(predicate::str::contains(
            "outside the writable capability scope",
        ));
}

#[test]
fn pre_edit_allows_an_ordinary_edit_silently() {
    let dir = guarded_project(
        "[yupana.policy]\nmode = \"enforce\"\ndeadline_ms = 30000\n\
         [yupana.policy.scopes.polecat]\nmax_impacted_files = 10\n",
    );
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "pre-edit", "--tenant", "polecat"])
        .write_stdin(pre_edit_payload(dir.path(), "leaf.rs", "fn leaf() {}"))
        .assert()
        .success()
        // Allow is *silence*. Emitting permissionDecision:"allow" would suppress
        // the user's own permission prompt — the guard only ever subtracts.
        .stdout(predicate::str::is_empty());
}

#[test]
fn pre_edit_resolves_the_tenant_from_bobbin_role() {
    let dir = guarded_project(
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.polecat]\nallow_paths = [\"caller*.rs\"]\n",
    );
    // Shantytown sets BOBBIN_ROLE per agent, so one hook registration serves
    // every role; this is the path that actually runs in the field.
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "pre-edit"])
        .env("BOBBIN_ROLE", "polecat")
        .write_stdin(pre_edit_payload(dir.path(), "leaf.rs", "fn leaf() {}"))
        .assert()
        .success()
        .stdout(predicate::str::contains("\"permissionDecision\":\"deny\""));
}

#[test]
fn pre_edit_fails_open_on_garbage_and_on_no_policy() {
    // Unparseable payload.
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "pre-edit", "--tenant", "polecat"])
        .write_stdin("not json at all")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    // A repo with no policy configured at all. The config is WRITTEN (empty of
    // policy) rather than omitted: an omitted file falls through to the host's,
    // which enables the graph plane, so this test asserted on a network service
    // instead of on the fail-open contract it is named for.
    let dir = project_with("a.rs", "fn foo() {}\n");
    with_config(dir.path(), "");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "pre-edit", "--tenant", "polecat"])
        .write_stdin(pre_edit_payload(dir.path(), "a.rs", "fn foo() {}"))
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn pre_edit_never_denies_in_advise_mode() {
    let dir = guarded_project(
        "[yupana.policy]\nmode = \"advise\"\ndeadline_ms = 30000\n\
         [yupana.policy.scopes.polecat]\nmax_impacted_files = 1\n",
    );
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "pre-edit", "--tenant", "polecat"])
        .write_stdin(pre_edit_payload(dir.path(), "leaf.rs", "fn leaf() {}"))
        .assert()
        .success()
        .stdout(predicate::str::contains("systemMessage"))
        .stdout(predicate::str::contains("not blocking"))
        // Staging a scope must never block, however badly it is misconfigured.
        .stdout(predicate::str::contains("permissionDecision").not());
}

// ── yupana verify: monitor-guided edit verification (FR-23/FR-24) ──────────

#[test]
fn verify_passes_a_clean_buffer_and_reports_its_tier() {
    let dir = project_with("helpers.rs", "fn helper() {}\n");
    let buffer = dir.path().join("proposed.rs");
    std::fs::write(&buffer, "fn f() { helper(); }\n").unwrap();

    Command::cargo_bin("yupana")
        .unwrap()
        .current_dir(dir.path())
        .args(["verify", "--file", "a.rs", "--buffer"])
        .arg(&buffer)
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"ok\": true"))
        // A clean verdict must never be read as "this compiles".
        .stdout(predicate::str::contains(
            "type-violation (needs the LSP tier)",
        ));
}

#[test]
fn verify_exits_nonzero_and_names_each_violation() {
    let dir = project_with("helpers.rs", "fn helper() {}\n");
    let buffer = dir.path().join("proposed.rs");
    std::fs::write(
        &buffer,
        "fn takes_two(a: u8, b: u8) {}\nfn f() { takes_two(1); ghost(); }\nmod missing;\n",
    )
    .unwrap();

    Command::cargo_bin("yupana")
        .unwrap()
        .current_dir(dir.path())
        .args(["verify", "--file", "a.rs", "--buffer"])
        .arg(&buffer)
        .arg("--json")
        .assert()
        // Non-zero so CI and scripts can gate on a verdict.
        .failure()
        .stdout(predicate::str::contains("identifier-does-not-exist"))
        .stdout(predicate::str::contains("wrong-arity"))
        .stdout(predicate::str::contains("unresolved-import"));
}

/// The guard's blocking channel is a JSON object on stdout, never an exit code.
/// Exit `2` is Claude Code's fail-CLOSED channel, so *any* hook invocation that
/// exits `2` blocks the agent's edit.
///
/// The path that matters is version skew, not a typo: a `yupana` older than the
/// subcommand answers `hook pre-edit` with clap's "invalid value" error and
/// exit `2`. Deploying the hook against a stale binary would therefore block
/// every Edit/Write in the fleet — the exact outcome the fail-open clause
/// exists to prevent. Absence already fails open (exit `127`); staleness is the
/// case that did not.
///
/// An unknown hook event stands in for "this yupana is too old to know the event
/// you asked for", which is indistinguishable from skew at the CLI boundary.
#[test]
fn an_unknown_hook_event_fails_open_instead_of_exiting_2() {
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "some-event-this-yupana-does-not-have"])
        .write_stdin(r#"{"tool_name":"Edit","tool_input":{"file_path":"/tmp/x.rs"}}"#)
        .assert()
        .code(0)
        // Silence on stdout: a guard that cannot parse its arguments has not
        // decided anything, and must not appear to have allowed or denied.
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("failed open"));
}

/// The same protection must not swallow ordinary CLI misuse: a non-hook command
/// still exits `2`, so typos stay loud everywhere it is safe for them to be.
#[test]
fn a_non_hook_command_still_exits_2_on_bad_arguments() {
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["definitely-not-a-command"])
        .assert()
        .code(2);
}

/// `yupana hook --help` is an "error" in clap's model; it must still print and
/// exit `0` rather than being mistaken for a fail-open.
#[test]
fn hook_help_still_prints_and_exits_0() {
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "--help"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("post-edit"));
}

/// aegis-ll3p acceptance #1: `--config` makes `status` read the named file, not
/// the ambient config in the cwd.
#[test]
fn config_flag_makes_status_read_the_named_file() {
    let dir = tempfile::tempdir().unwrap();
    // Ambient config in the cwd says one thing...
    let bobbin = dir.path().join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    std::fs::write(
        bobbin.join("config.toml"),
        hermetic("[yupana]\nbase_ref = \"from-cwd\"\n"),
    )
    .unwrap();
    // ...the override file says another.
    let other = dir.path().join("other.toml");
    std::fs::write(&other, "[yupana]\nbase_ref = \"from-flag\"\n").unwrap();

    Command::cargo_bin("yupana")
        .unwrap()
        .current_dir(dir.path())
        .args(["status", "--json", "--config"])
        .arg(&other)
        .assert()
        .success()
        .stdout(predicate::str::contains("from-flag"))
        .stdout(predicate::str::contains("from-cwd").not());
}

/// aegis-ll3p acceptance #2, the load-bearing one: a `deny_paths`/scope rule
/// supplied ONLY via `--config` causes the guard to DENY an edit the ambient
/// config would allow. Negative control: without `--config`, the same edit is
/// allowed. Distinguishes "the override was read" from "the guard failed open".
#[test]
fn config_flag_points_the_guard_at_a_scope_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
    // An ambient config declaring NO POLICY — which is what the negative
    // control below actually depends on. It is written rather than omitted
    // because "omitted" does not mean "empty": yupana layers the developer's
    // `~/.config/bobbin/config.toml`, so an absent file made this test reach a
    // live graph and assert on whether it answered within 2s.
    with_config(dir.path(), "");
    let scope = dir.path().join("scope.toml");
    std::fs::write(
        &scope,
        hermetic(
            "[yupana.policy]\nmode = \"enforce\"\n\
             [yupana.policy.scopes.polecat]\nallow_paths = [\"src/**\"]\n",
        ),
    )
    .unwrap();
    let payload = pre_edit_payload(dir.path(), "leaf.rs", "fn leaf() {}");

    // Negative control: no --config → ambient (absent) config → allow (silent).
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "pre-edit", "--tenant", "polecat"])
        .write_stdin(payload.clone())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    // With --config, `leaf.rs` is outside `src/**` and is denied.
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["hook", "pre-edit", "--tenant", "polecat", "--config"])
        .arg(&scope)
        .write_stdin(payload)
        .assert()
        .success()
        .stdout(predicate::str::contains("\"permissionDecision\":\"deny\""))
        .stdout(predicate::str::contains(
            "outside the writable capability scope",
        ));
}

/// A `--config` path that does not exist is a loud failure on an ordinary
/// command, not a silent fall-back to discovery.
#[test]
fn a_missing_config_path_is_a_loud_error_on_status() {
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["status", "--config", "/no/such/yupana-config.toml"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
}

/// aegis-hac0 observability: `yupana status` must surface the policy layer — the
/// guard's own state was invisible in the command meant to show configuration.
#[test]
fn status_surfaces_policy_and_the_absent_signed_rule_set() {
    let dir = tempfile::tempdir().unwrap();
    let scope = dir.path().join("scope.toml");
    std::fs::write(
        &scope,
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.weaver]\nallow_paths = [\"src/**\"]\nmax_impacted_files = 3\n",
    )
    .unwrap();

    Command::cargo_bin("yupana")
        .unwrap()
        .args(["status", "--json", "--tenant", "weaver", "--config"])
        .arg(&scope)
        .assert()
        .success()
        // The policy layer is now observable...
        .stdout(predicate::str::contains("\"policy\""))
        .stdout(predicate::str::contains("\"mode\": \"enforce\""))
        .stdout(predicate::str::contains("\"scope_configured\": true"))
        // ...and the not-yet-existing signed rule set is reported ABSENT, loudly,
        // rather than omitted (aegis-hac0's second clause).
        .stdout(predicate::str::contains("\"signed_rule_set\""))
        .stdout(predicate::str::contains("\"never-loaded\""));
}

/// The armed-but-inert state — enforce mode with no scope for the tenant — must
/// be a visible caveat, not read as a healthy enforcing guard.
#[test]
fn status_warns_on_enforce_without_a_scope_for_the_tenant() {
    let dir = tempfile::tempdir().unwrap();
    let scope = dir.path().join("scope.toml");
    std::fs::write(
        &scope,
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.someone_else]\nallow_paths = [\"src/**\"]\n",
    )
    .unwrap();

    Command::cargo_bin("yupana")
        .unwrap()
        .args(["status", "--json", "--tenant", "weaver", "--config"])
        .arg(&scope)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"enforcing_without_scope\": true",
        ));
}

// --- config keys made real (aegis-ltjo) -------------------------------------

/// `.bobbin/config.toml` under `dir` with the given `[yupana]` body.
fn with_config(dir: &std::path::Path, body: &str) {
    let bobbin = dir.join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    std::fs::write(bobbin.join("config.toml"), hermetic(body)).unwrap();
}

#[cfg(feature = "langs-extra")] // needs the python grammar compiled in
#[test]
fn languages_restricts_what_analyze_counts() {
    // A mixed-language tree: 2 Rust + 2 Python + 1 TypeScript symbols.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn r(){}\nfn r2(){}\n").unwrap();
    std::fs::write(
        dir.path().join("b.py"),
        "def p():\n    pass\ndef p2():\n    pass\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("c.ts"), "export function t(){}\n").unwrap();
    let p = dir.path().to_str().unwrap();

    // languages = ["rust"] -> only the 2 Rust symbols.
    with_config(dir.path(), "[yupana]\nlanguages = [\"rust\"]\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["analyze", "--json", p])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"symbols\": 2"));

    // Adding python -> 4. The key RESTRICTS; a user who narrows it gets narrowing.
    with_config(dir.path(), "[yupana]\nlanguages = [\"rust\",\"python\"]\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["analyze", "--json", p])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"symbols\": 4"));
}

#[test]
fn serve_read_only_refuses_a_write() {
    if !cfg!(feature = "quipu") {
        // The featureless stub exits 2 with its phase note BEFORE any guard
        // runs (it must — its exit 0 once let a cron book an unpromoted commit
        // as done), so there is no read_only path to pin in this build.
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.rs"), "fn x(){}\n").unwrap();
    // git init so promote's own preconditions don't mask the guard.
    Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(dir.path())
        .assert()
        .success();

    // read_only = true -> promotion (the write) is REFUSED with a distinguishable
    // error naming the key. This is the guard the docs claimed and did not perform.
    with_config(dir.path(), "[yupana.serve]\nread_only = true\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .arg("promote")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("read_only"))
        .stderr(predicate::str::contains("refused"));

    // read_only = false -> the write guard PASSES. What happens next depends on the
    // build, and the point of this half is only that the failure (if any) is NOT the
    // guard: whatever stops the promotion, it is never `read_only`.
    with_config(dir.path(), "[yupana.serve]\nread_only = false\n");
    let assert = Command::cargo_bin("yupana")
        .unwrap()
        .arg("promote")
        // HOME is redirected because yupana LAYERS ~/.config/bobbin/config.toml
        // in, and a fleet host's real user config now carries a live
        // [yupana.quipu] endpoint (the m9ln guard rollout) — which turns "no
        // endpoint configured" into "endpoint found, fail later" and makes
        // this test's outcome depend on whose machine runs it.
        .env("HOME", dir.path())
        .current_dir(dir.path())
        .assert();
    // With promotion wired, `promote` with no `--to` refuses for lack of an
    // endpoint — a real precondition, reached only because the guard let it
    // through. The guard is proven passed by the absence of its name here.
    assert
        .failure()
        .stderr(predicate::str::contains("--to").or(predicate::str::contains("endpoint")))
        .stderr(predicate::str::contains("read_only").not());
}

/// `yupana export` prints Turtle; `yupana export --to <url>` PROMOTES instead — one
/// promotion path, two spec spellings (§15). This pins the routing: `--to`
/// reaches the same validate-then-write path `promote` does, and plain `export`
/// still prints.
#[test]
fn export_to_routes_through_promotion_not_print() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("x.rs"),
        "pub fn x() -> u32 { y() }\nfn y() -> u32 { 1 }\n",
    )
    .unwrap();

    // Plain export prints Turtle in the bobbin ontology — a read, always.
    Command::cargo_bin("yupana")
        .unwrap()
        .arg("export")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("bobbin:"))
        .stdout(predicate::str::contains("CodeSymbol"));

    // export --to <unreachable>: with quipu it routes into promotion, validates
    // the (valid) Turtle, then fails to REACH the endpoint — proving it took the
    // write path, not the print path (a print would have succeeded and emitted
    // Turtle). Without quipu it is the phase-4 stub.
    let assert = Command::cargo_bin("yupana")
        .unwrap()
        .args(["export", "--to", "http://127.0.0.1:1"])
        .current_dir(dir.path())
        .assert();
    if cfg!(feature = "quipu") {
        assert
            .failure()
            .stdout(predicate::str::contains("bobbin:").not());
    } else {
        // The featureless stub exits 2 (a stub exit 0 once let the quipu-ingest
        // cron advance its promote marker past a commit that never promoted —
        // aegis-ucoh). Pin the non-zero contract here so it cannot regress to a
        // silent success.
        assert
            .failure()
            .stderr(predicate::str::contains("--features quipu"));
    }
}

/// Build a committed one-file git repo with an `origin`, so promotion's identity
/// and committed-tree preconditions are satisfied and cannot mask what a test is
/// actually pinning.
fn promotable_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("x.rs"),
        "pub fn x() -> u32 { y() }\nfn y() -> u32 { 1 }\n",
    )
    .unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "t@t"],
        &["config", "user.name", "t"],
        &[
            "remote",
            "add",
            "origin",
            "https://example.com/owner/realname.git",
        ],
        &["add", "-A"],
        &["commit", "-qm", "base"],
    ] {
        Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .assert()
            .success();
    }
    dir
}

/// A DISCOVERED `[yupana.quipu] endpoint` must never authorize a write (aegis-o2h97).
///
/// `--to`'s help promised "without it, promotion is unwired and refuses" and the
/// code then fell back to the configured endpoint — so on every host in this
/// fleet, where that key is set host-wide for the pre-edit guard's READS, a bare
/// `yupana promote` wrote to the live graph. It was found by an operator running it
/// EXPECTING a dry run and getting a real 25k-triple promotion.
///
/// The assertion that matters is not the message: it is that the endpoint is never
/// CONNECTED to. The config points at a listener this test owns, so a fallback
/// that survived would show up here as an accepted connection.
#[test]
fn promote_refuses_a_discovered_endpoint_and_never_connects_to_it() {
    if !cfg!(feature = "quipu") {
        return; // stub build: promotion is a phase notice, nothing to pin
    }
    let dir = promotable_repo();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();

    // The endpoint is configured, exactly as a fleet host configures it.
    with_config(
        dir.path(),
        &format!("[yupana.quipu]\nenabled = true\nendpoint = \"http://{addr}\"\n"),
    );

    // Bare `promote`: refused, naming the discovered endpoint AND both remedies.
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["promote", "--repo", "realname"])
        // HOME is redirected so the layered user config of whoever runs this
        // cannot supply a second endpoint and change which one is reported.
        .env("HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("DISCOVERED endpoint"))
        .stderr(predicate::str::contains(addr.to_string()))
        .stderr(predicate::str::contains("--to"))
        .stderr(predicate::str::contains("--dry-run"));

    // Nothing reached the graph. This is the whole bead: a refusal that still
    // posted would satisfy every string assertion above.
    assert!(
        listener.accept().is_err(),
        "promote connected to the discovered endpoint — the fallback is back"
    );
}

/// `--dry-run` answers "would this projection conform?" without writing — the
/// capability that did not exist, and the reason the defect above was found by
/// someone reaching for `promote` and hoping it was inert (aegis-o2h97).
///
/// Pointed at a DEAD port on purpose: a dry run that still posted would fail to
/// connect, so success here is positive evidence of no write, not the absence of
/// evidence. `promote_refuses_dir_name_identity_and_accepts_origin` is the control
/// that the same port DOES fail a real promotion.
#[test]
fn dry_run_validates_and_writes_nothing() {
    if !cfg!(feature = "quipu") {
        return; // stub build: promotion is a phase notice, nothing to pin
    }
    let dir = promotable_repo();

    Command::cargo_bin("yupana")
        .unwrap()
        .args([
            "promote",
            "--dry-run",
            "--repo",
            "realname",
            "--to",
            "http://127.0.0.1:1",
        ])
        .env("HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("DRY RUN"))
        .stdout(predicate::str::contains("WROTE NOTHING"))
        // It names the graph a real run would have hit — the fact the operator
        // who filed this was missing.
        .stdout(predicate::str::contains("http://127.0.0.1:1/knot"));

    // And it needs no target at all: validation is in-process, so a checkout with
    // no endpoint anywhere can still ask the question.
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["promote", "--dry-run", "--repo", "realname"])
        .env("HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("WROTE NOTHING"))
        .stdout(predicate::str::contains("needs --to"));
}

/// Repo identity in promoted IRIs comes from `--repo` or the `origin` remote —
/// NEVER the directory name. An agent worktree named `gennaro` once promoted an
/// entire real graph as `code/gennaro/…`: structurally fragmented islands no
/// entity resolution can rejoin, because the IRIs differ, not the labels. So a
/// promotion with neither `--repo` nor an origin REFUSES, naming the flag.
#[test]
fn promote_refuses_dir_name_identity_and_accepts_origin() {
    if !cfg!(feature = "quipu") {
        return; // stub build: promotion is a phase notice, nothing to pin
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.rs"), "pub fn x() -> u32 { 1 }\n").unwrap();
    // Promotion reads the COMMITTED tree (FR-22), so the fixture must commit —
    // a working-tree-only file would (correctly) promote nothing.
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "t@t"],
        &["config", "user.name", "t"],
        &["add", "-A"],
        &["commit", "-qm", "base"],
    ] {
        Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .assert()
            .success();
    }

    // No origin, no --repo: refused BEFORE any network I/O, naming the remedy.
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["promote", "--to", "http://127.0.0.1:1"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--repo"))
        .stderr(predicate::str::contains("repository identity"));

    // A local one-shot responder that answers 400 to whatever arrives: reaching
    // it proves identity resolution SUCCEEDED (the failure moved past the repo
    // check to the write), and a 4xx must fail immediately — no retry loop.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n");
        }
    });

    // With an origin remote, identity derives from its URL basename.
    Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://example.com/owner/realname.git",
        ])
        .current_dir(dir.path())
        .assert()
        .success();
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["promote", "--to", &format!("http://{addr}")])
        .current_dir(dir.path())
        .assert()
        .failure()
        // The failure is the endpoint's 400 — NOT the identity refusal, which
        // proves `origin` satisfied it; and it reports the status directly,
        // which proves a 4xx did not enter the transient-retry loop.
        .stderr(predicate::str::contains("repository identity").not())
        .stderr(predicate::str::contains("status 400"));
    server.join().unwrap();
}

#[test]
fn refs_at_a_position_resolves_one_symbol_where_the_name_resolves_many() {
    // yupana #8 / FR-4. Name lookup over-connects on `build`, `new`, `write` — the
    // reason the position form exists. A caller reading code knows WHERE it is,
    // not which of the twelve same-named symbols it is, so pointing must answer
    // with the one pointed at and not re-expand to the whole name class.
    let dir = project_with(
        "a.rs",
        "struct Alpha;\nimpl Alpha {\n    fn build() -> Self { Alpha }\n}\n\
         struct Beta;\nimpl Beta {\n    fn build() -> Self { Beta }\n}\n",
    );
    let path = dir.path().to_str().unwrap();

    // The name alone cannot separate them.
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["refs", "build", path])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.rs:3").and(predicate::str::contains("a.rs:7")));

    // The position can — and answers with exactly one.
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["refs", "--at", "a.rs:7", path])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.rs:7").and(predicate::str::contains("a.rs:3").not()));
}

#[test]
fn refs_at_refuses_a_column_rather_than_resolving_it_as_a_line() {
    // FR-3, in the parser. The extractor records LINES, so accepting
    // `a.rs:3:9` and answering for line 3 would serve a line-precise answer to
    // a column-precise question — an approximation presented as the finer tier.
    // Refusing names the missing tier instead, and says what to retry.
    let dir = project_with("a.rs", "fn one() {}\nfn two() {}\nfn three() {}\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["refs", "--at", "a.rs:3:9", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("FILE:LINE:COL")
                .and(predicate::str::contains("LSP tier"))
                .and(predicate::str::contains("a.rs:3")),
        );
}

#[test]
fn refs_at_a_line_between_definitions_explains_instead_of_answering_absent() {
    // A position that resolves to nothing must not borrow the vocabulary of "no
    // such symbol" — that is the yupana #76 confident-wrong-answer shape. It says
    // the line falls between definitions, and lists what the file does define.
    let dir = project_with("a.rs", "fn one() {}\n\n\n\nfn two() {}\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["refs", "--at", "a.rs:3", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("no symbol encloses a.rs:3")
                .and(predicate::str::contains("between definitions"))
                .and(predicate::str::contains("one"))
                .and(predicate::str::contains("two")),
        );
}

#[test]
fn refs_at_an_unparseable_file_says_so_rather_than_reporting_no_definitions() {
    let dir = project_with("a.rs", "fn one() {}\n");
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["refs", "--at", "nope.rs:1", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "no symbols in the graph for `nope.rs`",
        ));
}

// --- the rule plane is a FAILURE SURFACE, not prose (aegis-hac0) -------------

/// A rule plane that could not be projected must EXIT NON-ZERO.
///
/// This is aegis-hac0's second clause made checkable: "adding a token changes
/// enforcement without a redeploy" is necessary and not sufficient — you must be
/// able to OBSERVE that it took effect. Before this, `yupana status` printed
/// COULD NOT TELL in red and exited 0, so nothing could gate on it and a human
/// had to happen to look. A guard failing open silently is the whole bead.
// REQUIRES the quipu feature: without it the rule plane is `Off` by design,
// not `Degraded`, and status exits 0. Ungated, this failed in the `default`,
// `mcp` and `langs-extra` CI legs — red on main, over a test that was correct.
// Coverage is kept by the `quipu` and `mcp+quipu` matrix legs.
#[cfg(feature = "quipu")]
#[test]
fn status_exits_nonzero_when_the_rule_plane_is_degraded() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("dead.toml");
    // A port nothing listens on: the projection cannot succeed, which is the
    // state the guard fails open in.
    std::fs::write(
        &cfg,
        "[yupana.quipu]\nenabled = true\nendpoint = \"http://127.0.0.1:59999\"\n",
    )
    .unwrap();

    let assert = Command::cargo_bin("yupana")
        .unwrap()
        .args(["status", "--config"])
        .arg(&cfg)
        .assert()
        .code(3);
    // It says the CONSEQUENCE, not just the fault: "could not project" is a fact
    // about yupana; "every edit is sailing through" is what the reader must act on.
    assert.stdout(predicate::str::contains("FAILING OPEN"));
}

/// ...and `degraded` is reported as its own state, distinct from `empty`.
///
/// The two produce the SAME number of enforced rules (zero) and mean opposite
/// things. Collapsing them is how a policy layer goes green-and-dead, which is
/// the failure this bead exists to make impossible.
// REQUIRES the quipu feature: without it the rule plane is `Off` by design,
// not `Degraded`, and status exits 0. Ungated, this failed in the `default`,
// `mcp` and `langs-extra` CI legs — red on main, over a test that was correct.
// Coverage is kept by the `quipu` and `mcp+quipu` matrix legs.
#[cfg(feature = "quipu")]
#[test]
fn a_degraded_plane_is_not_an_empty_one() {
    let dir = tempfile::tempdir().unwrap();
    let dead = dir.path().join("dead.toml");
    std::fs::write(
        &dead,
        "[yupana.quipu]\nenabled = true\nendpoint = \"http://127.0.0.1:59999\"\n",
    )
    .unwrap();
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["status", "--json", "--config"])
        .arg(&dead)
        .assert()
        .code(3)
        .stdout(predicate::str::contains("\"state\": \"degraded\""))
        // and it says how hard it tried, rather than smoothing the flap away
        .stdout(predicate::str::contains("\"attempts\": 3"))
        // no rules => no digest. An unknown rule set must not report an identity.
        .stdout(predicate::str::contains("\"digest\": null"));

    // The graph plane being OFF is a CONFIGURATION, not a fault: exit 0.
    let off = dir.path().join("off.toml");
    std::fs::write(&off, "[yupana.quipu]\nenabled = false\n").unwrap();
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["status", "--json", "--config"])
        .arg(&off)
        .assert()
        .success()
        .stdout(predicate::str::contains("\"state\": \"off\""))
        // ...and it is still honest that nothing is verified.
        .stdout(predicate::str::contains("\"verification\": \"unsigned\""));
}
