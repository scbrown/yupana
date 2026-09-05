//! The DELEGATE-line advisory — an agent still editing after it already had
//! something to hand off (aegis-2o9eo).
//!
//! Stiwi, 2026-08-02: *"hank needs 'delegate' guardrails for us to leverage.
//! like it should be able to know that you're going much further than initial
//! investigations to create beads, which i think is where we should draw the
//! line."*
//!
//! ```text
//! investigate -> understand enough to write a GOOD bead -> DELEGATE
//!                                                       ^
//!                                                the line is HERE
//! ```
//!
//! # THE SIGNAL IS NOT DEPTH
//!
//! This is the constraint that makes the feature hard, and the bead states it
//! plainly: a guard that fires on "the agent is investigating deeply" makes us
//! worse. Deep investigation is where the value is — a four-server differential
//! that isolated a broken MCP server, a `max_over_time` reading that stopped a
//! real thermal event being closed as noise.
//!
//! The signal is depth AFTER a delegable artifact already exists. So this fires
//! only once a bead has actually been FILED in the session, and never before —
//! which means the ordinary shape (investigate, then file at the end) is silent
//! by construction, because the edits precede the bead.
//!
//! # WHY IT ONLY ADVISES, AND ONLY ONCE
//!
//! Advise tier per the aegis-mqnl ladder: no blocking until a false-positive
//! rate is measured. And once per session, because the failure being addressed
//! is a judgement call made repeatedly under momentum; a line repeated on every
//! edit is one the reader learns to skip, which would cost more than it saves.

use super::HookInput;

/// The session-event kind recorded when a bead is filed.
pub(super) const BEAD_FILED: &str = "delegate-bead-filed";

/// The session-event kind recorded when the advisory has spoken.
const ADVISED: &str = "delegate-advised";

/// Whether a shell command files a bead.
///
/// Deliberately narrow. It matches the store CLIs' `create` verb and nothing
/// else: `comments add`, `close` and `update` are ordinary traffic on a bead
/// that already exists, and treating them as "you produced something delegable"
/// would fire this on the act of writing the bead up properly — the behaviour
/// we want, flagged as the behaviour we don't.
pub(super) fn files_a_bead(command: &str) -> bool {
    // A line may chain several commands; judge each independently so that
    // `cd x && br create y` is seen, and `echo br create` is not mistaken for
    // an invocation in the segment that actually runs.
    command
        .split(['\n', ';'])
        .flat_map(|line| line.split("&&"))
        .flat_map(|line| line.split('|'))
        .any(segment_files_a_bead)
}

