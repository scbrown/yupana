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

    let reply = match crate::daemon::client::fetch_projection(host, port, DAEMON_TIMEOUT) {
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

#[cfg(test)]
#[path = "daemon_projection_test.rs"]
mod daemon_projection_test;
