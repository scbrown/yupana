//! Tests for the landing decision procedure. Size-exempt (`_test.rs`).

use super::*;
use crate::project_landing::{LandingRule, RepoLanding};

fn repo(rule: LandingRule, owner: Option<&str>) -> LandingAuthority {
    LandingAuthority::Governed(Box::new(RepoLanding {
        repo_iri: "aegis:repo_quipu".into(),
        matched_name: "repo_quipu".into(),
        owner: owner.map(str::to_string),
        rule,
        protected_refs: vec!["main".into()],
        protected_refs_declared: true,
        ownership_state: Some("RULED".into()),
        aliases: vec!["quipu".into()],
        override_authorities: Vec::new(),
    }))
}

/// The same repository, additionally authorising `authorities` to override.
fn repo_with_override(
    rule: LandingRule,
    owner: Option<&str>,
    authorities: &[&str],
) -> LandingAuthority {
    let LandingAuthority::Governed(mut r) = repo(rule, owner) else {
        unreachable!()
    };
    r.override_authorities = authorities.iter().map(|a| (*a).to_string()).collect();
    LandingAuthority::Governed(r)
}

fn req(agent: Option<&str>, bead: Option<&str>, git_ref: &str) -> LandingRequest {
    LandingRequest {
        verb: "merge",
        repo: "quipu".into(),
        git_ref: git_ref.into(),
        ref_assumed: false,
        agent: agent.map(str::to_string),
        bead: bead.map(str::to_string),
        work_item_readable: true,
        override_grant: None,
    }
}

/// The same request, carrying a host override grant.
fn req_granted(agent: Option<&str>, bead: Option<&str>, git_ref: &str) -> LandingRequest {
    LandingRequest {
        override_grant: Some("malcolm DOWN (st crew), tier lead merges per em5oaz".into()),
        ..req(agent, bead, git_ref)
    }
}

#[test]
fn the_owner_with_a_work_item_is_ALLOWED() {
    let d = decide(
        &repo(LandingRule::SingleWriter, Some("malcolm")),
        &req(Some("malcolm"), Some("aegis-1"), "main"),
    );
    assert!(matches!(d, Decision::Allow { .. }), "{d:?}");
}

#[test]
fn a_NON_owner_is_refused_under_single_writer() {
    let d = decide(
        &repo(LandingRule::SingleWriter, Some("malcolm")),
        &req(Some("grant"), Some("aegis-1"), "main"),
    );
    let Decision::Refuse { codes, .. } = &d else {
        panic!("expected refusal, got {d:?}")
    };
    assert_eq!(codes, &["agent_is_not_repo_owner"]);
}

// ── The tier-lead override (aegis-d7jpdw) ──────────────────────────────
//
// Modelled from two MEASURED landings on 2026-09-14 that the host guard
// ALLOWED on a consumed override token and this policy refused — the 2-of-8
// systematic false positive that disproved the zero-FP gate. Each test below
// names the arm it pins, because the value of an override is decided by what
// it refuses to relax.

#[test]
fn an_AUTHORISED_override_lets_a_NON_owner_land() {
    // Replays 13:16:36Z (PR #230) and 14:06:34Z (PR #241): agent wu, owner
    // malcolm, `agent_is_not_repo_owner` the sole fault, work item cited.
    let d = decide(
        &repo_with_override(LandingRule::SingleWriter, Some("malcolm"), &["wu"]),
        &req_granted(Some("wu"), Some("aegis-otg3xz"), "main"),
    );
    let Decision::Allow { reason } = &d else {
        panic!("expected allow, got {d:?}")
    };
    // Never silently: the record a soak reads must show an exception was
    // taken, not merely that the landing passed.
    assert!(reason.contains("override"), "{reason}");
    assert!(
        reason.contains("em5oaz"),
        "the grant's reason rides along: {reason}"
    );
}

#[test]
fn an_override_from_an_agent_the_graph_does_NOT_name_still_refuses() {
    // THE CONTROL. A grant is evidence that the host let the command
    // through; it is not authority to land. Authority is graph data.
    let d = decide(
        &repo_with_override(LandingRule::SingleWriter, Some("malcolm"), &["wu"]),
        &req_granted(Some("grant"), Some("aegis-1"), "main"),
    );
    let Decision::Refuse { codes, .. } = &d else {
        panic!("expected refusal, got {d:?}")
    };
    assert_eq!(
        codes,
        &["agent_is_not_repo_owner", "override_not_authorised"]
    );
}

