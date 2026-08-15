//! Tests for the verdict spool. Size-exempt (`_test.rs`).

use super::*;
use crate::trace::Response;

fn keypair() -> Ed25519KeyPair {
    let dir = tempfile::tempdir().unwrap();
    crate::verdict::load_or_generate(&dir.path().join("k.pk8")).unwrap()
}

fn fired(id: &str) -> ConstraintEvaluation {
    ConstraintEvaluation::new(id, Outcome::Unsatisfied, Response::Blocked)
}

#[test]
fn precedence_explicit_then_xdg_then_home() {
    assert_eq!(
        resolve_path(Some("/x/v.jsonl"), Some("/s"), Some("/h")).unwrap(),
        PathBuf::from("/x/v.jsonl")
    );
    assert_eq!(
        resolve_path(None, Some("/s"), Some("/h")).unwrap(),
        PathBuf::from("/s/yupana/verdicts.jsonl")
    );
    assert_eq!(
        resolve_path(None, None, Some("/h")).unwrap(),
        PathBuf::from("/h/.local/state/yupana/verdicts.jsonl")
    );
    assert!(resolve_path(None, None, None).is_none());
}

#[test]
fn a_fired_constraint_spools_a_signed_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let n = record_to(
        &path,
        &keypair(),
        &[fired("no-ticket-in-comment")],
        "src/a.rs",
        "// see ABC-123",
        Freshness::Fresh,
    );
    assert_eq!(n, 1);

    let spooled = read_spool(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(spooled.len(), 1);
    assert_eq!(spooled[0].predicate_id, "no-ticket-in-comment");
    assert_eq!(spooled[0].target_ref, "src/a.rs");
    // Every VerdictShape-required field, so quipu's /knot accepts it unchanged.
    for field in [
        "a aegis:Verdict",
        "aegis:predicateId \"no-ticket-in-comment\"",
        "aegis:outcome \"unsatisfied\"",
        "aegis:evidenceHash \"sha256:",
        "aegis:signature \"",
        "aegis:verifier \"yupana\"",
    ] {
        assert!(
            spooled[0].turtle.contains(field),
            "spooled verdict missing `{field}`"
        );
    }
}

#[test]
fn a_satisfied_constraint_records_satisfied_not_the_guard_outcome() {
    // The verdict says what the PREDICATE concluded. A constraint can be
    // unsatisfied while the mode declined to block, and the two must not be
    // conflated — an advise-mode fleet would otherwise be indistinguishable from
    // a compliant one in the governed record.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    record_to(
        &path,
        &keypair(),
        &[
            ConstraintEvaluation::new("clean", Outcome::Satisfied, Response::Logged),
            // Unsatisfied but only WARNED — advise mode.
            ConstraintEvaluation::new("noisy", Outcome::Unsatisfied, Response::Warned),
        ],
        "src/a.rs",
        "evidence",
        Freshness::Fresh,
    );
    let spooled = read_spool(&std::fs::read_to_string(&path).unwrap());
    assert!(spooled[0].turtle.contains("aegis:outcome \"satisfied\""));
    assert!(
        spooled[1].turtle.contains("aegis:outcome \"unsatisfied\""),
        "a warned violation is still an unsatisfied predicate"
    );
}

#[test]
fn an_unknown_outcome_spools_nothing() {
    // `unknown` asserts "there was no evidence", and a constraint yupana evaluated
    // had evidence by construction. Minting satisfied or unsatisfied for it
    // would be a signed claim about something that concluded neither.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let n = record_to(
        &path,
        &keypair(),
        &[ConstraintEvaluation::new(
            "tests-green",
            Outcome::Unknown,
            Response::NoAction,
        )],
        "src/a.rs",
        "evidence",
        Freshness::Fresh,
    );
    assert_eq!(n, 0);
    assert!(!path.exists() || std::fs::read_to_string(&path).unwrap().trim().is_empty());
}

