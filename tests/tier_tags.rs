//! FR-3 on the CLI surface: `yupana impact` / `callers` / `dataflow --json` carry
//! their provenance `tier` (aegis-8yrn). These served an unlabelled tree-sitter
//! approximation before the fix — no `tier` field — which is what FR-3 exists to
//! forbid, worst of all on `impact` (the blast-radius / trust-boundary surface).
//!
//! Kept out of `tests/cli.rs`, which already sits at the file-size limit.

use assert_cmd::Command;
use predicates::prelude::*;

/// A two-function project (`a` calls `b`) so the call graph is non-empty.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.rs"), "fn a() { b(); }\nfn b() {}\n").unwrap();
    dir
}

fn run(dir: &tempfile::TempDir, args: &[&str]) -> serde_json::Value {
    let out = Command::cargo_bin("yupana")
        .unwrap()
        .args(args)
        .arg(dir.path())
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).expect("stdout is JSON")
}

#[test]
fn impact_json_carries_tier() {
    let dir = project();
    let v = run(&dir, &["impact", "b"]);
    // The bug: this was served with no tier at all. It is the trust-boundary
    // surface, so an unlabelled approximation here is exactly what FR-3 forbids.
    assert_eq!(v["tier"], "treesitter", "impact --json lost its tier: {v}");
}

#[test]
fn impact_json_on_a_missing_symbol_still_carries_tier() {
    // The empty-case hole: a not-found answer has no items to tag, so the tier has
    // to ride on the response itself or the result arrives unlabelled.
    let dir = project();
    let v = run(&dir, &["impact", "does_not_exist"]);
    assert_eq!(v["found"], false);
    assert_eq!(
        v["tier"], "treesitter",
        "not-found impact lost its tier: {v}"
    );
}

#[test]
fn callers_json_carries_tier() {
    let dir = project();
    let v = run(&dir, &["callers", "b"]);
    assert_eq!(v["tier"], "treesitter", "callers --json lost its tier: {v}");
}

#[test]
fn dataflow_json_carries_tier() {
    let dir = project();
    let v = run(&dir, &["dataflow", "a"]);
    assert_eq!(
        v["tier"], "treesitter",
        "dataflow --json lost its tier: {v}"
    );
}

#[test]
fn refs_not_found_message_is_unchanged() {
    // Guard the shared not_found() edit: adding a tier field must not disturb the
    // human-readable path other commands rely on.
    let dir = project();
    Command::cargo_bin("yupana")
        .unwrap()
        .args(["callers", "does_not_exist"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("not found in the call graph"));
}

#[test]
fn status_json_advertises_only_implemented_tiers() {
    // `yupana status` used to push "lsp"/"cpg" onto the advertised tier
    // list under empty Cargo features that gated no code, so `--features lsp` made
    // the tool claim a precision tier it did not have. status now advertises a tier
    // only when something can actually produce one.
    //
    // `engine-state` (FR-35) is the one tier that varies by build, and it varies
    // WITH ITS ENGINE rather than with a bare flag: `game-state` gates
    // `src/state/`, which is the ingestion path itself. Asserted in both
    // directions — advertising it without the engine repeats the empty-feature
    // lie, and omitting it WITH the engine sends a consumer looking for a tier
    // this binary really serves.
    // Pin the graph plane OFF (aegis-0upyu). This test is about which TIERS the
    // build advertises, which has nothing to do with the graph — but bare
    // `status` layers the developer's `~/.config/bobbin/config.toml`, which on
    // this fleet points at a live endpoint, and `status` EXITS 3 when it cannot
    // project (aegis-hac0). So `.success()` here was really asserting that a
    // shared service answered three times within ~2s each, and the test failed
    // with `code=3` and an empty stderr whenever it did not.
    let dir = tempfile::tempdir().unwrap();
    let pinned = dir.path().join("pinned.toml");
    std::fs::write(&pinned, "[yupana.quipu]\nenabled = false\n").unwrap();
    let out = Command::cargo_bin("yupana")
        .unwrap()
        .args(["status", "--json", "--config"])
        .arg(&pinned)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    let tiers = v["tiers"].as_array().expect("tiers is an array").clone();
    assert!(tiers.contains(&serde_json::json!("treesitter")));
    assert_eq!(
        tiers.contains(&serde_json::json!("lsp")),
        cfg!(feature = "lsp"),
        "lsp must be advertised exactly when its engine is compiled"
    );
    assert!(!tiers.contains(&serde_json::json!("cpg")));
    assert_eq!(
        tiers.contains(&serde_json::json!("engine-state")),
        cfg!(feature = "game-state"),
        "status advertised a tier out of step with its engine: {v}"
    );
}
