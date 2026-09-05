use super::*;

fn rows(values: Vec<serde_json::Value>) -> String {
    let values = serde_json::Value::Array(values);
    serde_json::json!({"results":{"bindings":values}}).to_string()
}

fn props(values: &[(&str, &str)]) -> String {
    rows(
        values
            .iter()
            .map(|(property, value)| {
                serde_json::json!({
                    "property":{"value":property},"value":{"value":value}
                })
            })
            .collect(),
    )
}

fn server(replies: Vec<String>) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut buffer = [0; 2048];
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&bytes);
                if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                    let size: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    if body.len() >= size {
                        let query: serde_json::Value = serde_json::from_str(&body[..size]).unwrap();
                        assert!(!query["query"].as_str().unwrap().contains("OPTIONAL"));
                        break;
                    }
                }
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            )
            .unwrap();
        }
    });
    (endpoint, thread)
}

#[test]
fn indexed_fetch_checks_types_atoms_and_projects_the_complete_rule() {
    let ns = super::super::ONTOLOGY_NS;
    let policy_type = format!("{ns}Policy");
    let selector_type = format!("{ns}Selector");
    let predicate_type = format!("{ns}Predicate");
    let type_key = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    let policy = "https://example.org/delegate";
    let selector = "https://example.org/selector";
    let predicate = "https://example.org/predicate";
    let replies = vec![
        rows(vec![serde_json::json!({"policy":{"value":policy}})]),
        props(&[
            (type_key, &policy_type),
            (&format!("{ns}selector"), selector),
            (&format!("{ns}predicate"), predicate),
            (&format!("{ns}enforcementTier"), "warn"),
            (&format!("{ns}oncePer"), "session"),
            (&format!("{ns}effect"), "warn"),
            (&format!("{ns}verificationPoint"), "PAA"),
            (
                "http://www.w3.org/2000/01/rdf-schema#comment",
                "The graph explains why.",
            ),
        ]),
        props(&[
            (type_key, &selector_type),
            (
                &format!("{ns}evidenceSource"),
                r#"{"programs":["tracker"],"verbs":["file"]}"#,
            ),
        ]),
        props(&[
            (type_key, &predicate_type),
            (&format!("{ns}evidenceSource"), "command-before-edit"),
        ]),
    ];
    let (endpoint, thread) = server(replies);
    let policies = fetch_trajectory_policies(&endpoint).unwrap();
    thread.join().unwrap();
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0].trigger.programs, ["tracker"]);
    assert_eq!(policies[0].rationale, "The graph explains why.");
}

#[test]
fn discovered_incomplete_member_is_an_error_not_an_empty_catalogue() {
    let (endpoint, thread) = server(vec![
        rows(vec![
            serde_json::json!({"policy":{"value":"https://example.org/incomplete"}}),
        ]),
        rows(vec![]),
    ]);
    let error = fetch_trajectory_policies(&endpoint)
        .unwrap_err()
        .to_string();
    thread.join().unwrap();
    assert!(error.contains("must be typed Policy"), "{error}");
}
