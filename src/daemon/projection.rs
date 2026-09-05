//! The resident projected-policy subgraph (aegis-x894x2).
//!
//! ## What this fixes, stated as the measurement that motivated it
//!
//! Measured over 771 `kind:guard` spool rows to 2026-09-05: `p50=16ms`,
//! `p90=4584ms`, `p99=10055ms`, `max=26482ms`, and **100% of the 69 fail-opens
//! are `projection`**. The distribution is bimodal, and the two modes are the
//! two branches of [`crate::project::ProjectionRegistry::refresh_or_cached`]:
//!
//! * **cache HIT** (within `quipu.projection_cache_ttl_secs`) — served from
//!   disk, quipu never contacted. This is the 16ms mode.
//! * **cache MISS** — the hook issues its OWN live `/query`. quipu serves those
//!   effectively one at a time, ~10 crew agents each issue one per edit, and
//!   that is the whole tail.
//!
//! So the per-edit round-trip was already gone on a hit; what was never fixed is
//! that **a miss is answered by every hook independently**. The `7,37 * * * *`
//! cron refresher is a single-flight for the happy path only: when quipu is
//! unwell the refresh fails, logs `prior cache left in place` without advancing
//! `written_at`, the cache ages past its TTL, and every agent's every edit then
//! goes live against the quipu that is already struggling. The guard is least
//! available exactly when the graph is busiest — the same self-interference
//! [`crate::projection_cache`] was written to remove, reappearing one level up
//! at the TTL instead of at the per-edit query.
//!
//! ## The property this module adds
//!
//! **A hook never waits on quipu.** One resident refresher contacts quipu on its
//! own schedule; every hook reads the result from memory over localhost. On a
//! miss the hook gets an immediate answer — a servable projection, or an
//! immediate refusal — instead of blocking for the 4.5s-26s a contended
//! `/query` takes. That is what collapses p90/p99: the tail is hooks WAITING,
//! and after this there is nothing to wait for.
//!
//! N concurrent live queries per TTL-expiry become 1 background refresh.
//!
//! ## What it deliberately does NOT change
//!
//! The daemon serves the SAME [`CachedProjection`] the disk cache serves, and
//! the **caller applies the same TTL rule**. A projection past
//! `projection_cache_ttl_secs` is refused here exactly as
//! [`crate::projection_cache::load_servable`] refuses it, because
//! `a_cache_past_the_ttl_fails_open_and_the_reason_names_both_halves` is a
//! deliberate safety property: *a retired rule that keeps firing from cache is
//! worse than no rule — it is unfalsifiable from the outside.* Making the daemon
//! a faster SOURCE of the same artefact, rather than a second policy authority,
//! is what keeps that guarantee intact. This module introduces no new notion of
//! freshness and no new way to enforce.
//!
//! It also refreshes the DISK cache on every success, so a hook that is not a
//! daemon client still benefits from the single-flight.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::project::ProjectionRegistry;
use crate::projection_cache::CachedProjection;

/// What the daemon knows about the projection right now.
///
/// `projection` is `None` only before the first successful refresh with no
/// servable disk cache to seed from — reported as an explicit absence rather
/// than an empty catalogue, because "no rules" and "could not get the rules"
/// need opposite responses and an empty `Vec` conflates them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionReply {
    /// The last-known projection, or `None` if nothing has ever been obtained.
    pub projection: Option<CachedProjection>,
    /// Age in seconds of `projection` at the moment the reply was built. The
    /// CALLER decides servability from this against its own configured TTL;
    /// the daemon does not decide it for them.
    pub age_secs: Option<u64>,
    /// Why the most recent refresh attempt failed, if it did. Present even when
    /// `projection` is `Some`: "serving a 40-minute-old catalogue AND quipu has
    /// been failing for 20 minutes" is a different operational state from
    /// "serving a 40-minute-old catalogue", and only one of them is fine.
    pub last_error: Option<String>,
    /// Successful refreshes since the daemon started.
    pub refreshes: u64,
    /// Consecutive failures. Non-zero with a `Some(projection)` is the
    /// degrading case an operator wants to see before the TTL runs out.
    pub consecutive_failures: u64,
}

#[derive(Default)]
struct State {
    current: Option<CachedProjection>,
    last_error: Option<String>,
    refreshes: u64,
    consecutive_failures: u64,
}

/// The resident projection: one refresher, many readers.
pub struct ResidentProjection {
    inner: Arc<Inner>,
    stop: Arc<AtomicBool>,
}

struct Inner {
    endpoint: String,
    /// Where the shared disk cache lives, so a successful resident refresh also
    /// serves every hook that is not a daemon client. `None` disables the write
    /// (tests, and any deployment with no resolvable state dir).
    cache_path: Option<std::path::PathBuf>,
    state: Mutex<State>,
}

