//! Tests for `config` — the layered `[yupana]` table, its per-key overlay, and
//! the `--config` override contract. Child module of `config` (`super::*`
//! reaches its private helpers); size-exempt (`_test.rs`).

use super::*;

#[test]
fn defaults_are_sensible() {
    let config = YupanaConfig::default();
    assert_eq!(config.base_ref, "main");
    assert_eq!(config.serve.mcp_http_port, 3040);
    // The implemented model (§9.4 / GH #4). `"named_graph"` is preferred but
    // blocked on quipu#36, and a default that refuses every promotion is not a
    // default. `promote_branch_test::the_config_default_is_the_implemented_model`
    // is the other half of this pin.
    assert_eq!(config.quipu.branch_model, "qualifier");
    assert!(config.languages.contains(&"rust".to_string()));
}

#[test]
fn load_missing_project_returns_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let config = YupanaConfig::load(dir.path()).unwrap();
    assert_eq!(config.base_ref, "main");
}

#[test]
fn load_reads_yupana_table() {
    let dir = tempfile::tempdir().unwrap();
    let bobbin = dir.path().join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    std::fs::write(
        bobbin.join("config.toml"),
        "[yupana]\nbase_ref = \"develop\"\n",
    )
    .unwrap();
    let config = YupanaConfig::load(dir.path()).unwrap();
    assert_eq!(config.base_ref, "develop");
    // Unspecified keys fall back to defaults.
    assert_eq!(config.serve.mcp_http_port, 3040);
}

/// The shape a fleet actually deploys: capability scopes live in ONE
/// user-level config so six workspaces cannot drift, and each workspace
/// sets its own unrelated keys. The workspace file must not take the policy
/// down with it — a guard that silently stops enforcing is indistinguishable
/// from a guard finding nothing wrong.
#[test]
fn a_project_config_does_not_wipe_user_level_policy() {
    let user = tempfile::tempdir().unwrap();
    let user_config = user.path().join("config.toml");
    std::fs::write(
        &user_config,
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.weaver]\nallow_paths = [\"src/**\"]\n",
    )
    .unwrap();

    let project = tempfile::tempdir().unwrap();
    let bobbin = project.path().join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    // Sets one unrelated key; says nothing about policy.
    std::fs::write(
        bobbin.join("config.toml"),
        "[yupana]\nbase_ref = \"develop\"\n",
    )
    .unwrap();

    let config = YupanaConfig::load_layered(Some(&user_config), project.path()).unwrap();
    // The workspace's own key wins...
    assert_eq!(config.base_ref, "develop");
    // ...without disarming the guard.
    assert_eq!(config.policy.mode, crate::policy::Mode::Enforce);
    let scope = config
        .policy
        .scope_for(Some("weaver"))
        .expect("user-level scope must survive a project config");
    assert_eq!(scope.allow_paths, vec!["src/**".to_string()]);
}

/// Merging must not cost precedence: the project still wins key-for-key.
#[test]
fn a_project_config_overrides_the_same_key() {
    let user = tempfile::tempdir().unwrap();
    let user_config = user.path().join("config.toml");
    std::fs::write(
        &user_config,
        "[yupana]\nbase_ref = \"main\"\n[yupana.policy]\nmode = \"enforce\"\n",
    )
    .unwrap();

    let project = tempfile::tempdir().unwrap();
    let bobbin = project.path().join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    std::fs::write(
        bobbin.join("config.toml"),
        "[yupana.policy]\nmode = \"off\"\n",
    )
    .unwrap();

    let config = YupanaConfig::load_layered(Some(&user_config), project.path()).unwrap();
    assert_eq!(config.policy.mode, crate::policy::Mode::Off);
    // Untouched keys from the user config survive the override.
    assert_eq!(config.base_ref, "main");
}

