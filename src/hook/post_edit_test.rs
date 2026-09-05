//! Tests for the post-edit hook. Size-exempt (`_test.rs`), the same split
//! `pre_edit`/`pre_edit_test` already uses.

#[cfg(test)]
// Test names here shout the invariant they pin — `is_NEVER_observable`,
// `daemon_EXPECTED_but_DOWN`, `is_DOWN_not_UP`. That capitalisation is the same
// emphasis the prose and comments use throughout this repo, and it is load-
// bearing in a test name: it says which word the assertion turns on. Allowed
// explicitly, and scoped to tests, so the lint stays live everywhere else
// rather than being switched off crate-wide (yupana #83).
#[allow(non_snake_case)]
mod tests {
    use super::super::*;

    #[test]
    fn advises_on_cross_file_impact() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn mid() { leaf(); }\n").unwrap();

        let payload = serde_json::json!({
            "tool_name": "Edit",
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": { "file_path": dir.path().join("a.rs").to_str().unwrap() },
        })
        .to_string();

        let text = advisory_for(&payload, dir.path(), None).expect("expected an advisory");
        assert!(text.contains("leaf"));
        assert!(text.contains("b.rs"));
    }

    #[test]
    fn scopes_advisory_to_the_edited_symbol() {
        // a.rs defines `leaf` (edited) and `other` (untouched), both called from b.rs.
        // The file on disk is POST-edit, as the real hook sees it.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn leaf() {\n    let x = 2;\n}\nfn other() {}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn m() { leaf(); other(); }\n").unwrap();

        let payload = serde_json::json!({
            "tool_name": "Edit",
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": {
                "file_path": dir.path().join("a.rs").to_str().unwrap(),
                "old_string": "let x = 1;",
                "new_string": "let x = 2;",
            },
        })
        .to_string();

        let text = advisory_for(&payload, dir.path(), None).expect("expected an advisory");
        assert!(
            text.contains("leaf"),
            "advises on the edited symbol: {text}"
        );
        assert!(
            !text.contains("other"),
            "must NOT cry wolf on the untouched symbol: {text}"
        );
    }

    #[test]
    fn no_advice_when_edit_is_outside_every_symbol() {
        // A top-of-file comment edit touches no symbol body — the cry-wolf case.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "// header updated\nfn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn m() { leaf(); }\n").unwrap();

        let payload = serde_json::json!({
            "tool_name": "Edit",
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": {
                "file_path": dir.path().join("a.rs").to_str().unwrap(),
                "old_string": "// header",
                "new_string": "// header updated",
            },
        })
        .to_string();
        assert!(
            advisory_for(&payload, dir.path(), None).is_none(),
            "a comment-only edit must not advise on symbols it did not touch"
        );
    }

    #[test]
    fn edited_line_spans_locates_edit_and_falls_back() {
        let src = "aaa\nbbb\nccc\n";
        let ti = ToolInput {
            new_string: Some("bbb".into()),
            ..Default::default()
        };
        assert_eq!(edited_line_spans(&ti, src), Some(vec![(2, 2)]));
        // Deletion → None (unlocatable post-hoc).
        let ti = ToolInput {
            new_string: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(edited_line_spans(&ti, src), None);
        // No diff info (Write) → None → whole-file fallback.
        assert_eq!(edited_line_spans(&ToolInput::default(), src), None);
    }

    #[test]
    fn quiet_when_no_external_callers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn leaf() {}\nfn mid() { leaf(); }\n",
        )
        .unwrap();
        // leaf's only caller (mid) is in the same file → no cross-file impact.
        let payload = serde_json::json!({
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": { "file_path": dir.path().join("a.rs").to_str().unwrap() },
        })
        .to_string();
        assert!(advisory_for(&payload, dir.path(), None).is_none());
    }

    #[test]
    fn quiet_on_non_rust_or_garbage() {
        assert!(advisory_for("not json", Path::new("."), None).is_none());
        let payload = serde_json::json!({ "tool_input": { "file_path": "README.md" } }).to_string();
        assert!(advisory_for(&payload, Path::new("."), None).is_none());
    }

    // Project config expecting a daemon at 127.0.0.1:port. Written as the
    // PROJECT config so it wins over any developer user config for these keys.
    fn write_daemon_config(root: &Path, port: u16) {
        let bobbin = root.join(".bobbin");
        std::fs::create_dir_all(&bobbin).unwrap();
        std::fs::write(
            bobbin.join("config.toml"),
            format!(
                "[yupana.serve]\nuse_daemon = true\nbind_address = \"127.0.0.1\"\n\
                 mcp_http_port = {port}\n"
            ),
        )
        .unwrap();
    }

    fn edit_payload(root: &Path, file: &str) -> String {
        serde_json::json!({
            "tool_name": "Edit",
            "cwd": root.to_str().unwrap(),
            "tool_input": { "file_path": root.join(file).to_str().unwrap() },
        })
        .to_string()
    }

    #[test]
    fn daemon_EXPECTED_but_DOWN_falls_back_to_the_transient_advisory() {
        // Port 1 never listens. The advisory must still be produced (transient
        // fallback) — a down daemon degrades performance, never the advisory.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn mid() { leaf(); }\n").unwrap();
        write_daemon_config(dir.path(), 1);

        let text = advisory_for(&edit_payload(dir.path(), "a.rs"), dir.path(), None)
            .expect("the transient fallback must still advise");
        assert!(text.contains("leaf"));
        assert!(text.contains("b.rs"));
    }

    // Serving the router needs axum (`mcp` feature); the down/fallback quadrant
    // above runs feature-free.
    #[cfg(feature = "mcp")]
    #[tokio::test(flavor = "multi_thread")]
    async fn daemon_up_and_same_root_advises_from_the_RESIDENT_view_and_feeds_the_overlay() {
        use crate::daemon::{http, ResidentEngine};
        // The tenant layer anchors to a COMMIT, so the fixture is a real repo.
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.rs"), "fn leaf() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn mid() { leaf(); }\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "base"]);

        let engine = ResidentEngine::build(dir.path(), None).unwrap();
        let observer = engine.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, http::router(engine)).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        write_daemon_config(dir.path(), port);

        // A REAL edit to a.rs (differs from base, still defines `leaf`): the
        // daemon reads it from disk, so it must not match the baseline or the
        // FR-15 base-hit would make it a no-op.
        std::fs::write(dir.path().join("a.rs"), "fn leaf() {}\nfn added() {}\n").unwrap();
        // `late.rs` is UNCOMMITTED and untouched: the tenant view composes
        // over base@HEAD, so it must not appear — a transient build would see
        // it. Its absence below proves who answered.
        std::fs::write(dir.path().join("late.rs"), "fn late() { leaf(); }\n").unwrap();

        let payload = edit_payload(dir.path(), "a.rs");
        let root = dir.path().to_path_buf();
        let text = tokio::task::spawn_blocking(move || advisory_for(&payload, &root, Some("t1")))
            .await
            .unwrap()
            .expect("an up, same-root daemon must advise");
        assert!(text.contains("b.rs"), "{text}");
        assert!(
            !text.contains("late.rs"),
            "`late.rs` is uncommitted and untouched — its presence means a \
             transient build answered, not the tenant view: {text}"
        );

        // And the advisory FED the overlay (FR-30): the daemon now holds the
        // edit as tenant t1's touch of a.rs.
        let reg = observer.registry().expect("repo ⇒ tenant layer");
        let reg = reg.read().unwrap();
        let overlay = reg.overlay("t1").expect("the edit created t1's overlay");
        assert!(overlay.is_touched("a.rs"));
    }
}
