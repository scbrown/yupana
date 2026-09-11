//! Ask the resident daemon for the projected policy before going live
//! (aegis-x894x2).
//!
//! This is the hook half of [`crate::daemon::projection`]. The guard's measured
//! tail — p90 4584ms, p99 10055ms, max 26482ms, and 100% of the 69 fail-opens —
//! is hooks WAITING on their own live quipu `/query` after the shared disk cache
//! expired. quipu serves those effectively one at a time while ~10 agents each
//! issue one per edit, so the cost is superlinear in how busy the fleet is.
//!
//! With a daemon there is ONE refresher and the hook reads its answer from
//! localhost. The hook never waits on quipu, which is what collapses the tail.
//!
//! ## Three outcomes, deliberately distinct
//!
//! | return | meaning | what the caller does |
//! |---|---|---|
//! | `None` | no usable daemon answer | fall back to the LIVE path, unchanged |
//! | `Some(Ok(source))` | the daemon served a projection inside the TTL | enforce it |
//! | `Some(Err(reason))` | the daemon is authoritative and its copy is UNSERVABLE | fail open NOW, without a live query |
//!
//! The third row is the one that needs justifying, because it stops enforcing
//! without asking quipu. It is correct precisely BECAUSE the daemon is the
//! single-flight: its background refresher has been trying and failing, so a
//! hook's own attempt would very probably fail too — after 4.5s-26s of waiting,
//! and while adding load to the quipu that is already struggling. Failing open
//! immediately reaches the same outcome instantly and without amplifying the
//! outage. The daemon's backoff is capped at 5 minutes for exactly this reason:
//! it bounds how long this state can persist after quipu recovers.
//!
//! The projection's AGE is recomputed here from its `written_at` against the
//! local clock, never taken from the daemon's self-reported `age_secs`: the
//! decision to keep enforcing must not be delegated to the thing being checked.
//!
//! A daemon that is DOWN, or that holds nothing yet, is `None` — never
//! `Some(Err)`. "The resident guard is absent" must never be mistaken for "the
//! policy says stop enforcing"; that would make killing one process the cheapest
//! possible bypass, which is the invariant the whole daemon client is built
//! around.

use crate::config::YupanaConfig;
use crate::project::{ProjectionRegistry, ProjectionSource};
use crate::types::Freshness;

/// How long to wait on the daemon. It answers from memory over loopback, so
/// this is generous for the happy path and still an order of magnitude below
/// the live `/query` latency this exists to avoid — a hook must never trade one
/// stall for another.
const DAEMON_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// How long to wait on `/exposure`, which is NOT the same budget as
/// [`DAEMON_TIMEOUT`].
///
/// `/projection` answers from memory, so 1.5s is generous. `/exposure` answers
/// from memory only on a HIT; on a MISS the daemon does the live
/// `POST /policy/check`, measured at 2.4-7.2s. Giving that the projection's
/// budget would time out mid-call and be strictly WORSE than no cache: the hook
/// would print a spurious daemon-down notice and then make its OWN live call, so
/// a cold repo would cost TWO `/policy/check` round-trips instead of one.
///
/// So this must exceed the live path's own ceiling (`project::http_timeout`,
/// default 10s), not merely the loopback hop. The margin covers the hop itself —
/// giving up one tick before the daemon answers would produce exactly the double
/// call this exists to prevent.
fn daemon_exposure_timeout() -> std::time::Duration {
    crate::project::http_timeout() + std::time::Duration::from_secs(2)
}

