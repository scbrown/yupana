//! The landing arm of the command guard — the I/O half.
//!
//! Policy comes from the graph ([`crate::project_landing`]); the decision is
//! [`super::landing_decision::decide`]; this module only supplies the evidence
//! those two need and turns the verdict into a hook [`Outcome`].
//!
//! The deployment's `[yupana.policy] mode` remains the ceiling, so this policy
//! reports under `advise` and cannot deny until a measured soak promotes the
//! mode. That is not a soft launch — it is the standing gate for a block-tier
//! policy on this circuit, and this is the first one.
//!
//! ## Both failure directions are loud, and they are DIFFERENT failures
//!
//! * yupana's own bugs (unreadable config, unparseable payload) fail OPEN and
//!   say so. A guard that bricks the host is removed, and then nothing is
//!   guarded.
//! * the POLICY fails CLOSED, and only within the boundary
//!   `landing_decision` draws: a protected-ref landing whose authority cannot be
//!   resolved is refused; everything else is untouched.

use crate::hook::pre_edit::Outcome;

#[cfg(feature = "quipu")]
use super::landing_decision::{decide, Decision, LandingRequest};
#[cfg(feature = "quipu")]
use crate::landing::{Landing, RefTarget, RepoRef};
#[cfg(feature = "quipu")]
use crate::policy::Mode;
#[cfg(feature = "quipu")]
use crate::project_landing::LandingAuthority;

#[cfg(not(feature = "quipu"))]
pub(super) fn check(_payload: &str, _command: &str) -> Outcome {
    Outcome::Allow
}

/// A cheap routing superset, never the decision: an unrelated shell command
/// must not pay for a projection it will not consult.
#[cfg(feature = "quipu")]
fn might_be_a_landing(command: &str) -> bool {
    command.contains("push") || command.contains("merge")
}

