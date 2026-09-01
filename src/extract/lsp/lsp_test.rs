use super::*;

#[test]
fn parses_location_and_location_link() {
    let root = Path::new("/work");
    let plain = json!({"uri":"file:///work/src/a.rs","range":{"start":{"line":2,"character":4},"end":{"line":2,"character":7}}});
    let linked = json!({"targetUri":"file:///work/src/b.rs","targetSelectionRange":{"start":{"line":8,"character":1},"end":{"line":8,"character":3}}});
    assert_eq!(
        locations(&json!([plain, linked]), root),
        vec![
            Location {
                file: "src/a.rs".into(),
                start_line: 3,
                start_column: 5,
                end_line: 3,
                end_column: 8
            },
            Location {
                file: "src/b.rs".into(),
                start_line: 9,
                start_column: 2,
                end_line: 9,
                end_column: 4
            },
        ]
    );
}

#[test]
fn descriptors_cover_rust_and_a_second_language() {
    assert_eq!(server_for(Path::new("x.rs")).unwrap().language_id, "rust");
    assert_eq!(
        server_for(Path::new("x.ts")).unwrap().language_id,
        "typescript"
    );
    assert!(server_for(Path::new("x.txt")).is_none());
}

#[test]
fn language_agnostic_client_disambiguates_positions_and_meets_warm_budget() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("x.rs"),
        "fn target() {}\nfn use_it() { target(); }\n",
    )
    .unwrap();
    let script = dir.path().join("fake_lsp.py");
    std::fs::write(&script, r#"import json, sys
inp = sys.stdin.buffer
out = sys.stdout.buffer
while True:
    length = None
    while True:
        line = inp.readline()
        if not line:
            sys.exit(0)
        if line in (b'\r\n', b'\n'):
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':', 1)[1].strip())
    msg = json.loads(inp.read(length))
    if 'id' not in msg:
        continue
    if msg.get('method') == 'initialize':
        result = {'capabilities': {}}
    else:
        character = msg['params']['position']['character']
        start = 3 if character < 10 else 13
        result = [{'uri': 'file://' + sys.argv[1] + '/x.rs', 'range': {'start': {'line': 0, 'character': start}, 'end': {'line': 0, 'character': start + 6}}}]
    body = json.dumps({'jsonrpc': '2.0', 'id': msg['id'], 'result': result}).encode()
    out.write(('Content-Length: %d\r\n\r\n' % len(body)).encode() + body)
    out.flush()
"#).unwrap();
    let server = Server {
        program: "python3".into(),
        args: vec![
            script.to_string_lossy().into_owned(),
            dir.path().to_string_lossy().into_owned(),
        ],
        language_id: "rust".into(),
    };
    let mut client = Client::start(dir.path(), server).unwrap();
    let file = dir.path().join("x.rs");
    let first = Position {
        file: "x.rs".into(),
        line: 2,
        column: 5,
    };
    let second = Position {
        file: "x.rs".into(),
        line: 2,
        column: 15,
    };
    let first_definition = client.query(&file, &first, Query::Definition).unwrap();
    let second_definition = client.query(&file, &second, Query::Definition).unwrap();
    assert_eq!(first_definition[0].start_column, 4);
    assert_eq!(second_definition[0].start_column, 14);
    assert_ne!(first_definition, second_definition);

    let warm_start = std::time::Instant::now();
    assert_eq!(
        client
            .query(&file, &second, Query::References)
            .unwrap()
            .len(),
        1
    );
    assert!(
        warm_start.elapsed() < Duration::from_secs(1),
        "warm LSP query exceeded FR-2's one-second budget"
    );
}

#[test]
fn unknown_language_degrades_without_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(query(
        dir.path(),
        &Position {
            file: "x.unknown".into(),
            line: 1,
            column: 1
        },
        Query::Definition
    )
    .is_none());
}

#[test]
fn rust_analyzer_resolves_when_installed_and_skips_when_absent() {
    if !Command::new("rust-analyzer")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname='lsp_fixture'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn target() -> usize { 1 }\npub fn caller() -> usize { target() }\n",
    )
    .unwrap();
    let result = query_result(
        dir.path(),
        &Position {
            file: "src/lib.rs".into(),
            line: 2,
            column: 28,
        },
        Query::Definition,
    )
    .expect("installed rust-analyzer must complete the LSP exchange")
    .expect("Rust has an LSP adapter");
    assert_eq!(result.len(), 1, "target call must resolve once: {result:?}");
    assert_eq!(
        (result[0].file.as_str(), result[0].start_line),
        ("src/lib.rs", 1)
    );
}
