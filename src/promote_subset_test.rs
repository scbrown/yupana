//! Tests for the per-file partition (aegis-8o7r10).

use super::{empty_snapshot, file_key, partition_by_file};

const ONTO: &str = "http://aegis.gastown.local/ontology/";

/// Two modules, a symbol in each, and a call that CROSSES from a.rs into b.rs —
/// the shape the naive "extract only changed files" implementation gets wrong.
fn projection() -> String {
    format!(
        "@prefix bobbin: <{ONTO}> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\n\
         <{ONTO}mod/a> a bobbin:CodeModule ; bobbin:filePath \"src/a.rs\" .\n\
         <{ONTO}mod/b> a bobbin:CodeModule ; bobbin:filePath \"src/b.rs\" .\n\
         <{ONTO}sym/fa> a bobbin:CodeSymbol ; bobbin:definedIn <{ONTO}mod/a> .\n\
         <{ONTO}sym/fb> a bobbin:CodeSymbol ; bobbin:definedIn <{ONTO}mod/b> .\n\
         <{ONTO}sym/fa> bobbin:calls <{ONTO}sym/fb> .\n"
    )
}

#[test]
fn facts_are_grouped_under_the_file_that_owns_them() {
    let p = partition_by_file(&projection()).unwrap();
    assert_eq!(
        p.by_file.keys().collect::<Vec<_>>(),
        vec!["src/a.rs", "src/b.rs"]
    );
    let a = &p.by_file["src/a.rs"];
    assert!(a.contains("mod/a"), "the module's own facts");
    assert!(a.contains("sym/fa"), "and its symbols'");
    // NOT "a.rs never mentions sym/fb": it legitimately does, as the OBJECT of
    // the call edge it owns. The property is that a.rs does not DEFINE it —
    // b.rs's `definedIn` fact must not appear here, or promoting a.rs would
    // rewrite b.rs's facts.
    assert!(
        !a.contains(&format!("<{ONTO}sym/fb> <{ONTO}definedIn>")),
        "a.rs must not carry b.rs's definition: {a}"
    );
}

/// THE property the naive implementation loses. The call edge crosses from a.rs
/// into b.rs; it belongs to a.rs, because a.rs is the file whose snapshot would
/// legitimately retract it if the reference disappeared. Extracting a.rs alone
/// would not have resolved it at all, and the subset write would then have
/// retracted a still-true edge.
#[test]
fn a_CROSS_FILE_edge_belongs_to_the_file_it_is_asserted_on() {
    let p = partition_by_file(&projection()).unwrap();
    assert!(
        p.by_file["src/a.rs"].contains("calls"),
        "the caller's file owns the edge"
    );
    assert!(
        !p.by_file["src/b.rs"].contains("calls"),
        "the callee's file does not, or promoting b.rs would rewrite a.rs's facts"
    );
}

/// Every triple lands somewhere. A partitioner that dropped facts would shrink
/// the snapshot silently — and `replace_snapshot` would then retract whatever it
/// dropped, which is the failure this whole bead exists to avoid.
#[test]
fn no_triple_is_lost_between_the_whole_and_its_parts() {
    let p = partition_by_file(&projection()).unwrap();
    // 2 modules x (type + filePath) + 2 symbols x (type + definedIn) + 1 call.
    assert_eq!(
        p.owned_triples() + p.unowned_triples(),
        9,
        "every asserted triple must land in exactly one bucket"
    );
}

/// Facts belonging to no file are REPORTED, never dropped. A subset promote
/// cannot express them, so the caller must be able to refuse — silently omitting
/// them would retract them by absence.
#[test]
fn unowned_facts_are_reported_rather_than_silently_dropped() {
    let ttl = format!(
        "@prefix bobbin: <{ONTO}> .\n\n\
         <{ONTO}mod/a> a bobbin:CodeModule ; bobbin:filePath \"src/a.rs\" .\n\
         <{ONTO}commit/deadbeef> a bobbin:Commit ; bobbin:touched <{ONTO}mod/a> .\n"
    );
    let p = partition_by_file(&ttl).unwrap();
    assert!(p.by_file.contains_key("src/a.rs"));
    assert!(
        p.unowned.keys().any(|s| s.contains("commit/deadbeef")),
        "the commit node belongs to no file and must be surfaced: {:?}",
        p.unowned
    );
    assert!(p.unowned_triples() > 0);
}

/// A deleted file promotes as a VALID but empty document: absence under its
/// producer key is what authorizes the retraction. An empty string would risk a
/// write path reading it as "nothing to send" and skipping the retraction —
/// which would leave a deleted file's entities in the graph forever, the exact
/// defect `--replace-snapshot` exists to prevent.
#[test]
fn a_deleted_file_promotes_a_valid_but_empty_snapshot() {
    let empty = empty_snapshot();
    assert!(!empty.is_empty(), "not an empty string");
    assert!(empty.contains("@prefix"), "a parseable Turtle document");
    let p = partition_by_file(&empty).unwrap();
    assert!(p.by_file.is_empty(), "and it asserts nothing");
}

/// The producer key is per FILE, which is the whole mechanism: `replace_snapshot`
/// retracts what the key owns, so a repo-wide key can only ever replace
/// everything.
#[test]
fn the_producer_key_is_scoped_to_the_file() {
    assert_eq!(file_key("yupana", "src/a.rs"), "code:yupana:src/a.rs");
    assert_ne!(
        file_key("yupana", "src/a.rs"),
        file_key("yupana", "src/b.rs")
    );
}

/// Malformed input fails loudly. A partitioner that returned an empty map on a
/// parse error would hand the write path a snapshot that retracts everything.
#[test]
fn unparseable_turtle_is_an_error_not_an_empty_partition() {
    assert!(partition_by_file("this is not turtle {{{").is_err());
}
