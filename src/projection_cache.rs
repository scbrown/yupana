//! The projection cache — the DURABLE half of `src/project.rs`'s "hot,
//! one-directional cache" (aegis-0upyu).
//!
//! [`crate::project::ProjectionRegistry`] documents itself as a cache and has a
//! correct [`Freshness`](crate::types::Freshness) contract, but it is an
//! in-memory struct and the pre-edit hook is a SHORT-LIVED PROCESS PER EDIT. So
//! before this module the cache cached nothing: every edit, from every agent,
//! did a fresh live `/query` against quipu, and any failure to complete it
//! degraded the guard straight to "allow".
//!
//! That is measured, not theoretical: 5.2% of all pre-edit invocations and
//! **19% of one day's** failed open on projection timeouts, because quipu
//! serves `/query` effectively one at a time and ~20 crew agents each issue one
//! per edit. The failure is self-interfering — loading the graph disables the
//! guard that reads the graph — so the guard was least available exactly when
//! graph work was heaviest.
//!
//! **What this module changes, and what it deliberately does not.** It gives
//! the existing freshness machinery a durable store to fall back on, so a
//! projection failure degrades to *enforcing last-known policy, stale and
//! saying so* rather than to *not enforcing*. It does NOT make a cached verdict
//! look fresh, and it does NOT let a cache enforce forever:
//!
//! - a cache-served projection is [`Freshness::Stale`](crate::types::Freshness),
//!   never `Fresh`, and its AGE rides the record — a guard silently enforcing
//!   week-old rules is the next version of the bug this fixes;
//! - past [`QuipuConfig::projection_cache_ttl_secs`](crate::config::QuipuConfig),
//!   the cache is REFUSED and the guard fails open loudly, because a retired
//!   rule that keeps firing from cache is worse than no rule: it is
//!   unfalsifiable from the outside;
//! - a cache written against a DIFFERENT quipu endpoint is refused outright.
//!   Serving it would enforce another deployment's policy while claiming to
//!   enforce this one's;
//! - "served from cache" and "failed open" are DIFFERENT record kinds and must
//!   stay that way. Collapsing them would reintroduce precisely the ambiguity
//!   the sibling hook removed in aegis-tv9ri, and would make the aegis-mqnl
//!   advise-soak unadjudicable a second time — an unguarded edit is not a clean
//!   edit, and neither is a stale-guarded one, but they are not the same thing.
//!
//! Writes are FAIL-SILENT, on the same contract as `crate::metrics`: a cache
//! write is bookkeeping about enforcement, not enforcement, and must never be
//! able to change a guard outcome. Reads are not — a read failure is a decision
//! input, and it is reported as [`CacheMiss`] so the record can say which one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project::ProjectedPolicy;
use crate::textrules::TextRule;

/// The on-disk format version. Bumped when the persisted shape changes; a cache
/// written by another version is REFUSED rather than parsed leniently, because
/// a half-understood policy set is the one thing this cache must never serve.
pub const CACHE_VERSION: u32 = 1;

/// A persisted projection: both catalogues, the endpoint they came from, and
/// when they were last CONFIRMED against quipu.
///
/// Both planes or neither, mirroring `ProjectionRegistry::refresh` — a cache
/// that held structural policies from one sync and text rules from another
/// would let the two planes disagree about which sync they reflect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedProjection {
    /// On-disk format version; see [`CACHE_VERSION`].
    pub version: u32,
    /// Unix seconds at which this projection was last confirmed live against
    /// quipu — the age denominator, and the reason this is a WRITE on every
    /// successful refresh rather than only on change. The field answers "when
    /// did we last check", not "when did the policy last differ".
    pub written_at: u64,
    /// The quipu base URL this projection was fetched from.
    pub endpoint: String,
    /// Last-known structural policies.
    pub policies: Vec<ProjectedPolicy>,
    /// Last-known governed text rules (the aegis-mqnl catalogue).
    pub text_rules: Vec<TextRule>,
}

/// Why a cache could not be served. Every variant is a REASON, carried into the
/// record: "the guard failed open" and "the guard failed open because the cache
/// was 4 hours old against a 1 hour TTL" are different findings, and only the
/// second one tells an operator what to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheMiss {
    /// No cache file — the ordinary state before the first successful refresh.
    Absent,
    /// The file exists but could not be read or parsed.
    Unreadable(String),
    /// Written by a different on-disk format version.
    Version(u32),
    /// Written against a different quipu endpoint.
    Endpoint(String),
    /// Older than the configured TTL.
    Expired {
        /// How old the cache actually is, in seconds.
        age_secs: u64,
        /// The ceiling it exceeded.
        ttl_secs: u64,
    },
    /// Timestamped in the future — the clock moved backwards, so the age (and
    /// therefore the TTL check) cannot be trusted. Refused in the conservative
    /// direction: an untrustworthy age is not a young one.
    FutureDated {
        /// The cache's own timestamp.
        written_at: u64,
        /// Now, by this process's clock.
        now: u64,
    },
}

