//! Tests for the landing guard's I/O half. Size-exempt (`_test.rs`).

use super::*;

#[test]
fn the_spool_is_NEVER_relative_to_the_hooks_working_directory() {
    // The defect this pins: a bare filename sends every record to whatever
    // directory the agent was standing in, so the corpus a soak reads is
    // empty while the guard believes it is attesting.
    let p = resolve_spool(None, None, Some("/home/x")).unwrap();
    assert_eq!(
        p,
        std::path::PathBuf::from("/home/x/.local/state/yupana/action-certifications.jsonl")
    );
    assert!(p.is_absolute());
    // XDG wins over HOME, and an explicit path wins over both — the same
    // precedence the host certification adapter uses.
    assert_eq!(
        resolve_spool(None, Some("/state"), Some("/home/x")).unwrap(),
        std::path::PathBuf::from("/state/yupana/action-certifications.jsonl")
    );
    assert_eq!(
        resolve_spool(Some("/tmp/s.jsonl"), Some("/state"), Some("/home/x")).unwrap(),
        std::path::PathBuf::from("/tmp/s.jsonl")
    );
}

#[test]
fn a_RELATIVE_configured_key_is_ignored_not_resolved_against_the_cwd() {
    // `signing_key_path` defaults to the bare "yupana-signing.pk8". Honouring
    // that relative value would make the guard sign with whichever key is
    // underfoot, or — the common case — find none and record nothing while
    // reporting success.
    assert_eq!(
        resolve_key_path(None, "yupana-signing.pk8", Some("/home/x")).unwrap(),
        std::path::PathBuf::from("/home/x/.config/aegis/yupana-signing.pk8")
    );
    // An ABSOLUTE configured path is a deliberate deployment choice and wins.
    assert_eq!(
        resolve_key_path(None, "/opt/k.pk8", Some("/home/x")).unwrap(),
        std::path::PathBuf::from("/opt/k.pk8")
    );
    // The env var beats everything, matching the adapter's contract.
    assert_eq!(
        resolve_key_path(Some("/env/k.pk8"), "/opt/k.pk8", Some("/home/x")).unwrap(),
        std::path::PathBuf::from("/env/k.pk8")
    );
}

#[test]
fn the_routing_superset_admits_landings_and_skips_ordinary_work() {
    assert!(might_be_a_landing("git push origin main"));
    assert!(might_be_a_landing("gh pr merge 3"));
    // It is a SUPERSET, so false admissions are fine — `landing::resolve`
    // is the real filter. False EXCLUSIONS would be holes.
    assert!(!might_be_a_landing("cargo test"));
    assert!(!might_be_a_landing("ls -la"));
}

#[test]
fn a_named_ref_is_never_marked_assumed() {
    let landing = crate::landing::resolve("git push origin main").unwrap();
    let (git_ref, assumed) = resolve_ref(&landing, std::path::Path::new("."));
    assert_eq!(git_ref, "main");
    assert!(!assumed, "the command stated the ref");
}

#[test]
fn a_merge_ref_is_ALWAYS_marked_assumed() {
    // The command cannot carry the pull request's base branch, so the
    // stand-in must never present itself as stated.
    let landing = crate::landing::resolve("gh pr merge 3 -R scbrown/quipu").unwrap();
    let (git_ref, assumed) = resolve_ref(&landing, std::path::Path::new("."));
    assert_eq!(git_ref, crate::project_landing::DEFAULT_PROTECTED_REF);
    assert!(assumed);
}

#[test]
fn a_slug_resolves_to_the_bare_repo_name_without_touching_git() {
    let landing = crate::landing::resolve("gh pr merge 3 -R scbrown/quipu").unwrap();
    let repo = resolve_repo(&landing, std::path::Path::new("/nonexistent"));
    assert_eq!(repo.as_deref(), Some("quipu"));
}

#[test]
fn a_url_resolves_without_touching_git() {
    let landing =
        crate::landing::resolve("git push git@github.com:scbrown/yupana.git main").unwrap();
    let repo = resolve_repo(&landing, std::path::Path::new("/nonexistent"));
    assert_eq!(repo.as_deref(), Some("yupana"));
}

