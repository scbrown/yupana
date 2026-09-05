//! The landing policy's decision procedure — pure, so every arm is testable
//! without a graph, a repository or a harness.
//!
//! The I/O half (resolving a remote name, reading the current branch, signing
//! the verdict) is [`super::landing_guard`]. Keeping them apart is not tidiness:
//! this is the function a refusal cites, and a decision procedure that can only
//! be exercised through a live projection is one nobody can check.
//!
//! See `docs/design/landing-policy.md` for the rule this implements.

use crate::project_landing::{LandingAuthority, DEFAULT_PROTECTED_REF};

/// What the guard was asked to decide about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingRequest {
    /// `push` or `merge`.
    pub verb: &'static str,
    /// The resolved repository name.
    pub repo: String,
    /// The resolved ref the landing writes to.
    pub git_ref: String,
    /// Whether that ref was stated by the command or inferred by the resolver.
    /// Carried into the record so a false positive in the advise soak can be
    /// told apart from a true one without re-deriving anything.
    pub ref_assumed: bool,
    /// The acting agent, self-reported by the session environment. `None` when
    /// the session does not report one — which is a REFUSAL on a governed
    /// protected ref, because an unattributable landing is the thing the rule
    /// exists to prevent.
    pub agent: Option<String>,
    /// The work item cited for this landing.
    pub bead: Option<String>,
}

/// The verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The policy applies and is satisfied.
    Allow {
        /// Why, in the words the record carries.
        reason: String,
    },
    /// The policy applies and is not satisfied.
    Refuse {
        /// The model-facing refusal text.
        reason: String,
        /// Stable reason codes for the signed record, so a refusal is
        /// aggregatable without parsing prose.
        codes: Vec<String>,
    },
    /// The policy does not apply. NOT a lesser allow — no verdict is recorded,
    /// because recording one would put an unbounded stream of ordinary topic
    /// branch pushes into the attestation spool.
    NotApplicable {
        /// Why the policy did not apply.
        reason: String,
    },
}

impl Decision {
    /// Whether this decision refuses the action.
    #[must_use]
    pub fn refuses(&self) -> bool {
        matches!(self, Decision::Refuse { .. })
    }

    /// The wire form for the certification record's `result` field.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Decision::Allow { .. } => "allow",
            Decision::Refuse { .. } => "refuse",
            Decision::NotApplicable { .. } => "not-applicable",
        }
    }
}

/// Decide one landing.
///
/// The ordering of the arms is the design. Applicability is settled BEFORE
/// identity, so an agent pushing a topic branch is never asked who it is and
/// never appears in the spool; and the `Unknown` arm is bounded by the same
/// applicability test, so an unreachable graph refuses landings on a protected
/// ref rather than every command on the host.
#[must_use]
pub fn decide(authority: &LandingAuthority, req: &LandingRequest) -> Decision {
    match authority {
        LandingAuthority::Ungoverned { name } => Decision::NotApplicable {
            reason: format!("repository `{name}` declares no landing policy"),
        },

        // The graph could not be asked. We do not know which refs this
        // repository protects, so the default stands in — and if the landing is
        // not aimed at it, there is nothing to refuse.
        LandingAuthority::Unknown(why) => {
            if !ref_matches(&req.git_ref, DEFAULT_PROTECTED_REF) {
                return Decision::NotApplicable {
                    reason: format!(
                        "landing policy is UNRESOLVED ({why}), but `{}` is not the default \
                         protected ref — nothing to enforce",
                        req.git_ref
                    ),
                };
            }
            Decision::Refuse {
                reason: format!(
                    "REFUSED: {} onto `{}` of `{}`.\n\
                     The landing policy could not be resolved: {why}.\n\
                     This guard refuses rather than guessing, because an unreachable graph \
                     must not become the way around the rule. Topic branches and every \
                     non-protected ref are unaffected.",
                    req.verb, req.git_ref, req.repo
                ),
                codes: vec!["landing_policy_unresolved".into()],
            }
        }

        LandingAuthority::Governed(repo) => {
            if !repo.protects(&req.git_ref) {
                return Decision::NotApplicable {
                    reason: format!("`{}` is not a protected ref of `{}`", req.git_ref, req.repo),
                };
            }
            let mut codes = Vec::new();
            let mut faults = Vec::new();

            // Attribution first: without an acting agent no other check means
            // anything, since both remaining checks are about who is acting.
            let Some(agent) = req.agent.as_deref().filter(|a| !a.is_empty()) else {
                return Decision::Refuse {
                    reason: format!(
                        "REFUSED: {} onto protected ref `{}` of `{}`.\n\
                         The acting agent is not reported by this session, so the landing \
                         cannot be attributed to anyone.",
                        req.verb, req.git_ref, req.repo
                    ),
                    codes: vec!["acting_agent_unknown".into()],
                };
            };

            if repo.rule.owner_only() && !repo.is_owner(agent) {
                codes.push("agent_is_not_repo_owner".into());
                faults.push(match repo.owner.as_deref() {
                    Some(owner) => format!(
                        "`{}` declares the `single-writer` rule and the graph names `{owner}` \
                         as its owner, not `{agent}`",
                        req.repo
                    ),
                    // A rule naming an owner nobody recorded cannot be satisfied
                    // by anyone — including the agent who believes it is theirs.
                    None => format!(
                        "`{}` declares the `single-writer` rule but the graph records NO owner, \
                         so no agent can satisfy it. Fix the ownership fact, do not override",
                        req.repo
                    ),
                });
            }

            if req.bead.as_deref().filter(|b| !b.is_empty()).is_none() {
                codes.push("work_item_missing".into());
                faults.push(
                    "the landing cites no work item, and every governed landing must be \
                     traceable to one"
                        .to_string(),
                );
            }

            if faults.is_empty() {
                return Decision::Allow {
                    reason: format!(
                        "`{agent}` satisfies the `{}` rule on `{}` ({})",
                        repo.rule.as_str(),
                        req.repo,
                        req.bead.as_deref().unwrap_or("no work item")
                    ),
                };
            }
            Decision::Refuse {
                reason: format!(
                    "REFUSED: {} onto protected ref `{}` of `{}` by `{agent}`.\n  - {}{}",
                    req.verb,
                    req.git_ref,
                    req.repo,
                    faults.join("\n  - "),
                    if req.ref_assumed {
                        format!(
                            "\nNote: the command did not name a ref; `{}` was resolved, not \
                             stated.",
                            req.git_ref
                        )
                    } else {
                        String::new()
                    }
                ),
                codes,
            }
        }
    }
}

/// Compare refs by short name, so `refs/heads/main` and `main` are one ref.
fn ref_matches(a: &str, b: &str) -> bool {
    let short = |r: &str| r.rsplit('/').next().unwrap_or(r).to_string();
    a == b || short(a) == short(b)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
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
        }))
    }

    fn req(agent: Option<&str>, bead: Option<&str>, git_ref: &str) -> LandingRequest {
        LandingRequest {
            verb: "merge",
            repo: "quipu".into(),
            git_ref: git_ref.into(),
            ref_assumed: false,
            agent: agent.map(str::to_string),
            bead: bead.map(str::to_string),
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
}
