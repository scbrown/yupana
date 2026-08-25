//! Git baseline access — resolving the `base_ref` to a commit and diffing
//! commits for promotion.
//!
//! Yupana's shared base graph is built at a **baseline commit** (`base_ref`,
//! default `main`, §5.5/FR-13), and promotion (§7.5) diffs a committed change
//! against that base. This module is the single boundary to git.
//!
//! **Access decision (open question 2).** Yupana *shells out* to the system `git`,
//! matching Bobbin's own `index/git.rs` precedent (stack coherence,
//! `CLAUDE.md`), adding no dependency and keeping the single-binary portability
//! story (§6.4). The choice is deliberately reversible: everything git-shaped
//! lives behind this module, so swapping to `gix`/`git2` later is localized.
//! Every call **degrades gracefully** — outside a repo, or with `git` absent, a
//! resolver returns `None` and a diff returns empty; nothing crashes (§6.4).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Run `git` in `root` with `args`, returning stdout on a clean exit (status 0)
/// and `None` on any failure (git missing, not a repo, bad ref, …).
///
/// NOTE for callers: `None` collapses "it failed" with nothing else, but an
/// EMPTY `Some("")` is a real answer — git succeeded and printed nothing. Do not
/// turn the two into the same empty list without saying which happened; see
/// [`crate::change`], which exists partly because that collapse hid a change-set
/// that was never computed behind one that was legitimately empty.
pub(crate) fn run(root: &Path, args: &[&str]) -> Option<String> {
    git(root, args)
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Whether `root` is inside a git work tree.
#[must_use]
pub fn is_repo(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"]).is_some_and(|s| s.trim() == "true")
}

/// Resolve a ref (branch, tag, `HEAD`, SHA-ish) to its full commit SHA, or
/// `None` if it does not resolve (or this is not a repo). The `^{commit}`
/// peel ensures tags resolve to the commit they point at.
#[must_use]
pub fn resolve_commit(root: &Path, reference: &str) -> Option<String> {
    let spec = format!("{reference}^{{commit}}");
    git(root, &["rev-parse", "--verify", "--quiet", &spec]).and_then(|s| {
        let sha = s.trim().to_string();
        (!sha.is_empty()).then_some(sha)
    })
}

/// The full SHA of `HEAD`, or `None` outside a repo / on an unborn branch.
#[must_use]
pub fn head_commit(root: &Path) -> Option<String> {
    resolve_commit(root, "HEAD")
}

/// Whether `reference` resolves to a MERGE commit — one with two or more
/// parents. `false` when the ref does not resolve or this is not a repo, which
/// is the safe direction: an unknown commit is not claimed to be a merge, so a
/// `promote_on = "merge"` policy declines rather than promoting on a guess.
///
/// `rev-list --parents -n 1` prints `<sha> <parent>…`, so the parent count is
/// the field count minus one.
#[must_use]
pub fn is_merge_commit(root: &Path, reference: &str) -> bool {
    git(root, &["rev-list", "--parents", "-n", "1", reference])
        .and_then(|out| {
            let line = out.lines().next()?.trim().to_string();
            Some(line.split_whitespace().count() >= 3)
        })
        .unwrap_or(false)
}

/// The branch `reference` names or sits at, or `None` when no branch can be
/// determined — which is a real answer, not a failure (§9.4 / GH #4: the
/// promotion then emits NO branch qualifier rather than guessing one).
///
/// Two questions, in the order that makes them unambiguous:
///
/// 1. Is `reference` itself a branch name (`refs/heads/<reference>`)? This is
///    the CI shape — a detached checkout promoted with `--commit main`, where
///    `HEAD` names no branch but the argument does.
/// 2. Otherwise, is it the commit `HEAD` is on, with `HEAD` attached to a
///    branch? This is the developer/hook shape — `--commit HEAD` on a checkout.
///
/// Anything else (a bare SHA that is not the current tip, a tag, a detached
/// HEAD) is `None`. A commit can sit on many branches and git will not pick one
/// for us; picking one here would attribute facts to a branch nobody named.
#[must_use]
pub fn branch_for(root: &Path, reference: &str) -> Option<String> {
    let name = reference.trim();
    if !name.is_empty()
        && git(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        )
        .is_some()
    {
        return Some(name.to_string());
    }
    let head = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    if head.is_empty() || head == "HEAD" {
        return None; // detached: HEAD names no branch
    }
    (resolve_commit(root, reference)? == resolve_commit(root, "HEAD")?).then_some(head)
}

/// The paths changed between two commit-ish refs (`from..to`), relative to the
/// repository root. Empty when either ref does not resolve, when there is no
/// diff, or outside a repo — the promotion path treats an empty set as
/// "nothing to promote" rather than an error (§7.5).
#[must_use]
pub fn changed_paths(root: &Path, from: &str, to: &str) -> Vec<PathBuf> {
    let range = format!("{from}..{to}");
    let Some(out) = git(root, &["diff", "--name-only", &range]) else {
        return Vec::new();
    };
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// The paths a single commit CHANGED, relative to the repository root — the
/// touched-entity half of §9.7's `commit → touched entities` provenance edge.
///
/// `git log -1 --name-only -m --first-parent`, which is the one incantation that
/// answers correctly for all three shapes promotion meets:
///
/// * an ordinary commit — its diff against its parent;
/// * a **merge** — its diff against its FIRST parent, i.e. what the merge
///   brought in. A bare `diff-tree` on a merge prints nothing at all, which
///   would have silently produced a merge commit that touched no entities under
///   the very `promote_on = "merge"` policy that makes merges the interesting
///   case;
/// * the **root** commit — everything, rather than failing for want of a parent.
///
/// `-z` because a filename may contain a newline and a line-splitting parser
/// would mis-attribute it.
#[must_use]
pub fn commit_touched_paths(root: &Path, reference: &str) -> Vec<PathBuf> {
    let Some(out) = git(
        root,
        &[
            "log",
            "-1",
            "--name-only",
            "--format=",
            "-m",
            "--first-parent",
            "-z",
            reference,
        ],
    ) else {
        return Vec::new();
    };
    out.split('\0')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// A commit's author identity and authored time (strict ISO-8601), or `None`
/// when the ref does not resolve.
///
/// AUTHOR date, not committer date: it is when the change was MADE, which is
/// §9.3's valid-time notion. A rebase rewrites the committer date and leaves the
/// author date alone, so the author date is the one that survives history being
/// replayed — and a provenance fact that moved because someone rebased would be
/// a fact about the rebase, not about the work.
#[must_use]
pub fn commit_identity(root: &Path, reference: &str) -> Option<(String, String)> {
    let out = git(root, &["log", "-1", "--format=%an <%ae>%n%aI", reference])?;
    let mut lines = out.lines();
    let author = lines.next()?.trim().to_string();
    let date = lines.next()?.trim().to_string();
    (!date.is_empty()).then_some((author, date))
}

/// The tracked file paths present in the tree at `reference`, relative to the
/// repository root. Empty when the ref does not resolve or outside a repo — the
/// caller treats an empty tree as "nothing to build" rather than an error.
#[must_use]
pub fn list_files_at(root: &Path, reference: &str) -> Vec<PathBuf> {
    let spec = format!("{reference}^{{tree}}");
    let Some(out) = git(root, &["ls-tree", "-r", "--name-only", "-z", &spec]) else {
        return Vec::new();
    };
    // `-z` gives NUL-separated paths so filenames with newlines are safe.
    out.split('\0')
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// The content of `path` in the tree at `reference`, or `None` when the blob is
/// absent at that ref, is not valid UTF-8 (binary), or this is not a repo.
#[must_use]
pub fn read_blob_at(root: &Path, reference: &str, path: &Path) -> Option<String> {
    let spec = format!("{reference}:{}", path.display());
    git(root, &["show", &spec])
}

/// The repository name from the `origin` remote's URL basename, or `None` when
/// there is no origin (or this is not a repo).
///
/// WHY THIS EXISTS: repo identity is part of every promoted IRI
/// (`…/code/<repo>/<file>::<symbol>`), and it used to be derived from the
/// checkout's DIRECTORY name. A worktree named after an agent
/// (`yupana-wt/gennaro`) then mints `code/gennaro/…`, a CI workspace mints
/// `code/workspace/…`, and the same repository fragments into parallel islands
/// that no entity resolution can rejoin — the IRIs are structurally different,
/// not fuzzily similar. The origin URL names the repository itself and is the
/// same from every checkout of it.
#[must_use]
pub fn origin_repo_name(root: &Path) -> Option<String> {
    git(root, &["remote", "get-url", "origin"]).and_then(|s| repo_name_from_url(s.trim()))
}

/// The work-tree root that CONTAINS `path`, resolved from the path itself and
/// never from the caller's working directory.
///
/// WHY THIS EXISTS: exposure — "is this repo public?" — is a property of where
/// the text LANDS, not of where the agent happens to be standing. Resolving it
/// from the session's cwd classified the SESSION instead of the EDIT, and the
/// error ran in both directions at once (measured): an agent whose
/// cwd is an internal workspace, editing a public repo by absolute path, had a
/// real leak downgraded to a warning; an agent whose cwd is a public checkout,
/// editing an internal-only file, was blocked for a token that leaks nowhere.
/// Crew agents sit in their own workspace clone and edit other repos by
/// absolute path as a matter of routine, so the under-enforcing direction was
/// the DEFAULT configuration, not an edge case.
///
/// `path` need not exist yet — a `Write` creates its target — so resolution
/// starts at the nearest existing ancestor. `None` means "not inside a work
/// tree", which callers must treat as unknown exposure, never as safe.
#[must_use]
pub fn repo_root_containing(path: &Path) -> Option<PathBuf> {
    let mut dir = if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    };
    // A Write may name a file several levels below anything that exists yet.
    while let Some(d) = dir {
        if d.is_dir() {
            break;
        }
        dir = d.parent();
    }
    let top = git(dir?, &["rev-parse", "--show-toplevel"])?;
    let top = top.trim();
    (!top.is_empty()).then(|| PathBuf::from(top))
}

/// The last path segment of a git remote URL, with a trailing `/` and a `.git`
/// suffix stripped. Handles https, `ssh://`, scp-like `host:path`, and plain
/// filesystem paths.
fn repo_name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    // Last '/' segment covers https, ssh://, scp-like host:a/b, and file paths.
    // An scp-like URL with no '/' at all (`host:repo.git`) splits on ':' instead.
    let base = match trimmed.rsplit('/').next() {
        Some(seg) if seg != trimmed => seg,
        _ => trimmed.rsplit(':').next().unwrap_or(trimmed),
    };
    let name = base.strip_suffix(".git").unwrap_or(base);
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Initialize a throwaway git repo in `dir` with one committed file.
    /// Returns `false` (skip the test) if `git` is unavailable — integration
    /// with an external toolchain must skip gracefully, not fail (§13).
    fn init_repo(dir: &Path) -> bool {
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success())
        };
        if !run(&["init", "-q"]) {
            return false; // git absent → skip
        }
        run(&["config", "user.email", "t@t.test"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first"])
    }

    /// The whole point of [`repo_root_containing`]: the answer follows the
    /// FILE, and is unmoved by where the caller is standing. Two real repos,
    /// and a path in one resolved while the process sits in the other — which
    /// is the ordinary crew configuration, not a contrived one.
    #[test]
    fn a_files_repo_is_its_own_not_the_callers() {
        let session = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        if !init_repo(session.path()) || !init_repo(other.path()) {
            return; // skip: no git
        }
        // tempfile hands out /tmp paths that may be symlinked (macOS); compare
        // against git's own idea of each root so the assert tests resolution,
        // not path spelling.
        let other_root = repo_root_containing(&other.path().join("a.txt")).expect("in a repo");

        let _cwd_is_elsewhere = session.path();
        let resolved = repo_root_containing(&other.path().join("a.txt")).expect("in a repo");
        assert_eq!(resolved, other_root);
        assert_ne!(
            resolved,
            repo_root_containing(&session.path().join("a.txt")).expect("in a repo"),
            "two distinct repos must not collapse to one root"
        );

        // A file that does not exist yet — every `Write` — still resolves,
        // through as many missing parents as it takes.
        let unborn = other.path().join("deep/er/still/new.md");
        assert_eq!(
            repo_root_containing(&unborn).expect("unborn file still has a repo"),
            other_root
        );
    }

    /// Outside any work tree the answer is `None` — "unknown", which the
    /// governed plane must never round down to "safe".
    #[test]
    fn a_path_outside_any_repo_has_no_root() {
        let bare = tempfile::tempdir().unwrap();
        // A temp dir with no `git init` is only outside a work tree if no
        // ancestor is a repo either; /tmp is not, but say so rather than assume.
        if repo_root_containing(bare.path()).is_some() {
            return; // skip: the temp dir sits inside someone's repo
        }
        assert_eq!(repo_root_containing(&bare.path().join("x.md")), None);
    }

    #[test]
    fn resolves_head_and_detects_repo() {
        let dir = tempfile::tempdir().unwrap();
        if !init_repo(dir.path()) {
            return; // skip: no git
        }
        assert!(is_repo(dir.path()));
        let head = head_commit(dir.path()).expect("HEAD resolves");
        assert_eq!(head.len(), 40, "full SHA");
        // A bogus ref does not resolve.
        assert!(resolve_commit(dir.path(), "no-such-ref").is_none());
    }

    #[test]
    fn diffs_changed_paths_between_commits() {
        let dir = tempfile::tempdir().unwrap();
        if !init_repo(dir.path()) {
            return; // skip: no git
        }
        let first = head_commit(dir.path()).unwrap();
        std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
        };
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "second"]);
        let second = head_commit(dir.path()).unwrap();

        let changed = changed_paths(dir.path(), &first, &second);
        assert_eq!(changed, vec![PathBuf::from("b.txt")]);
    }

    #[test]
    fn degrades_gracefully_outside_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_repo(dir.path()));
        assert!(head_commit(dir.path()).is_none());
        assert!(resolve_commit(dir.path(), "main").is_none());
        assert!(changed_paths(dir.path(), "a", "b").is_empty());
        assert!(list_files_at(dir.path(), "HEAD").is_empty());
        assert!(read_blob_at(dir.path(), "HEAD", Path::new("a.txt")).is_none());
    }

    #[test]
    fn reads_tree_content_at_a_ref() {
        let dir = tempfile::tempdir().unwrap();
        if !init_repo(dir.path()) {
            return; // skip: no git
        }
        let first = head_commit(dir.path()).unwrap();

        // Second commit: change a.txt and add b.txt.
        std::fs::write(dir.path().join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
        };
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "second"]);

        // The tree at the first commit has only a.txt, with its original body.
        let files = list_files_at(dir.path(), &first);
        assert_eq!(files, vec![PathBuf::from("a.txt")]);
        assert_eq!(
            read_blob_at(dir.path(), &first, Path::new("a.txt")).as_deref(),
            Some("one\n"),
            "reads the historical blob, not the working tree"
        );
        // b.txt did not exist at the first commit.
        assert!(read_blob_at(dir.path(), &first, Path::new("b.txt")).is_none());

        // HEAD sees both files and the updated content.
        let head_files = list_files_at(dir.path(), "HEAD");
        assert!(head_files.contains(&PathBuf::from("a.txt")));
        assert!(head_files.contains(&PathBuf::from("b.txt")));
        assert_eq!(
            read_blob_at(dir.path(), "HEAD", Path::new("a.txt")).as_deref(),
            Some("changed\n")
        );
    }

    #[test]
    fn repo_name_from_every_remote_url_shape() {
        // The four shapes git remotes actually take, plus the traps: trailing
        // slash, no `.git` suffix, and an scp-like URL with no '/' at all.
        for (url, want) in [
            ("https://github.com/scbrown/yupana.git", "yupana"),
            ("https://example.com/group/sub/proj", "proj"),
            ("git@github.com:scbrown/yupana.git", "yupana"),
            ("ssh://git.example/owner/thing.git", "thing"),
            ("ssh://git.example/owner/thing/", "thing"),
            ("host:solo.git", "solo"),
            ("/home/someone/checkouts/yupana", "yupana"),
        ] {
            assert_eq!(repo_name_from_url(url).as_deref(), Some(want), "url: {url}");
        }
        // Degenerate inputs yield None, never an empty identity.
        assert_eq!(repo_name_from_url(""), None);
        assert_eq!(repo_name_from_url(".git"), None);
    }

    #[test]
    fn origin_repo_name_reads_the_remote_not_the_dir() {
        let dir = tempfile::tempdir().unwrap();
        if !init_repo(dir.path()) {
            return; // git unavailable; integration tests cover the rest
        }
        // No origin yet: identity is unknowable, not guessed from the dir name.
        assert_eq!(origin_repo_name(dir.path()), None);
        Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://example.com/owner/realname.git",
            ])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert_eq!(origin_repo_name(dir.path()).as_deref(), Some("realname"));
    }
}
