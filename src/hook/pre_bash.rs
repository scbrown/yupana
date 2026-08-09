//! `pre_bash` — the input path the action resolver never had.
//!
//! `crate::action::resolve` was written for gap 2 of the trace phase and shipped
//! **unreachable**: yupana supported only `PreEdit`/`PostEdit`, the only live
//! wiring was `post-edit` on `Write|Edit`, and the fleet's Bash guard chain
//! referenced yupana zero times. So a resolver with tests and documentation sat in
//! the tree with no caller and no way to acquire a command line — present,
//! plausible, and structurally incapable of running.
//!
//! That is precisely the failure class this epic exists to prevent (a control
//! that cannot fail, believed to be working), occurring inside the epic's own
//! first phase. This module is the correction: a hook event that receives the
//! harness's Bash payload and feeds it to the resolver.
//!
//! TWO-SIDED LIVENESS (aegis-tv9ri). Every invocation emits `pre_bash_invoked`
//! BEFORE the payload is inspected, so the trace can tell apart the two ways
//! this hook goes quiet — which need opposite fixes and used to look identical:
//!
//! ```text
//! {"kind":"pre_bash_invoked","parsed":true,"payload_bytes":123,…}
//! {"kind":"pre_bash_invoked","parsed":false,"payload_bytes":123,
//!  "payload_keys":"session_id,tool_name,…"}      <- ran, shape unrecognised
//! ```
//!
//! No `pre_bash_invoked` at all across known Bash traffic means the hook is not
//! wired into the settings THAT SESSION loads — which is a settings-scope
//! question, not a yupana one.
//!
//! RECORD-ONLY. This never denies, never warns, and prints NOTHING — not even
//! on a resolved dangerous-looking command. Enforcement on the action path is a
//! later phase gated on evals, and a hook that started advising here would be
//! enforcement arriving without the gate. It also shares the Bash matcher with
//! the existing guard chain, so anything printed would interleave with a guard
//! whose refusals are load-bearing.
//!
//! ALWAYS EXIT 0. A bookkeeping hook that can fail a command is worse than no
//! bookkeeping: it converts an observability feature into an outage. Every path
//! here returns Ok, and the emit itself is fail-silent by construction.
//!
//! WHAT IS RECORDED, and why `target_class` is present even when unknown:
//!
//! ```text
//! {"kind":"action","target_class":"host","verb":"ssh","target":"build-01",
//!  "agent":"…","tenant":"…","item":"…","ts":…}
//! ```
//!
//! `verb` and `target` are OMITTED when the resolver abstained — an absent field
//! is honestly silent, whereas `""` or `"unknown"` would be replayed later as if
//! it were a value. But `target_class` is ALWAYS written, including
//! `"unknown"`, because the replay in the next phase needs to divide resolved
//! actions by TOTAL actions. Without a row for the abstentions the denominator
//! is invisible and a resolver covering 5% of traffic looks identical to one
//! covering 95%.

use std::io::Read;

use crate::action;

/// Handle a Claude Code `PreToolUse` payload for the Bash tool.
///
/// Reads the payload on stdin, resolves the command to (verb, target,
/// `target_class`), and records it. Errors are swallowed: an unparseable or
/// unexpected payload records nothing and still succeeds.
pub fn run_pre_bash() -> anyhow::Result<()> {
    let mut buf = String::new();
    // A read failure is not an error worth surfacing: the command the operator
    // asked for must run either way.
    std::io::stdin().lock().read_to_string(&mut buf).ok();

    let cmd = command_of(&buf);
    record_invocation(&buf, cmd.is_some());

    if let Some(cmd) = cmd {
        record(&action::resolve(&cmd));
    }
    Ok(())
}

