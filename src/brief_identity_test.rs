use super::*;
use crate::test_stub::stub;
use serde_json::{json, Value};

fn binding(entity: &str, id: &str) -> Value {
    json!({"entity": {"value": entity}, "id": {"value": id}})
}

fn field(entity: &str, var: &str, value: &str) -> Value {
    json!({"entity": {"value": entity}, var: {"value": value}})
}

/// Which single-pattern field lookup a request is: identity, outcome or label.
fn which(request: &Value) -> &'static str {
    let query = request["query"].as_str().unwrap_or_default();
    if query.contains("aegis:identifier ?id") {
        "id"
    } else if query.contains("aegis:outcome ?outcome") {
        "outcome"
    } else if query.contains("label> ?label") {
        "label"
    } else {
        "other"
    }
}

#[test]
fn batch_preserves_identity_optional_fields_and_excludes_unrequested_rows() {
    let (endpoint, server) = stub(3, |_, request| {
        let rows = match which(request) {
            "id" => json!([
                binding("urn:b", "b-2"),
                binding("urn:a", "a-1"),
                binding("urn:other", "other-9")
            ]),
            "outcome" => json!([field("urn:a", "outcome", "done")]),
            _ => json!([
                field("urn:a", "label", "Alpha"),
                field("urn:no-id", "label", "Not a work item")
            ]),
        };
        (200, json!({"results":{"bindings": rows}}))
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
    for request in server.join().unwrap() {
        let query = request["query"].as_str().unwrap();
        assert!(query.contains("BIND(<urn:a> AS ?entity)"));
        // Once in the BIND, once as the bound subject: asked about exactly once.
        assert_eq!(query.matches("<urn:a>").count(), 2);
        assert!(!query.contains("bad"));
        // One bound pattern per request: no OPTIONAL, no joined patterns.
        assert!(!query.contains("OPTIONAL"), "{query}");
    }
}

#[test]
fn bounded_batches_do_not_retry_failed_candidates_individually() {
    // 65 candidates = batches of 32, 32, 1 for each of the three fields.
    let (endpoint, server) = stub(9, |index, request| match (which(request), index % 3) {
        ("id", 0) => (503, json!({"error":"unavailable"})),
        ("id", 1) => (200, json!({"not_results":true})),
        ("id", _) => (
            200,
            json!({"results":{"bindings":[binding("urn:064", "last-1")]}}),
        ),
        _ => (200, json!({"results":{"bindings":[]}})),
    });
    let iris: Vec<_> = (0..65).map(|n| format!("urn:{n:03}")).collect();
    let identities = fetch(&endpoint, iris.iter().map(String::as_str));
    assert_eq!(identities.len(), 1);
    assert_eq!(identities["urn:064"].0, "last-1");
    let requests = server.join().unwrap();
    let sizes: Vec<_> = requests
        .iter()
        .map(|r| r["query"].as_str().unwrap().matches("BIND(<urn:").count())
        .collect();
    assert_eq!(sizes, [32, 32, 1, 32, 32, 1, 32, 32, 1]);
}

#[test]
fn real_similarity_pipeline_batches_candidates_without_changing_the_brief() {
    let (endpoint, server) = stub(6, |index, request| {
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
                _ => json!({"results":{"bindings": match which(request) {
                    "id" => json!([binding("urn:self", "current-1"), binding("urn:context", "context-2"), binding("urn:semantic", "semantic-3")]),
                    "outcome" => json!([field("urn:semantic", "outcome", "done")]),
                    _ => json!([field("urn:context", "label", "Graph label"), field("urn:semantic", "label", "Semantic label")]),
                }}}),
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

#[test]
fn prefixed_candidates_are_queried_as_full_iris_and_answered_under_their_own_name() {
    // `/search` and `/context` report `aegis:<local>`; the store's SPARQL-JSON
    // answers with the full IRI. Before aegis-h9c0no the query carried
    // `<aegis:x>` (a nonexistent IRI) and the answer was then dropped as
    // unrequested, so every quipu-sourced candidate silently had no identity.
    let full: &'static str =
        Box::leak(format!("{}aegis-l50p", crate::export::ONTO).into_boxed_str());
    let (endpoint, server) = stub(3, move |_, request| match which(request) {
        "id" => (
            200,
            json!({"results":{"bindings":[binding(full, "aegis-l50p")]}}),
        ),
        _ => (200, json!({"results":{"bindings":[]}})),
    });
    let identities = fetch(&endpoint, ["aegis:aegis-l50p"]);
    assert_eq!(identities.len(), 1);
    assert_eq!(identities["aegis:aegis-l50p"].0, "aegis-l50p");
    let query = server.join().unwrap()[0]["query"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(query.contains(&format!("<{full}>")));
    assert!(!query.contains("<aegis:"));
}