#[test]
fn policy_mode_provenance_names_a_workspace_lowering() {
    let user = tempfile::tempdir().unwrap();
    let user_config = user.path().join("config.toml");
    std::fs::write(&user_config, "[yupana.policy]\nmode = \"enforce\"\n").unwrap();
    let project = tempfile::tempdir().unwrap();
    let bobbin = project.path().join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    std::fs::write(
        bobbin.join("config.toml"),
        "[yupana.policy]\nmode = \"off\"\n",
    )
    .unwrap();

    let effective = YupanaConfig::load_layered(Some(&user_config), project.path()).unwrap();
    let provenance = policy_mode_provenance_from_paths(
        Some(&user_config),
        &bobbin.join("config.toml"),
        effective.policy.mode,
    )
    .unwrap();
    assert!(provenance.lowered_by_project);
    assert_eq!(provenance.user_mode, Some(crate::policy::Mode::Enforce));
    assert_eq!(provenance.effective, crate::policy::Mode::Off);
}

/// A scope narrowed by the user config must not be widened by a workspace
/// appending to it — arrays replace, they do not accumulate.
#[test]
fn a_project_config_replaces_rather_than_widens_allow_paths() {
    let user = tempfile::tempdir().unwrap();
    let user_config = user.path().join("config.toml");
    std::fs::write(
        &user_config,
        "[yupana.policy]\nmode = \"enforce\"\n\
         [yupana.policy.scopes.weaver]\nallow_paths = [\"src/**\"]\n",
    )
    .unwrap();

    let project = tempfile::tempdir().unwrap();
    let bobbin = project.path().join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    std::fs::write(
        bobbin.join("config.toml"),
        "[yupana.policy.scopes.weaver]\nallow_paths = [\"docs/**\"]\n",
    )
    .unwrap();

    let config = YupanaConfig::load_layered(Some(&user_config), project.path()).unwrap();
    let scope = config.policy.scope_for(Some("weaver")).unwrap();
    assert_eq!(scope.allow_paths, vec!["docs/**".to_string()]);
}

/// `resolve(Some(path), ..)` reads exactly that file over defaults and never
/// consults the ambient project config — the core of the `--config` fix.
#[test]
fn resolve_with_override_reads_the_named_file_not_the_cwd() {
    let project = tempfile::tempdir().unwrap();
    let bobbin = project.path().join(".bobbin");
    std::fs::create_dir_all(&bobbin).unwrap();
    std::fs::write(
        bobbin.join("config.toml"),
        "[yupana]\nbase_ref = \"from-cwd\"\n",
    )
    .unwrap();

    let other = project.path().join("other.toml");
    std::fs::write(&other, "[yupana]\nbase_ref = \"from-flag\"\n").unwrap();

    let overridden = YupanaConfig::resolve(Some(&other), project.path()).unwrap();
    assert_eq!(overridden.base_ref, "from-flag");

    // And without the override, discovery still finds the cwd config.
    let discovered = YupanaConfig::resolve(None, project.path()).unwrap();
    assert_eq!(discovered.base_ref, "from-cwd");
}

/// A `--config` path that does not exist is a loud error, not a silent
/// fall-back to discovery.
#[test]
fn load_from_a_missing_path_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.toml");
    let err = YupanaConfig::load_from(&missing).unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

/// A file that exists but has no `[yupana]` table is a valid request for
/// defaults, not an error.
#[test]
fn load_from_a_file_without_a_yupana_table_yields_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.toml");
    std::fs::write(&path, "[something_else]\nkey = 1\n").unwrap();
    let config = YupanaConfig::load_from(&path).unwrap();
    assert_eq!(config.base_ref, "main");
}

/// `serve.read_only` refuses a write, and is silent otherwise — the guard the
/// docs promised and did not perform (aegis-ltjo).
#[test]
fn read_only_guards_a_write() {
    let mut config = YupanaConfig::default();
    assert!(
        config.write_guard("promotion").is_ok(),
        "default must allow writes"
    );

    config.serve.read_only = true;
    let err = config.write_guard("promotion").unwrap_err().to_string();
    assert!(
        err.contains("read_only"),
        "the error must name the key: {err}"
    );
    assert!(
        err.contains("promotion"),
        "the error must name the operation: {err}"
    );
}

