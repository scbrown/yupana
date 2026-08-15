//! Tests for `project` — the quipu policy projection registry and its
//! freshness/exposure semantics. Child module of `project` (`super::*` reaches
//! its private helpers); size-exempt (`_test.rs`).

use super::*;
// The decode moved to `project_decode` for size; these tests exercise it
// through `project`'s re-exports, so name the types it produces explicitly.
use crate::constraint::{ConstraintClass, VerificationPoint};
use crate::rules::MatchType;

#[test]
fn text_projection_explicitly_traverses_text_rule_subclasses() {
    // Quipu's asserted-only type queries intentionally withhold subclass
    // inference. A plain `a aegis:TextRule` therefore drops every concrete
    // rule class; keep the traversal in the wire query, not as a server default.
    assert!(
        TEXT_POLICY_QUERY.contains("a/rdfs:subClassOf* aegis:TextRule"),
        "the text-rule projection must include concrete TextRule subclasses"
    );
}

/// A standard SPARQL-results JSON body with the two shipped policies.
fn catalog_json() -> String {
    serde_json::json!({
        "head": { "vars": ["name", "language", "query", "pattern", "matchType", "gate", "effect"] },
        "results": { "bindings": [
            {
                "name": { "type": "literal", "value": "todo-needs-ticket" },
                "language": { "type": "literal", "value": "rust" },
                "query": { "type": "literal", "value": "(line_comment) @c" },
                "pattern": { "type": "literal", "value": "\\b[A-Z]+-[0-9]+\\b" },
                "matchType": { "type": "literal", "value": "must-match" },
                "gate": { "type": "literal", "value": "\\bTODO\\b" },
                "effect": { "type": "literal", "value": "warn" }
            },
            {
                "name": { "type": "literal", "value": "no-ticket-in-comment" },
                "language": { "type": "literal", "value": "rust" },
                "query": { "type": "literal", "value": "(line_comment) @c" },
                "pattern": { "type": "literal", "value": "\\b[A-Z]+-[0-9]+\\b" },
                "matchType": { "type": "literal", "value": "must-not-match" },
                "effect": { "type": "literal", "value": "deny" }
            }
        ]}
    })
    .to_string()
}

#[test]
fn decodes_the_catalog_into_projected_policies() {
    let policies = decode_policies(&catalog_json()).unwrap();
    assert_eq!(policies.len(), 2);

    let todo = &policies[0];
    assert_eq!(todo.rule.name, "todo-needs-ticket");
    assert_eq!(todo.rule.language, "rust");
    assert_eq!(todo.rule.match_type, MatchType::MustMatch);
    assert_eq!(todo.rule.gate.as_deref(), Some("\\bTODO\\b"));
    assert_eq!(todo.effect, "warn");

    let ban = &policies[1];
    assert_eq!(ban.rule.match_type, MatchType::MustNotMatch);
    assert_eq!(ban.effect, "deny");
    assert!(ban.rule.gate.is_none());
}

#[test]
fn a_decoded_rule_actually_evaluates() {
    // The whole point of congruence: a projected policy is a Rule that runs.
    let policies = decode_policies(&catalog_json()).unwrap();
    let rules: Vec<Rule> = policies.into_iter().map(|p| p.rule).collect();
    let violations = crate::rules::evaluate(&rules, "// see ABC-123\n", "rust", "src/a.rs");
    assert!(
        violations.iter().any(|v| v.rule == "no-ticket-in-comment"),
        "the projected no-ticket rule must fire on a ticket comment"
    );
}

#[test]
fn a_missing_required_binding_is_an_error_not_a_dropped_row() {
    // A broken sync must be loud, never look like "quipu has no policies".
    let bad = serde_json::json!({
        "results": { "bindings": [
            { "language": { "value": "rust" }, "query": { "value": "(line_comment) @c" } }
        ]}
    })
    .to_string();
    let err = decode_policies(&bad).unwrap_err();
    assert!(matches!(err, Error::Projection(_)));
}

#[test]
fn an_unknown_match_type_is_rejected() {
    let bad = serde_json::json!({
        "results": { "bindings": [{
            "language": { "value": "rust" },
            "query": { "value": "(line_comment) @c" },
            "pattern": { "value": "x" },
            "matchType": { "value": "must-implode" }
        }]}
    })
    .to_string();
    assert!(matches!(
        decode_policies(&bad).unwrap_err(),
        Error::Projection(_)
    ));
}

