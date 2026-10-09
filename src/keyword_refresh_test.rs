use super::*;
use std::io::{Read, Write};

#[test]
fn decoder_refuses_errors_missing_rows_and_truncation_but_accepts_both_wire_formats() {
    for raw in [
        r#"{"error":"timeout"}"#,
        r#"{}"#,
        r#"{"rows":[],"truncated":true}"#,
    ] {
        assert!(rows(raw).is_err(), "{raw}");
    }
    assert!(rows(r#"{"rows":[]}"#).unwrap().is_empty());
    assert_eq!(
        rows(r#"{"results":{"bindings":[{"s":{"type":"uri","value":"ex:x"}}]}}"#).unwrap()[0]["s"],
        "ex:x"
    );
}

#[test]
fn actual_fetch_is_class_bounded_and_keeps_entities_and_existing_edges() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for kind in CLASSES.split_whitespace() {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 4096];
            let mut length = None;
            loop {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..pos]);
                    length = header
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|l| l.trim().parse::<usize>().ok())
                        })
                        .map(|n| pos + 4 + n);
                }
                if length.is_some_and(|n| bytes.len() >= n) {
                    break;
                }
            }
            let raw = String::from_utf8(bytes).unwrap();
            assert!(raw.contains(&format!("https://example.org/{kind}>")));
            assert!(raw.contains("LIMIT 10001"));
            assert!(!raw.contains("DELETE"));
            let body = if kind == "Host" {
                r#"{"results":{"bindings":[{"s":{"type":"uri","value":"ex:host"},"l":{"type":"literal","value":"build-01.example"},"p":{"type":"uri","value":"ex:connects_to"},"o":{"type":"uri","value":"ex:other"}}]}}"#
            } else {
                r#"{"results":{"bindings":[]}}"#
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let result = fetch(&endpoint, "https://example.org/").unwrap();
    server.join().unwrap();
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.edges.len(), 1);
    assert_eq!(result.entries[0].kind, "Host");
    assert_eq!(result.edges[0].predicate, "ex:connects_to");
}

#[test]
fn malformed_namespace_is_refused_before_a_request_starts() {
    for ns in [
        "",
        "https://example.org/> ?s ?p ?o",
        "https://example.org/\n",
    ] {
        assert!(fetch("http://127.0.0.1:1", ns).is_err());
    }
}