#[test]
fn a_NON_owner_with_NO_grant_still_refuses_even_when_authorised() {
    // Replays 14:06:21Z — wu's FIRST attempt at PR #241, thirteen seconds
    // before the one above. The host REFUSED that one too (no token was
    // armed yet), so it is a TRUE positive and must stay refused. This is
    // the whole reason the policy evaluates the host's grant rather than
    // the agent's authority alone.
    let d = decide(
        &repo_with_override(LandingRule::SingleWriter, Some("malcolm"), &["wu"]),
        &req(Some("wu"), Some("aegis-otg3xz"), "main"),
    );
    let Decision::Refuse { codes, .. } = &d else {
        panic!("expected refusal, got {d:?}")
    };
    assert_eq!(codes, &["agent_is_not_repo_owner"]);
}

#[test]
fn an_override_does_NOT_relax_a_missing_work_item() {
    // Traceability should survive an override, not be what it buys. The
    // host's own override lines carry `bead=-`, so this DIVERGES from host
    // parity deliberately: parity is on the owner arm only.
    let d = decide(
        &repo_with_override(LandingRule::SingleWriter, Some("malcolm"), &["wu"]),
        &req_granted(Some("wu"), None, "main"),
    );
    let Decision::Refuse { codes, .. } = &d else {
        panic!("expected refusal, got {d:?}")
    };
    assert_eq!(codes, &["work_item_missing"]);
}

#[test]
fn an_override_does_NOT_rescue_a_rule_with_NO_recorded_owner() {
    // An override relaxes "you are not the owner". Where the graph records
    // no owner there is no such fault to relax, and granting one would
    // convert a missing fact into a permanent bypass.
    let d = decide(
        &repo_with_override(LandingRule::SingleWriter, None, &["wu"]),
        &req_granted(Some("wu"), Some("aegis-1"), "main"),
    );
    let Decision::Refuse { codes, reason } = &d else {
        panic!("expected refusal, got {d:?}")
    };
    assert_eq!(
        codes,
        &["agent_is_not_repo_owner", "override_not_authorised"]
    );
    assert!(reason.contains("Fix the ownership fact"), "{reason}");
}

#[test]
fn an_override_does_NOT_attribute_an_unreported_agent() {
    // Attribution is the precondition for every other check: an
    // unattributable landing cannot be authorised by anyone, because there
    // is nobody for the authority list to match.
    let d = decide(
        &repo_with_override(LandingRule::SingleWriter, Some("malcolm"), &["wu"]),
        &req_granted(None, Some("aegis-1"), "main"),
    );
    let Decision::Refuse { codes, .. } = &d else {
        panic!("expected refusal, got {d:?}")
    };
    assert_eq!(codes, &["acting_agent_unknown"]);
}

#[test]
fn the_OWNERs_own_landing_is_never_reported_as_an_override() {
    // A grant present on a landing that needed no relaxation relaxed
    // nothing. Reporting it as an override would put a fictitious exception
    // into the signed corpus.
    let d = decide(
        &repo_with_override(LandingRule::SingleWriter, Some("malcolm"), &["wu"]),
        &req_granted(Some("malcolm"), Some("aegis-1"), "main"),
    );
    let Decision::Allow { reason } = &d else {
        panic!("expected allow, got {d:?}")
    };
    assert!(!reason.contains("override"), "{reason}");
}

#[test]
fn a_repo_naming_NO_override_authority_behaves_exactly_as_before() {
    // The default. Every governed repository that predates this field must
    // keep refusing non-owners, grant or no grant.
    let d = decide(
        &repo(LandingRule::SingleWriter, Some("malcolm")),
        &req_granted(Some("wu"), Some("aegis-1"), "main"),
    );
    assert!(d.refuses(), "{d:?}");
}

#[test]
fn the_owner_WITHOUT_a_work_item_is_still_refused() {
    // The rule the host guard enforces too: single-writer is not a licence
    // for the owner to land untraceably.
    let d = decide(
        &repo(LandingRule::SingleWriter, Some("malcolm")),
        &req(Some("malcolm"), None, "main"),
    );
    let Decision::Refuse { codes, .. } = &d else {
        panic!("expected refusal, got {d:?}")
    };
    assert_eq!(codes, &["work_item_missing"]);
}

#[test]
fn a_non_owner_with_no_work_item_reports_BOTH_faults() {
    let d = decide(
        &repo(LandingRule::SingleWriter, Some("malcolm")),
        &req(Some("grant"), None, "main"),
    );
    let Decision::Refuse { codes, .. } = &d else {
        panic!("expected refusal")
    };
    assert_eq!(codes, &["agent_is_not_repo_owner", "work_item_missing"]);
}

