//! rudof <-> quipu shape-verdict AGREEMENT (promotion tail, item 2).
//!
//! yupana validates promotions in-process (rudof); quipu validates the same
//! shapes server-side. The engines agreeing on ACCEPTANCE was proven early;
//! nothing asserted they agree on REJECTION — and that exact gap shipped a
//! live drift: a symbol-IRI collision that yupana's compiled shapes passed and
//! quipu's stored registry refused (the promote lane failed hourly until the
//! shapes were synced). These tests are the mechanism that catches the next
//! drift in CI instead of production.
//!
//! Layer 1 (always runs, pure): rudof itself must be a DISCRIMINATING
//! validator over the shipped fixtures — refuse `violating.ttl`, accept
//! `conforming.ttl`. A validator that has never been seen rejecting is
//! indistinguishable from no validator.
//!
//! Layer 2 (`#[ignore]`, needs live quipu — run with
//! `QUIPU_URL=… cargo test --test shape_agreement -- --ignored`):
//! POST the same fixtures + the same compiled shapes to quipu's `/validate`
//! and assert BOTH engines return the SAME verdict on BOTH fixtures. Verdict
//! agreement, not just each-side sanity.

#![cfg(feature = "quipu")]

use yupana::promote::{validate, CODE_EDGE_SHAPES};

/// Minimal JSON string encoder (no serde in integration-test scope).
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const CONFORMING: &str = include_str!("../shapes/fixtures/conforming.ttl");
const VIOLATING: &str = include_str!("../shapes/fixtures/violating.ttl");

#[test]
fn rudof_accepts_the_conforming_fixture() {
    let v = validate(CONFORMING, CODE_EDGE_SHAPES).expect("validation ran");
    assert!(
        v.conforms,
        "rudof refused the conforming fixture: {:?}",
        v.violations
    );
}

#[test]
fn rudof_refuses_the_violating_fixture() {
    let v = validate(VIOLATING, CODE_EDGE_SHAPES).expect("validation ran");
    assert!(
        !v.conforms,
        "rudof ACCEPTED the violating fixture — the in-process validator cannot reject, \
         which is the exact present-but-inert defect this test exists to catch"
    );
    assert!(
        !v.violations.is_empty(),
        "a refusal must name its violations"
    );
}

/// The agreement assertion proper. Ignored by default: requires a reachable
/// quipu (`QUIPU_URL`, e.g. <http://quipu.example>). A verdict MISMATCH here is
/// shape drift between the engines — fix the shapes sync before promoting.
#[test]
#[ignore = "needs live quipu: QUIPU_URL=... cargo test --test shape_agreement -- --ignored"]
fn rudof_and_quipu_agree_on_both_verdicts() {
    let endpoint =
        std::env::var("QUIPU_URL").expect("set QUIPU_URL to run the live agreement test");

    // Dependency-free HTTP: integration tests cannot see the lib's private
    // deps, and adding dev-deps for one ignored test is weight without value.
    let quipu_verdict = |data: &str| -> bool {
        let url = format!("{}/validate", endpoint.trim_end_matches('/'));
        let body = format!(
            "{{\"shapes\": {}, \"data\": {}}}",
            json_string(CODE_EDGE_SHAPES),
            json_string(data)
        );
        let out = std::process::Command::new("curl")
            .args([
                "-s",
                "-m",
                "60",
                "-X",
                "POST",
                "-H",
                "Content-Type: application/json",
                "--data-binary",
                "@-",
                &url,
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut ch| {
                use std::io::Write;
                ch.stdin.take().unwrap().write_all(body.as_bytes())?;
                ch.wait_with_output()
            })
            .expect("curl ran");
        let text = String::from_utf8_lossy(&out.stdout);
        // Verdict extraction without a JSON dep: the field is authoritative and
        // the two literal forms are the only ones quipu emits.
        if text.contains("\"conforms\":true") || text.contains("\"conforms\": true") {
            true
        } else if text.contains("\"conforms\":false") || text.contains("\"conforms\": false") {
            false
        } else {
            panic!(
                "quipu /validate gave no verdict: {}",
                &text[..text.len().min(200)]
            );
        }
    };

    for (name, data, expect_conform) in [
        ("conforming", CONFORMING, true),
        ("violating", VIOLATING, false),
    ] {
        let rudof = validate(data, CODE_EDGE_SHAPES)
            .expect("rudof ran")
            .conforms;
        let quipu = quipu_verdict(data);
        assert_eq!(
            rudof, quipu,
            "ENGINE DISAGREEMENT on {name} fixture: rudof={rudof} quipu={quipu} — \
             shape drift between in-process and server-side validation"
        );
        assert_eq!(
            rudof, expect_conform,
            "both engines agree but on the WRONG verdict for {name}"
        );
    }
}
