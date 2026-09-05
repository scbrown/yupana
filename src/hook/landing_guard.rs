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
        // A projection succeeded — but "succeeded" includes being served from a
        // cache written before this plane existed, which carries no catalogue
        // and must not read as "nothing is governed".
        Ok(_) => match registry.landing_policies() {
            Some(catalogue) => crate::project_landing::resolve(catalogue, &repo),
            None => LandingAuthority::Unknown(
                "the served projection predates the landing plane and carries no \
                 landing catalogue; refresh it (`yupana promote`/scheduled refresh) \
                 so the rule can be read"
                    .into(),
            ),
        },
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

/// The directory a remote NAME must be resolved in.
///
/// The command's own `cd` wins over the payload's cwd, because the payload
/// carries the SESSION's directory and the command may have moved. A relative
/// `cd` is joined onto the session cwd, which is what the shell would do.
#[cfg(feature = "quipu")]
fn effective_root(landing: &Landing, root: &std::path::Path) -> std::path::PathBuf {
    match landing.cwd_hint.as_deref() {
        Some(dir) => {
            let p = std::path::Path::new(dir);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            }
        }
        None => root.to_path_buf(),
    }
}

/// Resolve the command's repository reference to a bare repository name.
#[cfg(feature = "quipu")]
fn resolve_repo(landing: &Landing, root: &std::path::Path) -> Option<String> {
    let root = effective_root(landing, root);
    match &landing.repo {
        RepoRef::Url(url) => crate::git::repo_name_from_url(url.trim()),
        RepoRef::Slug(slug) => slug.rsplit('/').next().map(str::to_string),
        RepoRef::Remote(name) => crate::git::run(&root, &["remote", "get-url", name])
            .and_then(|url| crate::git::repo_name_from_url(url.trim())),
        RepoRef::Cwd => crate::git::origin_repo_name(&root),
    }
}

