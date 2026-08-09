//! The attribution tuple `α` — who is answerable for an action.
//!
//! SARC (arXiv:2605.07728) §9.6 wants every trace record to carry
//! `α = ⟨P, planner, executor, tool, auth, C_eval⟩`, because the two multi-agent
//! failure modes of §9.5 are both *attribution* failures before they are
//! enforcement failures:
//!
//! - **attribution dilution** — an orchestrator dispatches, a worker acts, and
//!   the record names only one of them, so no auditor can say which link was
//!   answerable;
//! - **authority escalation via tool capability** — a sub-agent whose own
//!   credentials are broader than its caller's uses them, which is only
//!   *detectable* if the record says what the whole chain was.
//!
//! ## What yupana can honestly supply, and what it deliberately will not
//!
//! `C_eval` is the `constraints` array [`crate::trace`] already emits, and
//! `tool` comes straight off the hook payload. The rest turns on one question:
//! is there a declared chain, or is yupana being asked to invent one?
//!
//! - **`P`, the principal-and-agent chain** — read from `YUPANA_PRINCIPAL_CHAIN`,
//!   a comma-separated list ordered caller-first. A dispatcher that spawns a
//!   sub-agent appends itself and exports the extended chain; the sub-agent's
//!   hook then records the real chain rather than its own leaf identity.
//!   **Absent when undeclared.** Filling it with `[$SHANTY_AGENT]` would assert
//!   "this action had exactly one principal" to precisely the auditor the field
//!   exists for, and an undeclared chain is not a one-link chain.
//! - **`planner`** — read from `YUPANA_PLANNER`, absent when unset. It is *not*
//!   derived from the chain's head: which link deliberated and which executed is
//!   a fact about the dispatch, and reading it off list position would be an
//!   inference wearing a record's clothes.
//! - **`executor`** — `$SHANTY_AGENT`, the identity of the process that actually
//!   ran. Ground truth rather than a claim, which is what makes the conflict
//!   check below possible.
//! - **`auth`** — **not recorded here, on purpose.** The effective authority is
//!   the intersection of every link's grant, those grants live in quipu
//!   (`governance::authority`), and yupana cannot read them inside a 100 ms
//!   pre-edit budget. Recording `P` is what lets quipu's checker *derive* `auth`
//!   at audit time from the authoritative source; recording a locally-guessed
//!   `auth` would put a number in the field that the grant store never agreed
//!   to. The tuple is completed by the checker, not faked by the hook.
//!
//! ## The conflict flag
//!
//! `YUPANA_PRINCIPAL_CHAIN` is a declaration; `$SHANTY_AGENT` is what is running.
//! When a chain is declared and its last link disagrees with the executor, the
//! record says so (`attribution_conflict`) rather than silently preferring one.
//! That disagreement is the observable signature of a laundered chain — an agent
//! acting under a dispatch record that names somebody else — and a record that
//! resolved it by precedence would delete the only evidence of it.

use serde_json::Value;

/// The environment variable carrying the declared principal chain.
pub const CHAIN_ENV: &str = "YUPANA_PRINCIPAL_CHAIN";
/// The environment variable naming the planner, when the dispatcher declares one.
pub const PLANNER_ENV: &str = "YUPANA_PLANNER";

/// The attribution tuple, as far as yupana can honestly fill it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Attribution {
    /// `P`, caller-first. Empty when undeclared — *not* a one-element chain.
    pub chain: Vec<String>,
    /// The agent that decided the action, when declared.
    pub planner: Option<String>,
    /// The identity of the process that ran it.
    pub executor: Option<String>,
    /// The tool it ran through.
    pub tool: Option<String>,
    /// A declared chain whose tail is not the executor. See the module doc.
    pub conflict: bool,
}

/// Split a declared chain. Comma-separated, caller-first; blanks dropped so a
/// trailing comma cannot mint an anonymous link.
#[must_use]
pub fn parse_chain(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

impl Attribution {
    /// Compose the tuple from its four sources. Pure, so the precedence and the
    /// conflict rule are testable without touching the process environment.
    #[must_use]
    pub fn compose(
        chain_raw: Option<&str>,
        planner: Option<&str>,
        executor: Option<&str>,
        tool: Option<&str>,
    ) -> Self {
        let chain = chain_raw.map(parse_chain).unwrap_or_default();
        // Only a DECLARED chain can conflict. Without one there is nothing to
        // disagree with, and flagging every unattributed edit would bury the
        // real signal under the ordinary case.
        let conflict = match (chain.last(), executor) {
            (Some(tail), Some(exec)) => tail != exec,
            _ => false,
        };
        Self {
            chain,
            planner: planner.map(str::to_string),
            executor: executor.map(str::to_string),
            tool: tool.map(str::to_string),
            conflict,
        }
    }

    /// Read the tuple from the environment and the hook payload's tool name.
    #[must_use]
    pub fn capture(tool: Option<&str>) -> Self {
        Self::compose(
            std::env::var(CHAIN_ENV).ok().as_deref(),
            std::env::var(PLANNER_ENV).ok().as_deref(),
            std::env::var("SHANTY_AGENT").ok().as_deref(),
            tool,
        )
    }

    /// Whether anything is worth recording. An all-absent tuple emits nothing
    /// rather than a row of nulls: a reader must never have to tell "recorded as
    /// empty" from "not recorded".
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chain.is_empty()
            && self.planner.is_none()
            && self.executor.is_none()
            && self.tool.is_none()
    }

    /// The trace-record fields for this tuple. Omits every absent element.
    #[must_use]
    pub fn fields(&self) -> Vec<(&'static str, Value)> {
        let mut fields: Vec<(&'static str, Value)> = Vec::new();
        if !self.chain.is_empty() {
            fields.push(("principal_chain", Value::from(self.chain.clone())));
        }
        for (key, value) in [
            ("planner", self.planner.as_ref()),
            ("executor", self.executor.as_ref()),
            ("tool", self.tool.as_ref()),
        ] {
            if let Some(v) = value {
                fields.push((key, Value::from(v.clone())));
            }
        }
        // Emitted only when true. A `false` on every line would train a reader
        // to skip the field, which is the opposite of what it is for.
        if self.conflict {
            fields.push(("attribution_conflict", Value::Bool(true)));
        }
        fields
    }
}

#[cfg(test)]
#[path = "attribution_test.rs"]
mod tests;