#[test]
fn evaluate_projected_tags_blocking_by_governed_effect() {
    let policies = decode_policies(&catalog_json()).unwrap();
    // The no-ticket policy has effect "deny" (blocking); todo-needs-ticket is
    // "warn" (advisory). A comment carrying a ticket trips the deny policy.
    let violations = evaluate_projected(
        &policies,
        "// see ABC-123\n",
        "rust",
        "src/a.rs",
        crate::policy::Mode::Enforce,
    );
    assert_eq!(violations.len(), 1);
    assert!(violations[0].blocking, "a deny-effect policy must block");
    assert!(violations[0].message.contains("no-ticket-in-comment"));
}

#[test]
fn advise_mode_is_a_ceiling_no_class_blocks_under_it() {
    // The staging guarantee. A deployment in advise mode never blocks, whatever
    // the class or effect says — that is what makes it safe to project a new
    // hard constraint before anyone has measured its false-positive rate.
    let policies = decode_policies(&catalog_json()).unwrap();
    let violations = evaluate_projected(
        &policies,
        "// see ABC-123\n",
        "rust",
        "src/a.rs",
        crate::policy::Mode::Advise,
    );
    assert_eq!(violations.len(), 1, "it still REPORTS");
    assert!(!violations[0].blocking, "and does not block");
}

#[test]
fn effect_blocks_maps_governed_effects() {
    assert!(effect_blocks("deny"));
    assert!(effect_blocks("require-approval"));
    assert!(effect_blocks("escalate"));
    assert!(!effect_blocks("warn"));
    assert!(!effect_blocks("record"));
    assert!(!effect_blocks("allow"));
    // Unknown effects are conservatively blocking.
    assert!(effect_blocks("mystery"));
}

#[test]
fn a_fresh_registry_starts_stale_and_empty() {
    // Before the first sync there is nothing, and it is honestly stale — never
    // a fresh-looking empty policy set that would silently enforce nothing.
    let reg = ProjectionRegistry::new("http://localhost:8080");
    assert!(reg.policies().is_empty());
    assert_eq!(reg.freshness(), Freshness::Stale);
}

#[test]
fn set_policies_marks_the_cache_fresh() {
    let mut reg = ProjectionRegistry::new("http://localhost:8080");
    reg.set_policies(decode_policies(&catalog_json()).unwrap());
    assert_eq!(reg.freshness(), Freshness::Fresh);
    assert_eq!(reg.policies().len(), 2);
}

#[test]
fn a_failed_refresh_goes_stale_but_keeps_last_known_policies() {
    let mut reg = ProjectionRegistry::new("http://127.0.0.1:1"); // unreachable
    reg.set_policies(decode_policies(&catalog_json()).unwrap());
    assert_eq!(reg.freshness(), Freshness::Fresh);
    // The refresh fails (nothing is listening); the cache goes stale but the
    // last-known policies survive so the guard keeps enforcing, honestly stale.
    assert!(reg.refresh().is_err());
    assert_eq!(reg.freshness(), Freshness::Stale);
    assert_eq!(reg.policies().len(), 2);
}

// --- one entity is one rule (the OPTIONAL cross-product) ----------------------

/// A pattern entity returned TWICE because it carries two `rdfs:comment`s.
/// SPARQL returns the cross product of the query's OPTIONALs, so this is what
/// the live catalogue actually sends — not a contrived shape.
fn two_comments_json() -> String {
    let row = |comment: &str| {
        serde_json::json!({
            "s": { "type": "uri", "value": "http://example.invalid/ontology/pattern_demo" },
            "regex": { "type": "literal", "value": "\\bwidget\\b" },
            "tier": { "type": "literal", "value": "block" },
            "class": { "type": "literal", "value": "hostname" },
            "rationale": { "type": "literal", "value": comment }
        })
    };
    serde_json::json!({
        "head": { "vars": ["s", "regex", "tier", "class", "rationale"] },
        "results": { "bindings": [row("the reason"), row("the exemption note")] }
    })
    .to_string()
}