fn segment_files_a_bead(segment: &str) -> bool {
    let mut tokens = segment.split_whitespace().peekable();
    // The first bare word is the program. Anything else (echo, git, printf)
    // means this segment is not a store invocation, however it reads.
    let Some(program) = tokens.next() else {
        return false;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    if program != "br" && program != "bd" {
        return false;
    }
    // Skip flags AND THEIR VALUES to reach the verb. `--db <path> create` and
    // `create` must resolve alike, or the guard is silent for anyone who works
    // with an explicit store — which on this fleet is everyone, since the guard
    // rules require an explicit `--db` against the crew store.
    //
    // Skipping the flag alone is not enough and that is not a hypothetical: the
    // first version of this did exactly that, and its own test caught
    // `br --db /path/beads.db create` resolving the PATH as the verb.
    //
    // KNOWN LIMIT, stated rather than papered over: a value-less long flag
    // before the verb (`br --json create`) would swallow `create` as its value
    // and read as silence. These CLIs take no such flag before the verb today,
    // and the failure direction is silence — a missed advisory, not a false
    // one — which is the right way round for an advisory that must not cry wolf.
    let mut expect_value = false;
    for token in tokens {
        if expect_value {
            expect_value = false;
            continue;
        }
        if let Some(flag) = token.strip_prefix('-') {
            // `--key=value` carries its own value; a bare flag takes the next.
            expect_value = !flag.contains('=');
            continue;
        }
        return token == "create";
    }
    false
}

/// Record that this session produced a delegable artifact, if the command did.
pub(super) fn note_command(session: Option<&str>, command: &str) {
    if files_a_bead(command) {
        super::record_session_event(session, BEAD_FILED);
    }
}

/// The advisory for an edit, or `None` for silence.
///
/// Silent unless a bead was filed EARLIER in this session, and silent after it
/// has spoken once.
pub(super) fn advisory(input_json: &str) -> Option<String> {
    let input = HookInput::parse(input_json)?;
    let session = input.session_id.as_deref();
    if !super::session_event_recorded(session, BEAD_FILED) {
        return None;
    }
    if !super::first_notice_for_session(session, ADVISED) {
        return None;
    }
    Some(
        "yupana delegate line (advisory, once per session): you filed a bead \
         earlier in this session and are still editing. Investigating far enough \
         to write a GOOD bead is the job; implementing what the bead describes \
         may be work that belongs to its owner. Check that these edits are on \
         YOUR tracked item — if they are, carry on; if they are the bead you \
         just filed, hand it off instead (aegis-2o9eo)."
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(session: &str) -> String {
        serde_json::json!({
            "session_id": session,
            "tool_name": "Edit",
            "tool_input": {"file_path": "/repo/a.rs", "old_string": "a", "new_string": "b"},
        })
        .to_string()
    }

    #[test]
    fn the_store_create_verb_is_recognised_through_flags_and_chaining() {
        assert!(files_a_bead("br create \"a title\""));
        assert!(files_a_bead("bd create \"a title\" -p 2"));
        // The form every agent on this fleet actually types.
        assert!(files_a_bead("br --db /path/to/beads.db create \"a title\""));
        assert!(files_a_bead("cd /repo && br create \"a title\""));
        assert!(files_a_bead("/home/x/.local/bin/br create \"a title\""));
        // `--key=value` carries its own value, so the NEXT token is the verb.
        assert!(files_a_bead("br --db=/path/beads.db create \"a title\""));
    }

    #[test]
    fn a_flag_value_is_not_mistaken_for_the_verb() {
        // The bug this file's own test caught: skipping the flag but not its
        // value resolved the PATH as the verb, and the guard went silent for
        // every invocation that names its store — which on this fleet is all
        // of them.
        assert!(files_a_bead("br --db /path/to/beads.db create \"t\""));
        assert!(!files_a_bead("br --db /path/to/create list"));
    }

    #[test]
    fn ordinary_traffic_on_an_existing_bead_is_not_filing_one() {
        // Writing the bead up properly must not trip the guard against writing
        // beads up properly.
        assert!(!files_a_bead("br comments add aegis-x --file /tmp/m.md"));
        assert!(!files_a_bead("br close aegis-x --reason \"done\""));
        assert!(!files_a_bead("br update aegis-x --status=in_progress"));
        assert!(!files_a_bead("br list --limit 0"));
        assert!(!files_a_bead("br show aegis-x"));
    }

    #[test]
    fn a_mention_is_not_an_invocation() {
        assert!(!files_a_bead("echo 'run br create to file one'"));
        assert!(!files_a_bead("git commit -m \"br create\""));
        assert!(!files_a_bead("grep -r 'br create' ."));
        assert!(!files_a_bead(""));
    }

    #[test]
    fn silent_when_no_bead_was_filed_this_session() {
        // THE CASE THE BEAD SAYS MUST STAY SILENT: deep investigation that has
        // not yet produced a delegable artifact.
        let session = super::super::unique_test_session("delegate-none");
        assert_eq!(advisory(&payload(&session)), None);
    }

    #[test]
    fn advises_once_after_a_bead_is_filed_and_then_stays_quiet() {
        let session = super::super::unique_test_session("delegate-filed");
        note_command(Some(&session), "br create \"something worth delegating\"");

        let first = advisory(&payload(&session)).expect("must advise after a bead is filed");
        assert!(first.contains("delegate line"));
        // Once, not on every edit: a line repeated on each edit is one the
        // reader learns to skip.
        assert_eq!(advisory(&payload(&session)), None);
    }

    #[test]
    fn a_non_filing_command_does_not_arm_the_advisory() {
        let session = super::super::unique_test_session("delegate-close");
        note_command(Some(&session), "br close aegis-x --reason \"done\"");
        assert_eq!(advisory(&payload(&session)), None);
    }

    #[test]
    fn no_session_id_is_silence_not_an_advisory() {
        // Without a session there is no trajectory to reason about, and
        // advising anyway would fire on every edit of every unkeyed harness.
        let unkeyed = serde_json::json!({
            "tool_name": "Edit",
            "tool_input": {"file_path": "/repo/a.rs"},
        })
        .to_string();
        assert_eq!(advisory(&unkeyed), None);
    }
}
