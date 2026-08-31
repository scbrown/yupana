//! yupana's own tree must not carry internal identifiers (aegis-wvuhj).
//!
//! WHY THIS FILE EXISTS, AND WHY ITS ABSENCE WAS THE ACTUAL BUG. yupana IS the
//! enforcement point for "internal identifiers must not enter public-remote
//! repos" — the rule is `crate::textrules`, the guard is the pre-edit hook, and
//! `src/hook/pre_edit_test.rs` asserts the guard fires on a `.lan` hostname. And
//! `scbrown/yupana` is PUBLIC and its default branch carried **60 lines of them
//! across 15 files** (measured 2026-08-04; gennaro measured 58/14 two days
//! earlier, so it was still GROWING while the bead sat open).
//!
//! The edit-time guard was never the gap. It only ever sees text an edit
//! INTRODUCES — by deliberate design, so a dirty file does not brick every edit
//! to it. Nothing ever scanned the tree that already existed, so pre-existing
//! debt was permanently invisible to the mechanism built to prevent it. bobbin,
//! same fleet and same rule, has had `tests/no_internal_identifiers.rs` all
//! along. This is that ratchet, for the repo that enforces the rule.
//!
//! THE SYNTHETIC-NAME RULE, which is the whole remedy for test fixtures: a test
//! that needs a forbidden-looking token must invent one. `db.lan` proves the
//! `.lan` rule fires exactly as well as a real hostname does, and leaks nothing.
//! "The guard needs real data to be tested" is false and bobbin already proved
//! it. Wherever a token did NOT need to match a pattern at all — doc comments,
//! ssh/scp examples, book pages — the scrub used names outside the patterns
//! entirely (`web.example`, `$QUIPU_URL`), so they need no exemption here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;

/// The identifier classes, mirroring the `aegis:InternalIdentifierPattern`
/// catalogue the pre-edit hook enforces. Self-contained regexes rather than a
/// projection from quipu ON PURPOSE: this test has to run in CI with no graph
/// reachable, and a ratchet that silently skips when its data source is absent
/// is the failure it exists to prevent.
fn patterns() -> Vec<(&'static str, Regex)> {
    vec![
        (
            "internal hostname",
            Regex::new(r"[a-z0-9_.-]+\.lan\b").unwrap(),
        ),
        (
            "internal service host",
            Regex::new(r"[a-z0-9_-]+\.svc\b").unwrap(),
        ),
        (
            "private address",
            Regex::new(r"\b192\.168\.\d{1,3}\.\d{1,3}\b").unwrap(),
        ),
        (
            "operator home path",
            Regex::new(r"/home/([a-z][a-z0-9_-]*)").unwrap(),
        ),
    ]
}

/// Home-directory names that are obviously stand-ins, not real operators.
///
/// THIS LIST IS WHY THE PATTERN IS GENERIC. The first version of this file
/// matched `/home/(?:braino|stiwi)` — it named the two real operators, IN A
/// PUBLIC REPO, inside the test written to stop exactly that. It was caught by
/// grepping the pushed branch rather than by any test, because this file is
/// `exempt()` and so is the one place the ratchet cannot see itself.
///
/// Matching ANY `/home/<name>` and subtracting known placeholders is both safer
/// and strictly more general: it catches an operator this fleet has not hired
/// yet, and it publishes nobody's username.
const PLACEHOLDER_HOMES: &[&str] = &[
    "agent", "x", "user", "you", "me", "jsmith", "example", "someone",
];

/// True when a capture is a real finding rather than a documentation stand-in.
fn is_real_hit(label: &str, caps: &regex::Captures<'_>) -> bool {
    if label != "operator home path" {
        return true;
    }
    match caps.get(1) {
        Some(name) => !PLACEHOLDER_HOMES.contains(&name.as_str()),
        None => true,
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn tracked_files() -> Vec<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root())
        .arg("ls-files")
        .output()
        .expect("git ls-files");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| root().join(l))
        .collect()
}

/// Files allowed to contain a matching token, and WHY. Kept to the two that
/// genuinely assert the guard FIRES — a file that must name a forbidden shape to
/// forbid it. This is the same carve-out `TextRule::exempt_path_regex` exists
/// for. Every name inside them is synthetic; the exemption buys the ability to
/// test detection, never the right to name a real host.
fn exempt(rel: &Path) -> bool {
    matches!(
        rel.to_string_lossy().as_ref(),
        "tests/no_internal_identifiers.rs"      // this file names the patterns
            | "src/textrules.rs"                // asserts the .lan rule matches
            | "src/hook/pre_edit_test.rs" // asserts the verdict text
    )
}

/// The ontology namespace is NOT a leak to scrub and must not be quietly
/// swallowed either (aegis-wvuhj category 1).
///
/// `http://aegis.gastown.local/ontology/` is a live DATA CONTRACT, not an
/// example: `src/export.rs` mints every IRI under it and ~102,945 subjects are
/// already stored against it in quipu. Repointing it is a data migration plus a
/// cross-repo decision with bobbin (bobbin#58 is blocked on the same one), and
/// `shapes/code-edges.ttl` states outright that changing the prefix breaks
/// validation.
///
/// So it is ALLOWED — and COUNTED, by the test below, so that "allowed" cannot
/// decay into "unnoticed" the way the rest of this debt did. The day the
/// namespace decision lands, that test fails and points here.
const ONTOLOGY_NS: &str = "aegis.gastown.local";

