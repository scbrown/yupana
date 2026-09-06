//! Which yupana caller a quipu request came from (`X-Quipu-Client`).
//!
//! quipu attributes request time per caller, falling back to `User-Agent` when
//! the header is absent — so every unlabelled client collapses into one bucket
//! and no instrument can split it. That is not hypothetical here: during the
//! aegis-vimo5 wedge an unlabelled bloc was the majority of load on the store
//! being diagnosed, and nothing could say whose it was.
//!
//! ## Why this is a per-PROCESS label and not a constant
//!
//! The projection read in [`crate::project::query`] has two callers that must be
//! told apart, because telling them apart IS the measurement:
//!
//! * the **pre-edit hook**, one short-lived process per edit, ~10 agents at once
//!   — the herd that made every hook issue its own `/query` once the shared disk
//!   cache expired (aegis-x894x2);
//! * the **resident daemon's** single-flight refresher, one process, one request
//!   per interval, which exists to replace that herd.
//!
//! Both call the same function. A constant here would label them identically and
//! the store-side view would show the herd collapsing into... nothing
//! distinguishable. With separate labels, `yupana-hook` falling toward zero while
//! `yupana-daemon` holds at one-per-interval is the after-measure, visible from
//! quipu's own timing rather than only from yupana's spool.
//!
//! That matters because the herd was measured from the store side first
//! (aegis-6uqni, malcolm): ureq held 4.38 s/s and waited 3.31 s/s against ~0.3
//! writes/s — self-contention from concurrent hook reads.
//!
//! ## The label set is SHARED and CAPPED
//!
//! quipu caps distinct client labels (folding the overflow into `other`), so
//! these are stable caller-KINDS, never per-agent or per-session names. A label
//! that encodes who is running loses the property that makes it aggregatable.

use std::sync::OnceLock;

/// The default: a short-lived hook process. Chosen as the default deliberately —
/// it is the overwhelmingly common case, and an unset label should name the
/// caller that generates the load, not an "unknown" bucket nobody owns.
pub const HOOK: &str = "yupana-hook";
/// The resident daemon's single-flight projection refresher.
pub const DAEMON: &str = "yupana-daemon";
/// Promotion — writes, not the read path.
pub const PROMOTE: &str = "yupana-promote";
/// The namespace prefix for a one-shot CLI invocation. The label sent is
/// `yupana-cli:<verb>` — see `cli_use::quipu_caller_kind`, which owns the verbs.
///
/// SEPARATE FROM [`HOOK`] BECAUSE CONFLATING THEM MAKES THE ONE QUESTION AN
/// OPERATOR ASKS UNANSWERABLE. Measured 2026-09-06 (aegis-tjyhh4): quipu showed
/// 2.16 s/s held under `yupana-hook` while `yupana-daemon` sat at 0/0 and
/// `use_daemon = true`, which reads as "a hook path is bypassing the daemon".
/// Measured per path, every hook path was at ZERO — plain `pre-bash` 0 requests,
/// `pre-edit` with a usable daemon 0, `post-edit` 0. The traffic was CLI:
/// `yupana status` issues 19-21 `/query` calls and `yupana refresh-projection`
/// (a twice-hourly cron) 6, and every one of them reported itself as a hook,
/// because [`HOOK`] is the process default and only the daemon called [`set`].
///
/// The label set already told the hook herd from the daemon that replaced it.
/// What it could not do was tell either from the callers that never moved, so
/// the cutover's own after-measure pointed at the wrong path.
pub const CLI_PREFIX: &str = "yupana-cli:";
/// The resident MCP server.
///
/// Not folded into [`CLI`] for the same reason [`CLI`] is not folded into
/// [`HOOK`]: `serve` is a long-lived process answering tool calls, so its load
/// has a different shape and a different owner from a one-shot command, and a
/// bucket that mixes them cannot be read.
pub const MCP: &str = "yupana-mcp";

static LABEL: OnceLock<&'static str> = OnceLock::new();

/// Declare this PROCESS's caller kind. Called once, at daemon startup; every
/// other entry point leaves the default.
///
/// Idempotent and first-write-wins rather than last: a label that could change
/// mid-process would split one caller's timing across two buckets, which is the
/// confusion this exists to remove.
pub fn set(label: &'static str) {
    let _ = LABEL.set(label);
}

/// This process's `X-Quipu-Client` value.
#[must_use]
pub fn current() -> &'static str {
    LABEL.get().copied().unwrap_or(HOOK)
}

/// A JSON POST already carrying this process's caller label.
///
/// Exists so the header contract lives in ONE place: a call site that sets
/// `Content-Type` by hand and forgets `X-Quipu-Client` is invisible until
/// someone tries to attribute load and finds an unlabelled bloc, which is the
/// failure this module was written for.
pub fn json_post(url: &str, label: &'static str) -> ureq::Request {
    ureq::post(url)
        .set("Content-Type", "application/json")
        .set("X-Quipu-Client", label)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unset must name the load-generating caller, not an "unknown" bucket: an
    /// unattributed majority is exactly the state aegis-vimo5 could not
    /// diagnose.
    #[test]
    fn the_default_is_the_hook() {
        assert_eq!(HOOK, "yupana-hook");
        // `current()` is process-global and other tests may have set it; assert
        // the DEFAULT explicitly rather than depending on test ordering.
        assert_eq!(LABEL.get().copied().unwrap_or(HOOK), current());
    }

    /// The labels are distinct, or the after-measure cannot be read at all.
    #[test]
    fn the_caller_kinds_are_distinct() {
        // Every pair, not a sample: two kinds sharing a string is exactly the
        // collision that made the aegis-tjyhh4 floor unattributable, and it
        // would be introduced by a copy-paste that a spot check would miss.
        let kinds = [HOOK, DAEMON, PROMOTE, MCP];
        for (i, a) in kinds.iter().enumerate() {
            for b in &kinds[i + 1..] {
                assert_ne!(a, b, "two caller kinds share a label");
            }
        }
    }

    /// The set is CAPPED server-side (quipu folds the overflow into `other`), so
    /// growth here is deliberate. This is not a count in prose — the assertion
    /// reads the array — but it does mean adding a kind is a decision someone
    /// takes on purpose rather than a line that slips in.
    #[test]
    fn every_kind_names_yupana_so_the_store_side_view_groups_them() {
        for k in [HOOK, DAEMON, PROMOTE, MCP] {
            assert!(
                k.starts_with("yupana-"),
                "{k} is not attributable to yupana"
            );
        }
    }

    /// Stable, aggregatable caller KINDS — never per-agent names, which would
    /// blow the store's distinct-label cap and fold everyone into `other`.
    #[test]
    fn labels_name_a_caller_kind_and_carry_no_identity() {
        for label in [HOOK, DAEMON, PROMOTE] {
            assert!(label.starts_with("yupana-"), "{label} names the tool");
            assert!(
                !label.contains(char::is_numeric),
                "{label} must not encode a session or agent"
            );
        }
    }
}
