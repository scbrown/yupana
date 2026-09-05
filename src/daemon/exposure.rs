//! The resident repo-exposure cache (aegis-q4tt56).
//!
//! ## Why this is the bigger half
//!
//! `aegis-x894x2` made the PROJECTION read resident, and measured afterwards the
//! daemon answers `/projection` in ~200 microseconds. The guard still took
//! 2.5-9.7s per governed edit, because `governed_check` also resolves the repo's
//! exposure through `POST /policy/check` — **uncached, once per governed edit,
//! from every agent**. Measured 2026-09-05, five consecutive calls:
//!
//! ```text
//! 200 in 2.498s   200 in 2.426s   200 in 3.308s   200 in 7.023s   200 in 7.204s
//! ```
//!
//! Projection load is BURSTY (only on TTL expiry, and the deployed TTL of 14400s
//! needs eight consecutive refresher failures to reach one). Exposure load is
//! CONSTANT. So this is the half that matches what aegis-6uqni measured from the
//! store side: ureq holding 4.38 s/s and waiting 3.31 s/s against ~0.3 writes/s.
//!
//! ## What may be cached, and what must never be
//!
//! Keyed by `(repo, rule-set hash)` and bounded by the projection TTL. The hash
//! is what makes it safe: a policy change that could alter an exposure verdict
//! changes the rule set, so a cached verdict cannot outlive the rules it was
//! computed against. That is invalidation by MEANING, not merely by clock.
//!
//! **Only what quipu ANSWERED is stored.** [`ExposureAnswer`] separates "quipu
//! said it does not know this repo" — a stable fact, safe to cache — from "we
//! never got an answer", which is a timeout or a 502. Caching the second would
//! freeze a transient failure for the whole TTL, turning a blip into hours of
//! degraded enforcement, and it would do so during precisely the quipu trouble
//! that produced it. An unreachable answer is returned to the caller and
//! forgotten.
//!
//! Note what this does NOT change: `Unknown` still never blocks. A governed rule
//! never blocks on a guess, cached or live.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::project_exposure::{ExposureAnswer, RepoExposure};
use crate::projection_cache::CachedProjection;

/// One cached verdict, with the rule set it was computed against.
struct Entry {
    verdict: RepoExposure,
    at: u64,
    rules_hash: u64,
}

/// What the daemon reports for one repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExposureReply {
    /// `public` | `internal` | `unknown` — the decision value.
    pub verdict: String,
    /// Why, when the verdict is `unknown`. Carried so the guard's notice can
    /// still explain itself rather than saying only "unknown".
    pub reason: Option<String>,
    /// Whether this was served from the resident cache. A caller uses it for
    /// reporting only; the DECISION is identical either way.
    pub from_cache: bool,
    /// Age of the cached verdict in seconds; 0 when freshly resolved.
    pub age_secs: u64,
    /// The rule-set hash this verdict is bound to.
    pub rules_hash: String,
}

impl ExposureReply {
    /// Rebuild the decision value a caller acts on.
    #[must_use]
    pub fn exposure(&self) -> RepoExposure {
        match self.verdict.as_str() {
            "public" => RepoExposure::Public,
            "internal" => RepoExposure::Internal,
            _ => RepoExposure::Unknown(
                self.reason
                    .clone()
                    .unwrap_or_else(|| "unknown exposure".to_string()),
            ),
        }
    }
}

fn label(exposure: &RepoExposure) -> (&'static str, Option<String>) {
    match exposure {
        RepoExposure::Public => ("public", None),
        RepoExposure::Internal => ("internal", None),
        RepoExposure::Unknown(why) => ("unknown", Some(why.clone())),
    }
}