#[test]
fn no_internal_identifiers_in_any_tracked_file() {
    let pats = patterns();
    let mut offenders: BTreeSet<String> = BTreeSet::new();

    for path in tracked_files() {
        let rel = path.strip_prefix(root()).unwrap_or(&path).to_path_buf();
        if exempt(&rel) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue; // unreadable or deleted-but-tracked: not this test's job
        };
        let text = String::from_utf8_lossy(&bytes);
        for (label, rx) in &pats {
            for caps in rx.captures_iter(&text) {
                let hit = caps.get(0).unwrap().as_str();
                // The namespace is a data contract, tracked separately below.
                if hit.contains(ONTOLOGY_NS) || !is_real_hit(label, &caps) {
                    continue;
                }
                offenders.insert(format!("{}: {label} {hit:?}", rel.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "internal identifier(s) in a PUBLIC repo (scbrown/yupana):\n  {}\n\n\
         Use a synthetic name. If the token does NOT need to match a rule \
         pattern, put it outside them entirely (`web.example`, `$QUIPU_URL`). \
         If a test must prove the guard FIRES, invent one that matches \
         (`db.lan`) and add the file to `exempt()` with a reason.",
        offenders.into_iter().collect::<Vec<_>>().join("\n  ")
    );
}

#[test]
fn the_ratchet_catches_each_class() {
    // POSITIVE CONTROL, one per class. A guard never seen catching anything is a
    // function returning an empty set, and it looks exactly like a clean repo —
    // which is precisely how 60 lines accumulated in a repo that enforces this
    // rule on everyone else.
    let pats = patterns();
    for (expect, sample) in [
        ("internal hostname", "connect to db.lan now"),
        ("internal service host", "http://thing.svc/knot"),
        ("private address", "addr 192.168.0.1"),
        // A NON-placeholder home, spelled so it is obviously not a real
        // operator here — the control must prove the class is caught without
        // naming anyone (see PLACEHOLDER_HOMES).
        ("operator home path", "/home/opnametwo/src/x"),
    ] {
        let caught = pats.iter().any(|(label, rx)| {
            *label == expect && rx.captures_iter(sample).any(|c| is_real_hit(label, &c))
        });
        assert!(caught, "the ratchet missed a planted {expect}: {sample:?}");
    }
}

#[test]
fn placeholder_home_paths_are_not_flagged() {
    // The other half of the control: the tree legitimately documents
    // `/home/agent` and `/home/x`, and a ratchet that screamed about those
    // would be switched off within a day.
    let pats = patterns();
    for sample in ["/home/agent/work", "/home/x/bin", "/home/jsmith/src"] {
        let flagged = pats
            .iter()
            .any(|(label, rx)| rx.captures_iter(sample).any(|c| is_real_hit(label, &c)));
        assert!(!flagged, "a placeholder home was flagged: {sample:?}");
    }
}

#[test]
fn the_ontology_namespace_allowance_is_still_needed_and_still_bounded() {
    // The allowance above is deliberate, and this is what stops it rotting into
    // a silent permanent exception. It pins the namespace to the files that are
    // genuinely part of the data contract. A NEW file reaching for the real
    // namespace fails here and has to justify itself; the day bobbin#58's
    // repointing decision lands, this fails too and leads straight to the
    // allowance to delete.
    let mut carriers: BTreeSet<String> = BTreeSet::new();
    for path in tracked_files() {
        let rel = path.strip_prefix(root()).unwrap_or(&path).to_path_buf();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if String::from_utf8_lossy(&bytes).contains(ONTOLOGY_NS) {
            carriers.insert(rel.display().to_string());
        }
    }

    let expected: BTreeSet<String> = [
        "docs/book/src/concepts/promotion.md",
        "docs/yupana-spec.md",
        "scripts/delegate-boundary-guard.py",
        // The e2e harness and the F1 eval seed a live quipu with work items
        // and query them back — both in the deployed ontology namespace, the
        // same data contract as src/project_queries.rs. The briefing sources
        // carry it for the same reason (label/provenance/centrality SPARQL).
        "scripts/e2e/eval_f1.py",
        "scripts/e2e/harness.py",
        "shapes/code-edges.ttl",
        // The deviation-seeded counterpart to brief_sources: same vocabulary,
        // same data contract, asked of a path rather than of an item.
        "src/brief_deviation.rs",
        "src/brief_sources.rs",
        "shapes/fixtures/conforming.ttl",
        "shapes/fixtures/violating.ttl",
        "src/export.rs",
        "src/project_exposure.rs",
        // Project-memory policy asks the governed graph which commands are
        // memory-heavy, so its SPARQL uses the same deployed vocabulary as
        // the other project query surfaces.
        "src/project_memory.rs",
        "src/project_queries.rs",
        // §9.4's branch qualifier and §9.7's commit provenance both write into
        // the promoted graph, so their fixtures and assertions are the same data
        // contract `src/export.rs` and `src/promote_test.rs` already carry: a
        // projection fixture is only a fixture if it uses the namespace the
        // emitter emits, and a test that asserted on a stand-in namespace would
        // pass while the emitter wrote to a graph nobody joins.
        "src/promote_branch_test.rs",
        "src/promote_provenance.rs",
        "src/promote_provenance_test.rs",
        "src/promote_test.rs",
        "src/verdict.rs",
        // The end-to-end promotion tests assert on the BYTES that reached a
        // `/knot` stand-in. The commit IRI's base is the fact under test —
        // the out-of-tree ingest lane minting under a DIFFERENT base is exactly
        // what left its provenance edges unjoinable (GH #5) — so the literal
        // has to be here or the assertion checks nothing.
        "tests/cli.rs",
        "tests/no_internal_identifiers.rs",
    ]
    .iter()
    .map(std::string::ToString::to_string)
    .collect();

    assert_eq!(
        carriers, expected,
        "the ontology-namespace footprint moved. This is NOT a scrub target — it \
         is a live data contract (see ONTOLOGY_NS above and bobbin#58). If a new \
         file legitimately needs it, add it here. If the repointing decision has \
         landed, delete the allowance and this test together."
    );
}