/// Emit ONE record per invocation, unconditionally, before anything can
/// suppress it. This is what makes the trace two-sided (aegis-tv9ri).
///
/// The `action` record only appears when the payload parsed, so its ABSENCE was
/// ambiguous in exactly the way that matters: "the hook never ran" and "the hook
/// ran and did not recognise the payload" produced identical evidence — an
/// empty trace — and they need opposite fixes. Measured 2026-08-04: 22 action
/// records total, 21 of them from one 62-second verification burst on 08-02,
/// while ordinary agent Bash traffic produced none for two days. Nothing in the
/// trace could say which failure that was.
///
/// So: `pre_bash_invoked` present with no `action` means the payload shape is
/// wrong and this hook is running; `pre_bash_invoked` absent across known Bash
/// traffic means the hook is not wired into the settings that session loads.
///
/// `payload_keys` is recorded ONLY on the failure path, and only the top-level
/// KEY NAMES — never values. It is the field that names an unexpected payload
/// shape without putting command text into a record whose whole purpose is to
/// be readable by someone debugging a hook. `payload_bytes` separates "stdin was
/// empty" from "stdin was a payload we did not understand"; both are silent
/// no-ops today and they are not the same bug.
fn record_invocation(payload: &str, parsed: bool) {
    crate::metrics::emit("pre_bash_invoked", &invocation_fields(payload, parsed));
}

/// The fields of a `pre_bash_invoked` record. Pure, so the contract is testable
/// without touching the process environment or the real spool — the same reason
/// [`crate::metrics::resolve_path`] is pure (parallel tests race on env vars).
#[must_use]
pub fn invocation_fields(payload: &str, parsed: bool) -> Vec<(&'static str, serde_json::Value)> {
    let mut fields: Vec<(&'static str, serde_json::Value)> = vec![
        ("parsed", parsed.into()),
        ("payload_bytes", payload.len().into()),
    ];
    if !parsed && !payload.is_empty() {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(payload) {
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            fields.push(("payload_keys", keys.join(",").into()));
        }
    }
    fields
}

/// Pull `tool_input.command` out of a harness payload.
///
/// Returns `None` for anything that is not a Bash-shaped payload, which
/// includes the case where this hook is wired to a matcher that also delivers
/// other tools: recording a non-command as a command would put fiction in the
/// trace, and the trace's only value is that it is evidence.
#[must_use]
pub fn command_of(payload: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let cmd = v.get("tool_input")?.get("command")?.as_str()?;
    if cmd.trim().is_empty() {
        return None;
    }
    Some(cmd.to_string())
}

