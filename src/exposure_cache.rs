//! The exposure cache — last-known repo exposure on disk, so a FAILED LOOKUP
//! degrades the guard to enforcing stale truth instead of to enforcing nothing
//! (aegis-8tumi4, item 2).
//!
//! This is [`crate::projection_cache`]'s remedy applied to the guard's SECOND
//! network call. The two are deliberately parallel, and the reason they both
//! have to exist is that they fail independently: the projection answers "what
//! are the rules", exposure answers "does this repo publish", and a guard needs
//! BOTH to block. Before this module the projection had a durable fallback and
//! exposure did not, so a healthy cached rule set was routinely spent on an
//! exposure verdict of `Unknown` — and `Unknown` never blocks.
//!
//! **The measured defect this closes.** Of the 14 pre-edit evaluations that
//! actually required an exposure lookup, **6 timed out at a ~10s ceiling (43%)**
//! and every one of them scored `unknown`, which downgrades block-tier rules to
//! warnings. Interleaved seconds apart, same agent, same repo: a lookup that
//! answered `internal` in 5.4s sat between two that hit the ceiling. So the
//! guard's enforcement was a coin flip on quipu's latency, and nothing in the
//! record said so.
//!
//! **What is cacheable, and the distinction is the whole design.**
//! [`ExposureAnswer`](crate::project_exposure::ExposureAnswer) already draws it:
//! only `Answered` is stored. A repo's absence from the graph IS an answer and
//! is cached; a timeout, a 502 or an unparseable body is NOT, because caching
//! our failure to ask would freeze a transient blip for the whole TTL and
//! convert it into hours of degraded enforcement — during exactly the quipu
//! trouble that produced it.
//!
//! **Why one file per repo rather than one map.** ~20 crew agents run this hook
//! concurrently, one process per edit. A single map file would make every write
//! a read-modify-write, and two agents confirming two different repos in the
//! same instant would each write back a map missing the other's entry — silent
//! cache erosion under exactly the concurrency this cache exists to survive.
//! Per-repo files have no shared mutable state: each write is an atomic rename
//! of one independent fact, and each fact carries its own confirmation time,
//! which is what the age on the record has to mean.
//!
//! **What this does NOT do**, on the same contract as the projection cache:
//!
//! - a cache-served verdict is reported as `cache`, never as `answered`. "The
//!   guard enforced last-known exposure" and "the guard could not resolve
//!   exposure" are different findings and must stay different records, or the
//!   aegis-mqnl advise-soak becomes unadjudicable a second time;
//! - past the TTL the cache is REFUSED and the lookup fails open loudly. A
//!   verdict that keeps enforcing from cache forever is worse than no verdict,
//!   because it is unfalsifiable from outside. The TTL is a ceiling on AGE: `0`
//!   is the tightest ceiling, not an off switch, matching the sibling cache
//!   that reads the same knob;
//! - a cache written against a different quipu endpoint is refused outright —
//!   serving it would enforce another deployment's notion of "public";
//! - it never invents a verdict. No cache means the answer stays `Unknown`, and
//!   the reason carried is BOTH why the lookup failed and why the cache could
//!   not cover for it.
//!
//! **The direction of danger is not symmetric, and it sets the TTL.** Serving a
//! stale `Public` over-blocks: annoying, visible, self-correcting. Serving a
//! stale `Internal` or `Unknown` for a repo that has since gone public
//! UNDER-enforces, silently, which is the failure mode of the original bug. So
//! this cache reuses [`projection_cache_ttl_secs`](crate::config::QuipuConfig::projection_cache_ttl_secs)
//! rather than taking a longer ceiling of its own: exposure changes rarely, and
//! the temptation is therefore to cache it for a day, but the thing a long TTL
//! buys is precisely the under-enforcing direction. One knob answers one
//! question — "how long may this guard enforce from last-known instead of
//! live" — and a second knob would let an operator lengthen the dangerous half
//! while believing they had tuned the safe one.
//!
//! Writes are FAIL-SILENT (bookkeeping about enforcement must never change an
//! enforcement outcome); reads are not, because a read failure is a decision
//! input and the record has to say which one it was.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project_exposure::RepoExposure;

