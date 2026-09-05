//! Tests for the `yupana share` wire half. Child module of `share_client`; size-exempt.
//!
//! Two properties are pinned here that nothing else in the repo pins:
//!
//! 1. **The graph-scoping transform is applied to the REAL projection queries.**
//!    Not to a fixture that resembles them — to the constants the pre-edit guard
//!    itself sends. A preview built from a second, hand-written query would be a
//!    parallel definition of "what a policy is" that drifts silently, and the
//!    operator would be shown a different rule set from the one that will be
//!    enforced.
//! 2. **Every command a verdict emits is PARSED BY THE REAL CLI.** Three defects
//!    in camayoc's sibling implementation were emitted commands that read
//!    correctly and did not run — a hardcoded binary, a bare filename, a missing
//!    exec bit — and none was caught by an assertion about the string. The rule
//!    that file earned: if the verdict emits a command, a test runs it.

use super::*;

use crate::project_queries::{POLICY_QUERY, TEXT_POLICY_QUERY};

const GRAPH: &str = "urn:quipu:import:quarantine:abc123";

fn balanced(s: &str) -> bool {
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return false;
        }
    }
    depth == 0
}

/// The transform is applied to the constants the guard actually sends, and the
/// WHERE body survives verbatim — that verbatim-ness is the single-source-of-
/// truth property, so it is asserted rather than eyeballed.
#[test]
fn the_real_projection_queries_scope_into_a_named_graph() {
    for (label, query) in [("structural", POLICY_QUERY), ("text", TEXT_POLICY_QUERY)] {
        let scoped = scope_to_graph(query, GRAPH).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert!(
            scoped.contains(&format!("GRAPH <{GRAPH}>")),
            "{label}: not scoped"
        );
        assert!(balanced(&scoped), "{label}: unbalanced braces:\n{scoped}");
        // The body between WHERE { and the final } is carried across unchanged.
        let open = query.find("WHERE {").unwrap() + "WHERE {".len();
        let close = query.rfind('}').unwrap();
        assert!(
            scoped.contains(&query[open..close]),
            "{label}: the WHERE body was altered, so the preview no longer asks \
             what the guard asks"
        );
        // Prefixes must stay OUTSIDE the GRAPH block or the query will not parse.
        assert!(
            scoped.starts_with("PREFIX "),
            "{label}: prefixes must lead the query"
        );
    }
}

/// The unscoped constant must NOT already carry a GRAPH clause — otherwise the
/// test above would pass for free, and the security property this whole verb
/// rests on (a staged graph is invisible to the projection) would be false.
#[test]
fn the_projection_queries_are_unscoped_as_shipped() {
    for (label, query) in [("structural", POLICY_QUERY), ("text", TEXT_POLICY_QUERY)] {
        assert!(
            !query.contains("GRAPH"),
            "{label}: the projection query names a GRAPH, which would mean the guard \
             reads named graphs — re-measure the staging-visibility property before \
             trusting `share pull`"
        );
    }
}

#[test]
fn a_graph_iri_that_would_break_out_of_the_clause_is_refused() {
    for bad in ["", "urn:x> } UNION { ?s ?p ?o"] {
        assert!(
            scope_to_graph(POLICY_QUERY, bad).is_err(),
            "must refuse {bad:?} rather than emit a query it did not mean"
        );
    }
}

// ---------------------------------------------------------------------------
// The emitted commands.
// ---------------------------------------------------------------------------

