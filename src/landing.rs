//! landing — resolve a shell command line to a LANDING attempt, or abstain.
//!
//! A *landing* is the act of putting code onto a repository's protected branch:
//! `git push` and `gh pr merge` (including its REST spelling). It is the action
//! the governed single-writer policy is written against
//! (`docs/design/landing-policy.md`).
//!
//! This module is the ACTION SELECTOR half of that policy's vocabulary, and it
//! is **pure**: it reads a command string and returns what the string says.
//! Resolving a remote *name* to a repository, or an agent to an owner, needs
//! I/O and the graph, and both live in the evaluator.
//!
//! ## Two disciplines, and they pull in opposite directions
//!
//! [`crate::action`] abstains on any compound line, because it feeds a TRACE and
//! a wrong guess there becomes the evidence for a wrong rule later. This module
//! feeds a GUARD, where the same choice has the opposite consequence: abstaining
//! on `cd repo && git push origin main` does not produce a cautious non-answer,
//! it produces a bypass, and a one-character bypass is not a guard. So this
//! module splits the line into shell segments and inspects each one.
//!
//! What it does NOT do is guess the *target*. Every field is either something
//! the command literally said, or an explicit "the command did not say"
//! ([`RefTarget::Unstated`], [`RepoRef::Cwd`]) that the evaluator has to resolve
//! or refuse. There is no branch here that infers a repository from likelihood.
//!
//! ## Heredoc bodies are data
//!
//! Stripped before matching, for the reason the sibling host guards strip them:
//! the documented safe way to write "never run `gh pr merge`" into a work item
//! is a heredoc. A guard that fires inside one blocks the act of documenting the
//! hazard, is recognised as broken, and gets removed — leaving no guard.

/// Which landing verb the command spells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingVerb {
    /// `git push` — writes refs directly.
    Push,
    /// `gh pr merge`, or `gh api repos/<slug>/pulls/<n>/merge`. The verb that
    /// lands code where branch protection refuses a push.
    Merge,
}

impl LandingVerb {
    /// The wire form, shared with the verdict record.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LandingVerb::Push => "push",
            LandingVerb::Merge => "merge",
        }
    }
}

/// How the command names the repository. Never a resolved name — the variants
/// record what kind of naming was used, so the evaluator knows what lookup it
/// owes and can refuse rather than assume when it cannot perform it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoRef {
    /// A remote URL on the command line (`git push git@host:owner/name.git`).
    /// Unambiguous: the repository is named in the text.
    Url(String),
    /// An `owner/name` slug (`gh pr merge -R owner/name`). Unambiguous.
    Slug(String),
    /// A remote NAME (`origin`). The repository is whatever that remote points
    /// at in the working directory — a lookup, not a guess.
    Remote(String),
    /// The command named no repository at all (`gh pr merge 12`), so it acts on
    /// the working directory's repository.
    Cwd,
}

/// Which ref the landing targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefTarget {
    /// The command named it (`git push origin main`).
    Named(String),
    /// The command did not name one.
    ///
    /// This is NOT a guess that the target is the default branch — it is the
    /// statement that the text does not say, which is a different claim and has
    /// to stay different. `git push origin` pushes the current branch;
    /// `gh pr merge` lands on the pull request's base. Both are knowable, and
    /// neither is knowable *from the command line*. The evaluator decides what
    /// to do with the ignorance and records that it was ignorance, so a
    /// false positive in the soak is diagnosable rather than mysterious.
    Unstated,
}

/// One resolved landing attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    /// The verb the command spells.
    pub verb: LandingVerb,
    /// How the command named the repository.
    pub repo: RepoRef,
    /// Which ref it targets, or [`RefTarget::Unstated`].
    pub git_ref: RefTarget,
    /// The matched segment, trimmed — evidence for the verdict record. A
    /// refusal that cannot show what it matched is not auditable.
    pub evidence: String,
}