/// On-disk format version. A cache written by another version is REFUSED rather
/// than parsed leniently — a half-understood exposure verdict is the one thing
/// this cache must not serve, because the lenient reading of a missing field is
/// always the permissive one.
pub const EXPOSURE_CACHE_VERSION: u32 = 1;

/// The longest repo name this cache will store a file for. Not a limit on what
/// yupana can EVALUATE — an over-long name simply is not cached, and the lookup
/// behaves exactly as it did before this module existed.
const MAX_REPO_NAME_LEN: usize = 64;

/// One repo's last-known exposure, with the endpoint and the moment it was
/// CONFIRMED — not the moment it last changed. The age on a degraded record is
/// "how long since we checked", which is the number an operator needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedExposure {
    /// On-disk format version; see [`EXPOSURE_CACHE_VERSION`].
    pub version: u32,
    /// Unix seconds at which quipu last CONFIRMED this verdict.
    pub written_at: u64,
    /// The quipu base URL this verdict came from.
    pub endpoint: String,
    /// The repo label this verdict is about. Stored as well as encoded in the
    /// filename so a read can prove the file it found is the file it wanted:
    /// the filename is derived, and a derivation collision must be caught
    /// rather than served as another repo's exposure.
    pub repo: String,
    /// The verdict itself. Only ever written from an `Answered` lookup.
    pub exposure: RepoExposure,
}

/// Why a cached exposure could not be served. Every variant is a REASON carried
/// into the record: "the guard failed open" and "the guard failed open because
/// the only cached verdict was 3 hours old against a 1 hour TTL" are different
/// findings, and only the second tells an operator what to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExposureCacheMiss {
    /// No state directory could be resolved at all, so there is nowhere to
    /// cache. Distinct from [`Absent`](Self::Absent): nothing is wrong with the
    /// cache, there is no cache.
    NoStateDir,
    /// No cached verdict for this repo — the ordinary state before the first
    /// successful lookup.
    Absent,
    /// The file exists but could not be read or parsed.
    Unreadable(String),
    /// Written by a different on-disk format version.
    Version(u32),
    /// Written against a different quipu endpoint.
    Endpoint(String),
    /// The file's own `repo` field disagrees with the repo we asked about — a
    /// filename collision or a hand-edited cache. Refused: serving it would
    /// apply one repo's exposure to another, which is the single worst thing
    /// this cache could do.
    RepoMismatch(String),
    /// Older than the configured TTL.
    Expired {
        /// How old the cached verdict actually is, in seconds.
        age_secs: u64,
        /// The ceiling it exceeded.
        ttl_secs: u64,
    },
    /// Timestamped in the future — the clock moved backwards, so the age, and
    /// therefore the TTL check, cannot be trusted. Refused in the conservative
    /// direction: an untrustworthy age is not a young one.
    FutureDated {
        /// The cached verdict's own timestamp.
        written_at: u64,
        /// Now, by this process's clock.
        now: u64,
    },
    /// The repo label cannot be turned into a filename safely — it is empty,
    /// over-long, or contains characters outside `[A-Za-z0-9._-]`, or it is
    /// `.`/`..`.
    ///
    /// This is REFUSED rather than sanitised-and-stored, and it is a named miss
    /// rather than a silent skip. A repo label is derived from a remote URL, so
    /// it is externally influenced; quietly mapping `../../x` onto some
    /// neighbouring path is how a cache becomes a write primitive. Naming the
    /// refusal is the other half: a guard whose coverage silently has holes
    /// reports the same success for a repo it protects and a repo it has never
    /// been able to cache.
    UnsafeName(String),
}

