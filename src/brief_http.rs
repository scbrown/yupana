//! The briefing's HTTP calls to quipu, bounded by the hook budget (aegis-drywac).
//!
//! Split from `brief_sources.rs`, which is at this repo's 500-line cap.

/// POST a JSON body to a quipu endpoint, returning the parsed response.
/// Every caller treats `None` as "this section stays empty" — a briefing
/// source failing must never fail the briefing.
///
/// Bounded by the hook's TOTAL budget, not a flat per-call ceiling (aegis-drywac).
/// With `http_timeout()` here a ~12s `/context` outlived the 10s `SessionStart`
/// kill, and because the hook prints only on exit the agent got NOTHING. A call
/// that would start with the budget spent is not started, so the briefing
/// prints whatever sections finished.
pub(crate) fn post(
    endpoint: &str,
    route: &str,
    body: &serde_json::Value,
) -> Option<serde_json::Value> {
    post_within(
        endpoint,
        route,
        body,
        crate::projection_budget::call_timeout(),
    )
}

/// [`post`] with the timeout passed in, so it is testable without the
/// process-global budget. `None` means do not start.
fn post_within(
    endpoint: &str,
    route: &str,
    body: &serde_json::Value,
    timeout: Option<std::time::Duration>,
) -> Option<serde_json::Value> {
    let timeout = timeout?;
    let text = ureq::post(&format!("{}{route}", endpoint.trim_end_matches('/')))
        .timeout(timeout)
        .set("Content-Type", "application/json")
        // Without it these calls land in quipu's unattributed bucket, which is
        // how a `/context` 408 hid from the per-client accounting (aegis-h9c0no).
        .set("X-Quipu-Client", crate::quipu_label::current())
        .send_string(&body.to_string())
        .ok()?
        .into_string()
        .ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
#[path = "brief_http_test.rs"]
mod tests;
