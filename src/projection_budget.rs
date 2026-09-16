//! The TOTAL projection budget for one hook invocation (aegis-h9c0no).
//!
//! Split out of `project.rs` rather than living beside `http_timeout`, because
//! that file crossed this repo's 500-line hard cap when the budget was added —
//! the same reason `landing_test.rs` is its own file.

/// The ceiling on ANY single projection HTTP call. This path runs inside the
/// PRE-EDIT hook, so an unbounded call does not fail — it HANGS EVERY EDIT
/// (measured: a transiently wedged quipu held the guard for the full two
/// minutes a caller was willing to wait; only the harness's own hook timeout
/// stood between that and a frozen fleet). The live policy projection has been
/// measured above four seconds, so the former two-second ceiling made failure
/// deterministic. Ten seconds leaves room for that measured query while still
/// bounding a wedged service. Operators can tune the ceiling without rebuilding
/// via `YUPANA_PROJECTION_HTTP_TIMEOUT_SECS`.
pub(crate) fn http_timeout() -> std::time::Duration {
    const DEFAULT_SECS: u64 = 10;
    let configured = std::env::var("YUPANA_PROJECTION_HTTP_TIMEOUT_SECS").ok();
    let secs = configured
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_SECS);
    std::time::Duration::from_secs(secs)
}

/// The TOTAL projection budget for this process, as a deadline.
///
/// `http_timeout` bounds ONE call; nothing bounded their SUM, and the pre-edit
/// hook makes 7-9 of them SERIALLY (measured 2026-09-16, aegis-h9c0no: 7 with no
/// work item, 9 with one). So the hook's own ceiling was up to 9 x 10s = 90s
/// while its PARENT — the harness hook timeout — is far shorter. The comment on
/// `http_timeout` already noted that "only the harness's own hook timeout stood
/// between that and a frozen fleet"; this is that relationship made explicit
/// instead of left to chance.
///
/// WHY BEING KILLED IS THE EXPENSIVE OUTCOME, and why a shorter budget is not a
/// loss. Killed and self-stopped both degrade the guard to cache-or-fail-open —
/// identical guard outcome. They differ at the STORE: a killed process abandons
/// an in-flight read that quipu cannot cancel, and it keeps executing it for
/// nobody. Measured the same day: `yupana-hook` abandoned 26.4% of 424 reads
/// while `yupana-daemon` (same code, same store, same deadline, no parent that
/// kills it) abandoned 0.0% of 42. Stopping ourselves is strictly better than
/// being stopped.
///
/// Deliberately a process-scoped deadline, on the same pattern and for the same
/// reason as `quipu_label`: every projection query in one hook invocation shares
/// one budget, and a budget that could be reset mid-process would not be one.
static BUDGET: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// A call that would start with the budget spent is NOT STARTED, never
/// started-and-abandoned: a request we do not send cannot become a read the
/// store holds for a caller that is gone. That distinction is the whole point —
/// being killed and stopping ourselves degrade the guard identically, and
/// differ only at the store.
///
/// Open the total projection budget for this process. First-write-wins.
///
/// Called at hook entry. Other entry points (the daemon, the CLI) deliberately
/// do NOT call it: they have no parent that kills them mid-request, so bounding
/// their total would trade a correct projection for nothing.
pub fn open_budget(total: std::time::Duration) {
    let _ = BUDGET.set(std::time::Instant::now() + total);
}

/// The default total, used when a caller opens a budget without naming one.
///
/// Generous ON PURPOSE. The pathological case this closes is the 71.87s chain,
/// not ordinary slowness, and a tight total would trade orphans for a guard that
/// fails open more often — which is the worse defect of the two (a perf
/// curiosity on the store side is GUARD DISABLED on this side). The harness
/// timeout is the number that actually matters and yupana cannot see it, so the
/// emitter that owns both should pass it via
/// `YUPANA_PROJECTION_TOTAL_BUDGET_SECS`.
pub fn default_total_budget() -> std::time::Duration {
    const DEFAULT_SECS: u64 = 30;
    let secs = std::env::var("YUPANA_PROJECTION_TOTAL_BUDGET_SECS")
        .ok()
        .as_deref()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_SECS);
    std::time::Duration::from_secs(secs)
}

/// What one call may spend, given the remaining total. Pure, so it is testable
/// without a process-global deadline that one test could set for every other.
///
/// `None` means DO NOT START. That is the whole point: a request never sent
/// cannot become a read quipu holds for a caller that is gone.
pub fn budgeted(
    remaining: Option<std::time::Duration>,
    per_call: std::time::Duration,
) -> Option<std::time::Duration> {
    match remaining {
        // No budget opened — the daemon and the CLI, which have no parent that
        // kills them. Unchanged behaviour, deliberately.
        None => Some(per_call),
        Some(left) if left.is_zero() => None,
        Some(left) => Some(left.min(per_call)),
    }
}

/// What this call may spend: [`budgeted`] against the process-scoped deadline.
pub(crate) fn call_timeout() -> Option<std::time::Duration> {
    let remaining = BUDGET
        .get()
        .map(|d| d.saturating_duration_since(std::time::Instant::now()));
    budgeted(remaining, http_timeout())
}
