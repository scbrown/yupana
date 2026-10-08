use super::*;

#[test]
fn the_bundle_is_valid_owned_by_yupana_and_versioned_by_the_build() {
    let b = bundle();
    assert_eq!(b["schema"], "st.hook-bundle/1");
    assert_eq!(b["name"], "yupana");
    assert_eq!(b["owner"], "yupana");
    assert_eq!(b["version"], env!("CARGO_PKG_VERSION"));
    assert!(!hooks_for(&b, Harness::Claude).is_empty());
    assert!(!hooks_for(&b, Harness::Codex).is_empty());
}

#[test]
fn every_codex_hook_declares_evidence_so_st_can_grade_firing() {
    // aegis-5s32or.6: a codex hook without evidence can be live but never
    // graded firing; the probe found yupana's pre-edit in exactly that state.
    for h in bundle()["hooks"].as_array().unwrap() {
        if h["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "codex")
        {
            assert!(
                h["evidence"]["command"].is_string(),
                "{} has no evidence",
                h["command"]
            );
        }
    }
}

#[test]
fn merge_is_idempotent_and_keeps_foreign_hooks() {
    let b = bundle();
    let mut cfg = json!({"hooks": {"PostToolUse": [
        {"matcher": "Bash", "hooks": [{"type": "command", "command": "other-tool hook"}]}
    ]}, "model": "x"});
    let want = hooks_for(&b, Harness::Claude).len();
    assert_eq!(merge(&mut cfg, &b, Harness::Claude), want);
    assert_eq!(
        merge(&mut cfg, &b, Harness::Claude),
        0,
        "second install adds nothing"
    );
    assert_eq!(present(&cfg, &b, Harness::Claude), (want, want));
    assert_eq!(cfg["model"], "x");
    let bash = cfg["hooks"]["PostToolUse"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["matcher"] == "Bash")
        .unwrap();
    assert!(bash["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h["command"] == "other-tool hook"));
}

#[test]
fn remove_takes_only_yupanas_hooks_and_drops_empty_groups() {
    let b = bundle();
    let mut cfg = json!({"hooks": {"PostToolUse": [
        {"matcher": "Bash", "hooks": [{"type": "command", "command": "other-tool hook"}]}
    ]}});
    merge(&mut cfg, &b, Harness::Claude);
    let n = hooks_for(&b, Harness::Claude).len();
    assert_eq!(remove(&mut cfg, &b, Harness::Claude), n);
    assert_eq!(present(&cfg, &b, Harness::Claude).0, 0);
    assert_eq!(
        cfg["hooks"]["PostToolUse"].as_array().unwrap().len(),
        1,
        "foreign group survives"
    );
    assert!(
        cfg["hooks"].get("PostToolUseFailure").is_none(),
        "emptied event is dropped"
    );
}

#[test]
fn codex_config_round_trips_through_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "model = \"gpt\"\n[mcp_servers.x]\ncommand = \"x\"\n").unwrap();
    let b = bundle();
    let mut cfg = read_config(&path, Harness::Codex).unwrap();
    assert!(merge(&mut cfg, &b, Harness::Codex) > 0);
    write_config(&path, Harness::Codex, &cfg).unwrap();
    let back = read_config(&path, Harness::Codex).unwrap();
    let n = hooks_for(&b, Harness::Codex).len();
    assert_eq!(present(&back, &b, Harness::Codex), (n, n));
    assert_eq!(back["model"], "gpt");
    assert_eq!(back["mcp_servers"]["x"]["command"], "x");
    assert!(
        dir.path().join("config.toml.bak-yupana").exists(),
        "previous file kept"
    );
}

#[test]
fn an_absent_config_reads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let v = read_config(&dir.path().join("nope.json"), Harness::Claude).unwrap();
    assert_eq!(v, json!({}));
}