/// THE anti-drift guard the bead asked for: every `pub` field on the config
/// structs must either be READ somewhere outside config.rs, or be listed here
/// with the phase that will honour it. A new inert key — documented, settable,
/// doing nothing — fails this test until it is wired OR explicitly declared
/// phased. That is the whole defect: a control that looks live and is not.
#[test]
fn every_config_key_is_read_or_explicitly_phased() {
    // key -> why it is not yet read. Anything not here MUST have a reader.
    let phased: &[(&str, &str)] = &[
        ("enable_lsp", "Phase 2/3 — LSP tier not built"),
        ("enable_cpg", "Phase 2 — CPG tier not built"),
        ("lsp_on", "LSP tier not built"),
        // The [yupana.tenancy] keys (tenancy / max_overlays /
        // high_fanin_threshold / overlay_eviction) are now LIVE — read by
        // TenantRegistry's FR-18 lifecycle (yupana #6) — so they are no longer
        // phased. The guard's "allowlist must not rot" check enforces that.
        // `promote_on` WAS listed here, on the accurate reason that no trigger
        // point was wired. GH #3 wired one: `yupana promote --trigger
        // commit|merge` is the event a git hook or CI step declares, and
        // `cli_promote::trigger_admits` reads the key to decide whether that
        // event promotes (`src/promote_trigger.rs`). It is delisted because it
        // is live, which is the delisting this guard's rot check exists to
        // force.
        //
        // `shapes_path` read "Phase 4 — Quipu promotion not built" until
        // 2026-08. That was false: `src/promote.rs` is a working
        // implementation with two CI arms, graded ✅ in the spec's Appendix D.
        // The key really is unread, so the guard was right to exempt it — but
        // for a reason that had nothing to do with the one stated, and a reason
        // nobody can falsify is the same defect this guard exists to catch, one
        // level up. The real blocker:
        (
            "shapes_path",
            "unhonourable as written — promotion validates against \
             `promote::CODE_EDGE_SHAPES`, `include_str!`'d at promote.rs:43, \
             so the shapes are compiled into the binary and no directory on \
             disk can change what gates a write. Wiring this key would mean \
             deciding to load shapes at runtime, which promote.rs:39-43 \
             argues against on purpose (GH #8)",
        ),
    ];

    let manifest = env!("CARGO_MANIFEST_DIR");
    let config_rs = std::path::Path::new(manifest).join("src/config.rs");
    let this = std::fs::read_to_string(&config_rs).unwrap();

    // Every `pub <ident>:` field declared in this file.
    let fields: Vec<&str> = this
        .lines()
        .filter_map(|l| l.trim().strip_prefix("pub "))
        .filter_map(|rest| rest.split(':').next())
        .filter(|ident| ident.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .filter(|ident| !ident.is_empty())
        .collect();
    assert!(fields.len() >= 15, "field parse looks wrong: {fields:?}");

    // All of src EXCEPT config.rs AND this file — the "is it read anywhere
    // else" corpus. Excluding this file matters: it names every phased key in
    // the allowlist above, so counting itself would make the corpus
    // self-satisfying and the guard would report a phased key as wired the
    // moment it was listed as NOT wired.
    let skip: &[std::path::PathBuf] = &[
        config_rs.clone(),
        std::path::Path::new(manifest).join("src/config_test.rs"),
    ];
    let mut elsewhere = String::new();
    collect_rs(
        std::path::Path::new(manifest).join("src").as_path(),
        skip,
        &mut elsewhere,
    );

    for field in fields {
        let read = elsewhere.contains(field);
        let is_phased = phased.iter().any(|(k, _)| *k == field);
        assert!(
            read || is_phased,
            "config key `{field}` is read by NOTHING outside config.rs and is \
             not in the phased allowlist. Wire it, or add it to `phased` with \
             the phase that will honour it — a documented, settable, inert key \
             is the defect aegis-ltjo closed."
        );
    }

    // And the allowlist must not rot the other way: a key listed as phased
    // that HAS gained a reader should be removed from the list.
    for (key, _why) in phased {
        assert!(
            !elsewhere.contains(key),
            "`{key}` is in the phased allowlist but now HAS a reader outside \
             config.rs — remove it from the list and mark it live in the docs."
        );
    }
}

fn collect_rs(dir: &std::path::Path, skip: &[std::path::PathBuf], out: &mut String) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, skip, out);
        } else if path.extension().is_some_and(|e| e == "rs") && !skip.contains(&path) {
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push_str(&text);
            }
        }
    }
}
