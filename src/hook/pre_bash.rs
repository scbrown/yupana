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
//! RECORD-ONLY BY DEFAULT, and that default is the contract. This prints
//! NOTHING unless a deployment has explicitly set `[yupana.policy]
//! action_scope`, which is `off` out of the box. Enforcement on the action path
//! was always meant to be a later phase gated on evidence, and a hook that
//! started advising the moment the code landed would be enforcement arriving
//! without its gate.
//!
//! When it IS armed, the source is the DECLARED scope's `allow_targets` /
//! `deny_targets` and nothing else. There is no observed rung here: nothing in
//! the graph records which hosts an item's prior work touched, so there is no
//! record to infer from — and `declared` is the one provenance the trust ladder
//! permits to hard-deny anyway.
//!
//! ABSTENTIONS ARE NEVER VIOLATIONS. `crate::action` answers `Unknown` for
//! every command whose target is not unambiguous from syntax, and those reach
//! the scope check as "no check performed". A guard that refused what it could
//! not identify would refuse most of the shell — which is the same reason the
//! resolver's recognised set is deliberately small.
//!
//! It shares the Bash matcher with the deployment's guard chain, so it stays
//! silent on every allow: anything printed would interleave with a guard whose
//! refusals are load-bearing.
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

use super::pre_bash_grounding::action_fields;
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
        let input = crate::hook::HookInput::parse(&buf);
        let grounding = input
            .as_ref()
            .and_then(|i| i.grounding.as_ref())
            .map(|reference| {
                let state = crate::turn_grounding::assess(
                    Some(reference),
                    crate::turn_grounding::cache_dir().as_deref(),
                    crate::turn_grounding::now_secs(),
                    crate::turn_grounding::max_age_secs(),
                );
                (reference, state)
            });
        let resolved = action::resolve(&cmd);
        record(&resolved, grounding);
        // The scope check rides AFTER the record, deliberately: the trace is
        // the product and must not depend on a policy decision succeeding.
        if let Some(text) = scope_refusal(&buf, &resolved) {
            println!("{}", super::deny_envelope(&text));
        }
    }
    Ok(())
}