/// A content hash of the projected rule set.
///
/// Deliberately over the RULES, not over the whole projection: a projection
/// refresh that changed only a timestamp must not evict verdicts that are still
/// correct, or the cache would churn every refresh and buy nothing. What must
/// evict them is a change to the rules a verdict was computed against.
#[must_use]
pub fn rules_hash(projection: &CachedProjection) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Serialised form rather than field-by-field: the rule types are data
    // decoded from quipu and gain fields over time, and a hash that silently
    // ignored a new field would keep serving verdicts across a change it could
    // not see.
    for value in [
        serde_json::to_string(&projection.policies).unwrap_or_default(),
        serde_json::to_string(&projection.text_rules).unwrap_or_default(),
    ] {
        value.hash(&mut hasher);
    }
    hasher.finish()
}

/// Resident, TTL-bounded exposure verdicts keyed by `(repo, rule-set hash)`.
#[derive(Default)]
pub struct ExposureCache {
    entries: Mutex<HashMap<String, Entry>>,
}

impl ExposureCache {
    /// Serve `repo`'s exposure, resolving live only on a miss.
    ///
    /// `rules_hash` binds the verdict to the rule set; `ttl_secs` bounds it in
    /// time. A hit requires BOTH to still hold.
    pub fn resolve(
        &self,
        endpoint: &str,
        repo: &str,
        rules_hash: u64,
        ttl_secs: u64,
        now: u64,
    ) -> ExposureReply {
        if let Some(hit) = self.hit(repo, rules_hash, ttl_secs, now) {
            return hit;
        }
        let answer = crate::project_exposure::fetch_exposure_answer(endpoint, repo);
        let (cacheable, exposure) = match answer {
            ExposureAnswer::Answered(e) => (true, e),
            ExposureAnswer::Unreachable(why) => (false, RepoExposure::Unknown(why)),
        };
        if cacheable {
            if let Ok(mut entries) = self.entries.lock() {
                entries.insert(
                    repo.to_string(),
                    Entry {
                        verdict: exposure.clone(),
                        at: now,
                        rules_hash,
                    },
                );
            }
        }
        let (verdict, reason) = label(&exposure);
        ExposureReply {
            verdict: verdict.to_string(),
            reason,
            from_cache: false,
            age_secs: 0,
            rules_hash: format!("{rules_hash:x}"),
        }
    }

    fn hit(&self, repo: &str, rules_hash: u64, ttl_secs: u64, now: u64) -> Option<ExposureReply> {
        let entries = self.entries.lock().ok()?;
        let entry = entries.get(repo)?;
        if entry.rules_hash != rules_hash {
            return None;
        }
        let age = now.saturating_sub(entry.at);
        if age > ttl_secs {
            return None;
        }
        let (verdict, reason) = label(&entry.verdict);
        Some(ExposureReply {
            verdict: verdict.to_string(),
            reason,
            from_cache: true,
            age_secs: age,
            rules_hash: format!("{rules_hash:x}"),
        })
    }

    /// Drop every verdict. Called when the projection refreshes to a different
    /// rule set — the `(repo, rules_hash)` key would miss anyway, so this is
    /// housekeeping that stops the map growing one dead entry per repo per
    /// policy change, not a correctness step.
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }

    /// How many verdicts are held. For `/status`-style reporting and tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().map_or(0, |e| e.len())
    }

    /// Whether the cache holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Seed a verdict directly. Test-only: the eviction rules are what these
    /// tests are about, and driving them through a live `resolve` would test
    /// quipu's availability instead.
    #[cfg(test)]
    fn insert_for_test(&self, repo: &str, verdict: RepoExposure, at: u64, rules_hash: u64) {
        self.entries.lock().unwrap().insert(
            repo.to_string(),
            Entry {
                verdict,
                at,
                rules_hash,
            },
        );
    }

    /// The lookup half, without the live fallback.
    #[cfg(test)]
    fn hit_for_test(
        &self,
        repo: &str,
        rules_hash: u64,
        ttl_secs: u64,
        now: u64,
    ) -> Option<ExposureReply> {
        self.hit(repo, rules_hash, ttl_secs, now)
    }
}

#[cfg(test)]
#[path = "exposure_test.rs"]
mod exposure_test;
