//! Appendix-D drift guard: the spec's implementation-status numbers must still
//! describe the tree.
//!
//! `tests/docs_drift.rs` pins the MCP tool count. Nothing pinned the rest, and
//! the rest rotted further than anything a reader would suspect: Appendix D
//! claimed "~10,300 LOC across 39 `.rs` files" against 162 files and ~43k
//! lines, and "Tests: 27" against ~790 — a 4× and a 29× error, sitting under a
//! heading that told newcomers to start there. An out-of-date number is worse
//! than no number, because it is quoted. (Those figures are deliberately
//! approximate: this docstring describes a past failure, so pinning it to an
//! exact count would be the same rot one level up.)
//!
//! ## Why a BAND and not an equality
//!
//! The tool-count test asserts equality, and that is right for a figure that
//! changes a handful of times a year and is a *set* you can diff by name. It is
//! the wrong instrument here. An exact pin on the test count would fail every
//! commit that adds a test, and the reflex that produces is "bump the number in
//! the appendix until green" — which re-derives nothing, leaves every OTHER
//! figure stale, and converts a drift guard into a chore that teaches people to
//! silence it. That is strictly worse than what we have.
//!
//! So each figure is checked within [`TOLERANCE`]. This is a deliberate trade
//! with a bounded downside: the appendix can be up to 10% stale before the gate
//! fires, and it can never be worse than that, because every firing forces a
//! re-derivation from the tree. It catches the failure that actually happened
//! — order-of-magnitude rot — and stays quiet for the ordinary week.
//!
//! ## Why the counts are derived from source TEXT
//!
//! Same reason `docs_drift.rs` reads `src/mcp/server.rs` rather than the
//! compiled server: this test must run on every CI arm, including `default`,
//! and a count taken from `cargo test --list` would vary by feature set. The
//! attribute count and cargo's count differ by a couple of tests (a few are
//! defined through macros), which the band absorbs — and which is a second
//! reason not to demand equality.

use std::fs;
use std::path::{Path, PathBuf};

/// How far Appendix D's stated figures may sit from the tree before this fails.
/// See the module docs for why this is a band rather than an equality.
const TOLERANCE: f64 = 0.10;

fn manifest() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let p = manifest().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Every `.rs` file under `dir`, recursively.
fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rs_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// The first run of digits at or after `anchor`, with `,` and `_` stripped so
/// "42,900" parses. Panics naming the anchor when it is absent — a renamed
/// heading must fail loudly, not silently stop checking anything.
fn number_after(text: &str, anchor: &str) -> usize {
    let start = text
        .find(anchor)
        .unwrap_or_else(|| panic!("Appendix D no longer contains the anchor `{anchor}`"));
    let tail = &text[start + anchor.len()..];
    let digits: String = tail
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '_')
        .filter(char::is_ascii_digit)
        .collect();
    digits
        .parse()
        .unwrap_or_else(|e| panic!("no number after `{anchor}`: {e}"))
}

/// Fail when `stated` is more than [`TOLERANCE`] away from `actual`.
fn assert_close(what: &str, stated: usize, actual: usize) {
    #[allow(clippy::cast_precision_loss)]
    let drift = (stated as f64 - actual as f64).abs() / (actual as f64).max(1.0);
    assert!(
        drift <= TOLERANCE,
        "Appendix D says {stated} {what}; the tree has {actual} \
         ({:.0}% off, tolerance {:.0}%). Recompute the appendix from the tree \
         — do NOT just edit this one number, because the figures rot together.",
        drift * 100.0,
        TOLERANCE * 100.0,
    );
}

/// Only Appendix D, so a number elsewhere in the spec cannot satisfy an anchor
/// by accident.
fn appendix_d() -> String {
    let spec = read("docs/yupana-spec.md");
    let start = spec
        .find("## Appendix D")
        .expect("the spec has an Appendix D");
    let after = &spec[start..];
    let end = after.find("\n## Appendix E").unwrap_or(after.len());
    after[..end].to_string()
}

#[test]
fn appendix_d_source_counts_match_the_tree() {
    let d = appendix_d();
    let files = rs_files(&manifest().join("src"));
    let lines: usize = files
        .iter()
        .map(|p| fs::read_to_string(p).map_or(0, |t| t.lines().count()))
        .sum();

    assert_close(
        "`.rs` files under src/",
        number_after(&d, "`src/`,"),
        files.len(),
    );
    assert_close(
        "lines under src/",
        number_after(&d, "`.rs` files, ~"),
        lines,
    );
}

#[test]
fn appendix_d_test_count_matches_the_tree() {
    let mut attributes = 0usize;
    for dir in ["src", "tests"] {
        for path in rs_files(&manifest().join(dir)) {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            attributes += text.matches("#[test]").count() + text.matches("#[tokio::test]").count();
        }
    }
    assert_close("tests", number_after(&appendix_d(), "**Tests:"), attributes);
}

/// The ratchet's frozen set, checked by NAME rather than by count.
///
/// This one is an equality, not a band, and the difference is the point: the
/// baseline is a debt list that may only shrink, so a file entering or leaving
/// it is exactly the kind of event the appendix must not miss. It also cannot
/// drift by accident the way a line count does — nobody adds a baseline entry
/// without meaning to.
#[test]
fn appendix_d_names_every_baselined_file() {
    let baseline = read("scripts/file-size-baseline.txt");
    let frozen: Vec<&str> = baseline
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| l.split('\t').next())
        .collect();
    assert!(!frozen.is_empty(), "baseline parse looks wrong: {baseline}");

    let d = appendix_d();
    for path in &frozen {
        // The appendix cites these by basename (`promote.rs`), not full path.
        let name = path.rsplit('/').next().unwrap_or(path);
        assert!(
            d.contains(name),
            "`{path}` is frozen in scripts/file-size-baseline.txt but Appendix D \
             does not mention `{name}`. The ratchet's debt list and the status \
             appendix must name the same files."
        );
    }
    assert!(
        d.contains(&format!("{} are the only entries", frozen.len()))
            || d.contains(&format!("Those {} are", frozen.len()))
            || d.contains("are the only entries in `scripts/file-size-baseline.txt`"),
        "Appendix D must say how many files the ratchet freezes; it lists {} \
         in scripts/file-size-baseline.txt",
        frozen.len()
    );
}
