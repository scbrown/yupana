use super::*;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

fn stub(
    count: usize,
    mut reply: impl FnMut(usize, &Value) -> (u16, Value) + Send + 'static,
) -> (String, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for index in 0..count {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                if let Ok((stream, _)) = listener.accept() {
                    break stream;
                }
                assert!(
                    Instant::now() < deadline,
                    "missing expected request {index}"
                );
                std::thread::sleep(Duration::from_millis(5));
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let request = serde_json::from_slice(&body).unwrap();
            let (status, body) = reply(index, &request);
            requests.push(request);
            let body = body.to_string();
            write!(stream, "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        requests
    });
    (endpoint, thread)
}

fn binding(entity: &str, id: &str) -> Value {
    json!({"entity": {"value": entity}, "id": {"value": id}})
}

#[test]
fn batch_preserves_identity_optional_fields_and_excludes_unrequested_rows() {
    let (endpoint, server) = stub(1, |_, _| {
        (
            200,
            json!({"results":{"bindings":[
                binding("urn:b", "b-2"),
                {"entity":{"value":"urn:a"},"id":{"value":"a-1"},"label":{"value":"Alpha"}},
                {"entity":{"value":"urn:a"},"id":{"value":"a-1"},"outcome":{"value":"done"}},
                binding("urn:other", "other-9"),
                {"entity":{"value":"urn:no-id"},"label":{"value":"Not a work item"}}
            ]}}),
        )
    });
    let identities = fetch(
        &endpoint,
        [
            "urn:a",
            "urn:b",
            "urn:a",
            "urn:no-id",
            "bad>iri",
            "bad\niri",
        ],
    );
    assert_eq!(identities.len(), 2);
    assert_eq!(
        identities["urn:a"],
        ("a-1".into(), Some("done".into()), Some("Alpha".into()))
    );
    assert_eq!(identities["urn:b"], ("b-2".into(), None, None));
    let requests = server.join().unwrap();
    let query = requests[0]["query"].as_str().unwrap();
    assert!(query.contains("VALUES ?entity"));
    assert_eq!(query.matches("<urn:a>").count(), 1);
    assert!(!query.contains("bad"));
}

#[test]
fn bounded_batches_do_not_retry_failed_candidates_individually() {
    let (endpoint, server) = stub(3, |index, _| match index {
        0 => (503, json!({"error":"unavailable"})),
        1 => (200, json!({"not_results":true})),
        _ => (
            200,
            json!({"results":{"bindings":[binding("urn:064", "last-1")]}}),
        ),
    });
    let iris: Vec<_> = (0..65).map(|n| format!("urn:{n:03}")).collect();
    let identities = fetch(&endpoint, iris.iter().map(String::as_str));
    assert_eq!(identities.len(), 1);
    assert_eq!(identities["urn:064"].0, "last-1");
    let requests = server.join().unwrap();
    let sizes: Vec<_> = requests
        .iter()
        .map(|r| r["query"].as_str().unwrap().matches("<urn:").count())
        .collect();
    assert_eq!(sizes, [32, 32, 1]);
}

#[test]
fn real_similarity_pipeline_batches_candidates_without_changing_the_brief() {
    let (endpoint, server) = stub(4, |index, _| {
        (
            200,
            match index {
                0 => {
                    json!({"results":[{"entity":"urn:self","score":1.0},{"entity":"urn:semantic","score":0.9}]})
                }
                1 => json!({"entities":[
                    {"iri":"urn:context","types":["WorkItem"],"label":"Context label","score":0.8},
                    {"iri":"urn:self","types":["WorkItem"],"score":1.0},
                    {"iri":"urn:nonitem","types":["CodeSymbol"],"score":0.9}
                ]}),
                2 => json!({"results":{"bindings":[{"w":{"value":"urn:self"}}]}}),
                _ => json!({"results":{"bindings":[
                    binding("urn:self", "current-1"),
                    {"entity":{"value":"urn:context"},"id":{"value":"context-2"},"label":{"value":"Graph label"}},
                    {"entity":{"value":"urn:semantic"},"id":{"value":"semantic-3"},"label":{"value":"Semantic label"},"outcome":{"value":"done"}}
                ]}}),
            },
        )
    });
    let similar = super::super::similar_items(&endpoint, "current-1", "some label", None, &[]);
    assert_eq!(similar.len(), 2);
    assert_eq!(similar[0].id, "semantic-3");
    assert_eq!(similar[0].label.as_deref(), Some("Semantic label"));
    assert_eq!(similar[0].outcome.as_deref(), Some("done"));
    assert_eq!(similar[1].id, "context-2");
    assert_eq!(similar[1].label.as_deref(), Some("Context label"));
    let requests = server.join().unwrap();
    let query = requests[3]["query"].as_str().unwrap();
    assert!(query.contains("<urn:semantic>") && query.contains("<urn:context>"));
    assert!(!query.contains("<urn:nonitem>"));
}