impl ResidentProjection {
    /// A resident projection for `endpoint`, seeded from `cache_path` if it
    /// holds anything readable.
    ///
    /// Seeding ignores the TTL on purpose: the daemon's job is to hold the
    /// last-known catalogue and report its age honestly, not to decide
    /// servability. A caller applying its own TTL will refuse a stale seed
    /// anyway, and refusing it HERE would mean a daemon restart during a quipu
    /// outage threw away the only copy anyone had.
    pub fn new(endpoint: &str, cache_path: Option<std::path::PathBuf>, now: u64) -> Self {
        let mut state = State::default();
        if let Some(path) = cache_path.as_deref() {
            // `u64::MAX` as the TTL: load whatever is there, then report its
            // real age. Every other check in `load_servable` — version,
            // endpoint match, future-dating — still applies and still refuses.
            if let Ok(cached) =
                crate::projection_cache::load_servable(path, endpoint, u64::MAX, now)
            {
                state.current = Some(cached);
            }
        }
        Self {
            inner: Arc::new(Inner {
                endpoint: endpoint.to_string(),
                cache_path,
                state: Mutex::new(state),
            }),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The current projection and its age — a lock read, never a network call.
    ///
    /// This is the whole point: it is the method the HTTP surface answers from,
    /// so a hook's cost is a localhost round-trip against resident memory
    /// rather than a contended `/query`.
    pub fn snapshot(&self, now: u64) -> ProjectionReply {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ProjectionReply {
            age_secs: state.current.as_ref().map(|c| c.age_secs(now)),
            projection: state.current.clone(),
            last_error: state.last_error.clone(),
            refreshes: state.refreshes,
            consecutive_failures: state.consecutive_failures,
        }
    }

    /// Contact quipu once and install the result. Returns the failure reason
    /// rather than an error type: it is recorded and served, not propagated.
    ///
    /// A FAILURE NEVER CLEARS THE LAST-KNOWN PROJECTION. That is the same rule
    /// the cron refresher follows (`prior cache left in place`) and it matters
    /// more here, because this copy is what every hook reads: dropping it on a
    /// transient 502 would turn one failed query into a fleet-wide loss of
    /// enforcement.
    pub fn refresh_once(&self, now: u64) -> Result<(), String> {
        let mut registry = ProjectionRegistry::new(&self.inner.endpoint);
        let result = registry.refresh().map_err(|e| e.to_string());

        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match result {
            Ok(()) => {
                let cached = registry.cached_projection(now);
                // Write through to the shared disk cache so non-client hooks
                // get the same single-flight benefit. Fail-silent, like every
                // other cache write: bookkeeping must not be able to change an
                // enforcement outcome.
                if let Some(path) = self.inner.cache_path.as_deref() {
                    crate::projection_cache::save(path, &cached);
                }
                state.current = Some(cached);
                state.last_error = None;
                state.refreshes += 1;
                state.consecutive_failures = 0;
                Ok(())
            }
            Err(reason) => {
                state.last_error = Some(reason.clone());
                state.consecutive_failures += 1;
                Err(reason)
            }
        }
    }

    /// Start the background refresher and return a handle that stops it on drop.
    ///
    /// `interval` is the steady-state period. On failure the retry backs off
    /// geometrically from `RETRY_FLOOR` up to `interval`, because the common
    /// failure is quipu being overloaded and a tight retry loop is the thing
    /// this module exists to stop doing to it.
    pub fn spawn_refresher(&self, interval: Duration) -> RefresherHandle {
        let inner = Arc::clone(&self.inner);
        let stop = Arc::clone(&self.stop);
        let handle = std::thread::Builder::new()
            .name("yupana-projection".into())
            .spawn(move || {
                const RETRY_FLOOR: Duration = Duration::from_secs(15);
                // Cap the backoff well BELOW the steady-state interval. The
                // backoff exists to stop hammering a struggling quipu, but it
                // also bounds how long after quipu RECOVERS the resident copy
                // stays stale — and while it is stale, clients that trust this
                // daemon are not enforcing. Five minutes keeps the recovery
                // latency bounded at something an operator would accept, where
                // capping at `interval` (half the TTL, so 30 min by default)
                // would not.
                const RETRY_CEILING: Duration = Duration::from_secs(300);
                let resident = ResidentProjection {
                    inner,
                    stop: Arc::clone(&stop),
                };
                let mut wait = Duration::ZERO;
                loop {
                    // Sleep in slices so a stop is honoured promptly even when
                    // the next attempt is a long way off.
                    let mut slept = Duration::ZERO;
                    while slept < wait {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        let slice = Duration::from_millis(200).min(wait - slept);
                        std::thread::sleep(slice);
                        slept += slice;
                    }
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let now = crate::projection_cache::now_secs();
                    wait = match resident.refresh_once(now) {
                        Ok(()) => interval,
                        Err(_) => {
                            let failures = resident.snapshot(now).consecutive_failures;
                            let backoff =
                                RETRY_FLOOR.saturating_mul(1u32 << failures.min(6) as u32);
                            backoff.min(RETRY_CEILING).min(interval)
                        }
                    };
                }
            })
            .ok();
        RefresherHandle {
            stop: Arc::clone(&self.stop),
            handle,
        }
    }
}

/// Stops the background refresher when dropped, so a daemon shutdown does not
/// leave a thread holding a quipu connection.
pub struct RefresherHandle {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for RefresherHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
#[path = "projection_test.rs"]
mod projection_test;