/// The action-scope verdict for a resolved command, or `None` for silence.
///
/// Returns the DENY text only at `enforce`. At `advise` it emits the record and
/// a stderr line and returns `None` — the same staging the path rung uses, and
/// for the same reason: a deployment arms the boundary once it has seen what it
/// would have refused.
///
/// Every failure path here is silence. No config, no tenant, an unresolvable
/// target, or a scope with no target globs all mean "no check performed", which
/// is not the same as "permitted" and is recorded as neither.
fn scope_refusal(payload: &str, resolved: &action::Action) -> Option<String> {
    // An abstention is not a target. See the module note: `Unknown` must never
    // reach a glob, or every pipeline on the host becomes a violation.
    if resolved.target_class == action::TargetClass::Unknown {
        return None;
    }
    let target = resolved.target.as_deref()?;

    let input = super::HookInput::parse(payload)?;
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let root = input.root(&root);
    let config = crate::config::YupanaConfig::resolve(None, &root).ok()?;

    let rung = if config.policy.action_scope.is_lower_than(config.policy.mode) {
        config.policy.action_scope
    } else {
        config.policy.mode
    };
    if rung == crate::policy::Mode::Off {
        return None;
    }

    // Tenant resolution matches the edit guard's: the explicit identity the
    // launcher exports. No tenant means no declared scope to check against.
    let tenant = std::env::var("BOBBIN_ROLE").ok()?;
    let scope = config.policy.scopes.get(&tenant)?;
    let violation = scope.check_target(resolved.target_class.as_str(), target, &tenant)?;

    let denying = rung == crate::policy::Mode::Enforce;
    crate::metrics::emit(
        "action_scope",
        &[
            ("class", resolved.target_class.as_str().into()),
            ("target", target.to_string().into()),
            ("rule", violation.rule.clone().into()),
            ("result", if denying { "deny" } else { "advise" }.into()),
        ],
    );
    if !denying {
        eprintln!(
            "yupana: {} (advisory: action_scope is not \"enforce\")",
            violation.message
        );
        return None;
    }
    Some(violation.message)
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

fn record(
    a: &action::Action,
    grounding: Option<(
        &crate::turn_grounding::GroundingRef,
        crate::turn_grounding::GroundingState,
    )>,
) {
    crate::metrics::emit("action", &action_fields(a, grounding));
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

    #[test]
    fn action_trace_binds_the_known_grounding_answer() {
        let reference = crate::turn_grounding::GroundingRef {
            scope: Some("na".into()),
            grounding_id: Some(format!("sha256:{}", "a".repeat(64))),
            faction_id: Some("raptors".into()),
            worldview_sha256: Some("sha256:worldview".into()),
        };
        let action = action::resolve("ssh deploy@build-01 uptime");
        let fields = action_fields(
            &action,
            Some((&reference, crate::turn_grounding::GroundingState::Used)),
        );
        let field = |name| fields.iter().find(|(key, _)| *key == name).map(|(_, v)| v);
        assert_eq!(
            field("grounding_outcome").and_then(serde_json::Value::as_str),
            Some("used")
        );
        assert_eq!(field("constraints").unwrap()[0]["outcome"], "satisfied");
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

#[cfg(test)]
// Test names shout the invariant they turn on, the repo's emphasis convention.
#[allow(non_snake_case)]
mod scope_tests {
    use super::*;
    use crate::policy::Scope;

    fn scope(allow: &[&str], deny: &[&str]) -> Scope {
        Scope {
            allow_targets: allow.iter().map(|s| (*s).to_string()).collect(),
            deny_targets: deny.iter().map(|s| (*s).to_string()).collect(),
            ..Scope::default()
        }
    }

    /// RED. A target outside the declared scope is a violation, and the message
    /// names both the subject and what would satisfy it — a refusal that does
    /// not say the right move strands whoever reads it.
    #[test]
    fn a_target_outside_the_scope_is_refused() {
        let v = scope(&["host:build-*"], &[])
            .check_target("host", "prod-01", "polecat")
            .expect("out-of-scope target must violate");
        assert!(v.message.contains("host:prod-01"), "{}", v.message);
        assert!(v.message.contains("build-*"), "{}", v.message);
        assert_eq!(v.rule, "allow_targets");
    }

    /// GREEN, and the control. Without it the assertion above would pass
    /// against a scope that refused everything.
    #[test]
    fn a_target_inside_the_scope_is_silent() {
        assert!(scope(&["host:build-*"], &[])
            .check_target("host", "build-01", "polecat")
            .is_none());
    }

    /// deny beats allow, the same precedence `deny_paths` has — an operator who
    /// has learned one has learned both.
    #[test]
    fn deny_targets_beats_allow_targets() {
        let v = scope(&["service:*"], &["service:etcd"])
            .check_target("service", "etcd", "polecat")
            .expect("an explicit deny must win");
        assert_eq!(v.rule, "deny_targets:service:etcd");
    }

    /// EMPTY MEANS ANY, matching `allow_paths`. A scope that named no targets
    /// must not become a scope that permits none — that would turn adding the
    /// first `deny_targets` entry into a fleet-wide refusal.
    #[test]
    fn an_empty_allow_list_permits_any_target() {
        assert!(scope(&[], &[])
            .check_target("host", "anything", "polecat")
            .is_none());
    }

    /// AN ABSTENTION IS NOT A TARGET. The resolver answers Unknown for anything
    /// whose target is not unambiguous from syntax — a pipeline, a shell
    /// function, a script that ssh's internally. Those must reach the check as
    /// "no check performed"; a scope that refused what it could not identify
    /// would refuse most of the shell.
    #[test]
    fn an_unresolved_command_is_never_a_violation() {
        let resolved = action::resolve("some_shell_function | tee /dev/null");
        assert_eq!(resolved.target_class, action::TargetClass::Unknown);
        assert!(
            scope_refusal(r#"{"tool_name":"Bash"}"#, &resolved).is_none(),
            "an abstention must not reach a glob"
        );
    }

    /// RECORD-ONLY IS STILL THE DEFAULT. With no `action_scope` configured the
    /// hook must stay silent even on a command it fully resolved — enforcement
    /// must not arrive merely because the code shipped.
    #[test]
    fn a_resolved_command_is_silent_when_the_rung_is_OFF() {
        // A DOTTED host, because the resolver deliberately abstains on a bare
        // word: `ssh myhost` is real, but so is a stray flag value, and the
        // recognised set is small on purpose. This fixture has to clear that
        // bar or the test would pass for the wrong reason — an abstention is
        // silent whatever the rung says.
        let resolved = action::resolve("ssh prod-01.example uptime");
        assert_eq!(resolved.target_class, action::TargetClass::Host);
        assert!(
            scope_refusal(
                r#"{"tool_name":"Bash","cwd":"/nonexistent-root"}"#,
                &resolved
            )
            .is_none(),
            "the default posture must print nothing"
        );
    }
}
