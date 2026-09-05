//! Tests for delta re-sharing. Child module of `share_reshare`; size-exempt.
//!
//! The property under test is LINEAGE: a share that came from someone else and
//! goes back out must name its parent. The derivation exists so that forgetting
//! is structurally difficult rather than merely discouraged, so most of what
//! follows is about the ways a derivation can be wrong — inventing a parent for
//! a local graph is a worse failure than omitting one, because a false lineage
//! claim gets published in a manifest and believed downstream.

use super::*;

const HASH: &str = "a350c7a72df0a95b6286b12c8bc75ce7826d53e221db4a4dd1684474acb59f6d";

#[test]
fn a_pulled_graph_yields_the_share_it_came_from() {
    for prefix in [STAGING, QUARANTINE] {
        assert_eq!(
            parent_of(&format!("{prefix}{HASH}")).as_deref(),
            Some(format!("sha256:{HASH}").as_str()),
            "a graph quipu named after a share must recover that share"
        );
    }
}

/// The half that matters more: a graph that is NOT pulled must yield NOTHING.
///
/// Every case here would otherwise publish a manifest asserting a lineage that
/// does not exist. The last two are the subtle ones — they carry the right
/// prefix and a wrong body, which is exactly what a truncated or hand-edited
/// IRI looks like.
#[test]
fn a_graph_that_is_not_a_pulled_share_yields_no_parent() {
    for graph in [
        "urn:quipu:align:abc",
        "https://camayoc.local/window/shuttle/runs/2026-09",
        "",
        "urn:quipu:import:staging:",
        // right prefix, hash too short — a truncated IRI
        "urn:quipu:import:staging:a350c7a7",
        // right prefix, right length, NOT hex
        &format!("urn:quipu:import:quarantine:{}", "z".repeat(64)),
    ] {
        assert_eq!(
            parent_of(graph),
            None,
            "{graph:?} is not a pulled share and must not be given a parent"
        );
    }
}

#[test]
fn the_request_names_the_parent_and_scopes_to_the_graph() {
    let body = request("urn:g", Some("sha256:abc"), &["code".to_string()], false);
    assert_eq!(body["scope"]["kind"], "graph");
    assert_eq!(body["scope"]["value"], "urn:g");
    assert_eq!(body["parent_share"], "sha256:abc");
    assert_eq!(body["no_shapes"], false);
    assert_eq!(body["shapes"][0], "code");
}

/// A root share must OMIT `parent_share` rather than send it null: quipu reads
/// an absent field as "no parent", and sending an explicit null is a different
/// statement that the wire format does not have to accept.
#[test]
fn a_root_share_omits_the_parent_field_entirely() {
    let body = request("urn:g", None, &[], true);
    assert!(
        body.get("parent_share").is_none(),
        "a parentless share must omit the field, not send null: {body}"
    );
    assert_eq!(body["no_shapes"], true);
}

fn payload(files: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "manifest": {"share_id": "sha256:x"}, "files": files })
}

#[test]
fn the_bundle_is_written_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bundle");
    let nt = "<https://example.org/a> <http://x/p> \"A\" .\n";
    let written = write_bundle(
        &payload(&serde_json::json!({
            "manifest.json": "{\"share_id\":\"sha256:x\"}",
            "export.nt": nt,
            "shapes.ttl": "",
        })),
        &out,
    )
    .expect("writes");
    assert_eq!(written.len(), 3);
    // Verbatim: the manifest's hashes are over these exact bytes, so anything
    // that reformats on the way through breaks the bundle's own verification.
    assert_eq!(std::fs::read_to_string(out.join("export.nt")).unwrap(), nt);
}

/// A filename from the server is a path component we are about to join. It must
/// not be able to reach outside the output directory.
#[test]
fn a_share_filename_that_could_escape_the_output_directory_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    for bad in ["../escaped.nt", "nested/file.nt", ".hidden"] {
        let err = write_bundle(
            &payload(&serde_json::json!({ bad: "x" })),
            &dir.path().join("bundle"),
        )
        .expect_err("must refuse {bad}");
        assert!(err.to_string().contains("suspicious name"), "{bad}: {err}");
    }
    assert!(
        !dir.path().join("escaped.nt").exists(),
        "nothing may be written outside the bundle directory"
    );
}

/// An empty bundle is refused rather than written, because a directory of
/// nothing looks exactly like a successful share until someone tries to use it.
#[test]
fn an_empty_or_malformed_payload_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bundle");
    assert!(write_bundle(&payload(&serde_json::json!({})), &out).is_err());
    assert!(write_bundle(&serde_json::json!({"manifest": {}}), &out).is_err());
    assert!(
        write_bundle(&payload(&serde_json::json!({"export.nt": 7})), &out).is_err(),
        "a non-string file body must be refused, not stringified"
    );
}
