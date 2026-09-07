//! Public dataflow opt-in and the default-build refusal arm.
use assert_cmd::Command;
use predicates::prelude::*;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"),
        "fn pass(p:i32)->i32 { p }\nfn sink(s:i32) {}\nfn main(a:i32,flag:bool) {\n if flag {\n let x=pass(a);\n sink(x);\n }\n}\n").unwrap();
    dir
}

#[test]
fn interprocedural_build_contract() {
    let dir = fixture();
    let mut command = Command::cargo_bin("yupana").unwrap();
    command.args(["dataflow", "main"]).arg(dir.path()).args([
        "--interprocedural",
        "--var",
        "a",
        "--forward",
        "--hops",
        "32",
        "--json",
    ]);
    if cfg!(feature = "cpg") {
        let output = command.assert().success().get_output().stdout.clone();
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["tier"], "cpg");
        assert_eq!(value["found"], true);
        assert_eq!(value["truncated"], false);
        assert!(value["flow"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "s" && s["tier"] == "cpg"));
        assert!(value["control_edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["line"] == 5
                && e["condition_line"] == 4
                && e["relation"] == "bobbin:controlDependsOn"));
    } else {
        command.assert().failure().stderr(predicate::str::contains(
            "requires a build with the cpg feature",
        ));
    }
}

#[test]
fn default_query_stays_intra_procedural_even_in_cpg_build() {
    let dir = fixture();
    let out = Command::cargo_bin("yupana")
        .unwrap()
        .args(["dataflow", "main"])
        .arg(dir.path())
        .args(["--var", "a", "--forward", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["tier"], "treesitter");
    assert!(value.get("control_edges").is_none());
}