/// Try the resident daemon. See the module docs for the three outcomes.
pub(super) fn from_daemon(
    config: &YupanaConfig,
    registry: &mut ProjectionRegistry,
    now: u64,
) -> Option<Result<ProjectionSource, String>> {
    if !config.serve.use_daemon {
        return None;
    }
    let host = &config.serve.bind_address;
    let port = config.serve.mcp_http_port;

    let reply = match crate::daemon::client_policy::fetch_projection(host, port, DAEMON_TIMEOUT) {
        Ok(reply) => reply,
        Err(why) => {
            // LOUD, per the daemon-down contract: `use_daemon` is only set by an
            // operator who actually started one, so its absence is a real
            // finding rather than ambient noise. Then fall back — the guard
            // still runs, it just runs the slow way.
            eprintln!(
                "yupana: resident daemon expected at {host}:{port} but not usable \
                 ({why}) — projecting live instead"
            );
            return None;
        }
    };

    let cached = reply.projection?;

    // The daemon could be projecting a DIFFERENT quipu. `install_cached` does
    // not check, because on the disk path `load_servable` already has — so the
    // check has to be made here or not at all. Serving another deployment's
    // catalogue would enforce its policy while claiming to enforce ours.
    if cached.endpoint.trim_end_matches('/') != config.quipu.endpoint.trim_end_matches('/') {
        eprintln!(
            "yupana: resident daemon projects `{}` but this repo is configured for \
             `{}` — ignoring it and projecting live",
            cached.endpoint, config.quipu.endpoint
        );
        return None;
    }

    // Compute the age LOCALLY from `written_at`, rather than trusting the
    // daemon's self-reported `age_secs`. Same denominator `load_servable` uses,
    // and it means a daemon that under-reports its age — through a bug, a wrong
    // clock, or otherwise — cannot hold stale policy in force indefinitely. The
    // reported figure stays in the reply for operators; the DECISION uses ours.
    let age_secs = now.saturating_sub(cached.written_at);
    let ttl = config.quipu.projection_cache_ttl_secs;
    if age_secs <= ttl {
        registry.install_cached(cached, Freshness::Fresh);
        return Some(Ok(ProjectionSource::FreshCache { age_secs }));
    }

    // Past the TTL. The same refusal `load_servable` makes, made in the same
    // place in the decision — a retired rule that keeps firing from a stale
    // catalogue is worse than no rule, because it is unfalsifiable from
    // outside. The reason names the refresher's own failure so the notice says
    // WHY the resident copy went stale, not merely that it did.
    let because = reply.last_error.as_deref().unwrap_or("no reason recorded");
    Some(Err(format!(
        "the resident daemon's projection is {age_secs}s old, past the {ttl}s TTL, \
         after {} consecutive failed refreshes ({because}); not adding a live query \
         to a projection path that is already failing",
        reply.consecutive_failures
    )))
}

/// Project governed policy: the RESIDENT DAEMON FIRST, then the live path.
///
/// THE ONE PLACE THAT DECISION IS MADE. It used to be written inline, in
/// `rule_planes` only — and that is not a tidiness observation, it is the root
/// cause of aegis-kjz0hg: three other hook sites called `refresh_or_cached`
/// directly and therefore could never be served by the daemon, no matter how
/// healthy it was. Nobody chose that; there was simply no shared thing to call.
///
/// Measured 2026-09-06 through a capture proxy: with a usable daemon the
/// pre-edit path (which routed through here) issued ZERO quipu requests, while
/// `pre-bash` on a `git push` still issued one — `landing_guard`, going direct.
/// One query is not a floor, but it is a synchronous quipu round trip in front
/// of an agent's `git push`, which is where a 10-30s stall is least affordable.
///
/// The second thing the direct sites lost is quieter and worse: `from_daemon`
/// REFUSES a resident projection past the TTL, with a reason naming the
/// refresher's own failure, so a stale catalogue cannot keep enforcing
/// unfalsifiably. A direct `refresh_or_cached` has no equivalent for the
/// resident copy, so those sites had no staleness contract at all.
///
/// A down or unusable daemon falls through to exactly the previous behaviour.
pub(crate) fn projected(
    config: &YupanaConfig,
    registry: &mut ProjectionRegistry,
) -> Result<ProjectionSource, String> {
    let now = crate::projection_cache::now_secs();
    match from_daemon(config, registry, now) {
        Some(result) => result,
        None => registry.refresh_or_cached(
            crate::projection_cache::cache_path().as_deref(),
            config.quipu.projection_cache_ttl_secs,
            now,
        ),
    }
}

