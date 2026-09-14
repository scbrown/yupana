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
    /// Whether a fresh session plate was positively read; unknown fails open.
    pub work_item_readable: bool,
    /// The host guard's override GRANT for this landing, when it issued one —
    /// carrying the reason it recorded.
    ///
    /// Evidence, not a credential. The override token is single-use and is
    /// consumed by the host guard before this policy runs, so re-reading it
    /// here is not merely redundant, it is impossible: by the time the governed
    /// policy is asked, the token has already been unlinked (aegis-d7jpdw,
    /// measured through the real guard chain). Taking the grant as evidence
    /// inherits every check the host makes — regular file, owning uid, TTL,
    /// non-empty reason, removability — without replicating one of them, and a
    /// replica of a security check drifts.
    pub override_grant: Option<String>,
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

            // An override relaxes ONE fault and only when the graph names this
            // agent as an authority for it. `may_override` additionally
            // requires a recorded owner, so the ownerless case below keeps its
            // refusal — an override there would turn a missing fact into a
            // permanent bypass.
            let grant = req.override_grant.as_deref().filter(|g| !g.is_empty());
            let authorised = grant.filter(|_| repo.may_override(agent));
            let overrode_owner_rule =
                repo.rule.owner_only() && !repo.is_owner(agent) && authorised.is_some();

            if repo.rule.owner_only() && !repo.is_owner(agent) && !overrode_owner_rule {
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
                // A grant was offered and did NOT apply. Said separately from
                // the fault it failed to relax, because "refused, no override
                // involved" and "refused despite an override" are different
                // events and only the second is worth an operator's attention.
                if grant.is_some() {
                    codes.push("override_not_authorised".into());
                    faults.push(format!(
                        "the host guard granted an override, but the graph does not authorise \
                         `{agent}` to override the owner-only rule on `{}`{}",
                        req.repo,
                        if repo.owner.is_none() {
                            " — and no override can stand in for an owner the graph never recorded"
                        } else {
                            ""
                        }
                    ));
                }
            }

            if req.work_item_readable && req.bead.as_deref().filter(|b| !b.is_empty()).is_none() {
                codes.push("work_item_missing".into());
                faults.push(
                    "the landing cites no work item, and every governed landing must be \
                     traceable to one"
                        .to_string(),
                );
            }

            if faults.is_empty() {
                return Decision::Allow {
                    reason: match authorised.filter(|_| overrode_owner_rule) {
                        // Never silently. An overridden landing reads differently
                        // from a satisfied one in the record a soak adjudicates,
                        // and collapsing the two would hide the exception the
                        // whole mechanism exists to make visible.
                        Some(why) => format!(
                            "`{agent}` is not the owner of `{}`, but the graph authorises it to \
                             override the `{}` rule and the host guard granted one: {why} ({})",
                            req.repo,
                            repo.rule.as_str(),
                            req.bead.as_deref().unwrap_or("no work item")
                        ),
                        None => format!(
                            "`{agent}` satisfies the `{}` rule on `{}` ({})",
                            repo.rule.as_str(),
                            req.repo,
                            req.bead.as_deref().unwrap_or("no work item")
                        ),
                    },
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
#[path = "landing_decision_test.rs"]
mod tests;
