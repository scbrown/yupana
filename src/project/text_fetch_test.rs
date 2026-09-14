use super::*;

fn literal(value: &str) -> Value {
    json!({"type":"literal","value":value})
}

fn subject() -> Value {
    json!({"type":"uri","value":"https://example.org/rule"})
}

fn property(name: &str, value: &str) -> Value {
    json!({"property":{"type":"uri","value":name},"value":literal(value)})
}

fn properties() -> Vec<Value> {
    vec![
        property("http://aegis.gastown.local/ontology/regex", "danger"),
        property(
            "http://aegis.gastown.local/ontology/enforcementTier",
            "block",
        ),
        property("http://www.w3.org/2000/01/rdf-schema#label", "Rule"),
        property("http://www.w3.org/2000/01/rdf-schema#label", "Alias"),
        property(
            "http://www.w3.org/2000/01/rdf-schema#comment",
            "First reason",
        ),
        property(
            "http://www.w3.org/2000/01/rdf-schema#comment",
            "Second reason",
        ),
        property(
            "http://aegis.gastown.local/ontology/exemptPathRegex",
            "tests/",
        ),
        property(
            "http://aegis.gastown.local/ontology/exemptPathRegex",
            "fixtures/",
        ),
    ]
}

fn body(rows: Vec<Value>) -> Value {
    json!({"results":{"bindings":Value::Array(rows)}})
}

fn selector_link() -> Value {
    json!({
        "property": {"type":"uri", "value":"http://aegis.gastown.local/ontology/exemptionSelector"},
        "value": {"type":"uri", "value":"https://example.org/shared-guard-paths"},
    })
}

#[test]
fn shared_selector_is_read_once_and_unioned_with_local_exemptions() {
    let mut props = properties()[..2].to_vec();
    props.push(property("aegis:exemptPathRegex", "^tests/"));
    props.push(selector_link());
    let (endpoint, thread) = server(vec![
        body(vec![
            json!({"s":subject()}),
            json!({"s":{"type":"uri","value":"https://example.org/another-rule"}}),
        ]),
        body(props.clone()),
        body(vec![property(
            "http://aegis.gastown.local/ontology/exemptPathRegex",
            "^src/guard[.]rs$",
        )]),
        body(props),
    ]);
    let rules = fetch(&endpoint).unwrap();
    assert_eq!(rules.len(), 2);
    for rule in rules {
        assert!(!rule.applies("src/guard.rs"));
        assert!(!rule.applies("tests/fixture.rs"));
        assert!(rule.applies("src/production.rs"));
        assert!(rule.applies("docs/guide.md"));
    }
    let queries = thread.join().unwrap();
    assert_eq!(queries.len(), 4);
    assert_eq!(
        queries
            .iter()
            .filter(|q| q.contains("shared-guard-paths"))
            .count(),
        1
    );
}

#[test]
fn broken_selector_cannot_silently_drop_scope_or_inject_a_query() {
    for paths in [vec![], vec![property("aegis:exemptPathRegex", "[")]] {
        let (endpoint, thread) = server(vec![body(paths)]);
        assert!(
            shared_exemptions(&endpoint, &mut vec![selector_link()], &mut HashMap::new()).is_err()
        );
        thread.join().unwrap();
    }
    for value in [
        literal("https://example.org/not-an-iri"),
        json!({"type":"uri","value":"https://example.org/> ?s ?p ?o"}),
    ] {
        let mut link = selector_link();
        link["value"] = value;
        assert!(shared_exemptions(
            "http://unused.invalid",
            &mut vec![link],
            &mut HashMap::new()
        )
        .is_err());
    }
    let mut props = properties()[..2].to_vec();
    shared_exemptions("http://unused.invalid", &mut props, &mut HashMap::new()).unwrap();
    let rules = decode_text_rules(&body(expand(&subject(), &props).unwrap()).to_string()).unwrap();
    assert!(rules[0].applies("src/guard.rs")); // no explicit link: still governed
}

