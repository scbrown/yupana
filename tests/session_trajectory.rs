//! Exercise real hook processes with isolated homes, cache and session markers.
#![cfg(feature = "quipu")]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn setup(root: &Path) -> serde_json::Value {
    std::fs::create_dir_all(root.join(".bobbin")).unwrap();
    std::fs::write(root.join(".bobbin/config.toml"), "[yupana.policy]\nmode = 'advise'\n[yupana.quipu]\nenabled = true\nendpoint = 'http://127.0.0.1:1'\nprojection_cache_ttl_secs = 3600\n").unwrap();
    serde_json::json!({
        "version":2, "written_at":yupana::projection_cache::now_secs(),
        "endpoint":"http://127.0.0.1:1", "policies":[], "text_rules":[],
        "trajectory_policies":[{
            "id":"https://example.org/policy/delegate", "label":"delegate line",
            "trigger":{"programs":["br","bd"],"verbs":["create"]},
            "ordering":"command-before-edit", "tier":"warn", "once_per":"session",
            "effect":"warn", "verification_point":"PAA",
            "rationale":"GOVERNED_TEST_NOTICE: check ownership before continuing."
        }]
    })
}

fn save(root: &Path, cache: &serde_json::Value) {
    std::fs::write(root.join("projection.json"), cache.to_string()).unwrap();
}

fn hook(root: &Path, event: &str, session: &str, command: Option<&str>) -> String {
    let binary = std::env::var_os("YUPANA_TRAJECTORY_TEST_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_yupana").into());
    let payload = serde_json::json!({
        "session_id":session, "cwd":root,
        "tool_name":if command.is_some() {"Bash"} else {"Edit"},
        "tool_input":if let Some(command) = command {
            serde_json::json!({"command":command})
        } else {
            serde_json::json!({"file_path":root.join("note.txt"),"old_string":"a","new_string":"b"})
        }
    });
    let mut child = Command::new(binary)
        .args(["hook", event])
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("YUPANA_PROJECTION_CACHE_PATH", root.join("projection.json"))
        .env("YUPANA_FAILOPEN_MARKER_DIR", root.join("markers"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn installed_hook_contract_and_data_only_trigger_change() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut cache = setup(root);
    save(root, &cache);
    assert!(!hook(root, "post-edit", "control", None).contains("GOVERNED_TEST_NOTICE"));
    hook(
        root,
        "pre-bash",
        "filed",
        Some("br --db /tmp/store create 'title'"),
    );
    assert!(hook(root, "post-edit", "filed", None).contains("GOVERNED_TEST_NOTICE"));
    assert!(!hook(root, "post-edit", "filed", None).contains("GOVERNED_TEST_NOTICE"));
    hook(
        root,
        "pre-bash",
        "existing",
        Some("br comments add item --file note"),
    );
    assert!(!hook(root, "post-edit", "existing", None).contains("GOVERNED_TEST_NOTICE"));

    cache["trajectory_policies"][0]["trigger"] =
        serde_json::json!({"programs":["tracker"],"verbs":["file"]});
    cache["trajectory_policies"][0]["once_per"] = serde_json::json!("edit");
    save(root, &cache);
    hook(root, "pre-bash", "custom", Some("br create title"));
    assert!(!hook(root, "post-edit", "custom", None).contains("GOVERNED_TEST_NOTICE"));
    hook(root, "pre-bash", "custom", Some("tracker file title"));
    assert!(hook(root, "post-edit", "custom", None).contains("GOVERNED_TEST_NOTICE"));
    assert!(hook(root, "post-edit", "custom", None).contains("GOVERNED_TEST_NOTICE"));
    cache["trajectory_policies"] = serde_json::json!([]);
    save(root, &cache);
    assert!(!hook(root, "post-edit", "custom", None).contains("GOVERNED_TEST_NOTICE"));
}

#[test]
fn missing_expired_and_block_tier_caches_report_unevaluated() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut cache = setup(root);
    cache["trajectory_policies"][0]["tier"] = serde_json::json!("block");
    save(root, &cache);
    let block = hook(root, "post-edit", "block", None);
    assert!(block.contains("NOT EVALUATED"), "{block}");
    assert!(block.contains("pre-edit enforcement point"), "{block}");
    cache.as_object_mut().unwrap().remove("trajectory_policies");
    save(root, &cache);
    assert!(hook(root, "post-edit", "old", None).contains("predates the trajectory channel"));
    cache["trajectory_policies"] = serde_json::json!([]);
    save(root, &cache);
    assert!(!hook(root, "post-edit", "empty", None).contains("NOT EVALUATED"));
    cache["written_at"] = serde_json::json!(1);
    save(root, &cache);
    assert!(hook(root, "post-edit", "expired", None).contains("NOT EVALUATED"));
}
