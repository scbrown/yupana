//! `yupana exemplar` (bobbin-9k3): the policy-by-example drafting feed.

use assert_cmd::Command;

#[test]
fn exemplar_emits_selector_and_all_three_tiers_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x.rs");
    std::fs::write(&file, "fn f() {\n    // see ABC-123\n}\n").unwrap();
    let out = Command::cargo_bin("yupana")
        .unwrap()
        .args(["exemplar", "ABC-123", "--file"])
        .arg(&file)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    assert_eq!(v["selector"]["node_kind"], "line_comment");
    assert_eq!(v["selector"]["query"], "(line_comment) @c");
    let tiers: Vec<&str> = v["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["tier"].as_str().unwrap())
        .collect();
    assert_eq!(tiers, ["tree-sitter+graph", "lexical", "embedding"]);
    // FR-3: the extraction itself is tier-tagged.
    assert_eq!(v["tier"], "treesitter");
}

#[test]
fn exemplar_reads_the_newest_spooled_denial() {
    let dir = tempfile::tempdir().unwrap();
    let spool = dir.path().join("verdicts.jsonl");
    std::fs::write(
        &spool,
        format!(
            "{}\n{}\n",
            serde_json::json!({
                "ts": 1, "predicate_id": "old-rule", "target_ref": "a.md",
                "turtle": "…", "denied_excerpt": "older denial",
            }),
            serde_json::json!({
                "ts": 2, "predicate_id": "no-hostname", "target_ref": "b.md",
                "turtle": "…", "denied_excerpt": "db1.internal leaked",
            }),
        ),
    )
    .unwrap();
    let out = Command::cargo_bin("yupana")
        .unwrap()
        .args(["exemplar", "--spool"])
        .arg(&spool)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    assert_eq!(v["offending"], "db1.internal leaked");
}

#[test]
fn exemplar_with_nothing_to_draft_from_refuses_loudly() {
    Command::cargo_bin("yupana")
        .unwrap()
        .arg("exemplar")
        .assert()
        .failure()
        .stderr(predicates::str::contains("offending TEXT"));
}
