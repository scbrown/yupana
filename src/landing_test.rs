//! Tests for `landing` — the LANDING action selector. Child module of
//! `landing` (`super::*` reaches its private helpers); size-exempt
//! (`_test.rs`), which is why the tests live here and not inline: the parent
//! crossed this repo's 500-line hard cap.
//!
//! Test names shout the invariant they turn on, the repo's house convention.
#![allow(non_snake_case)]

use super::*;

fn r(cmd: &str) -> Option<Landing> {
    resolve(cmd)
}

#[test]
fn a_plain_push_names_its_remote_and_ref() {
    let l = r("git push origin main").expect("a push is a landing");
    assert_eq!(l.verb, LandingVerb::Push);
    assert_eq!(l.repo, RepoRef::Remote("origin".into()));
    assert_eq!(l.git_ref, RefTarget::Named("main".into()));
}

#[test]
fn a_url_push_is_distinguished_from_a_remote_NAME() {
    let l = r("git push git@github.com:scbrown/yupana.git main").unwrap();
    assert_eq!(
        l.repo,
        RepoRef::Url("git@github.com:scbrown/yupana.git".into())
    );
    // The distinction is the whole point: a URL needs no lookup, a name does.
    assert!(!matches!(l.repo, RepoRef::Remote(_)));
}

#[test]
fn a_refspec_resolves_to_the_ref_that_RECEIVES_the_write() {
    // `wt/grant:main` writes to main. Reading the left side would let any
    // agent land on a protected branch by naming a topic branch first.
    let l = r("git push origin wt/grant:main").unwrap();
    assert_eq!(l.git_ref, RefTarget::Named("main".into()));
    let forced = r("git push origin +wt/grant:main").unwrap();
    assert_eq!(forced.git_ref, RefTarget::Named("main".into()));
}

#[test]
fn an_unnamed_ref_is_UNSTATED_never_assumed() {
    assert_eq!(r("git push origin").unwrap().git_ref, RefTarget::Unstated);
    assert_eq!(r("git push").unwrap().repo, RepoRef::Cwd);
}

#[test]
fn force_flags_do_not_hide_a_push() {
    let l = r("git push --force-with-lease origin main").unwrap();
    assert_eq!(l.git_ref, RefTarget::Named("main".into()));
    assert_eq!(l.repo, RepoRef::Remote("origin".into()));
}

#[test]
fn git_leading_options_and_their_values_are_skipped() {
    let l = r("git -C /tmp/repo push origin main").unwrap();
    assert_eq!(l.verb, LandingVerb::Push);
    assert_eq!(l.repo, RepoRef::Remote("origin".into()));
}

#[test]
fn non_landing_git_subcommands_abstain() {
    for cmd in [
        "git fetch origin",
        "git pull origin main",
        "git commit -m x",
    ] {
        assert!(r(cmd).is_none(), "{cmd} is not a landing");
    }
}

#[test]
fn gh_pr_merge_resolves_with_and_without_an_explicit_repo() {
    let explicit = r("gh pr merge 12 -R scbrown/quipu").unwrap();
    assert_eq!(explicit.verb, LandingVerb::Merge);
    assert_eq!(explicit.repo, RepoRef::Slug("scbrown/quipu".into()));
    let implicit = r("gh pr merge 12").unwrap();
    assert_eq!(implicit.repo, RepoRef::Cwd);
}

#[test]
fn the_REST_spelling_of_merge_is_the_same_verb() {
    let l = r("gh api repos/scbrown/quipu/pulls/12/merge -X PUT").unwrap();
    assert_eq!(l.verb, LandingVerb::Merge);
    assert_eq!(l.repo, RepoRef::Slug("scbrown/quipu".into()));
}

#[test]
fn a_non_merge_gh_api_call_abstains() {
    assert!(r("gh api repos/scbrown/quipu/pulls/12").is_none());
    assert!(r("gh pr view 12").is_none());
    assert!(r("gh pr list").is_none());
}