#[test]
fn one_entity_with_two_comments_is_one_rule() {
    // Measured on the live catalogue before this fix: 7 pattern entities
    // projected as 11 rules. The duplicates fired twice for one edit, and the
    // two copies carried different rationales — an advisory arguing with itself.
    let rules = decode_text_rules(&two_comments_json()).unwrap();
    assert_eq!(
        rules.len(),
        1,
        "the OPTIONAL cross product produced duplicates"
    );
    assert_eq!(rules[0].name, "pattern_demo");
}

#[test]
fn merging_keeps_every_distinct_rationale() {
    // Dropping one would lose the author's reasoning, and picking by row order
    // would be arbitrary AND unstable across graph writes.
    let rules = decode_text_rules(&two_comments_json()).unwrap();
    let rationale = rules[0].rationale.as_deref().unwrap();
    assert!(rationale.contains("the reason"));
    assert!(rationale.contains("the exemption note"));
}

#[test]
fn an_identical_repeated_optional_does_not_duplicate_itself() {
    let row = serde_json::json!({
        "s": { "type": "uri", "value": "http://example.invalid/ontology/pattern_demo" },
        "regex": { "type": "literal", "value": "\\bwidget\\b" },
        "tier": { "type": "literal", "value": "warn" },
        "rationale": { "type": "literal", "value": "same" }
    });
    let body = serde_json::json!({
        "head": { "vars": ["s", "regex", "tier", "rationale"] },
        "results": { "bindings": [row.clone(), row] }
    })
    .to_string();
    let rules = decode_text_rules(&body).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].rationale.as_deref(), Some("same"));
}

#[test]
fn a_conflicting_required_field_is_refused_not_merged() {
    // Two different regexes under one name are two rules wearing one name.
    // Picking either silently is how a guard enforces something nobody wrote.
    let mk = |regex: &str| {
        serde_json::json!({
            "s": { "type": "uri", "value": "http://example.invalid/ontology/pattern_demo" },
            "regex": { "type": "literal", "value": regex },
            "tier": { "type": "literal", "value": "block" }
        })
    };
    let body = serde_json::json!({
        "head": { "vars": ["s", "regex", "tier"] },
        "results": { "bindings": [mk("\\bone\\b"), mk("\\btwo\\b")] }
    })
    .to_string();
    let err = decode_text_rules(&body).unwrap_err();
    assert!(
        format!("{err}").contains("conflicting definitions"),
        "a conflicting rule must refuse loudly, got: {err}"
    );
}

#[test]
fn distinct_entities_are_still_distinct_rules() {
    // The dedup keys on the entity, never on the pattern: two entities may
    // legitimately share a regex while differing in tier or exemptions.
    let mk = |name: &str| {
        serde_json::json!({
            "s": { "type": "uri", "value": format!("http://example.invalid/ontology/{name}") },
            "regex": { "type": "literal", "value": "\\bwidget\\b" },
            "tier": { "type": "literal", "value": "warn" }
        })
    };
    let body = serde_json::json!({
        "head": { "vars": ["s", "regex", "tier"] },
        "results": { "bindings": [mk("pattern_a"), mk("pattern_b")] }
    })
    .to_string();
    assert_eq!(decode_text_rules(&body).unwrap().len(), 2);
}

// ── SARC constraint metadata (Phase 1) ───────────────────────────────────────

/// One policy row, with whatever SARC fields the caller supplies. Built as a
/// map so a test can express "this binding is ABSENT" — the case that matters
/// most here — rather than only "this binding is empty".
fn policy_row(name: &str, effect: &str, extra: &[(&str, &str)]) -> serde_json::Value {
    let lit = |v: &str| serde_json::json!({ "type": "literal", "value": v });
    let mut row = serde_json::Map::new();
    row.insert("policy".into(), lit(&format!("http://aegis/{name}")));
    row.insert("name".into(), lit(name));
    row.insert("language".into(), lit("rust"));
    row.insert("query".into(), lit("(line_comment) @c"));
    row.insert("pattern".into(), lit("\\b[A-Z]+-[0-9]+\\b"));
    row.insert("matchType".into(), lit("must-not-match"));
    row.insert("effect".into(), lit(effect));
    for (k, v) in extra {
        row.insert((*k).into(), lit(v));
    }
    serde_json::Value::Object(row)
}

