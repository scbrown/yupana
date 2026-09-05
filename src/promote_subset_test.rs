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
fn a_cross_file_edge_belongs_to_the_file_it_is_asserted_on() {
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

// --- the SUBSET PLAN (aegis-8o7r10) --------------------------------------
//
// The partition above answers "who owns this fact". These answer "what does a
// subset promote actually WRITE", which is the half that can retract.

use super::{plan, provenance_key, SubsetPlan, SubsetWrite, Why};

/// Writes that touch a FILE key. The unfiled key is written on every plan and is
/// not what most of these properties are about, so it is filtered out here once
/// rather than special-cased in every assertion.
fn file_writes(p: &SubsetPlan) -> Vec<&SubsetWrite> {
    p.writes.iter().filter(|w| w.file.is_some()).collect()
}

/// THE ACCEPTANCE TEST this work exists for, and the one the naive
/// implementation fails.
///
/// A file whose only change is a NEW CALL into an unchanged file must, after a
/// subset promote, still have its call edge present — and the callee's facts
/// must be UNTOUCHED. Counting triples is not enough: the naive version loses
/// edges while the counts still look plausibly smaller, which is exactly what a
/// correct subset promote is supposed to look like.
#[test]
fn a_new_cross_file_call_survives_a_subset_promote_and_the_callee_is_untouched() {
    let p = plan("yupana", &projection(), &["src/a.rs".to_string()]).unwrap();

    let fw = file_writes(&p);
    assert_eq!(fw.len(), 1, "only the changed file's key is written: {p:?}");
    let w = fw[0];
    assert_eq!(w.key, "code:yupana:src/a.rs");
    assert_eq!(w.why, Why::Changed);

    // The edge is IN the write. If it were not, replace_snapshot on this key
    // would retract a call that is still true.
    assert!(
        w.turtle.contains("calls"),
        "the new cross-file edge must be asserted by the file that owns it: {}",
        w.turtle
    );

    // And b.rs is not written AT ALL — not written empty, not written with
    // fewer facts. An unchanged file's snapshot must not be reopened, because
    // reopening it is what would retract facts the changed file's extraction
    // happens not to mention.
    assert!(
        !p.writes
            .iter()
            .any(|w| w.file.as_deref() == Some("src/b.rs")),
        "the callee's key must not be touched: {p:?}"
    );
    assert_eq!(p.unchanged_files, 1, "b.rs is the untouched one");
}

/// The negative arm, and it is the one that matters. If `plan` ever starts
/// writing every partition rather than the changed ones, the assertions above
/// still pass on content — a.rs would still carry its edge. What breaks is the
/// COUNT, so it is asserted directly.
#[test]
fn an_unchanged_file_is_never_written_even_though_its_facts_were_partitioned() {
    let p = plan("yupana", &projection(), &[]).unwrap();
    assert!(
        file_writes(&p).is_empty(),
        "no file changed, so no file key is written — NOT 'write everything': {p:?}"
    );
    assert_eq!(p.unchanged_files, 2);
    assert_eq!(p.triples(), 0, "and this projection has nothing unfiled");
}

/// A changed file with no facts in the projection is written as an EMPTY
/// snapshot, never skipped. Skipping is what leaves a deleted file's entities in
/// the graph forever, and it is the tempting bug because "nothing to send" reads
/// as "nothing to do".
#[test]
fn a_deleted_file_is_planned_as_a_retraction_not_skipped() {
    let p = plan(
        "yupana",
        &projection(),
        &["src/a.rs".to_string(), "src/gone.rs".to_string()],
    )
    .unwrap();
    assert_eq!(
        file_writes(&p).len(),
        2,
        "the deleted file gets a write too: {p:?}"
    );
    let gone = p
        .writes
        .iter()
        .find(|w| w.file.as_deref() == Some("src/gone.rs"))
        .expect("a write for the deleted file");
    assert_eq!(gone.why, Why::Retracted);
    assert_eq!(gone.triples, 0);
    assert_eq!(
        gone.turtle,
        super::empty_snapshot(),
        "a valid but empty document — not an empty string a writer could skip"
    );
    assert_eq!(p.retractions(), 1);
}

/// Facts belonging to no file are CARRIED under their own key, never dropped.
/// Dropping them would leave them stale forever, or see them retracted by the
/// next full resync.
#[test]
fn unfiled_facts_are_carried_under_their_own_key_never_dropped() {
    let prov = format!("<{ONTO}commit/abc123> a bobbin:Commit ; rdfs:label \"abc123\" .\n");
    let p = plan(
        "yupana",
        &format!("{}{prov}", projection()),
        &["src/a.rs".to_string()],
    )
    .unwrap();

    let w = p
        .writes
        .iter()
        .find(|w| w.key == provenance_key("yupana"))
        .expect("unfiled facts are written under their own key");
    assert_eq!(w.why, Why::Unfiled);
    assert!(w.file.is_none());
    assert_eq!(w.triples, 2, "both provenance facts, prefixes excluded");
    assert!(
        w.turtle.contains("commit/abc123"),
        "and the facts themselves are in it: {}",
        w.turtle
    );
    assert_eq!(p.unfiled_subjects.len(), 1, "reported, not silent");
}

/// The unfiled key is written EVEN WHEN EMPTY. Writing it only when non-empty
/// would let a previous commit's provenance outlive the commit — every fact
/// still true, none of them current, and nothing in the output to reveal it.
#[test]
fn the_unfiled_key_is_written_even_with_nothing_to_put_in_it() {
    let p = plan("yupana", &projection(), &["src/a.rs".to_string()]).unwrap();
    let w = p
        .writes
        .iter()
        .find(|w| w.key == provenance_key("yupana"))
        .expect("written even though this projection has no unfiled facts");
    assert_eq!(w.triples, 0);
    assert_eq!(
        w.turtle,
        super::empty_snapshot(),
        "a valid empty document, so the write path cannot skip it"
    );
}

/// THE conservation property, restated at the PLAN level: a plan over every file
/// writes exactly the whole projection. Nothing is lost between partition and
/// plan, and nothing is written twice.
#[test]
fn a_plan_over_every_file_writes_exactly_the_whole_projection() {
    let prov = format!("<{ONTO}commit/abc123> a bobbin:Commit .\n");
    let full = format!("{}{prov}", projection());
    let total = partition_by_file(&full).unwrap();
    let p = plan(
        "yupana",
        &full,
        &["src/a.rs".to_string(), "src/b.rs".to_string()],
    )
    .unwrap();
    assert_eq!(
        p.triples(),
        total.owned_triples() + total.unowned_triples(),
        "every fact is carried by exactly one key: {p:?}"
    );
}

/// Write order must not depend on the order git printed the diff, or a run that
/// fails part-way through is not reproducible.
#[test]
fn writes_are_ordered_and_deduplicated_independently_of_the_diffs_order() {
    let forward = plan(
        "yupana",
        &projection(),
        &["src/a.rs".to_string(), "src/b.rs".to_string()],
    )
    .unwrap();
    let reversed = plan(
        "yupana",
        &projection(),
        &[
            "src/b.rs".to_string(),
            "src/a.rs".to_string(),
            "src/a.rs".to_string(),
        ],
    )
    .unwrap();
    let keys = |p: &SubsetPlan| p.writes.iter().map(|w| w.key.clone()).collect::<Vec<_>>();
    assert_eq!(keys(&forward), keys(&reversed));
    assert_eq!(file_writes(&forward).len(), 2, "the duplicate is collapsed");
}

/// The measurable this whole bead is for: a one-file change writes a fraction of
/// the projection. Asserted as a strict inequality against the full partition,
/// so it cannot pass by the two numbers happening to be equal.
#[test]
fn a_one_file_change_writes_strictly_fewer_triples_than_the_whole_projection() {
    let whole = partition_by_file(&projection()).unwrap().owned_triples();
    let p = plan("yupana", &projection(), &["src/a.rs".to_string()]).unwrap();
    assert!(
        p.triples() < whole,
        "subset {} must be strictly under the full {whole}",
        p.triples()
    );
    assert!(p.triples() > 0, "and not zero, which would retract a.rs");
}