#[test]
fn optional_products_equal_the_join_without_losing_any_values() {
    let mut expected = Vec::new();
    for label in ["Rule", "Alias"] {
        for exempt in ["tests/", "fixtures/"] {
            for rationale in ["First reason", "Second reason"] {
                expected.push(json!({
                    "s":subject(), "regex":literal("danger"), "tier":literal("block"),
                    "label":literal(label), "exempt":literal(exempt), "rationale":literal(rationale),
                }));
            }
        }
    }
    let actual = expand(&subject(), &properties()).unwrap();
    assert_eq!(actual.len(), 8);
    let canonical = |rows: &[Value]| {
        let mut rows: Vec<_> = rows.iter().map(Value::to_string).collect();
        rows.sort();
        rows
    };
    assert_eq!(canonical(&actual), canonical(&expected));
    assert_eq!(
        decode_text_rules(&body(actual).to_string()).unwrap(),
        decode_text_rules(&body(expected).to_string()).unwrap()
    );
}

#[test]
fn missing_required_triples_and_conflicting_values_keep_join_semantics() {
    assert!(expand(&subject(), &[]).unwrap().is_empty());
    let minimal = properties()[..2].to_vec();
    let rows = expand(&subject(), &minimal).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("exempt").is_none());
    assert_eq!(decode_text_rules(&body(rows).to_string()).unwrap().len(), 1);

    let mut conflict = minimal;
    conflict.push(property("aegis:regex", "a different expression"));
    let rows = expand(&subject(), &conflict).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(decode_text_rules(&body(rows).to_string()).is_err());
    assert!(expand(&subject(), &[json!({"property":literal("aegis:regex")})]).is_err());
}

fn server(replies: Vec<Value>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let mut queries = Vec::new();
        for reply in replies {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < deadline, "missing request");
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(e) => panic!("accept: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut raw = Vec::new();
            loop {
                let mut chunk = [0; 2048];
                let n = stream.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                raw.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&raw);
                if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                    let len: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    if body.len() >= len {
                        let request: Value = serde_json::from_str(&body[..len]).unwrap();
                        queries.push(request["query"].as_str().unwrap().to_string());
                        break;
                    }
                }
            }
            let reply = reply.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            )
            .unwrap();
        }
        queries
    });
    (endpoint, thread)
}

#[test]
fn wire_fetch_discovers_subclasses_and_reads_each_identity_once() {
    let members = body(vec![json!({"s":subject()}), json!({"s":subject()})]);
    let (endpoint, thread) = server(vec![members, body(properties())]);
    let fetched = fetch(&endpoint);
    let queries = thread.join().unwrap();
    let expected =
        decode_text_rules(&body(expand(&subject(), &properties()).unwrap()).to_string()).unwrap();
    assert_eq!(fetched.unwrap(), expected);
    assert!(queries[0].contains("a/rdfs:subClassOf* aegis:TextRule"));
    assert_eq!(
        queries[1],
        "SELECT ?property ?value WHERE { <https://example.org/rule> ?property ?value }"
    );
    assert!(queries
        .iter()
        .all(|q| !q.contains("OPTIONAL") && !q.contains("LIMIT")));
}

#[test]
fn partial_or_malformed_reads_never_become_a_successful_small_catalogue() {
    for replies in [
        vec![json!({"truncated":true,"results":{"bindings":[]}})],
        vec![json!({"error":"query failed"})],
        vec![
            body(vec![json!({"s":subject()})]),
            json!({"truncated":true,"results":{"bindings":[]}}),
        ],
        vec![body(vec![
            json!({"s":{"type":"uri","value":"urn:escape> } UNION {"}}),
        ])],
        vec![body(vec![json!({"s":literal("https://example.org/rule")})])],
    ] {
        let (endpoint, thread) = server(replies);
        let result = fetch(&endpoint);
        thread.join().unwrap();
        assert!(result.is_err());
    }
    let (endpoint, thread) = server(vec![body(vec![])]);
    let result = fetch(&endpoint);
    thread.join().unwrap();
    assert!(result.unwrap().is_empty());
}

#[test]
fn a_failed_subject_read_keeps_the_previous_registry_stale() {
    let old =
        decode_text_rules(&body(expand(&subject(), &properties()).unwrap()).to_string()).unwrap();
    let mut replies = vec![
        body(vec![]),
        body(vec![json!({"s":subject()})]),
        json!({"error":"subject read failed"}),
    ];
    replies.extend(std::iter::repeat_n(body(vec![]), 5));
    let (endpoint, thread) = server(replies);
    let mut registry = super::super::ProjectionRegistry::new(&endpoint);
    registry.text_rules = old.clone();
    registry.freshness = crate::types::Freshness::Fresh;
    let result = registry.refresh();
    thread.join().unwrap();
    assert!(result.is_err());
    assert_eq!(registry.text_rules(), old);
    assert_eq!(registry.freshness(), crate::types::Freshness::Stale);
}