#[test]
fn a_stale_projection_produces_a_stale_verdict() {
    // The field was a hardcoded "fresh". Every verdict yupana could have promoted
    // would have claimed currency it never checked.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    record_to(
        &path,
        &keypair(),
        &[fired("c")],
        "src/a.rs",
        "e",
        Freshness::Stale,
    );
    let spooled = read_spool(&std::fs::read_to_string(&path).unwrap());
    assert!(spooled[0].turtle.contains("aegis:freshness \"stale\""));
}

#[test]
fn recomputing_maps_to_stale_never_fresh() {
    // A yupana-internal state with no governed counterpart. A verdict computed
    // mid-refresh was computed against something that may already have moved;
    // the conservative reading is the only one that cannot overstate.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    record_to(
        &path,
        &keypair(),
        &[fired("c")],
        "src/a.rs",
        "e",
        Freshness::Recomputing,
    );
    let spooled = read_spool(&std::fs::read_to_string(&path).unwrap());
    assert!(spooled[0].turtle.contains("aegis:freshness \"stale\""));
}

#[test]
fn a_missing_key_yields_no_key_rather_than_minting_one() {
    // Key custody: a keypair materialising as a side effect of an agent's edit
    // is not something that should happen quietly. `yupana verifier` is how an
    // operator creates one, deliberately.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent.pk8");
    assert!(existing_key(&path).is_none());
    assert!(!path.exists(), "asking must not create the key");

    // Once it exists, it loads.
    crate::verdict::load_or_generate(&path).unwrap();
    assert!(existing_key(&path).is_some());
}

#[test]
fn an_unwritable_spool_is_swallowed_whole() {
    // The contract that outranks the others: a verdict that cannot be recorded
    // must not become an edit that cannot happen.
    let dir = tempfile::tempdir().unwrap();
    let n = record_to(
        dir.path(), // a directory cannot be opened for append
        &keypair(),
        &[fired("c")],
        "src/a.rs",
        "e",
        Freshness::Fresh,
    );
    assert_eq!(n, 0, "reaching here without panicking IS the assertion");
}

#[test]
fn a_torn_line_is_skipped_not_fatal() {
    // Appending from a short-lived process makes a half-written tail line the
    // expected case, and one bad record must not dam the rest.
    let good = serde_json::json!({
        "predicate_id": "c", "target_ref": "src/a.rs", "turtle": "ttl"
    })
    .to_string();
    let text = format!("{good}\n{{\"predicate_id\": \"tor\n{good}\n");
    assert_eq!(read_spool(&text).len(), 2);
}

#[test]
fn the_spool_rotates_at_the_ceiling_instead_of_growing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    std::fs::write(&path, vec![b'x'; (ROTATE_BYTES + 1) as usize]).unwrap();
    record_to(
        &path,
        &keypair(),
        &[fired("c")],
        "src/a.rs",
        "e",
        Freshness::Fresh,
    );
    assert!(path.with_extension("jsonl.old").exists());
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
}

#[test]
fn a_denied_verdict_spools_its_excerpt_and_a_satisfied_one_does_not() {
    // bobbin-fjh: the spool doubles as the denied-edit similarity corpus, so
    // an UNSATISFIED verdict carries a capped excerpt of the judged text on
    // its spool line (local only, never inside the signed Turtle) — and a
    // satisfied one carries none, because a clean edit is not a denial to
    // learn from.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    record_to(
        &path,
        &keypair(),
        &[
            fired("denied-rule"),
            ConstraintEvaluation::new("clean-rule", Outcome::Satisfied, Response::Logged),
        ],
        "src/a.rs",
        "// the judged text",
        Freshness::Fresh,
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let denied = lines
        .iter()
        .find(|l| l["predicate_id"] == "denied-rule")
        .unwrap();
    assert_eq!(denied["denied_excerpt"], "// the judged text");
    assert!(
        !denied["turtle"].as_str().unwrap().contains("judged text"),
        "the excerpt must never enter the signed Turtle"
    );
    let clean = lines
        .iter()
        .find(|l| l["predicate_id"] == "clean-rule")
        .unwrap();
    assert!(clean.get("denied_excerpt").is_none());
}