#[test]
fn record_ids_are_stable_per_command_and_differ_across_commands() {
    // The spool refuses a repeat record_id with a different payload, so a
    // colliding id would turn two distinct landings into an error.
    assert_eq!(
        md5_ish("git push origin main"),
        md5_ish("git push origin main")
    );
    assert_ne!(md5_ish("git push origin main"), md5_ish("gh pr merge 3"));
}

#[test]
fn two_agents_landing_the_same_command_in_one_second_get_DIFFERENT_record_ids() {
    // aegis-an6v8i. This is the whole defect in one assertion: the key was
    // `landing-{ts}-{hash(command)}`, `ts` is whole seconds, and the hash covers
    // only the command — so these two collided, `append` refused the second
    // because its payload differed, and `record` discarded that error. The
    // second verdict was written nowhere and reported nowhere.
    //
    // Measured on the live path before the fix: wu and grant, same command, same
    // second, ONE row in the spool and exit 0 from both invocations.
    let cmd = "gh pr merge 230 --repo scbrown/quipu --merge";
    assert_ne!(
        landing_record_id(1_789_528_644, "wu", "refuse", cmd),
        landing_record_id(1_789_528_644, "grant", "refuse", cmd),
        "same command, same second, different agents MUST NOT share a key"
    );
    // The test that stood here before checked `md5_ish` alone and passed
    // throughout the defect's life — the hash was never the broken part. A key
    // needs testing as a key.
}

#[test]
fn the_SAME_agent_retrying_the_same_command_in_one_second_keeps_ONE_record_id() {
    // Load-bearing in the opposite direction, and the reason the fix is not
    // simply "make every id unique". An identical retry must still collide, so
    // that `append` sees a matching id AND a matching signed payload hash and
    // treats it as the idempotent re-send it is. Making ids unique per call
    // would turn every duplicate hook invocation into a second spool row and
    // inflate the very denominator the soak divides by.
    let cmd = "gh pr merge 230 --repo scbrown/quipu --merge";
    assert_eq!(
        landing_record_id(1_789_528_644, "wu", "refuse", cmd),
        landing_record_id(1_789_528_644, "wu", "refuse", cmd)
    );
}

#[test]
fn the_SAME_agent_retrying_with_a_DIFFERENT_outcome_in_one_second_gets_a_NEW_record_id() {
    // aegis-djxl44: refuse, arm an override, retry. an6v8i's agent field cannot
    // separate these, because the agent and command are the same; only the outcome
    // differs. Before this, the ALLOW was refused by `append` and lost, keeping the
    // REFUSE: a manufactured false positive inside the soak.
    let cmd = "gh pr merge 241 --repo scbrown/quipu";
    let refused = landing_record_id(1_789_528_644, "wu", &landing_outcome("refuse", false), cmd);
    let allowed = landing_record_id(1_789_528_644, "wu", &landing_outcome("allow", true), cmd);
    assert_ne!(refused, allowed);
    // An override that did not change the verdict is still a different event.
    assert_ne!(
        landing_record_id(1_789_528_644, "wu", &landing_outcome("refuse", false), cmd),
        landing_record_id(1_789_528_644, "wu", &landing_outcome("refuse", true), cmd)
    );
}

#[test]
fn the_record_id_still_varies_by_second_and_by_command() {
    // Guard the two properties the agent field must not have cost us.
    let cmd = "gh pr merge 230 --repo scbrown/quipu --merge";
    assert_ne!(
        landing_record_id(1_789_528_644, "wu", "refuse", cmd),
        landing_record_id(1_789_528_645, "wu", "refuse", cmd)
    );
    assert_ne!(
        landing_record_id(1_789_528_644, "wu", "refuse", cmd),
        landing_record_id(1_789_528_644, "wu", "refuse", "git push origin main")
    );
}

#[test]
fn the_record_id_keeps_the_landing_prefix_every_consumer_filters_on() {
    // `soak-landing-policy.sh` and `check-landing-verdict-coverage.sh` both
    // partition the shared spool with `record_id.startswith("landing-")` — the
    // host adapter's rows start `quipu-writer-`. Inserting the agent must not
    // disturb that, or the soak silently stops seeing governed evaluations and
    // reports a corpus of zero as an honest answer.
    assert!(
        landing_record_id(1_789_528_644, "wu", "allow", "gh pr merge 3").starts_with("landing-")
    );
}