/// Resolve the ref the landing writes to, and whether the command stated it.
///
/// The `bool` is not decoration. A refusal on a ref the command never named is
/// the shape a false positive takes here, and the soak has to be able to count
/// those separately without re-deriving anything.
#[cfg(feature = "quipu")]
fn resolve_ref(landing: &Landing, root: &std::path::Path) -> (String, bool) {
    let root = &effective_root(landing, root);
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

/// Where the signed action-certification records go.
///
/// `$YUPANA_ACTION_SPOOL`, else `$XDG_STATE_HOME/yupana/…`, else
/// `~/.local/state/yupana/action-certifications.jsonl`.
///
/// NOT a bare relative filename, which is what this first shipped as. The hook
/// is a short-lived process spawned in whatever directory the agent happens to
/// be standing in, so a relative spool scatters records across the filesystem
/// and none of them join a corpus anything reads. The variable and the default
/// are deliberately the ones the host guard's certification adapter already
/// passes to `yupana certify`, so a record written AT the gate lands in the same
/// signed corpus as one written after the fact — which is the point of moving
/// attestation to the gate, and the only way a soak can compare the two.
///
/// Pure, so the precedence is testable without touching the process environment.
#[cfg(feature = "quipu")]
#[must_use]
pub fn resolve_spool(
    explicit: Option<&str>,
    xdg_state: Option<&str>,
    home: Option<&str>,
) -> Option<std::path::PathBuf> {
    const FILE: &str = "action-certifications.jsonl";
    if let Some(p) = explicit {
        return Some(std::path::PathBuf::from(p));
    }
    if let Some(x) = xdg_state {
        return Some(std::path::PathBuf::from(x).join("yupana").join(FILE));
    }
    home.map(|h| {
        std::path::PathBuf::from(h)
            .join(".local")
            .join("state")
            .join("yupana")
            .join(FILE)
    })
}

/// Where the signing key lives.
///
/// `$YUPANA_ACTION_KEY`, else the configured path *if it is absolute*, else
/// `~/.config/aegis/yupana-signing.pk8`.
///
/// The absolute-only rule is the load-bearing part. `[yupana.quipu]
/// signing_key_path` defaults to a bare filename, and resolving that against the
/// hook's inherited working directory means the guard signs with whichever key
/// happens to be underfoot — or, far more often, finds none and silently records
/// nothing at all. A relative configured value is therefore ignored in favour of
/// the deployment default rather than honoured against an arbitrary directory.
#[cfg(feature = "quipu")]
#[must_use]
pub fn resolve_key_path(
    explicit: Option<&str>,
    configured: &str,
    home: Option<&str>,
) -> Option<std::path::PathBuf> {
    if let Some(p) = explicit {
        return Some(std::path::PathBuf::from(p));
    }
    let configured = std::path::Path::new(configured);
    if configured.is_absolute() {
        return Some(configured.to_path_buf());
    }
    home.map(|h| {
        std::path::PathBuf::from(h)
            .join(".config")
            .join("aegis")
            .join("yupana-signing.pk8")
    })
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
    let home = std::env::var("HOME").ok();
    let Some(key_path) = resolve_key_path(
        std::env::var("YUPANA_ACTION_KEY").ok().as_deref(),
        &config.quipu.signing_key_path,
        home.as_deref(),
    ) else {
        return;
    };
    let Some(key) = crate::verdict_spool::existing_key(&key_path) else {
        return;
    };
    let Some(spool) = resolve_spool(
        std::env::var("YUPANA_ACTION_SPOOL").ok().as_deref(),
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        home.as_deref(),
    ) else {
        return;
    };
    let ts = crate::projection_cache::now_secs();
    let session = std::env::var("CLAUDE_SESSION_ID")
        .or_else(|_| std::env::var("SHANTY_SESSION"))
        .unwrap_or_else(|_| "unknown".to_string());
    // ONE check, deliberately. `certify` derives `certification_status` from
    // whether every check is satisfied, so anything listed here is a condition
    // the landing had to MEET. Whether the command stated its ref is not such a
    // condition — it is how the evidence was obtained — and modelling it as a
    // check marked a perfectly good owner landing `uncertified`, which is the
    // record lying about the decision it accompanies. It rides
    // `scope_provenance` below instead, where derivation facts belong.
    let checks = vec![crate::action_certification::CheckInput {
        id: "landing-permitted".into(),
        expected: true.into(),
        observed: (!decision.refuses()).into(),
        evidence_ref: format!("landing:{}:{}", request.repo, request.git_ref),
    }];
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
        scope_provenance: serde_json::json!({
            "as_of": ts,
            "query_id": "landing-policy",
            // How the ref was obtained. A refusal on a ref the command never
            // named is the shape a false positive takes here, so the soak has
            // to be able to count those without re-deriving anything.
            "ref_stated_by_command": !request.ref_assumed,
            "command": landing.evidence,
        }),
        checks,
    };
    if let Ok(record) = crate::action_certification::sign(input, &key) {
        let _ = crate::action_certification::append(&spool, &record);
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
    fn the_spool_is_NEVER_relative_to_the_hooks_working_directory() {
        // The defect this pins: a bare filename sends every record to whatever
        // directory the agent was standing in, so the corpus a soak reads is
        // empty while the guard believes it is attesting.
        let p = resolve_spool(None, None, Some("/home/x")).unwrap();
        assert_eq!(
            p,
            std::path::PathBuf::from("/home/x/.local/state/yupana/action-certifications.jsonl")
        );
        assert!(p.is_absolute());
        // XDG wins over HOME, and an explicit path wins over both — the same
        // precedence the host certification adapter uses.
        assert_eq!(
            resolve_spool(None, Some("/state"), Some("/home/x")).unwrap(),
            std::path::PathBuf::from("/state/yupana/action-certifications.jsonl")
        );
        assert_eq!(
            resolve_spool(Some("/tmp/s.jsonl"), Some("/state"), Some("/home/x")).unwrap(),
            std::path::PathBuf::from("/tmp/s.jsonl")
        );
    }

    #[test]
    fn a_RELATIVE_configured_key_is_ignored_not_resolved_against_the_cwd() {
        // `signing_key_path` defaults to the bare "yupana-signing.pk8". Honouring
        // that relative value would make the guard sign with whichever key is
        // underfoot, or — the common case — find none and record nothing while
        // reporting success.
        assert_eq!(
            resolve_key_path(None, "yupana-signing.pk8", Some("/home/x")).unwrap(),
            std::path::PathBuf::from("/home/x/.config/aegis/yupana-signing.pk8")
        );
        // An ABSOLUTE configured path is a deliberate deployment choice and wins.
        assert_eq!(
            resolve_key_path(None, "/opt/k.pk8", Some("/home/x")).unwrap(),
            std::path::PathBuf::from("/opt/k.pk8")
        );
        // The env var beats everything, matching the adapter's contract.
        assert_eq!(
            resolve_key_path(Some("/env/k.pk8"), "/opt/k.pk8", Some("/home/x")).unwrap(),
            std::path::PathBuf::from("/env/k.pk8")
        );
    }

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
