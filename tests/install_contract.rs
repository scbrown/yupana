//! Execute the source-derived checker against real and selectively stale CLIs.
#[path = "../examples/install-contract.rs"]
#[allow(dead_code)] // The example's main is not the integration-test entrypoint.
mod contract;

#[test]
fn the_current_binary_matches_its_compiled_cli_contract() {
    let count = contract::verify(std::path::Path::new(env!("CARGO_BIN_EXE_yupana"))).unwrap();
    assert!(count > 1, "the check must cover more than root help");
}

#[test]
fn a_hidden_staging_filename_matches_its_compiled_cli_contract() {
    let dir = tempfile::tempdir().unwrap();
    let candidate = dir.path().join(".yupana-candidate.test");
    std::fs::copy(env!("CARGO_BIN_EXE_yupana"), &candidate).unwrap();
    assert!(contract::verify(&candidate).unwrap() > 1);
}

#[cfg(all(unix, feature = "quipu"))]
#[test]
fn a_missing_nested_verb_fails_even_when_root_help_and_version_match() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let candidate = dir.path().join("yupana");
    let real = env!("CARGO_BIN_EXE_yupana").replace('\'', "'\"'\"'");
    std::fs::write(&candidate, format!(
        "#!/bin/bash\nif [ \"${{1:-}}\" = share ] && [ \"${{2:-}}\" = reshare ]; then exit 0; fi\nexec -a \"$0\" '{real}' \"$@\"\n"
    )).unwrap();
    std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o755)).unwrap();
    let failure = contract::verify(&candidate).unwrap_err().to_string();
    assert!(failure.contains("share reshare --help"), "{failure}");
}