#[cfg(feature = "quipu")]
pub(super) fn check(payload: &str, command: &str) -> Outcome {
    if !might_be_a_landing(command) {
        return Outcome::Allow;
    }
    let Some(landing) = crate::landing::resolve(command) else {
        return Outcome::Allow;
    };
    let Some(input) = super::HookInput::parse(payload) else {
        return Outcome::Allow;
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let root = input.root(&cwd);
    let config = match crate::config::YupanaConfig::resolve(None, &root) {
        Ok(config) => config,
        Err(e) => {
            return Outcome::Notify(format!(
                "{} landing policy was NOT EVALUATED: unreadable config ({e})",
                super::CONFIG_ERROR_PREFIX
            ))
        }
    };
    if config.policy.mode == Mode::Off || !config.quipu.enabled || config.quipu.endpoint.is_empty()
    {
        return Outcome::Allow;
    }

    // Identify the target BEFORE projecting: a landing we cannot even name is
    // not something a catalogue lookup can help with.
    let Some(repo) = resolve_repo(&landing, &root) else {
        return Outcome::Allow;
    };
    let (git_ref, ref_assumed) = resolve_ref(&landing, &root);

    let mut registry = crate::project::ProjectionRegistry::new(&config.quipu.endpoint);
    let authority = match registry.refresh_or_cached(
        crate::projection_cache::cache_path().as_deref(),
        config.quipu.projection_cache_ttl_secs,
        crate::projection_cache::now_secs(),
    ) {
        // An empty catalogue from a SYNCED registry is a real answer.
        Ok(_) => crate::project_landing::resolve(registry.landing_policies(), &repo),
        // …and from an unsynced one it is not. This is the only place the
        // Unknown arm is produced, and it must stay that way: the emptiness of
        // the slice can never distinguish these two cases on its own.
        Err(e) => LandingAuthority::Unknown(e),
    };

    let request = LandingRequest {
        verb: landing.verb.as_str(),
        repo,
        git_ref,
        ref_assumed,
        agent: acting_agent(),
        bead: crate::plate::current(),
    };
    let decision = decide(&authority, &request);

    // Attest before answering: a verdict the guard acted on but never recorded
    // is exactly the gap moving attestation to the gate was meant to close.
    // Fail-silent, like every other piece of bookkeeping about enforcement.
    if !matches!(decision, Decision::NotApplicable { .. }) {
        record(&config, &request, &decision, &landing);
    }

    match decision {
        Decision::Allow { .. } | Decision::NotApplicable { .. } => Outcome::Allow,
        Decision::Refuse { reason, .. } => {
            // The mode is the ceiling. Under advise the refusal is reported in
            // full and the command proceeds — which is what makes the soak
            // adjudicable: the text an operator compares against the host
            // guard's log is the same text a denial would carry.
            if crate::constraint::ConstraintClass::Hard.blocks(config.policy.mode) {
                Outcome::Deny(reason)
            } else {
                Outcome::Notify(format!("yupana (governed, not blocking): {reason}"))
            }
        }
    }
}

/// The acting agent, self-reported by the session environment.
///
/// Deliberately NOT inferred from git or the forge identity: every agent here
/// commits and authenticates as the same account, so those fields identify the
/// host, not the actor. A self-report that can be absent is weaker evidence but
/// honest evidence; an inferred one would be confident and wrong.
#[cfg(feature = "quipu")]
fn acting_agent() -> Option<String> {
    ["SHANTY_AGENT", "GT_CREW", "YUPANA_AGENT"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Resolve the command's repository reference to a bare repository name.
#[cfg(feature = "quipu")]
fn resolve_repo(landing: &Landing, root: &std::path::Path) -> Option<String> {
    match &landing.repo {
        RepoRef::Url(url) => crate::git::repo_name_from_url(url.trim()),
        RepoRef::Slug(slug) => slug.rsplit('/').next().map(str::to_string),
        RepoRef::Remote(name) => crate::git::run(root, &["remote", "get-url", name])
            .and_then(|url| crate::git::repo_name_from_url(url.trim())),
        RepoRef::Cwd => crate::git::origin_repo_name(root),
    }
}

/// Resolve the ref the landing writes to, and whether the command stated it.
///
/// The `bool` is not decoration. A refusal on a ref the command never named is
/// the shape a false positive takes here, and the soak has to be able to count
/// those separately without re-deriving anything.
#[cfg(feature = "quipu")]
fn resolve_ref(landing: &Landing, root: &std::path::Path) -> (String, bool) {
    match &landing.git_ref {
        RefTarget::Named(r) => (r.clone(), false),
        RefTarget::Unstated => match landing.verb {
            // A push with no refspec writes the current branch.
            crate::landing::LandingVerb::Push => (
                crate::git::run(root, &["rev-parse", "--abbrev-ref", "HEAD"])
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && s != "HEAD")
                    .unwrap_or_else(|| crate::project_landing::DEFAULT_PROTECTED_REF.to_string()),
                true,
            ),
            // A merge lands on the pull request's base branch, which neither the
            // command nor the local work tree knows. The default protected ref
            // stands in — an over-approximation, marked as one, and the reason
            // the advise soak exists before this tier can block.
            crate::landing::LandingVerb::Merge => (
                crate::project_landing::DEFAULT_PROTECTED_REF.to_string(),
                true,
            ),
        },
    }
}

/// Append a signed action-certification record for this decision.
///
/// Fail-silent throughout: bookkeeping about enforcement must never be able to
/// change an enforcement outcome. A missing signing key is the ordinary state on
/// a host that has not run `yupana verifier`, not an error worth surfacing at
/// the moment somebody is trying to push.
#[cfg(feature = "quipu")]
fn record(
    config: &crate::config::YupanaConfig,
    request: &LandingRequest,
    decision: &Decision,
    landing: &Landing,
) {
    let Some(key) =
        crate::verdict_spool::existing_key(std::path::Path::new(&config.quipu.signing_key_path))
    else {
        return;
    };
    let ts = crate::projection_cache::now_secs();
    let session = std::env::var("CLAUDE_SESSION_ID")
        .or_else(|_| std::env::var("SHANTY_SESSION"))
        .unwrap_or_else(|_| "unknown".to_string());
    let checks = vec![
        crate::action_certification::CheckInput {
            id: "landing-permitted".into(),
            expected: true.into(),
            observed: (!decision.refuses()).into(),
            evidence_ref: format!("landing:{}:{}", request.repo, request.git_ref),
        },
        crate::action_certification::CheckInput {
            id: "ref-stated-by-command".into(),
            expected: true.into(),
            observed: (!request.ref_assumed).into(),
            evidence_ref: landing.evidence.clone(),
        },
    ];
    let input = crate::action_certification::ActionInput {
        record_id: format!(
            "landing-{ts}-{}",
            &format!("{:x}", md5_ish(&landing.evidence))[..8]
        ),
        correlation_id: session.clone(),
        session,
        ts,
        agent: request.agent.clone().unwrap_or_else(|| "unknown".into()),
        item: request.bead.clone(),
        verb: request.verb.to_string(),
        target: format!("repo_{}", request.repo),
        target_class: "repo".into(),
        tenant: request.agent.clone().unwrap_or_else(|| "unknown".into()),
        result: decision.as_str().to_string(),
        repo: request.repo.clone(),
        sha: String::new(),
        git_ref: request.git_ref.clone(),
        remote_authority: String::new(),
        scope_provenance: serde_json::json!({ "as_of": ts, "query_id": "landing-policy" }),
        checks,
    };
    if let Ok(record) = crate::action_certification::sign(input, &key) {
        let spool = std::path::Path::new("action-certifications.jsonl");
        let _ = crate::action_certification::append(spool, &record);
    }
}

/// A tiny non-cryptographic digest, used ONLY to give a record a stable id per
/// distinct command. Not a hash anyone should rely on — the record's integrity
/// comes from its ed25519 signature, not from this.
#[cfg(feature = "quipu")]
fn md5_ish(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x1000_0000_01b3)
    })
}