/// A SPARQL-results envelope around `rows`.
///
/// Takes a slice rather than a `Vec` so no caller has to hand over ownership of
/// rows it may want to reuse — and so CI's `--all-targets` clippy, which the
/// local pre-commit hook does not pass, has nothing to say about it.
fn body(rows: &[serde_json::Value]) -> String {
    serde_json::json!({ "head": { "vars": [] }, "results": { "bindings": rows } }).to_string()
}

#[test]
fn a_catalog_with_no_sarc_fields_still_projects() {
    // THE REGRESSION THIS SEAM HAS ALREADY SUFFERED ONCE. If the projection
    // required ?constraintClass, every policy in a quipu whose catalog predates
    // Q-SARC-CLASS would vanish — both sides shipped, zero rows, and a guard
    // that reports clean because it is evaluating nothing. Absent fields must
    // decode to None and behave exactly as they did before the field existed.
    let policies = decode_policies(&catalog_json()).unwrap();
    assert_eq!(policies.len(), 2, "a pre-SARC catalog still projects");
    assert!(policies.iter().all(|p| p.rule.class.is_none()));
    assert!(policies.iter().all(|p| p.rule.verification_point.is_none()));

    // ... and the governed effect still decides, under Enforce.
    let violations = evaluate_projected(
        &policies,
        "// see ABC-123\n",
        "rust",
        "src/a.rs",
        crate::policy::Mode::Enforce,
    );
    assert!(
        violations[0].blocking,
        "with no class, a deny-effect policy blocks as it always did"
    );
}

#[test]
fn sarc_fields_decode_when_present() {
    let json = body(&[policy_row(
        "no-ticket-in-comment",
        "deny",
        &[
            ("constraintClass", "hard"),
            ("verificationPoint", "PAG"),
            ("latencyBudgetMs", "5"),
        ],
    )]);
    let p = &decode_policies(&json).unwrap()[0];
    assert_eq!(p.rule.class, Some(ConstraintClass::Hard));
    assert_eq!(p.rule.verification_point, Some(VerificationPoint::Pag));
    assert_eq!(p.latency_budget_ms, Some(5));
}

#[test]
fn an_unknown_class_is_a_projection_error_not_a_default() {
    // Defaulting an unrecognised class to `soft` would silently downgrade a
    // hard constraint; defaulting it to `hard` would block on a typo. Neither
    // is a reading anyone can defend, so it is an error the guard fails open
    // on, loudly.
    let json = body(&[policy_row(
        "p",
        "deny",
        &[("constraintClass", "catastrophic")],
    )]);
    let err = decode_policies(&json).unwrap_err();
    assert!(
        matches!(&err, Error::Projection(m) if m.contains("catastrophic")),
        "got {err:?}"
    );
}

#[test]
fn an_unknown_verification_point_is_a_projection_error() {
    let json = body(&[policy_row("p", "deny", &[("verificationPoint", "🤷")])]);
    assert!(matches!(
        decode_policies(&json).unwrap_err(),
        Error::Projection(_)
    ));
}

#[test]
fn a_soft_class_never_blocks_even_with_a_deny_effect() {
    // The class outranks the effect. That combination is contradictory and
    // quipu's placement check now refuses to DEFINE it — but a store predating
    // the check can hold one, and honouring what the author declared it to BE
    // is the only reading that is not a guess.
    let json = body(&[policy_row(
        "p",
        "deny",
        &[("constraintClass", "soft"), ("verificationPoint", "PAA")],
    )]);
    let policies = decode_policies(&json).unwrap();
    // PAA, so it does not even run at the gate.
    assert!(!runs_at_pre_edit(&policies[0]));
    assert!(
        !policy_blocks(&policies[0], crate::policy::Mode::Enforce),
        "a soft constraint must not block, whatever its effect says"
    );
}