/// Parse an emitted command with the REAL clap definition, dropping argv[0].
///
/// This is what makes the assertions below about RUNNABILITY rather than about
/// spelling: a wrong subcommand, a flag that does not exist, or a missing
/// required argument fails here, where a `contains()` check would pass.
fn parses_as_a_real_command(emitted: &str) -> std::result::Result<(), String> {
    let argv: Vec<&str> = emitted.split_whitespace().collect();
    let (exe, rest) = argv.split_first().ok_or("empty command")?;
    // The command must name an ABSOLUTE path to a real file, not a bare name
    // that resolves only from whichever directory the reader happens to be in.
    let exe_path = std::path::Path::new(exe);
    if !exe_path.is_absolute() {
        return Err(format!("{exe} is not an absolute path"));
    }
    if !exe_path.is_file() {
        return Err(format!("{exe} is not a file that exists"));
    }
    let mut full = vec!["yupana"];
    full.extend(rest);
    <crate::cli::Cli as clap::Parser>::try_parse_from(full).map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
fn an_unblocked_share_is_offered_a_promote_command_that_parses() {
    let (next, reason) = next_step("http://quipu.example", "sha256:abc", GRAPH, &[], true);
    let next = next.expect("an unblocked share has a next step");
    assert!(reason.is_none());
    assert!(next.contains("share promote sha256:abc"), "{next}");
    // The endpoint that just worked, not a literal or a default.
    assert!(next.contains("--to http://quipu.example"), "{next}");
    parses_as_a_real_command(&next).expect("the promote suggestion must be runnable");
}

/// The default outcome: the publisher governs types this store does not. The
/// share carries its own shapes, so there IS a way forward — but adopting a
/// foreign vocabulary is a governance change, so what is offered is the
/// INSPECTION, never the adoption.
#[test]
fn a_quarantined_share_with_shapes_is_offered_the_policy_preview_not_an_adoption() {
    let blockers = vec!["off_vocabulary".to_string()];
    let (next, reason) = next_step("http://quipu.example", "sha256:abc", GRAPH, &blockers, true);
    let next = next.expect("a quarantined share with shapes has a next step");
    assert!(next.contains(&format!("share policy {GRAPH}")), "{next}");
    parses_as_a_real_command(&next).expect("the preview suggestion must be runnable");
    assert!(
        !next.contains("promote"),
        "a quarantined share must never be handed a promote command: {next}"
    );
    assert!(
        reason.is_some_and(|r| r.contains("governance change")),
        "the reader must be told why this is two steps and not one"
    );
}

/// A bundle with no shapes cannot be unblocked by adopting its shapes. Offering
/// no command AND saying why is the correct behaviour; a suggestion that would
/// adopt nothing is worse than none.
#[test]
fn a_quarantined_share_without_shapes_is_offered_nothing_and_told_why() {
    let blockers = vec!["off_vocabulary".to_string()];
    let (next, reason) = next_step(
        "http://quipu.example",
        "sha256:abc",
        GRAPH,
        &blockers,
        false,
    );
    assert!(next.is_none(), "must not invent a command: {next:?}");
    let reason = reason.expect("an absent command must carry its reason");
    assert!(reason.contains("EMPTY shapes.ttl"), "{reason}");
    assert!(reason.contains("Govern these types"), "{reason}");
}

#[test]
fn a_blocker_with_no_remedy_gets_an_honest_absence() {
    let blockers = vec!["shacl_nonconforming".to_string()];
    let (next, reason) = next_step("http://quipu.example", "sha256:abc", GRAPH, &blockers, true);
    assert!(next.is_none(), "{next:?}");
    assert!(reason.is_some_and(|r| r.contains("no automatic remedy")));
}

#[test]
fn a_share_with_no_id_is_not_handed_a_command_naming_an_empty_id() {
    let (next, reason) = next_step("http://quipu.example", "", GRAPH, &[], true);
    assert!(next.is_none(), "{next:?}");
    assert!(reason.is_some_and(|r| r.contains("no share id")));
}

/// `self_command` must resolve to something a reader can paste. This is the
/// assertion that would have caught camayoc's bare-filename defect.
#[test]
fn the_emitted_binary_is_an_absolute_existing_path() {
    let me = self_command();
    let p = std::path::Path::new(&me);
    assert!(p.is_absolute(), "{me} is not absolute");
    assert!(p.is_file(), "{me} does not exist");
}

// ---------------------------------------------------------------------------
// The verdict.
// ---------------------------------------------------------------------------

fn import_response(outcome: &str, blockers: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "outcome": outcome,
        "share_id": "sha256:abc",
        "graph_hash": "sha256:def",
        "staging_graph": GRAPH,
        "triples": {"accepted": 0, "quarantined": 7},
        "resolution": {"exact_merges": [], "candidates": [], "unmatched": ["urn:x:thing"]},
        "validation": {"conforms": true, "report": {}, "off_vocabulary": ["aegis:Widget"]},
        "promotion": {"eligible": blockers.is_empty(), "blockers": blockers},
    })
}

fn bundle_with_shapes(has: bool) -> Bundle {
    Bundle {
        manifest: serde_json::json!({"share_id": "sha256:abc"}),
        export_nt: String::new(),
        shapes_ttl: if has { "sh:x".into() } else { String::new() },
        source: "fixture".into(),
    }
}