/// Resolve a repo's exposure, asking the resident daemon first (aegis-q4tt56),
/// keeping WHETHER WE GOT AN ANSWER as well as what it was.
///
/// The measured reason this exists: `POST /policy/check` took 2.4-7.2s and ran
/// once per governed edit, uncached, from every agent — the CONSTANT half of the
/// guard's quipu load, next to which the projection (~200us once resident) is
/// noise.
///
/// A DOWN daemon falls through to the live path, exactly as the projection does.
/// It must never be read as "unknown exposure": unknown DOWNGRADES block-tier
/// rules to warnings, so folding a transport failure into it would silently
/// weaken enforcement every time one process was not running — a policy change
/// wearing the costume of a connection error.
///
/// The decision is identical either way — a governed rule never blocks on a
/// guess — so this exists solely for the RECORD. `RepoExposure::Unknown` is two
/// facts wearing one token: "quipu says it does not know this repo", which is a
/// real and stable answer, and "we never reached quipu", which is a failed
/// measurement. Both fail open, and until they are told apart in the spool a
/// fail-open leaves a row indistinguishable from a correct pass on an unexposed
/// target — so the advise-mode soak cannot count the false negatives it exists
/// to count (aegis-8tumi4, measured: 6 of 14 network-needing lookups timed out).
///
/// This mirrors `policy_source` on the same record, which already draws exactly
/// this distinction for the OTHER lookup the guard makes, and for the same
/// stated reason: two verdicts are not equally good evidence just because they
/// carry the same word.
pub(super) fn exposure_answer_for(
    config: &YupanaConfig,
    repo: &str,
) -> (crate::project::RepoExposure, &'static str) {
    let now = crate::projection_cache::now_secs();
    match resolve_exposure_answer(config, repo) {
        crate::project_exposure::ExposureAnswer::Answered(exposure) => {
            // A CONFIRMED verdict refreshes last-known, so the next lookup that
            // fails has something honest to fall back on. Fail-silent by
            // contract: bookkeeping about enforcement must never be able to
            // change an enforcement outcome (aegis-8tumi4 item 2).
            if let Some(dir) = crate::exposure_cache::cache_dir() {
                crate::exposure_cache::save(&dir, &config.quipu.endpoint, repo, &exposure, now);
            }
            (exposure, "answered")
        }
        // We never got an answer. Degrade to LAST-KNOWN, not to allow — and
        // never store this failure as though it were a verdict.
        crate::project_exposure::ExposureAnswer::Unreachable(why) => {
            serve_last_known(config, repo, &why, now)
        }
    }
}

/// Ask the resident daemon, then quipu directly, for one repo's exposure.
///
/// Split out so the cache policy above reads as one decision over ONE answer,
/// rather than being duplicated down two transport paths that fail differently.
/// `Err` from the daemon means NO USABLE DAEMON and we resolve live; a daemon
/// reply flagged `unreachable` means the daemon reached us but quipu did not
/// reach IT, which is a failed lookup and must not be cached.
fn resolve_exposure_answer(
    config: &YupanaConfig,
    repo: &str,
) -> crate::project_exposure::ExposureAnswer {
    if config.serve.use_daemon {
        match crate::daemon::client_policy::fetch_exposure(
            &config.serve.bind_address,
            config.serve.mcp_http_port,
            repo,
            daemon_exposure_timeout(),
        ) {
            Ok(reply) => {
                return if reply.unreachable {
                    crate::project_exposure::ExposureAnswer::Unreachable(
                        reply.reason.clone().unwrap_or_else(|| {
                            "the resident daemon could not reach quipu".to_string()
                        }),
                    )
                } else {
                    crate::project_exposure::ExposureAnswer::Answered(reply.exposure())
                };
            }
            Err(why) => eprintln!(
                "yupana: resident daemon expected at {}:{} but exposure not usable ({why}) \
                 — resolving live instead",
                config.serve.bind_address, config.serve.mcp_http_port
            ),
        }
    }
    crate::project_exposure::fetch_exposure_answer(&config.quipu.endpoint, repo)
}

/// Serve last-known exposure after a FAILED lookup, or stay `Unknown` and say
/// why twice over.
///
/// The reason carried is deliberately BOTH halves — why the lookup failed and
/// why the cache could not cover for it. One without the other sends an
/// operator to the wrong side: "quipu timed out" alone hides that the cache was
/// a day stale, and "no cached exposure" alone hides that quipu is down.
fn serve_last_known(
    config: &YupanaConfig,
    repo: &str,
    why: &str,
    now: u64,
) -> (crate::project::RepoExposure, &'static str) {
    let Some(dir) = crate::exposure_cache::cache_dir() else {
        let miss = crate::exposure_cache::ExposureCacheMiss::NoStateDir;
        return (
            crate::project::RepoExposure::Unknown(format!(
                "{why}; and no cached exposure could be served ({}: {miss})",
                crate::exposure_cache::miss_label(&miss)
            )),
            "unreachable",
        );
    };
    match crate::exposure_cache::load_servable(
        &dir,
        &config.quipu.endpoint,
        repo,
        config.quipu.projection_cache_ttl_secs,
        now,
    ) {
        Ok(cached) => {
            // Loud on purpose. A guard that quietly enforces yesterday's answer
            // is the next version of the bug this fixes, so the degradation
            // announces itself and its AGE every time it happens.
            eprintln!(
                "yupana: exposure for `{repo}` could not be resolved ({why}) — enforcing \
                 last-known verdict from {}s ago",
                cached.age_secs(now)
            );
            (cached.exposure, "cache")
        }
        Err(miss) => (
            crate::project::RepoExposure::Unknown(format!(
                "{why}; and no cached exposure could be served ({}: {miss})",
                crate::exposure_cache::miss_label(&miss)
            )),
            "unreachable",
        ),
    }
}

#[cfg(test)]
#[path = "daemon_projection_test.rs"]
mod daemon_projection_test;