/// Emit one `action` record. Fail-silent via the spool's own contract.
fn record(a: &action::Action) {
    let mut fields: Vec<(&str, serde_json::Value)> =
        vec![("target_class", a.target_class.as_str().into())];
    if let Some(v) = &a.verb {
        fields.push(("verb", v.clone().into()));
    }
    if let Some(t) = &a.target {
        fields.push(("target", t.clone().into()));
    }
    crate::metrics::emit("action", &fields);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_bash_command() {
        let p = r#"{"tool_name":"Bash","tool_input":{"command":"ssh build-01 uptime"}}"#;
        assert_eq!(command_of(p).as_deref(), Some("ssh build-01 uptime"));
    }

    #[test]
    fn non_bash_payloads_yield_nothing() {
        // Both directions: the positive above proves the extractor works, so
        // these Nones mean "correctly declined", not "extractor is broken".
        assert_eq!(
            command_of(r#"{"tool_input":{"file_path":"/a/b.rs"}}"#),
            None
        );
        assert_eq!(command_of(r#"{"tool_name":"Bash"}"#), None);
        assert_eq!(command_of("{not json"), None);
        assert_eq!(command_of(""), None);
    }

    #[test]
    fn a_blank_command_is_not_a_command() {
        assert_eq!(command_of(r#"{"tool_input":{"command":"   "}}"#), None);
    }

    #[test]
    fn resolution_reaches_the_resolver() {
        // The point of this module is that the resolver becomes REACHABLE.
        // Asserting payload -> resolve end to end is the test that would have
        // failed for as long as action.rs had no caller.
        //
        // The host carries an explicit user@, because the resolver deliberately
        // refuses a bare single-word operand as too weak to claim. My first
        // version of this test asserted that `ssh build-01 uptime` resolves and
        // it FAILED — correctly. The expectation was wrong, not the resolver.
        let cmd = command_of(r#"{"tool_input":{"command":"ssh deploy@build-01 uptime"}}"#).unwrap();
        let a = action::resolve(&cmd);
        assert_eq!(a.verb.as_deref(), Some("ssh"));
        assert_eq!(a.target.as_deref(), Some("build-01"));
        assert_eq!(a.target_class.as_str(), "host");
    }

    #[test]
    fn a_bare_host_operand_is_deliberately_refused() {
        // The abstain rule, asserted so a later "improvement" that loosens it
        // has to argue with a test instead of a comment.
        let a = action::resolve("ssh build-01 uptime");
        assert!(a.verb.is_none());
        assert_eq!(a.target_class.as_str(), "unknown");
    }

    #[test]
    fn an_unresolvable_command_still_carries_a_class() {
        // The denominator case: abstentions must remain countable.
        let a = action::resolve("frobnicate --wibble");
        assert_eq!(a.target_class.as_str(), "unknown");
        assert!(a.verb.is_none());
    }
}

#[cfg(test)]
mod liveness_tests {
    use super::*;

    fn field<'a>(f: &'a [(&str, serde_json::Value)], k: &str) -> Option<&'a serde_json::Value> {
        f.iter().find(|(n, _)| *n == k).map(|(_, v)| v)
    }

    /// The whole point: a record exists even when nothing was resolvable. Its
    /// absence must mean "did not run", and that is only true if EVERY
    /// invocation writes one.
    #[test]
    fn an_unrecognised_payload_still_records_that_the_hook_ran() {
        let p = r#"{"session_id":"abc","tool_name":"Bash","params":{"cmd":"ls"}}"#;
        let f = invocation_fields(p, false);
        assert_eq!(field(&f, "parsed"), Some(&serde_json::Value::Bool(false)));
        assert_eq!(
            field(&f, "payload_keys").and_then(serde_json::Value::as_str),
            Some("params,session_id,tool_name"),
            "an unrecognised shape must NAME itself, or the next reader repeats this diagnosis"
        );
    }

    /// Key NAMES only. This record is read by someone debugging a hook; it must
    /// not become a second copy of every command line.
    #[test]
    fn a_failed_parse_records_key_names_never_values() {
        let p = r#"{"tool_input":{"command":"curl -H 'Authorization: Bearer hunter2' x"}}"#;
        let f = invocation_fields(p, false);
        let joined = format!("{f:?}");
        assert!(!joined.contains("hunter2"), "leaked a value: {joined}");
        assert!(
            !joined.contains("Authorization"),
            "leaked a value: {joined}"
        );
        assert!(
            joined.contains("tool_input"),
            "must still name the key: {joined}"
        );
    }

    /// Empty stdin and an unparseable payload are different bugs — both silent
    /// no-ops today — so the record has to separate them.
    #[test]
    fn empty_stdin_is_distinguishable_from_an_unknown_shape() {
        let empty = invocation_fields("", false);
        assert_eq!(
            field(&empty, "payload_bytes"),
            Some(&serde_json::Value::from(0))
        );
        assert!(
            field(&empty, "payload_keys").is_none(),
            "there are no keys in an empty payload; inventing one would be fiction"
        );

        let unknown = invocation_fields(r#"{"a":1}"#, false);
        assert_ne!(
            field(&unknown, "payload_bytes"),
            Some(&serde_json::Value::from(0))
        );
        assert!(field(&unknown, "payload_keys").is_some());
    }

    /// On the success path the keys field is pointless noise — the action
    /// record already carries the resolution.
    #[test]
    fn a_parsed_payload_records_no_key_list() {
        let p = r#"{"tool_input":{"command":"ssh build-01 uptime"}}"#;
        let f = invocation_fields(p, true);
        assert_eq!(field(&f, "parsed"), Some(&serde_json::Value::Bool(true)));
        assert!(field(&f, "payload_keys").is_none());
    }

    /// Non-JSON stdin must not panic and must still record the invocation.
    #[test]
    fn garbage_stdin_still_records_and_does_not_panic() {
        let f = invocation_fields("not json at all", false);
        assert_eq!(field(&f, "parsed"), Some(&serde_json::Value::Bool(false)));
        assert!(field(&f, "payload_keys").is_none());
    }
}
