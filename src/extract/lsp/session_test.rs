use super::*;

fn position(file: &str, text: &str, line: usize, needle: &str) -> Position {
    Position {
        file: file.into(),
        line,
        column: text.lines().nth(line - 1).unwrap().find(needle).unwrap() + 1,
    }
}

fn installed(program: &str) -> bool {
    let available = Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !available {
        assert_ne!(
            std::env::var("YUPANA_REQUIRE_LSP_SERVERS").as_deref(),
            Ok("1"),
            "required real language server is unavailable: {program}"
        );
        eprintln!("SKIP real LSP test: {program} is unavailable");
    }
    available
}

fn exercise(root: &Path, file: &str, text: &str, server: &str) {
    let mut session = Session::start(root, Path::new(file)).unwrap().unwrap();
    let call = position(file, text, 3, "target()");
    let definition = session.locations(&call, Query::Definition).unwrap();
    assert_eq!(definition.tier, crate::types::Tier::Lsp);
    assert_eq!(definition.value.len(), 1);
    assert_eq!(definition.value[0].start_line, 2);
    assert_eq!(serde_json::to_value(&definition).unwrap()["tier"], "lsp");

    let declaration = position(file, text, 2, "target");
    let references = session.locations(&declaration, Query::References).unwrap();
    assert!(references.value.iter().any(|v| v.start_line == 3));
    let variable = position(file, text, 3, "result.value");
    let cold_type = std::time::Instant::now();
    let types = session.locations(&variable, Query::TypeDefinition).unwrap();
    eprintln!(
        "REAL LSP {server} first type-definition: {:?}",
        cold_type.elapsed()
    );
    assert!(types.value.iter().any(|v| v.start_line == 1), "{types:?}");
    let hover = session.hover(&call).unwrap();
    assert_eq!(hover.tier, crate::types::Tier::Lsp);
    assert!(hover.value.to_string().contains("target"), "{hover:?}");
    let symbols = session.document_symbols(file).unwrap();
    assert!(symbols.value.to_string().contains("target"), "{symbols:?}");
    let workspace = session.workspace_symbols("target").unwrap();
    assert!(
        workspace.value.to_string().contains("target"),
        "{workspace:?}"
    );

    for (name, at, query) in [
        ("definition", &call, Query::Definition),
        ("references", &declaration, Query::References),
    ] {
        let mut samples = Vec::new();
        for _ in 0..20 {
            let start = std::time::Instant::now();
            let result = session.locations(at, query).unwrap();
            samples.push(start.elapsed());
            assert!(!result.value.is_empty());
        }
        samples.sort();
        let p95 = samples[18];
        eprintln!("REAL LSP {server} {name}: n=20 warm p95={p95:?}");
        assert!(
            p95 < Duration::from_secs(1),
            "warm {server} {name}: {p95:?}"
        );
    }

    // One retained process must see saved contents, not its initial didOpen.
    let updated = text.replace("target", "renamed");
    std::fs::write(root.join(file), &updated).unwrap();
    let new_call = position(file, &updated, 3, "renamed()");
    let definition = session.locations(&new_call, Query::Definition).unwrap();
    assert_eq!(definition.value[0].start_line, 2);
    assert!(session
        .hover(&new_call)
        .unwrap()
        .value
        .to_string()
        .contains("renamed"));
    assert!(session
        .document_symbols(file)
        .unwrap()
        .value
        .to_string()
        .contains("renamed"));
}

#[test]
fn real_rust_session_covers_semantics_updates_and_warm_p95() {
    if !installed("rust-analyzer") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname='warm_lsp_fixture'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    let text = "pub struct Thing { pub value: usize }\npub fn target() -> Thing { Thing { value: 1 } }\npub fn caller() -> usize { let result = target(); result.value }\n";
    std::fs::write(dir.path().join("src/lib.rs"), text).unwrap();
    exercise(dir.path(), "src/lib.rs", text, "rust-analyzer");
}

#[test]
fn real_typescript_session_covers_semantics_updates_and_warm_p95() {
    if !installed("typescript-language-server") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true},"include":["*.ts"]}"#,
    )
    .unwrap();
    let text = "export class Thing { value = 1; }\nexport function target(): Thing { return new Thing(); }\nexport function caller(): number { const result = target(); return result.value; }\n";
    std::fs::write(dir.path().join("fixture.ts"), text).unwrap();
    exercise(dir.path(), "fixture.ts", text, "typescript-language-server");
}

#[test]
fn absent_server_is_an_error_not_an_lsp_fact() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server {
        program: dir.path().join("absent-server").to_string_lossy().into(),
        args: vec![],
        language_id: "rust".into(),
    };
    assert!(Client::start(dir.path(), server).is_err());
}
