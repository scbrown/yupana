use super::*;

fn payload(patch: &str) -> Value {
    json!({"tool_name":"apply_patch", "session_id":"session-control", "tool_use_id":"call-control", "tool_input":{"command":patch}})
}

#[test]
fn native_multifile_patch_preserves_correlation_and_only_added_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("one.rs"), "// existing\nfn old() {}\n").unwrap();
    let p = payload("*** Begin Patch\n*** Update File: one.rs\n@@\n // existing\n-fn old() {}\n+fn new() {}\n*** Add File: two.rs\n+fn two() {}\n*** End Patch");
    let result = inputs(&p, dir.path()).unwrap();
    assert_eq!(result.len(), 2);
    let one: Value = serde_json::from_str(&result[0]).unwrap();
    assert_eq!(one["session_id"], "session-control");
    assert_eq!(one["tool_use_id"], "call-control");
    assert_eq!(one["tool_name"], "apply_patch");
    assert_eq!(one["tool_input"]["content"], "// existing\nfn new() {}\n");
    assert_eq!(one["tool_input"]["new_string"], "fn new() {}\n");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("one.rs")).unwrap(),
        "// existing\nfn old() {}\n"
    );
    assert!(!dir.path().join("two.rs").exists());
}

#[test]
fn deletion_and_move_check_both_scopes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("old"), "one\n").unwrap();
    std::fs::write(dir.path().join("gone"), "gone\n").unwrap();
    let p = payload("*** Begin Patch\n*** Update File: old\n*** Move to: destination\n@@\n-one\n+two\n*** Delete File: gone\n*** End Patch");
    let result = inputs(&p, dir.path()).unwrap();
    assert_eq!(result.len(), 3);
    let rows: Vec<Value> = result
        .iter()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(rows[0]["tool_input"]["content"], "");
    assert_eq!(rows[1]["tool_input"]["content"], "two\n");
    assert_eq!(rows[1]["tool_input"]["new_string"], "two\n");
    assert_eq!(rows[2]["tool_input"]["content"], "");
}

#[test]
fn exact_multiple_hunks_and_end_anchor() {
    let (new, added, _) = update(
        "a\nx\nb\nx\n",
        &["@@", "-a", "+A", "@@", "-x", "+X", "*** End of File"],
    )
    .unwrap();
    assert_eq!(new, "A\nx\nb\nX\n");
    assert_eq!(added, "A\nX\n");
}

#[test]
fn missing_ambiguous_fuzzy_or_invalid_patches_are_unknown() {
    assert!(update("x\nx\n", &["@@", "-x", "+y"]).is_err());
    assert!(update("  x\n", &["@@", "-x", "+y"]).is_err());
    assert!(update("x\n", &["@@", "?bad"]).is_err());
    assert!(update("x\n", &[]).is_err());
    assert!(inputs(&payload("not a patch"), Path::new(".")).is_err());
    assert!(inputs(&payload("*** Begin Patch\n*** End Patch"), Path::new(".")).is_err());
}

#[test]
fn context_free_insertion_appends_and_absolute_paths_stay_absolute() {
    let (new, _, _) = update("old\n", &["@@", "+new"]).unwrap();
    assert_eq!(new, "old\nnew\n");
    assert_eq!(
        absolute(Path::new("ignored"), "/absolute"),
        PathBuf::from("/absolute")
    );
}