#[test]
fn a_paa_policy_does_not_fire_at_the_pre_edit_gate() {
    // Evaluating a post-action rule at the gate would tell the model to fix
    // something its author scoped to after the fact.
    let json = body(&[policy_row(
        "p",
        "warn",
        &[("constraintClass", "soft"), ("verificationPoint", "PAA")],
    )]);
    let policies = decode_policies(&json).unwrap();
    let violations = evaluate_projected(
        &policies,
        "// see ABC-123\n",
        "rust",
        "src/a.rs",
        crate::policy::Mode::Enforce,
    );
    assert!(
        violations.is_empty(),
        "a PAA-declared policy is skipped at pre-edit, not evaluated and ignored"
    );
}

#[test]
fn a_hard_pag_policy_blocks_under_enforce_and_not_under_advise() {
    // Both outcomes for the same rule — the RED and GREEN pair.
    let json = body(&[policy_row(
        "p",
        "deny",
        &[("constraintClass", "hard"), ("verificationPoint", "PAG")],
    )]);
    let policies = decode_policies(&json).unwrap();
    assert!(policy_blocks(&policies[0], crate::policy::Mode::Enforce));
    assert!(!policy_blocks(&policies[0], crate::policy::Mode::Advise));
    assert!(!policy_blocks(&policies[0], crate::policy::Mode::Off));
}

#[test]
fn throttle_is_advisory_at_the_gate() {
    // The soft-class PAA response. Until the post-edit auditor applies it
    // (Phase 3), a throttle policy warns — and must not be read as a block
    // simply because it is not in the advisory list.
    assert!(!effect_blocks("throttle"));
}

#[test]
fn one_policy_is_one_rule_across_a_cross_product() {
    // SPARQL returns the cross product of the OPTIONALs, so a policy with two
    // labels comes back as two rows. The text decoder learned this the
    // expensive way (7 entities -> 11 rules, 4 duplicates); the SARC fields add
    // three more multipliers to this one.
    let a = policy_row("p", "deny", &[("constraintClass", "hard")]);
    let mut b = a.clone();
    b["name"] = serde_json::json!({ "type": "literal", "value": "p (also known as)" });
    let policies = decode_policies(&body(&[a, b])).unwrap();
    assert_eq!(
        policies.len(),
        1,
        "two rows for one policy IRI collapse to one rule"
    );
}

#[test]
fn conflicting_rows_for_one_policy_are_refused_not_merged() {
    // Two different policies wearing one identity. Picking either silently is
    // how a guard enforces something nobody wrote.
    let a = policy_row("p", "deny", &[("constraintClass", "hard")]);
    let b = policy_row("p", "deny", &[("constraintClass", "soft")]);
    let err = decode_policies(&body(&[a, b])).unwrap_err();
    assert!(
        matches!(&err, Error::Projection(m) if m.contains("conflicting")),
        "got {err:?}"
    );
}

// ── Hosting layer (H-SARC-I6) ────────────────────────────────────────────────

#[test]
fn a_declared_hosting_layer_projects() {
    let policies = decode_policies(&body(&[policy_row(
        "secrets-guard",
        "deny",
        &[("hostedAtLayer", "tool")],
    )]))
    .unwrap();
    assert_eq!(
        policies[0].hosted_at_layer,
        Some(crate::hosting::HostingLayer::Tool)
    );
}

#[test]
fn an_absent_hosting_layer_is_none_not_a_default() {
    // Defaulting an absent claim to `orchestration` would put a claim in the
    // record that nobody made, and the I6 check would then be comparing yupana's
    // own guess against yupana's own placement — always agreeing, always useless.
    let policies = decode_policies(&body(&[policy_row("p", "warn", &[])])).unwrap();
    assert!(policies[0].hosted_at_layer.is_none());
}

#[test]
fn an_unknown_hosting_layer_is_a_projection_error() {
    // The only value quipu's vocabulary deliberately omits is "prompt", and
    // silently dropping it to None would turn the one claim I6 forbids into no
    // claim at all — the exact laundering the field exists to prevent.
    let err = decode_policies(&body(&[policy_row(
        "p",
        "warn",
        &[("hostedAtLayer", "prompt")],
    )]));
    let Err(crate::errors::Error::Projection(why)) = err else {
        panic!("expected a projection error, got {err:?}");
    };
    assert!(why.contains("prompt"), "{why}");
}

