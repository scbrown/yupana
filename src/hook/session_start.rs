//! The `SessionStart` hook adapter — context BEFORE the first mistake.
//!
//! Reads the harness payload on stdin, resolves the tracked work item, and
//! prints the work-item briefing (`crate::brief`) in the `SessionStart`
//! envelope, so the graph's knowledge of the item's ground reaches the agent
//! at assignment time rather than at the first denied edit. Silent (empty
//! stdout, exit 0) whenever there is nothing honest to say: no plate, no
//! quipu seam, or an unparseable payload.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::HookInput;
use crate::config::YupanaConfig;

/// Run the `session-start` hook: gather, render, wrap, print. Always `Ok` —
/// a briefing failure must never fail a session open.
pub fn run_session_start(config_override: Option<&Path>) -> anyhow::Result<()> {
    let mut buf = String::new();
    std::io::stdin().lock().read_to_string(&mut buf).ok();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = HookInput::parse(&buf).map_or_else(|| cwd.clone(), |input| input.root(&cwd));
    let Ok(config) = YupanaConfig::resolve(config_override, &root) else {
        return Ok(());
    };
    if let Some(brief) = crate::brief::gather(&config, &root) {
        println!(
            "{}",
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "SessionStart",
                    "additionalContext": crate::brief::render(&brief),
                }
            })
        );
    }
    Ok(())
}