#[test]
fn a_landing_hidden_in_a_COMPOUND_line_is_still_found() {
    // The bypass this module exists to close: `action::resolve` abstains
    // here, and abstaining in a guard is not caution, it is a hole.
    let l = r("cd /tmp/repo && git push origin main").unwrap();
    assert_eq!(l.git_ref, RefTarget::Named("main".into()));
    assert!(r("true; gh pr merge 3 -R scbrown/quipu").is_some());
}

#[test]
fn a_cd_BEFORE_the_landing_is_carried_as_the_resolution_directory() {
    // The measured bypass (2026-09-05): a bare remote name means nothing
    // without a directory, and the hook payload carries the SESSION's cwd.
    // `cd <governed repo> && git push origin main` resolved `origin` in the
    // agent's own workspace, found an ungoverned repo, and ALLOWED.
    let l = r("cd /home/x/quipu && git push origin main").unwrap();
    assert_eq!(l.cwd_hint.as_deref(), Some("/home/x/quipu"));
    assert_eq!(l.repo, RepoRef::Remote("origin".into()));
    assert_eq!(l.git_ref, RefTarget::Named("main".into()));
}

#[test]
fn the_LAST_cd_before_the_landing_wins() {
    let l = r("cd /a && cd /b && git push origin main").unwrap();
    assert_eq!(l.cwd_hint.as_deref(), Some("/b"));
}

#[test]
fn a_cd_AFTER_the_landing_does_not_apply_to_it() {
    // Ordering is the whole point: the directory in force is the one that
    // was in force when the push ran.
    let l = r("git push origin main && cd /elsewhere").unwrap();
    assert_eq!(l.cwd_hint, None);
}

#[test]
fn a_cd_naming_no_knowable_directory_is_NOT_guessed() {
    // A bare `cd`, `cd -` and `cd ~` all name a directory this resolver
    // cannot know. Inventing one would put a fabricated path in a refusal.
    for cmd in [
        "cd && git push origin main",
        "cd - && git push origin main",
        "cd ~ && git push origin main",
    ] {
        assert_eq!(r(cmd).unwrap().cwd_hint, None, "{cmd}");
    }
}

#[test]
fn a_relative_cd_is_carried_verbatim_for_the_caller_to_join() {
    let l = r("cd ../quipu && git push origin main").unwrap();
    assert_eq!(l.cwd_hint.as_deref(), Some("../quipu"));
}

#[test]
fn env_prefixes_do_not_hide_the_program() {
    let l = r("GIT_SSH_COMMAND=ssh git push origin main").unwrap();
    assert_eq!(l.verb, LandingVerb::Push);
}

#[test]
fn an_absolute_path_does_not_hide_the_program() {
    assert!(r("/usr/bin/gh pr merge 4 -R scbrown/quipu").is_some());
    assert!(r("/usr/bin/git push origin main").is_some());
}

#[test]
fn a_heredoc_BODY_naming_the_verb_is_data_not_a_landing() {
    // Documenting the hazard must not trip the guard that forbids it.
    let cmd = "cat > /tmp/note.md <<'EOF'\nnever run git push origin main here\ngh pr merge 1 -R scbrown/quipu\nEOF";
    assert!(r(cmd).is_none());
}

#[test]
fn a_heredoc_does_not_swallow_a_REAL_landing_after_it_closes() {
    let cmd = "cat > /tmp/n <<'EOF'\ntext\nEOF\ngit push origin main";
    assert_eq!(
        r(cmd).unwrap().git_ref,
        RefTarget::Named("main".into()),
        "the landing after the delimiter is command position again"
    );
}

#[test]
fn unrelated_commands_abstain() {
    for cmd in ["ls -la", "cargo test", "echo git push origin main | wc -l"] {
        // The echo case resolves nothing because `echo` is not a landing
        // program; the pipe segment `wc -l` is not either.
        assert!(r(cmd).is_none(), "{cmd}");
    }
}