#[test]
fn any_owner_with_bead_lets_a_NON_owner_land_when_cited() {
    let d = decide(
        &repo(LandingRule::AnyOwnerWithBead, Some("malcolm")),
        &req(Some("grant"), Some("aegis-1"), "main"),
    );
    assert!(matches!(d, Decision::Allow { .. }), "{d:?}");
}

#[test]
fn a_declared_rule_with_NO_owner_refuses_EVERYONE() {
    for agent in ["malcolm", "grant", "sattler"] {
        let d = decide(
            &repo(LandingRule::SingleWriter, None),
            &req(Some(agent), Some("aegis-1"), "main"),
        );
        assert!(d.refuses(), "{agent} must not satisfy an ownerless rule");
    }
}

#[test]
fn an_unreported_agent_is_refused_and_says_so() {
    let d = decide(
        &repo(LandingRule::SingleWriter, Some("malcolm")),
        &req(None, Some("aegis-1"), "main"),
    );
    let Decision::Refuse { codes, .. } = &d else {
        panic!("expected refusal")
    };
    assert_eq!(codes, &["acting_agent_unknown"]);
}

#[test]
fn a_TOPIC_branch_is_not_applicable_and_asks_nothing_about_identity() {
    // The applicability test runs BEFORE identity, so ordinary work never
    // reaches the spool and never depends on the agent being reported.
    let d = decide(
        &repo(LandingRule::SingleWriter, Some("malcolm")),
        &req(None, None, "wt/grant"),
    );
    assert!(matches!(d, Decision::NotApplicable { .. }), "{d:?}");
}

#[test]
fn an_UNGOVERNED_repo_is_allowed_even_on_main() {
    let d = decide(
        &LandingAuthority::Ungoverned {
            name: "bobbin".into(),
        },
        &req(Some("grant"), None, "main"),
    );
    assert!(matches!(d, Decision::NotApplicable { .. }), "{d:?}");
}

#[test]
fn an_UNKNOWN_authority_refuses_the_protected_ref() {
    let d = decide(
        &LandingAuthority::Unknown("projection timed out".into()),
        &req(Some("grant"), Some("aegis-1"), "main"),
    );
    assert!(d.refuses(), "{d:?}");
    let Decision::Refuse { reason, codes } = &d else {
        unreachable!()
    };
    assert_eq!(codes, &["landing_policy_unresolved"]);
    // The reason has to carry the CAUSE: "refused" and "refused because the
    // projection timed out" are different findings.
    assert!(reason.contains("projection timed out"), "{reason}");
}

#[test]
fn an_UNKNOWN_authority_leaves_every_other_ref_ALONE() {
    // The bounded blast radius, asserted rather than asserted-about: when
    // the graph is unreachable, only the default protected ref refuses.
    for git_ref in ["wt/grant", "release/1.2", "refs/tags/v1", "feature"] {
        let d = decide(
            &LandingAuthority::Unknown("quipu unreachable".into()),
            &req(Some("grant"), None, git_ref),
        );
        assert!(
            matches!(d, Decision::NotApplicable { .. }),
            "{git_ref} must be unaffected, got {d:?}"
        );
    }
}

#[test]
fn a_fully_qualified_ref_is_the_same_ref_as_its_short_name() {
    let d = decide(
        &repo(LandingRule::SingleWriter, Some("malcolm")),
        &req(Some("grant"), Some("aegis-1"), "refs/heads/main"),
    );
    assert!(d.refuses(), "refs/heads/main IS main: {d:?}");
}

#[test]
fn an_assumed_ref_says_so_in_the_refusal() {
    let mut r = req(Some("grant"), Some("aegis-1"), "main");
    r.ref_assumed = true;
    let Decision::Refuse { reason, .. } =
        decide(&repo(LandingRule::SingleWriter, Some("malcolm")), &r)
    else {
        panic!("expected refusal")
    };
    assert!(reason.contains("resolved, not"), "{reason}");
}
#[test]
fn unknown_plate_fails_open_without_bypassing_ownership() {
    let authority = repo(LandingRule::SingleWriter, Some("malcolm"));
    let mut request = req(Some("malcolm"), None, "main");
    request.work_item_readable = false;
    assert!(matches!(
        decide(&authority, &request),
        Decision::Allow { .. }
    ));
    request.agent = Some("other".into());
    let Decision::Refuse { codes, .. } = decide(&authority, &request) else {
        panic!("ownership must still refuse")
    };
    assert_eq!(codes, ["agent_is_not_repo_owner"]);
}