impl std::fmt::Display for ExposureCacheMiss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoStateDir => write!(f, "no state directory could be resolved for the cache"),
            Self::Absent => write!(f, "no exposure has been cached for this repo yet"),
            Self::Unreadable(why) => write!(f, "the cached exposure is unreadable ({why})"),
            Self::Version(found) => write!(
                f,
                "the cached exposure is format v{found}, this build reads \
                 v{EXPOSURE_CACHE_VERSION}"
            ),
            Self::Endpoint(found) => write!(
                f,
                "the cached exposure was fetched from a different quipu (`{found}`)"
            ),
            Self::RepoMismatch(found) => write!(
                f,
                "the cached exposure is about a different repo (`{found}`)"
            ),
            Self::Expired { age_secs, ttl_secs } => write!(
                f,
                "the cached exposure is {age_secs}s old, past the {ttl_secs}s TTL"
            ),
            Self::FutureDated { written_at, now } => write!(
                f,
                "the cached exposure is dated {written_at} but now is {now} — the \
                 clock moved backwards, so its age cannot be trusted"
            ),
            Self::UnsafeName(repo) => write!(
                f,
                "the repo label `{repo}` cannot be cached safely as a filename"
            ),
        }
    }
}

/// A miss kind as a stable one-word label for the metrics record, so fail-opens
/// can be grouped by WHY the cache did not save them without parsing prose.
#[must_use]
pub fn miss_label(miss: &ExposureCacheMiss) -> &'static str {
    match miss {
        ExposureCacheMiss::NoStateDir => "no-state-dir",
        ExposureCacheMiss::Absent => "absent",
        ExposureCacheMiss::Unreadable(_) => "unreadable",
        ExposureCacheMiss::Version(_) => "version",
        ExposureCacheMiss::Endpoint(_) => "endpoint",
        ExposureCacheMiss::RepoMismatch(_) => "repo-mismatch",
        ExposureCacheMiss::Expired { .. } => "expired",
        ExposureCacheMiss::FutureDated { .. } => "future-dated",
        ExposureCacheMiss::UnsafeName(_) => "unsafe-name",
    }
}

/// Where cached exposures live: `$YUPANA_EXPOSURE_CACHE_DIR`, else
/// `$XDG_STATE_HOME/yupana/exposure`, else `~/.local/state/yupana/exposure` —
/// beside `metrics.jsonl` and `projection.json`, the same precedence as every
/// other piece of yupana state.
///
/// Pure, so the precedence is testable without touching the process
/// environment: parallel tests race on env vars, and this crate denies
/// `unsafe_code`, which `set_var` now requires.
#[must_use]
pub fn resolve_dir(
    explicit: Option<&str>,
    xdg_state: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(PathBuf::from(p));
    }
    if let Some(x) = xdg_state {
        return Some(PathBuf::from(x).join("yupana").join("exposure"));
    }
    home.map(|h| {
        PathBuf::from(h)
            .join(".local")
            .join("state")
            .join("yupana")
            .join("exposure")
    })
}

/// The resolved cache directory for this process, or `None` when no state
/// directory can be determined at all.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    resolve_dir(
        std::env::var("YUPANA_EXPOSURE_CACHE_DIR").ok().as_deref(),
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// The file a repo's verdict lives in, or [`ExposureCacheMiss::UnsafeName`].
///
/// The charset is an ALLOWLIST on purpose. A denylist of separators would have
/// to anticipate every platform's idea of one; the set of names that are
/// unambiguously safe as a single path component is small, known, and covers
/// every repo label this fleet actually has (`shantytown`, `yupana`,
/// `goldblum`, …). Rejecting `.` and `..` explicitly matters even though both
/// pass the charset: as a filename each would resolve to a directory, not a
/// file.
pub fn path_for(dir: &Path, repo: &str) -> Result<PathBuf, ExposureCacheMiss> {
    let unsafe_name = || ExposureCacheMiss::UnsafeName(repo.to_string());
    if repo.is_empty() || repo.len() > MAX_REPO_NAME_LEN || repo == "." || repo == ".." {
        return Err(unsafe_name());
    }
    if !repo
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(unsafe_name());
    }
    Ok(dir.join(format!("{repo}.json")))
}

