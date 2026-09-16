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
    // THE DAEMON FIRST (aegis-kjz0hg). This was the last hook path measured
    // issuing a live quipu query — one per `git push`/`merge`, synchronously in
    // front of the command, on the path where a stall is least affordable.
    let authority = match super::daemon_projection::projected(&config, &mut registry) {
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

    let plate = crate::plate::observation(input.session_id.as_deref());
    let request = LandingRequest {
        verb: landing.verb.as_str(),
        repo,
        git_ref,
        ref_assumed,
        agent: acting_agent(),
        work_item_readable: plate.is_some(),
        bead: plate.flatten(),
        override_grant: override_grant(),
    };
    let decision = decide(&authority, &request);

    // Attest before answering: a verdict the guard acted on but never recorded
    // is exactly the gap moving attestation to the gate was meant to close.
    // Fail-silent, like every other piece of bookkeeping about enforcement.
    if !matches!(decision, Decision::NotApplicable { .. }) {
        record(&config, &request, &decision, &landing);
    }

    match decision {
        Decision::Allow { .. } if !request.work_item_readable => {
            Outcome::Notify("yupana: work-item plate UNKNOWN; fail-open on attribution only".into())
        }
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

/// The host guard's override grant for THIS command, if it issued one.
///
/// Published by the guard chain into the environment of this single invocation
/// (`bash-guards.sh` runs the host guard first and this hook last), so the grant
/// cannot outlive the tool call it was issued for and needs no expiry of its
/// own. That scoping is the design: the override token it derives from is
/// SINGLE-USE and the host guard unlinks it before deciding, so by the time this
/// policy runs there is nothing left to read and a second consumer is not
/// merely redundant but impossible (aegis-d7jpdw).
///
/// Absent means no grant — never "assume one". A landing the host refused
/// publishes nothing, which is exactly what keeps a refused attempt refused
/// here too.
#[cfg(feature = "quipu")]
fn override_grant() -> Option<String> {
    std::env::var("YUPANA_LANDING_OVERRIDE")
        .ok()
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
    let agent = request.agent.clone().unwrap_or_else(|| "unknown".into());
    let input = crate::action_certification::ActionInput {
        record_id: landing_record_id(ts, &agent, &landing.evidence),
        correlation_id: session.clone(),
        session,
        ts,
        agent: agent.clone(),
        item: request.bead.clone(),
        verb: request.verb.to_string(),
        target: format!("repo_{}", request.repo),
        target_class: "repo".into(),
        tenant: agent,
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
            "work_item_readable": request.work_item_readable,
            // Whether the host guard granted an override for this landing, and
            // why. A soak adjudicating this corpus against the host log must be
            // able to tell an overridden landing from an ordinary one WITHOUT
            // re-joining the two sources by timestamp.
            "override_granted": request.override_grant.is_some(),
            "override_reason": request.override_grant.clone(),
            // Preserve the actual policy diagnosis under the signature; the
            // generic certification mismatch alone cannot explain a refusal.
            "decision_codes": match decision {
                Decision::Refuse { codes, .. } => codes.clone(),
                _ => Vec::new(),
            },
        }),
        checks,
    };
    if let Ok(record) = crate::action_certification::sign(input, &key) {
        // Still fail-silent for the DECISION — bookkeeping must never change an
        // enforcement outcome — but no longer silent to the operator. A rejected
        // append is an anomaly, unlike the missing signing key above, which is the
        // ordinary state on a host that has never run `yupana verifier`.
        if let Err(error) = crate::action_certification::append(&spool, &record) {
            eprintln!("yupana: landing record NOT spooled ({error})");
        }
    }
}

/// The spool key for one governed landing.
///
/// The acting agent is PART OF THE KEY, not decoration (aegis-an6v8i). `ts` is
/// whole seconds and the hash covers only the command text, so without the agent
/// two landings of the SAME command inside one second collide.
/// [`crate::action_certification::append`] refuses a colliding id whose signed
/// payload differs, and `record` above discards that error, so the second verdict
/// was written nowhere and reported nowhere — no duplicate id, no gap, nothing
/// distinguishable from a landing that never happened. Measured on the live path:
/// wu and grant, same command, same second, ONE row.
///
/// The realistic shape is not two agents. It is ONE agent retrying inside a second
/// with a DIFFERENT outcome — refuse, arm an override token, retry — which is
/// exactly the 2026-09-14 wu sequence on scbrown/quipu #241, saved only by the
/// retry being 13 seconds later. Keeping the REFUSE and dropping the ALLOW
/// manufactures a false positive inside the soak that gates the block-tier flip.
///
/// A byte-identical retry by the same agent still dedupes: the id matches AND the
/// signed payload hash matches, which is the branch `append` treats as an
/// idempotent re-send. That property is load-bearing and is asserted in the tests.
#[cfg(feature = "quipu")]
fn landing_record_id(ts: u64, agent: &str, evidence: &str) -> String {
    format!(
        "landing-{ts}-{agent}-{}",
        &format!("{:x}", md5_ish(evidence))[..8]
    )
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
#[path = "landing_guard_test.rs"]
mod tests;
