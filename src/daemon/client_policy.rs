//! The daemon client's POLICY fetchers — projection and exposure.
//!
//! Split from [`super::client`] purely for the 500-line file limit, the same
//! move `project_queries` and `project_exposure` made. These two belong
//! together: both serve the governed-policy path, both are `quipu`-gated, and
//! both share the rule that an `Err` means NO USABLE DAEMON and the caller must
//! fall back to the live path — never that there is no policy.

use std::time::Duration;

use super::client::{http_get, urlencode};

/// The resident projected policy from `GET /projection` (aegis-x894x2).
///
/// This is the call that removes the guard's tail. A hook's projection cost
/// becomes a localhost read of the daemon's memory instead of its own live
/// quipu `/query` — measured p90 4584ms, p99 10055ms, max 26482ms, and 100% of
/// fail-opens, all of them hooks WAITING on a contended `/query` after the
/// shared disk cache expired.
///
/// `Err` means NO USABLE DAEMON and the caller must fall back to the live path.
/// It must never be read as "no policy": that would collapse "daemon down" into
/// "nothing to enforce", which is the cheapest possible bypass and the exact
/// fact-vs-absence bug the rest of this client is built to prevent. A daemon
/// that is up but holds no projection answers 503, which lands here too — also
/// a fallback, never a clean allow.
#[cfg(feature = "quipu")]
pub fn fetch_projection(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<super::projection::ProjectionReply, String> {
    let body = http_get(host, port, "/projection", timeout)?;
    serde_json::from_str(&body)
        .map_err(|e| format!("daemon at {host}:{port} sent an unparseable projection ({e})"))
}

/// One repo's exposure from `GET /exposure` (aegis-q4tt56).
///
/// `Err` means NO USABLE DAEMON and the caller must resolve exposure live. It
/// must never be read as "unknown exposure": that would silently downgrade
/// block-tier rules to warnings whenever the daemon is down, which is a policy
/// change wearing the costume of a transport failure.
#[cfg(feature = "quipu")]
pub fn fetch_exposure(
    host: &str,
    port: u16,
    repo: &str,
    timeout: Duration,
) -> Result<super::exposure::ExposureReply, String> {
    let body = http_get(
        host,
        port,
        &format!("/exposure?repo={}", urlencode(repo)),
        timeout,
    )?;
    serde_json::from_str(&body)
        .map_err(|e| format!("daemon at {host}:{port} sent an unparseable exposure ({e})"))
}
