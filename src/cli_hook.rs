//! The `yupana hook` event surface — split from `cli.rs` for file size when
//! the `session-start` event landed. The enum is the CLI contract; dispatch
//! stays a thin adapter over `crate::hook`.

use clap::ValueEnum;

/// Supported agent-harness hook events.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum HookEvent {
    /// Claude Code `PostToolUse` on Edit/Write: advise on cross-file blast radius.
    PostEdit,
    /// Claude Code `PreToolUse` on Edit/Write: deny an edit that exceeds the
    /// tenant's capability scope. Opt-in, and always fails open.
    PreEdit,
    /// Claude Code `PostToolUse` / `PostToolUseFailure` on Bash: record the
    /// action's OUTCOME, joined to its `pre-bash` record by the harness's
    /// `tool_use_id`. Wire it on BOTH events — the event name IS the outcome.
    /// Never denies, never prints (aegis-368cu.10).
    PostBash,
    /// Claude Code `PreToolUse` on Bash: record the action and evaluate governed
    /// command policies. Advises first; denial requires enforce mode.
    PreBash,
    /// Claude Code `SessionStart`: print the work-item briefing — the graph's
    /// knowledge of the tracked item's ground, related work, and governed
    /// rules — so it reaches the agent BEFORE its first edit, not at the
    /// first denial (quipu feature; silent without it).
    SessionStart,
}

/// Dispatch a hook event.
pub(crate) fn run(
    event: HookEvent,
    tenant: Option<&str>,
    config: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    match event {
        HookEvent::PostEdit => crate::hook::run_post_edit(tenant),
        HookEvent::PostBash => crate::hook::run_post_bash(),
        HookEvent::PreBash => crate::hook::run_pre_bash(),
        HookEvent::PreEdit => crate::hook::run_pre_edit(tenant, config),
        #[cfg(feature = "quipu")]
        HookEvent::SessionStart => crate::hook::run_session_start(config),
        // Without the projection there is no briefing to print; stay silent
        // and exit 0 — a session must open whatever this build can say.
        #[cfg(not(feature = "quipu"))]
        HookEvent::SessionStart => Ok(()),
    }
}