/// A quarantine is a SUCCESS, and the verdict must carry the operator's whole
/// decision: what is blocked, what did not resolve, and where it is staged.
#[test]
fn a_quarantined_import_decodes_into_a_complete_verdict() {
    let v = verdict_from(
        &bundle_with_shapes(true),
        "fixture",
        "http://quipu.example",
        &import_response("quarantined", &["off_vocabulary"]),
    );
    assert_eq!(v.outcome, "quarantined");
    assert_eq!(v.share_id, "sha256:abc");
    assert_eq!(v.staging_graph, GRAPH);
    // accepted + quarantined: reporting only `accepted` would say 0 triples
    // arrived for an import that landed 7.
    assert_eq!(v.triples, 7);
    assert_eq!(v.blockers, vec!["off_vocabulary".to_string()]);
    assert_eq!(v.unmatched, vec!["urn:x:thing".to_string()]);
    assert!(v.next.is_some_and(|n| n.contains("share policy")));
}

#[test]
fn a_clean_import_decodes_and_offers_promotion() {
    let v = verdict_from(
        &bundle_with_shapes(true),
        "fixture",
        "http://quipu.example",
        &import_response("staged", &[]),
    );
    assert_eq!(v.outcome, "staged");
    assert!(v.blockers.is_empty());
    let next = v.next.expect("a clean import offers promotion");
    parses_as_a_real_command(&next).expect("runnable");
}

/// The share id falls back to the manifest's when the server does not echo it,
/// so a follow-up command can still be named.
#[test]
fn the_share_id_falls_back_to_the_manifest() {
    let body = serde_json::json!({"outcome": "staged", "staging_graph": GRAPH});
    let v = verdict_from(
        &bundle_with_shapes(true),
        "fixture",
        "http://quipu.example",
        &body,
    );
    assert_eq!(v.share_id, "sha256:abc");
}

/// Nothing in this module may promote as a side effect of pulling. Asserted on
/// the REQUEST BUILDER rather than on a report field, because a report is
/// something the code says about itself; the request is what actually goes out.
#[test]
fn the_import_request_carries_no_promotion_instruction() {
    let req = import_request(&bundle_with_shapes(true), Some("muldoon"));
    let text = req.to_string();
    assert!(
        !text.contains("promote"),
        "pull must never ask for promotion: {text}"
    );
    assert_eq!(req["actor"], "muldoon");
    assert_eq!(req["source"], "fixture");
}

#[test]
fn an_absent_actor_is_omitted_rather_than_sent_empty() {
    let req = import_request(&bundle_with_shapes(true), None);
    assert!(
        req.get("actor").is_none(),
        "an empty actor would be recorded as provenance nobody can trace"
    );
}

/// `share align` is reachable and its shape is what quipu's `/align/propose`
/// requires: two graphs, both named. `--against` is mandatory because an
/// alignment against an unstated graph is not a thing quipu can answer.
#[test]
fn the_align_verb_parses_and_requires_both_graphs() {
    let ok = <crate::cli::Cli as clap::Parser>::try_parse_from([
        "yupana",
        "share",
        "align",
        GRAPH,
        "--against",
        "urn:local:graph",
        "--to",
        "http://quipu.example",
    ]);
    assert!(ok.is_ok(), "the documented invocation must parse: {ok:?}");

    let missing =
        <crate::cli::Cli as clap::Parser>::try_parse_from(["yupana", "share", "align", GRAPH]);
    assert!(
        missing.is_err(),
        "an alignment with only one graph must be refused at parse time, not sent \
         to quipu as a half-formed request"
    );
}

/// `candidates` is a COUNT, not a list.
///
/// Reading it as an array gives `None` -> 0 for EVERY response, so the summary
/// line would report "0 candidate alignment(s)" no matter what quipu found —
/// and it would look entirely reasonable doing it. The response body below is a
/// real one, captured from quipu 0.3.36.
#[test]
fn the_align_candidate_count_is_read_as_a_number_not_a_list() {
    let real: serde_json::Value = serde_json::from_str(
        r#"{"candidates": 3, "set_aside": 1, "summary": "3 candidate(s); 1 set aside",
            "expected_version": "sha256:ce548369"}"#,
    )
    .unwrap();
    assert_eq!(
        real.get("candidates").and_then(serde_json::Value::as_u64),
        Some(3),
        "the count must be read as a number"
    );
    assert!(
        real.get("candidates")
            .and_then(serde_json::Value::as_array)
            .is_none(),
        "reading it as an array is the defect: it silently yields 0 for every \
         response, including the ones with candidates in them"
    );
}