/// Strip heredoc bodies. See the module note: bodies are data, not command
/// position.
fn strip_heredocs(cmd: &str) -> String {
    let mut out = Vec::new();
    let mut delim: Option<String> = None;
    for line in cmd.lines() {
        if let Some(d) = &delim {
            if line.trim() == d {
                delim = None;
            }
            continue;
        }
        if let Some(found) = heredoc_delimiter(line) {
            delim = Some(found);
        }
        out.push(line);
    }
    out.join("\n")
}

/// The delimiter word of a `<<WORD` / `<<-'WORD'` redirect, if the line opens one.
fn heredoc_delimiter(line: &str) -> Option<String> {
    let at = line.find("<<")?;
    let rest = line[at + 2..].trim_start_matches('-').trim_start();
    let rest = rest.trim_start_matches(['\'', '"']);
    let word: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if word.is_empty() || !word.starts_with(|c: char| c.is_alphabetic() || c == '_') {
        return None;
    }
    Some(word)
}

/// Split into candidate command positions on shell operators.
fn segments(cmd: &str) -> Vec<&str> {
    cmd.split([';', '|', '&', '\n', '(', ')'])
        .filter(|s| !s.trim().is_empty())
        .collect()
}

/// Bare words, with simple surrounding quotes stripped.
fn words(seg: &str) -> Vec<&str> {
    seg.split_whitespace()
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Drop leading `VAR=value` assignments and shell keywords, then return the
/// program name without the path it was invoked by. `/usr/bin/gh` is `gh`.
fn program(w: &[&str]) -> Option<(usize, String)> {
    let mut i = 0;
    while i < w.len() {
        let word = w[i];
        if matches!(word, "then" | "do" | "else" | "{" | "!") {
            i += 1;
            continue;
        }
        // A VAR=value prefix, not a program.
        if let Some((name, _)) = word.split_once('=') {
            if !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            {
                i += 1;
                continue;
            }
        }
        let prog = word.rsplit('/').next().unwrap_or(word);
        return Some((i, prog.to_string()));
    }
    None
}

/// Does this word look like a remote URL or path rather than a remote name?
fn looks_like_url(word: &str) -> bool {
    word.contains("://") || word.contains(':') && word.contains('/') || word.starts_with('.')
}

/// Resolve one segment, or abstain.
fn resolve_segment(seg: &str) -> Option<Landing> {
    let w = words(seg);
    let (start, prog) = program(&w)?;
    let rest = &w[start + 1..];
    match prog.as_str() {
        "git" => resolve_git_push(rest, seg),
        "gh" => resolve_gh(rest, seg),
        _ => None,
    }
}

/// `git push [flags] [<repository> [<refspec>...]]`.
fn resolve_git_push(rest: &[&str], seg: &str) -> Option<Landing> {
    // The subcommand, skipping git's own leading options (`git -C dir push`).
    let mut it = rest.iter().enumerate();
    let sub_at = loop {
        let (i, word) = it.next()?;
        if word.starts_with('-') {
            // `-C <dir>` and `-c <cfg>` take a value; skip it too.
            if matches!(*word, "-C" | "-c" | "--git-dir" | "--work-tree") {
                it.next()?;
            }
            continue;
        }
        break i;
    };
    if rest[sub_at] != "push" {
        return None;
    }
    // Operands after `push`, minus flags. `--repo=<x>` names the repository.
    let mut operands: Vec<&str> = Vec::new();
    let mut explicit_repo: Option<&str> = None;
    let mut skip_next = false;
    for word in &rest[sub_at + 1..] {
        if skip_next {
            skip_next = false;
            continue;
        }
        if let Some(v) = word.strip_prefix("--repo=") {
            explicit_repo = Some(v);
            continue;
        }
        if *word == "--repo" {
            skip_next = true;
            continue;
        }
        if word.starts_with('-') {
            // Flags that take a separate value; everything else is a switch.
            if matches!(*word, "-o" | "--push-option" | "--receive-pack" | "--exec") {
                skip_next = true;
            }
            continue;
        }
        operands.push(word);
    }
    let repo = match explicit_repo.or_else(|| operands.first().copied()) {
        Some(r) if looks_like_url(r) => RepoRef::Url(r.to_string()),
        Some(r) => RepoRef::Remote(r.to_string()),
        None => RepoRef::Cwd,
    };
    // The refspec is the operand after the repository, unless the repository
    // came from `--repo=`, in which case the first operand is already the ref.
    let ref_operand = if explicit_repo.is_some() {
        operands.first().copied()
    } else {
        operands.get(1).copied()
    };
    let git_ref = match ref_operand {
        // `src:dst` pushes to dst — the ref that actually receives the write.
        Some(spec) => RefTarget::Named(
            spec.rsplit(':')
                .next()
                .unwrap_or(spec)
                .trim_start_matches('+')
                .to_string(),
        ),
        None => RefTarget::Unstated,
    };
    Some(Landing {
        verb: LandingVerb::Push,
        repo,
        git_ref,
        evidence: seg.trim().to_string(),
    })
}

/// `gh pr merge [<number>] [-R owner/name]`, and the REST spelling
/// `gh api repos/<owner>/<name>/pulls/<n>/merge`.
fn resolve_gh(rest: &[&str], seg: &str) -> Option<Landing> {
    let mut explicit: Option<String> = None;
    let mut skip_next = false;
    for (i, word) in rest.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if matches!(*word, "-R" | "--repo") {
            explicit = rest.get(i + 1).map(|v| (*v).to_string());
            skip_next = true;
        } else if let Some(v) = word.strip_prefix("--repo=") {
            explicit = Some(v.to_string());
        }
    }
    let bare: Vec<&str> = rest
        .iter()
        .copied()
        .filter(|w| !w.starts_with('-'))
        .collect();

    let is_pr_merge = bare.first() == Some(&"pr") && bare.get(1) == Some(&"merge");
    let api_slug = (bare.first() == Some(&"api"))
        .then(|| bare.iter().find_map(|w| api_merge_slug(w)))
        .flatten();

    if !is_pr_merge && api_slug.is_none() {
        return None;
    }
    let repo = match explicit.or(api_slug) {
        Some(slug) => RepoRef::Slug(slug),
        None => RepoRef::Cwd,
    };
    Some(Landing {
        verb: LandingVerb::Merge,
        repo,
        // A merge lands on the pull request's BASE branch, which the command
        // line does not carry. Unstated, never assumed to be the default
        // branch: see `RefTarget::Unstated`.
        git_ref: RefTarget::Unstated,
        evidence: seg.trim().to_string(),
    })
}

/// `repos/<owner>/<name>/pulls/<n>/merge` -> `<owner>/<name>`.
fn api_merge_slug(word: &str) -> Option<String> {
    let path = word.split('?').next()?.trim_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    // repos / owner / name / pulls / n / merge
    if parts.len() < 6 || parts[0] != "repos" || parts[3] != "pulls" || parts[5] != "merge" {
        return None;
    }
    if !parts[4].chars().all(|c| c.is_ascii_digit()) || parts[4].is_empty() {
        return None;
    }
    Some(format!("{}/{}", parts[1], parts[2]))
}

/// Resolve a command line to the first landing attempt it contains, or `None`.
///
/// Read any edit to this function against the module note: a new pattern earns
/// its place only if what it extracts is stated in the text. `RepoRef::Cwd` and
/// `RefTarget::Unstated` exist so that "the command did not say" never has to be
/// expressed as a guess.
#[must_use]
pub fn resolve(cmd: &str) -> Option<Landing> {
    let stripped = strip_heredocs(cmd);
    segments(&stripped).into_iter().find_map(resolve_segment)
}

#[cfg(test)]
// Test names shout the invariant they turn on, the repo's house convention.
#[allow(non_snake_case)]
mod tests {
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
}
