//! Throttle state — the soft-constraint response that acts on the NEXT action.
//!
//! A Post-Action Auditor cannot prevent the action it just watched. What it can
//! do is change what happens after it, and `throttle` is the response SARC
//! (arXiv:2605.07728 §4.2) places there for the soft class. In the paper's own
//! evaluation the declared PAA throttling response is responsible for the entire
//! 89.5% reduction in soft-window overages — not the framework's label, the
//! *placement* of a response at the point where completed-action data exists.
//!
//! ## What a throttle is here, and what it is not
//!
//! It is a **recorded, expiring advisory**: a soft window was crossed, and the
//! agent is told so on its subsequent edits until the backoff elapses. It is
//! **not** a block, and it must never become one. A soft constraint that blocks
//! is a hard constraint with a misleading name, and the class exists precisely
//! to say "this is admissible at a cost".
//!
//! That makes the honest description of this mechanism narrow: it is
//! `observed`, not `prevented`, in the enforcement gradient's terms. An agent
//! that ignores the advisory proceeds. The value is that the crossing is
//! recorded, bounded, and visible to the next action rather than dissolving
//! into a log line nobody reads — and that the operator can see, from the trace,
//! how often a window is crossed and by how much before deciding whether it
//! deserves to be hard.
//!
//! ## State, not a scheduler
//!
//! There is no timer and no sleeping. A throttle is a file with an expiry, read
//! by the next hook invocation that happens to run. If the agent stops editing,
//! nothing waits; if it edits immediately, it sees the advisory. That is the
//! only shape available to a per-edit process with no daemon, and it is
//! deliberately not described as rate limiting, which it cannot enforce.

use std::path::{Path, PathBuf};

/// Ceiling on a computed backoff, in seconds.
///
/// A declared formula is data from the graph, and an exponential one with a
/// large overage produces a number that is either meaningless or hostile. SARC's
/// own sample formula caps its exponent for the same reason. Six hours is long
/// enough to span a working session and short enough that a mistake expires
/// without an operator having to find the state file.
const MAX_BACKOFF_SECS: u64 = 6 * 60 * 60;

/// An active throttle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Throttle {
    /// The constraint whose soft window was crossed.
    pub constraint_id: String,
    /// WHERE it was crossed — the repo root the edit landed in.
    ///
    /// Throttle state is one file per user, not per checkout, so without a scope
    /// a window crossed in one repo would advise an agent editing an unrelated
    /// one. The advisory would be true about something and false about the work
    /// in front of it, which is the shape of advisory that teaches agents to
    /// ignore advisories.
    pub scope: String,
    /// Unix seconds after which this throttle no longer applies.
    pub until: u64,
}

impl Throttle {
    /// Whether this throttle still applies at `now`.
    #[must_use]
    pub fn active_at(&self, now: u64) -> bool {
        self.until > now
    }

    /// The model-facing advisory. Names the constraint, says a window was
    /// crossed, and says plainly that nothing is being blocked — an advisory
    /// that reads like a refusal teaches agents to treat refusals as advisory.
    #[must_use]
    pub fn advisory(&self, now: u64) -> String {
        let remaining = self.until.saturating_sub(now);
        format!(
            "yupana: soft constraint `{}` crossed its window on a previous edit; \
             backing off for another {remaining}s. This is advisory — nothing is \
             blocked. If the work genuinely needs to continue at this rate, it \
             does; the crossing is recorded either way.",
            self.constraint_id
        )
    }
}

/// Where throttle state lives: `$YUPANA_THROTTLE_PATH`, else
/// `$XDG_STATE_HOME/yupana/throttles.jsonl`, else the `~/.local/state` form.
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
        return Some(PathBuf::from(x).join("yupana").join("throttles.jsonl"));
    }
    home.map(|h| {
        PathBuf::from(h)
            .join(".local")
            .join("state")
            .join("yupana")
            .join("throttles.jsonl")
    })
}

fn state_path() -> Option<PathBuf> {
    resolve_path(
        std::env::var("YUPANA_THROTTLE_PATH").ok().as_deref(),
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Seconds of backoff for `overage`, per a declared `aegis:backoffFormula`.
///
/// Only one formula shape is understood today — SARC's own sample,
/// `exp(min(overage / D, C))` — and **an unrecognised formula yields `None`
/// rather than a default**. A default would be a backoff nobody declared,
/// applied under a constraint whose whole point is that its cost was declared;
/// the caller reports the unparsed formula instead, so a typo surfaces as a
/// visible gap rather than as a silently wrong number.
#[must_use]
pub fn backoff_secs(formula: &str, overage: f64) -> Option<u64> {
    let inner = formula
        .trim()
        .strip_prefix("exp(min(overage /")?
        .strip_suffix("))")?;
    let (divisor, cap) = inner.split_once(',')?;
    let divisor: f64 = divisor.trim().parse().ok()?;
    let cap: f64 = cap.trim().parse().ok()?;
    if divisor <= 0.0 {
        return None;
    }
    let exponent = (overage / divisor).min(cap);
    let secs = exponent.exp();
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((secs as u64).min(MAX_BACKOFF_SECS))
}

/// Record a throttle, expiring `secs` from `now`. Fail-silent by contract, like
/// every other recording path the hooks touch.
pub fn record(constraint_id: &str, scope: &str, secs: u64, now: u64) {
    let Some(path) = state_path() else { return };
    record_to(&path, constraint_id, scope, secs, now);
}

/// The writable core, path injected — the seam tests use.
pub fn record_to(path: &Path, constraint_id: &str, scope: &str, secs: u64, now: u64) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let line = serde_json::json!({
            "constraint_id": constraint_id,
            "scope": scope,
            "until": now.saturating_add(secs.min(MAX_BACKOFF_SECS)),
        })
        .to_string();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }));
}

/// The throttles still active at `now`.
///
/// Later records for the same constraint win, so re-crossing a window extends
/// the backoff rather than adding a second advisory about the same rule. Expired
/// entries are dropped on read rather than by a sweeper: there is no process to
/// run one, and a state file that only grows is a smaller problem than a hook
/// that has to schedule cleanup.
#[must_use]
pub fn active(text: &str, scope: &str, now: u64) -> Vec<Throttle> {
    let mut out: Vec<Throttle> = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let (Some(id), Some(until)) = (
            value.get("constraint_id").and_then(|v| v.as_str()),
            value.get("until").and_then(serde_json::Value::as_u64),
        ) else {
            continue;
        };
        // A record with no scope matches NOTHING rather than everything. The
        // permissive reading would make one unscoped line advise every repo on
        // the host, which is the failure the scope exists to prevent.
        let Some(recorded_scope) = value.get("scope").and_then(|v| v.as_str()) else {
            continue;
        };
        if recorded_scope != scope {
            continue;
        }
        match out.iter_mut().find(|t| t.constraint_id == id) {
            Some(existing) => existing.until = existing.until.max(until),
            None => out.push(Throttle {
                constraint_id: id.to_string(),
                scope: recorded_scope.to_string(),
                until,
            }),
        }
    }
    out.retain(|t| t.active_at(now));
    out.sort_by(|a, b| a.constraint_id.cmp(&b.constraint_id));
    out
}

/// The active throttles from the resolved state file, at `now`.
#[must_use]
pub fn active_now(scope: &str, now: u64) -> Vec<Throttle> {
    let Some(path) = state_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    active(&text, scope, now)
}

/// Unix seconds, or 0 if the clock is before the epoch.
#[must_use]
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
#[path = "throttle_test.rs"]
mod tests;
