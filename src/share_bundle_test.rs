//! Tests for share-bundle reading and hash verification. Child module of
//! `share_bundle` (`super::*` reaches its private helpers); size-exempt.
//!
//! The centre of gravity here is the REFUSALS. A verification step that has
//! only ever been observed to accept a good bundle has not been shown to do
//! anything at all — the same code with the comparison deleted passes that
//! test. Every check below therefore has an arm that makes it say no.

use super::*;

use sha2::{Digest, Sha256};

fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

const NT: &str = "<https://example.org/a> <http://www.w3.org/2000/01/rdf-schema#label> \"A\" .\n";
const TTL: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n";

/// Write a well-formed bundle whose manifest tells the truth about its bytes.
fn good_bundle(dir: &Path, nt: &str, ttl: &str) {
    std::fs::write(dir.join(EXPORT_NT), nt).unwrap();
    std::fs::write(dir.join(SHAPES_TTL), ttl).unwrap();
    let manifest = serde_json::json!({
        "schema": "quipu.share/v1",
        "share_id": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "graph_hash": hash(nt.as_bytes()),
        "shapes_hash": hash(ttl.as_bytes()),
    });
    std::fs::write(
        dir.join(MANIFEST),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn a_truthful_bundle_verifies() {
    let dir = tempfile::tempdir().unwrap();
    good_bundle(dir.path(), NT, TTL);
    let bundle = read(dir.path().to_str().unwrap()).expect("bundle reads");
    verify(&bundle).expect("a truthful bundle verifies");
    assert!(bundle.has_shapes());
}

/// THE load-bearing refusal: the bytes changed after the manifest was written.
///
/// This is the arm that separates a verifier from a formality. It is written as
/// a mutation of an otherwise-passing fixture so it cannot pass for the wrong
/// reason — the ONLY difference from `a_truthful_bundle_verifies` is one byte
/// of payload.
#[test]
fn tampered_graph_bytes_are_refused_and_nothing_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    good_bundle(dir.path(), NT, TTL);
    std::fs::write(
        dir.path().join(EXPORT_NT),
        format!("{NT}<https://example.org/b> <http://x/p> \"smuggled\" .\n"),
    )
    .unwrap();

    let bundle = read(dir.path().to_str().unwrap()).expect("a tampered bundle still READS");
    let err = verify(&bundle).expect_err("tampered graph bytes must be refused");
    let msg = err.to_string();
    assert!(msg.contains("MISMATCH"), "must name the failure: {msg}");
    assert!(
        msg.contains("Nothing was sent to quipu"),
        "the reader must be told the write did not happen: {msg}"
    );
    // Both hashes, so a truncated download can be told from a substituted one.
    assert!(
        msg.contains("manifest declares") && msg.contains("hash to"),
        "must report declared AND actual: {msg}"
    );
}

#[test]
fn tampered_shapes_bytes_are_refused_too() {
    let dir = tempfile::tempdir().unwrap();
    good_bundle(dir.path(), NT, TTL);
    std::fs::write(dir.path().join(SHAPES_TTL), format!("{TTL}# widened\n")).unwrap();
    let bundle = read(dir.path().to_str().unwrap()).unwrap();
    let err = verify(&bundle).expect_err("tampered shapes must be refused");
    assert!(err.to_string().contains("shapes hash MISMATCH"));
}

/// A manifest with no hash at all must not be treated as "nothing to check".
#[test]
fn a_manifest_that_vouches_for_nothing_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    good_bundle(dir.path(), NT, TTL);
    std::fs::write(dir.path().join(MANIFEST), r#"{"schema":"quipu.share/v1"}"#).unwrap();
    let bundle = read(dir.path().to_str().unwrap()).unwrap();
    let err = verify(&bundle).expect_err("an unvouched bundle must be refused");
    assert!(err.to_string().contains("nobody has vouched for"), "{err}");
}

/// `quipu share --no-shapes` is a documented way to produce a bundle, so an
/// absent `shapes.ttl` is ordinary — it must verify, and it must be visibly
/// shapeless so nothing downstream offers to adopt a vocabulary that is not there.
#[test]
fn a_no_shapes_bundle_verifies_and_reports_itself_shapeless() {
    let dir = tempfile::tempdir().unwrap();
    good_bundle(dir.path(), NT, "");
    std::fs::remove_file(dir.path().join(SHAPES_TTL)).unwrap();
    let bundle = read(dir.path().to_str().unwrap()).unwrap();
    verify(&bundle).expect("an empty shapes.ttl hashes to the empty-string hash and verifies");
    assert!(!bundle.has_shapes());
}

#[test]
fn a_directory_without_a_manifest_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
    let err = read(dir.path().to_str().unwrap()).expect_err("not a bundle");
    let msg = err.to_string();
    // Caught by NAME, so a wrong-directory mistake does not surface three calls
    // later as a confusing parse failure.
    assert!(msg.contains(MANIFEST), "{msg}");
    assert!(
        msg.contains("quipu share --output"),
        "names the producer: {msg}"
    );
}

/// A `.qpack.db` is refused, and the refusal hands over commands that WORK.
///
/// The refusal exists because quipu's `unpack`/`pack` have no REST route
/// (measured 2026-09-05) and operate on a local store, while yupana only ever
/// talks to a remote endpoint. A refusal that merely said "unsupported" would
/// leave the reader with an artifact and no route; naming the CLI is the whole
/// value of the branch.
#[test]
fn a_qpack_is_refused_with_the_commands_that_do_work() {
    let dir = tempfile::tempdir().unwrap();
    let pack = dir.path().join("graph1.qpack.db");
    std::fs::write(&pack, b"not really a pack").unwrap();
    let err = read(pack.to_str().unwrap()).expect_err("a pack is not a bundle");
    let msg = err.to_string();
    assert!(msg.contains("quipu pack --verify"), "{msg}");
    assert!(msg.contains("quipu unpack"), "{msg}");
    // The path must be substituted into the suggestion, not described.
    assert!(msg.contains(pack.to_str().unwrap()), "{msg}");
}

#[test]
fn an_archive_url_is_refused_with_the_quipu_command_that_takes_it() {
    let err = read("https://example.org/releases/share-v1.tar.gz").expect_err("archive refused");
    let msg = err.to_string();
    assert!(
        msg.contains("quipu import https://example.org/releases/share-v1.tar.gz"),
        "{msg}"
    );
}

#[test]
fn a_missing_path_is_refused_without_touching_the_network() {
    let err = read("/nonexistent/share/dir").expect_err("missing path refused");
    assert!(err.to_string().contains("no such share directory"), "{err}");
}