impl std::fmt::Display for CacheMiss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absent => write!(f, "no cached projection has been written yet"),
            Self::Unreadable(why) => write!(f, "the cached projection is unreadable ({why})"),
            Self::Version(found) => write!(
                f,
                "the cached projection is format v{found}, this build reads v{CACHE_VERSION}"
            ),
            Self::Endpoint(found) => write!(
                f,
                "the cached projection was fetched from a different quipu (`{found}`)"
            ),
            Self::Expired { age_secs, ttl_secs } => write!(
                f,
                "the cached projection is {age_secs}s old, past the {ttl_secs}s TTL"
            ),
            Self::FutureDated { written_at, now } => write!(
                f,
                "the cached projection is dated {written_at} but now is {now} — \
                 the clock moved backwards, so its age cannot be trusted"
            ),
        }
    }
}

/// A miss kind as a stable one-word label for the metrics record, so an
/// operator can group fail-opens by WHY the cache did not save them without
/// parsing prose.
#[must_use]
pub fn miss_label(miss: &CacheMiss) -> &'static str {
    match miss {
        CacheMiss::Absent => "absent",
        CacheMiss::Unreadable(_) => "unreadable",
        CacheMiss::Version(_) => "version",
        CacheMiss::Endpoint(_) => "endpoint",
        CacheMiss::Expired { .. } => "expired",
        CacheMiss::FutureDated { .. } => "future-dated",
    }
}

/// Where the cache lives: `$YUPANA_PROJECTION_CACHE_PATH`, else
/// `$XDG_STATE_HOME/yupana/projection.json`, else
/// `~/.local/state/yupana/projection.json` — beside `metrics.jsonl`, the same
/// precedence as every other piece of yupana state.
///
/// Pure, so the precedence is testable without touching the process
/// environment: parallel tests race on env vars, and this crate denies
/// `unsafe_code`, which `set_var` now requires.
#[must_use]
pub fn resolve_path(
    explicit: Option<&str>,
    xdg_state: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(PathBuf::from(p));
    }
    if let Some(x) = xdg_state {
        return Some(PathBuf::from(x).join("yupana").join("projection.json"));
    }
    home.map(|h| {
        PathBuf::from(h)
            .join(".local")
            .join("state")
            .join("yupana")
            .join("projection.json")
    })
}

/// The resolved cache path for this process, or `None` when no state directory
/// can be determined at all.
#[must_use]
pub fn cache_path() -> Option<PathBuf> {
    resolve_path(
        std::env::var("YUPANA_PROJECTION_CACHE_PATH").ok().as_deref(),
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Unix seconds now, or 0 if the clock is before the epoch.
#[must_use]
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Persist a successful projection, ATOMICALLY and fail-silently.
///
/// Atomic because ~20 agents write this file concurrently, one per successful
/// edit: the temp file is process-unique and the rename is what publishes it,
/// so a reader never sees a half-written catalogue and two writers never
/// interleave into one. Fail-silent because this runs on the hook path — see
/// the module docs.
pub fn save(path: &Path, projection: &CachedProjection) {
    // Nothing here may escape onto the enforcement path, panics included: this
    // is bookkeeping about the guard, not the guard.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(body) = serde_json::to_vec(projection) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Process-unique temp name: two concurrent hooks writing `.tmp` would
        // clobber each other's partial writes and one could rename the other's
        // truncated file into place — publishing a torn catalogue, which is the
        // one failure a cache of policy must not have.
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        if std::fs::write(&tmp, &body).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        if std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }));
}

/// Load the cache and decide whether it may be SERVED, given the endpoint the
/// caller is projecting from, the TTL and the current time.
///
/// Every refusal is a named [`CacheMiss`] rather than a bare `None`, because
/// the caller's job is to record why the guard degraded the way it did.
///
/// `ttl_secs == 0` disables cache serving entirely (the knob's "off" position),
/// and is reported as an immediate expiry rather than as an absent cache — the
/// file may be perfectly good; policy is what refused it.
pub fn load_servable(
    path: &Path,
    endpoint: &str,
    ttl_secs: u64,
    now: u64,
) -> Result<CachedProjection, CacheMiss> {
    if !path.exists() {
        return Err(CacheMiss::Absent);
    }
    let body = std::fs::read(path).map_err(|e| CacheMiss::Unreadable(e.to_string()))?;
    let cached: CachedProjection =
        serde_json::from_slice(&body).map_err(|e| CacheMiss::Unreadable(e.to_string()))?;
    if cached.version != CACHE_VERSION {
        return Err(CacheMiss::Version(cached.version));
    }
    // Endpoint equality is exact apart from a trailing slash, which `project`
    // already trims when it builds the URL — so `http://quipu.example` and
    // `http://quipu.example/` are the same deployment and must not invalidate a
    // cache.
    if cached.endpoint.trim_end_matches('/') != endpoint.trim_end_matches('/') {
        return Err(CacheMiss::Endpoint(cached.endpoint));
    }
    if cached.written_at > now {
        return Err(CacheMiss::FutureDated {
            written_at: cached.written_at,
            now,
        });
    }
    let age_secs = now - cached.written_at;
    if age_secs > ttl_secs {
        return Err(CacheMiss::Expired { age_secs, ttl_secs });
    }
    Ok(cached)
}

impl CachedProjection {
    /// How old this projection is at `now`, in seconds. Saturating, so a
    /// future-dated cache reports 0 here rather than wrapping — the refusal for
    /// that case is [`CacheMiss::FutureDated`], made in [`load_servable`]
    /// before any age arithmetic is trusted.
    #[must_use]
    pub fn age_secs(&self, now: u64) -> u64 {
        now.saturating_sub(self.written_at)
    }
}

#[cfg(test)]
#[path = "projection_cache_test.rs"]
mod projection_cache_test;