/// Persist a CONFIRMED exposure verdict, ATOMICALLY and fail-silently.
///
/// Call this only for an `Answered` lookup — see the module docs on why a failed
/// lookup must never be stored. Atomic because many hooks write concurrently:
/// the temp name is process-unique and the rename is what publishes the file,
/// so a reader never sees a half-written verdict and two writers cannot
/// interleave into one.
pub fn save(dir: &Path, endpoint: &str, repo: &str, exposure: &RepoExposure, now: u64) {
    // Nothing here may escape onto the enforcement path, panics included: this
    // is bookkeeping about the guard, not the guard.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(path) = path_for(dir, repo) else {
            return;
        };
        let record = CachedExposure {
            version: EXPOSURE_CACHE_VERSION,
            written_at: now,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            repo: repo.to_string(),
            exposure: exposure.clone(),
        };
        let Ok(body) = serde_json::to_vec(&record) else {
            return;
        };
        let _ = std::fs::create_dir_all(dir);
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        if std::fs::write(&tmp, &body).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }));
}

/// Load a repo's cached exposure and decide whether it may be SERVED, given the
/// endpoint the caller is projecting from, the TTL and the current time.
///
/// Every refusal is a named [`ExposureCacheMiss`] rather than a bare `None`,
/// because the caller's job is to record why the guard degraded the way it did.
///
/// The TTL is a CEILING ON AGE, not a kill switch, and `ttl_secs == 0` is the
/// tightest ceiling rather than a special case: a verdict confirmed this very
/// second still serves, and anything older is refused as an immediate expiry —
/// the file may be perfectly good; policy is what refused it. This is the same
/// contract [`crate::projection_cache::load_servable`] pins, deliberately,
/// because both read the SAME knob: one number must not mean "ceiling" on one
/// lookup and "off" on the other.
pub fn load_servable(
    dir: &Path,
    endpoint: &str,
    repo: &str,
    ttl_secs: u64,
    now: u64,
) -> Result<CachedExposure, ExposureCacheMiss> {
    let path = path_for(dir, repo)?;
    if !path.exists() {
        return Err(ExposureCacheMiss::Absent);
    }
    let body = std::fs::read(&path).map_err(|e| ExposureCacheMiss::Unreadable(e.to_string()))?;
    let cached: CachedExposure =
        serde_json::from_slice(&body).map_err(|e| ExposureCacheMiss::Unreadable(e.to_string()))?;
    if cached.version != EXPOSURE_CACHE_VERSION {
        return Err(ExposureCacheMiss::Version(cached.version));
    }
    // Endpoint equality is exact apart from a trailing slash, which `project`
    // already trims when it builds the URL — so `http://quipu.example` and
    // `http://quipu.example/` are the same deployment and must not invalidate a
    // cached verdict.
    if cached.endpoint.trim_end_matches('/') != endpoint.trim_end_matches('/') {
        return Err(ExposureCacheMiss::Endpoint(cached.endpoint));
    }
    if cached.repo != repo {
        return Err(ExposureCacheMiss::RepoMismatch(cached.repo));
    }
    if cached.written_at > now {
        return Err(ExposureCacheMiss::FutureDated {
            written_at: cached.written_at,
            now,
        });
    }
    let age_secs = now - cached.written_at;
    if age_secs > ttl_secs {
        return Err(ExposureCacheMiss::Expired { age_secs, ttl_secs });
    }
    Ok(cached)
}

impl CachedExposure {
    /// How old this verdict is at `now`, in seconds. Saturating, so a
    /// future-dated record reports 0 here rather than wrapping — the refusal for
    /// that case is [`ExposureCacheMiss::FutureDated`], made in
    /// [`load_servable`] before any age arithmetic is trusted.
    #[must_use]
    pub fn age_secs(&self, now: u64) -> u64 {
        now.saturating_sub(self.written_at)
    }
}

#[cfg(test)]
#[path = "exposure_cache_test.rs"]
mod exposure_cache_test;
