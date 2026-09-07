use super::*;

fn client(root: &Path, mode: &str) -> Client {
    let script = root.join("server.py");
    std::fs::write(&script, r#"import json, sys, time
def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n' % len(body)).encode() + body)
    sys.stdout.buffer.flush()
requests = 0
while True:
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line in (b'\n', b'\r\n'): break
        if line.lower().startswith(b'content-length:'): length = int(line.split(b':')[1])
    msg = json.loads(sys.stdin.buffer.read(length))
    if 'id' not in msg: continue
    reply = {'jsonrpc':'2.0', 'id':msg['id']}
    if msg['method'] == 'initialize': reply['result'] = {'capabilities':{}}
    elif sys.argv[2] == 'notifications':
        for _ in range(20):
            send({'jsonrpc':'2.0', 'method':'window/logMessage', 'params':{'type':3,'message':'busy'}})
            time.sleep(.02)
        reply['result'] = []
    elif sys.argv[2] == 'malformed': pass
    else:
        requests += 1
        if requests == 1: reply['error'] = {'code':-32801, 'message':'content modified'}
        elif requests == 2:
            reply['result'] = [{'uri':'file://' + sys.argv[1] + '/x.rs', 'range':{'start':{'line':0,'character':3},'end':{'line':0,'character':4}}}]
        else: reply['result'] = []
    send(reply)
"#).unwrap();
    Client::start(
        root,
        Server {
            program: "python3".into(),
            args: vec![
                script.to_string_lossy().into(),
                root.to_string_lossy().into(),
                mode.into(),
            ],
            language_id: "rust".into(),
        },
    )
    .unwrap()
}

#[test]
fn content_modified_retries_but_a_warm_empty_answer_returns_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x.rs");
    std::fs::write(&file, "fn x() {}\n").unwrap();
    let mut client = client(dir.path(), "retry");
    let position = Position {
        file: "x.rs".into(),
        line: 1,
        column: 4,
    };
    assert_eq!(
        client
            .query(&file, &position, Query::Definition)
            .unwrap()
            .len(),
        1
    );
    assert!(client.warmed_methods.contains("textDocument/definition"));
    let start = std::time::Instant::now();
    assert!(client
        .query(&file, &position, Query::References)
        .unwrap()
        .is_empty());
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn notifications_cannot_reset_the_request_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = client(dir.path(), "notifications");
    let start = std::time::Instant::now();
    let result = client.request_with_timeout("probe", &json!({}), Duration::from_millis(80));
    assert!(
        result.is_err(),
        "notification stream reset the request timeout"
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn malformed_success_is_not_an_empty_precise_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = client(dir.path(), "malformed");
    assert!(client.request("probe", &json!({})).is_err());
    assert!(!client.warmed_methods.contains("probe"));
}
