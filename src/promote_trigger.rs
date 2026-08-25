//! WHEN promotion runs: `[yupana.quipu] promote_on` × the event that invoked it.
//!
//! FR-19's last unmet criterion and risk §14.3's named mitigation. `promote_on`
//! parsed, defaulted to `"merge"`, and was read by NOTHING until this module —
//! documented, settable, inert, which `src/config_test.rs`'s live-control guard
//! exists to catch ("a control that looks live and is not").
//!
//! ── WHY A DECLARED TRIGGER, NOT A GIT HOOK YUPANA INSTALLS ──────────────────
//! Yupana installs no git hooks and owns no commit/merge event of its own; the
//! `hook` subcommand is the *agent-harness* adapter (pre-edit / post-edit /
//! pre-bash), not `.git/hooks`. So the event has to come from the caller that
//! actually has one — a `post-commit` / `post-merge` hook, a CI step, the
//! scheduled promoter — and `--trigger` is how it says so. Yupana then decides
//! whether the configured policy admits that event.
//!
//! Inventing a watcher on `.git/HEAD` to synthesise the event was the other
//! option and is rejected: it would fire on rebases, checkouts and fetches as
//! well as commits, so `promote_on = "commit"` would mean something other than
//! what it says — which is the same defect as the inert key, wearing a
//! different hat.
//!
//! ── THE RULES ───────────────────────────────────────────────────────────────
//! | `promote_on` | `--trigger manual` (default) | `--trigger commit`, plain | `--trigger commit`, MERGE commit | `--trigger merge` |
//! |---|---|---|---|---|
//! | `manual`     | promote | decline | decline | decline |
//! | `commit`     | promote | promote | promote | promote |
//! | `merge`      | promote | decline | **promote** | promote |
//!
//! Two deliberate calls in that table:
//!
//! **`manual` always promotes.** `promote_on` governs AUTOMATION — "every
//! commit? only merges?" is the question §14.3 asks — not authorization. An
//! operator typing `yupana promote` has authorized it; the flag that authorizes
//! a write is `--to`, and it stays the only one. A `promote_on` that could
//! refuse a person's explicit command would be a second, weaker write guard
//! beside `serve.read_only`, which is a real one.
//!
//! **A `commit` event on a merge commit counts as a merge.** A `post-commit`
//! hook cannot know which it saw; git can, and does — a merge commit is one with
//! two or more parents. Deriving it here means the default `promote_on =
//! "merge"` works from the simplest possible hook, instead of quietly promoting
//! nothing because the caller passed the wrong word.

use crate::errors::{Error, Result};

/// The event that caused this promotion invocation, as DECLARED by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Trigger {
    /// A person or agent asked for this promotion directly. The default, so
    /// every existing caller keeps promoting exactly as before.
    #[default]
    Manual,
    /// A commit landed (e.g. a `post-commit` hook).
    Commit,
    /// A merge landed (e.g. a `post-merge` hook).
    Merge,
}

/// Whether the configured policy admits this invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Proceed with the promotion.
    Promote,
    /// Policy says not on this event. Carries the sentence to print; nothing is
    /// written and the process exits 0, because a declined trigger is the
    /// configuration WORKING, not a failure.
    Declined(String),
}

/// The `promote_on` values that mean anything.
const VALUES: &[&str] = &["commit", "merge", "manual"];

/// Decide whether a promotion invoked by `trigger` runs under `promote_on`.
///
/// `commit_is_merge` is git's answer for the commit being promoted (two or more
/// parents), used only to upgrade a `commit` event to a `merge` one.
///
/// An UNRECOGNISED `promote_on` is an error, not a fallback to the default. A
/// typo (`"merges"`, `"on-merge"`) that silently behaved as `merge` would be
/// indistinguishable from the key working, and the operator would have no way to
/// find out short of watching the graph.
pub fn decide(promote_on: &str, trigger: Trigger, commit_is_merge: bool) -> Result<Decision> {
    let policy = promote_on.trim();
    if !VALUES.contains(&policy) {
        return Err(Error::Config(format!(
            "`[yupana.quipu] promote_on = {promote_on:?}` is not a recognised value. \
             Use one of: {}. Refusing rather than falling back to the default, which \
             would make a typo indistinguishable from the key working.",
            VALUES.join(" | ")
        )));
    }
    // An explicit ask always proceeds — see the module note.
    if trigger == Trigger::Manual {
        return Ok(Decision::Promote);
    }
    let effective = if trigger == Trigger::Commit && commit_is_merge {
        Trigger::Merge
    } else {
        trigger
    };
    let admitted = match policy {
        "commit" => true,
        "merge" => effective == Trigger::Merge,
        // "manual": no automated event promotes.
        _ => false,
    };
    if admitted {
        return Ok(Decision::Promote);
    }
    let saw = match (trigger, commit_is_merge) {
        (Trigger::Commit, false) => "a commit (not a merge commit)",
        (Trigger::Commit, true) => "a commit",
        (Trigger::Merge, _) => "a merge",
        (Trigger::Manual, _) => unreachable!("manual returns above"),
    };
    Ok(Decision::Declined(format!(
        "SKIPPED — `[yupana.quipu] promote_on = \"{policy}\"` does not promote on {saw}. \
         Wrote nothing. (Run `yupana promote` without --trigger to promote anyway, or set \
         promote_on = \"commit\" to promote on every commit.)"
    )))
}

#[cfg(test)]
#[path = "promote_trigger_test.rs"]
mod promote_trigger_test;