#[test]
fn an_overclaiming_policy_still_projects_and_still_blocks() {
    // The loud FAIL-OPEN. Refusing to project would disable a rule that does
    // still work, trading a documentation defect for an enforcement gap.
    let policies = decode_policies(&body(&[policy_row(
        "secrets-guard",
        "deny",
        &[("hostedAtLayer", "policy"), ("constraintClass", "hard")],
    )]))
    .unwrap();
    assert_eq!(policies.len(), 1, "the overclaim did not drop the rule");

    let violations = evaluate_projected(
        &policies,
        "// see ABC-123\n",
        "rust",
        "src/a.rs",
        crate::policy::Mode::Enforce,
    );
    assert!(
        violations[0].blocking,
        "a hard constraint with a bad layer claim still enforces"
    );
    assert!(
        crate::hosting::overclaims(
            "secrets-guard",
            policies[0].hosted_at_layer,
            crate::hosting::YUPANA_HOSTS_AT
        )
        .is_some(),
        "and the claim is still reported"
    );
}

// ---------------------------------------------------------------------------
// refresh_or_cached — the aegis-0upyu seam.
//
// Both outcomes, because the bug being fixed was a SILENT degradation: the guard
// stopped enforcing and nothing said so. A test that only proves the cache is
// served would leave the other half — the loud refusal past TTL — unmeasured,
// and that half is what stops a retired rule firing forever from disk.
//
// `127.0.0.1:1` is the unreachable endpoint throughout: a port nothing binds, so
// the connection is refused immediately and deterministically. Deliberately
// explicit rather than inherited from the environment — a fixture that picks up
// the host's live quipu is the aegis-enbzz flake.
// ---------------------------------------------------------------------------

const UNREACHABLE: &str = "http://127.0.0.1:1";

/// A stub quipu serving `serve` requests, answering the text-rule query and the
/// structural-policy query from the same canned catalogue. Returns its base URL.
fn stub_quipu(serve: usize) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for _ in 0..serve {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // Read headers, then exactly Content-Length body bytes: ureq may
            // split them across writes, and replying early would race.
            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let len: usize = text
                        .to_ascii_lowercase()
                        .split("content-length:")
                        .nth(1)
                        .and_then(|t| t.split("\r\n").next())
                        .and_then(|t| t.trim().parse().ok())
                        .unwrap_or(0);
                    if raw.len() >= head_end + 4 + len {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&raw).to_string();
            // The two queries are told apart by the class they target — the
            // same distinction quipu itself makes.
            // Text-rule and grounded-predicate queries get empty catalogues;
            // the structural-policy query gets the canned one.
            let body = if request.contains("aegis:TextRule") || request.contains("candidateSource")
            {
                empty_results_json()
            } else {
                catalog_json()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/sparql-results+json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}

/// A well-formed SPARQL result with no bindings — a real, empty catalogue.
fn empty_results_json() -> String {
    serde_json::json!({
        "head": { "vars": ["s", "regex", "tier"] },
        "results": { "bindings": [] }
    })
    .to_string()
}

/// A cache file holding `policies`, stamped `written_at` and bound to `endpoint`.
fn seed_cache(path: &std::path::Path, endpoint: &str, written_at: u64) {
    crate::projection_cache::save(
        path,
        &crate::projection_cache::CachedProjection {
            version: crate::projection_cache::CACHE_VERSION,
            written_at,
            endpoint: endpoint.to_string(),
            policies: decode_policies(&catalog_json()).unwrap(),
            text_rules: Vec::new(),
            grounded_rules: Vec::new(),
            grounding: None,
        },
    );
}

/// THE FIX. quipu cannot be projected, but the last-known catalogue is on disk
/// and recent — so the guard ENFORCES rather than failing open, and says the
/// verdict is stale. Before this, the same conditions produced zero policies and
/// an allow.
#[test]
fn a_failed_projection_serves_the_cache_and_declares_it_stale() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("projection.json");
    seed_cache(&cache, UNREACHABLE, 1_000);

    let mut registry = ProjectionRegistry::new(UNREACHABLE);
    let source = registry
        .refresh_or_cached(Some(&cache), 3600, 1_030)
        .expect("a recent cache is servable when quipu is not");

    match source {
        ProjectionSource::Cache { age_secs, error } => {
            assert_eq!(age_secs, 30, "the age is carried, not merely implied");
            assert!(
                !error.is_empty(),
                "the live failure is carried too — the record must name why"
            );
        }
        ProjectionSource::Live => panic!("nothing was live; 127.0.0.1:1 is refused"),
    }
    assert_eq!(
        registry.policies().len(),
        2,
        "the policies are REALLY loaded — this is the difference between \
         enforcing and not"
    );
    assert_eq!(
        registry.freshness(),
        Freshness::Stale,
        "served from cache is STALE, never Fresh"
    );
}

