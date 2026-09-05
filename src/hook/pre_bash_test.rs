//! Tests for the pre-bash hook — the three modules moved here verbatim.
//! Size-exempt (`_test.rs`), the same split `pre_edit`/`pre_edit_test` already uses.

#[cfg(test)]
mod tests {
    use super::super::*;

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
    use super::super::*;

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
    use super::super::*;
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