#[cfg(all(test, feature = "quipu"))]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn the_routing_superset_admits_landings_and_skips_ordinary_work() {
        assert!(might_be_a_landing("git push origin main"));
        assert!(might_be_a_landing("gh pr merge 3"));
        // It is a SUPERSET, so false admissions are fine — `landing::resolve`
        // is the real filter. False EXCLUSIONS would be holes.
        assert!(!might_be_a_landing("cargo test"));
        assert!(!might_be_a_landing("ls -la"));
    }

    #[test]
    fn a_named_ref_is_never_marked_assumed() {
        let landing = crate::landing::resolve("git push origin main").unwrap();
        let (git_ref, assumed) = resolve_ref(&landing, std::path::Path::new("."));
        assert_eq!(git_ref, "main");
        assert!(!assumed, "the command stated the ref");
    }

    #[test]
    fn a_merge_ref_is_ALWAYS_marked_assumed() {
        // The command cannot carry the pull request's base branch, so the
        // stand-in must never present itself as stated.
        let landing = crate::landing::resolve("gh pr merge 3 -R scbrown/quipu").unwrap();
        let (git_ref, assumed) = resolve_ref(&landing, std::path::Path::new("."));
        assert_eq!(git_ref, crate::project_landing::DEFAULT_PROTECTED_REF);
        assert!(assumed);
    }

    #[test]
    fn a_slug_resolves_to_the_bare_repo_name_without_touching_git() {
        let landing = crate::landing::resolve("gh pr merge 3 -R scbrown/quipu").unwrap();
        let repo = resolve_repo(&landing, std::path::Path::new("/nonexistent"));
        assert_eq!(repo.as_deref(), Some("quipu"));
    }

    #[test]
    fn a_url_resolves_without_touching_git() {
        let landing =
            crate::landing::resolve("git push git@github.com:scbrown/yupana.git main").unwrap();
        let repo = resolve_repo(&landing, std::path::Path::new("/nonexistent"));
        assert_eq!(repo.as_deref(), Some("yupana"));
    }

    #[test]
    fn record_ids_are_stable_per_command_and_differ_across_commands() {
        // The spool refuses a repeat record_id with a different payload, so a
        // colliding id would turn two distinct landings into an error.
        assert_eq!(
            md5_ish("git push origin main"),
            md5_ish("git push origin main")
        );
        assert_ne!(md5_ish("git push origin main"), md5_ish("gh pr merge 3"));
    }
}