/// The other half, and the one that keeps this cache honest: past the TTL the
/// guard refuses to enforce a catalogue nobody has confirmed since, and the
/// error names BOTH the live failure and the cache's age. A retired rule that
/// keeps firing from disk is worse than no rule — it is unfalsifiable from the
/// outside.
#[test]
fn a_cache_past_the_ttl_fails_open_and_the_reason_names_both_halves() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("projection.json");
    seed_cache(&cache, UNREACHABLE, 1_000);

    let mut registry = ProjectionRegistry::new(UNREACHABLE);
    let err = registry
        .refresh_or_cached(Some(&cache), 60, 5_000)
        .expect_err("a cache 4000s old must not be served under a 60s TTL");

    assert!(
        err.contains("127.0.0.1:1"),
        "the live failure is named: {err}"
    );
    assert!(
        err.contains("3999") || err.contains("4000"),
        "the cache's age is named: {err}"
    );
    assert!(err.contains("60"), "and the TTL it exceeded: {err}");
    assert!(
        registry.policies().is_empty(),
        "an expired cache is not quietly enforced anyway"
    );
}

/// No cache at all — the ordinary state before the first successful refresh, and
/// the one case that must still fail open. It reports as a MISS, not as a
/// success with an empty catalogue: a fresh-looking empty policy set is the
/// silent-no-enforcement failure one layer down.
#[test]
fn no_cache_and_no_quipu_fails_open_rather_than_serving_an_empty_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("never-written.json");

    let mut registry = ProjectionRegistry::new(UNREACHABLE);
    let err = registry
        .refresh_or_cached(Some(&cache), 3600, 1_000)
        .expect_err("nothing to serve");
    assert!(
        err.contains("no cached projection has been written yet"),
        "the miss is named, so the record can say which one: {err}"
    );
}

/// A successful projection WRITES the cache — the step that makes every
/// fallback above possible. Without it the registry is what it was before this
/// change: a cache that caches nothing, because the hook is a short-lived
/// process per edit.
#[test]
fn a_successful_refresh_persists_the_projection_for_the_next_process() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("projection.json");
    // Three requests per refresh: the structural catalogue, the text rules
    // and the grounded-predicate catalogue. (An empty grounded catalogue asks
    // for no grounding set — a fourth request would hang the refresh here.)
    let endpoint = stub_quipu(3);

    let mut registry = ProjectionRegistry::new(&endpoint);
    assert_eq!(
        registry
            .refresh_or_cached(Some(&cache), 3600, 9_000)
            .unwrap(),
        ProjectionSource::Live
    );
    assert_eq!(registry.freshness(), Freshness::Fresh);

    // The NEXT process — a separate registry, as the hook really is — finds it.
    let served = crate::projection_cache::load_servable(&cache, &endpoint, 3600, 9_010).unwrap();
    assert_eq!(served.written_at, 9_000);
    assert_eq!(
        served.policies.len(),
        2,
        "both projected policies survived the round trip to disk"
    );
    assert_eq!(served.policies[0].rule.name, "todo-needs-ticket");
}

/// The cache is bound to the quipu it came from. Serving another deployment's
/// catalogue would enforce policy this endpoint never declared, while the
/// verdict claimed to be enforcing this one's.
#[test]
fn a_cache_written_against_another_quipu_is_not_served_here() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("projection.json");
    seed_cache(&cache, "http://someone-elses-quipu.invalid", 1_000);

    let mut registry = ProjectionRegistry::new(UNREACHABLE);
    let err = registry
        .refresh_or_cached(Some(&cache), 3600, 1_010)
        .expect_err("another deployment's policy is not ours to enforce");
    assert!(err.contains("different quipu"), "{err}");
    assert!(registry.policies().is_empty());
}
