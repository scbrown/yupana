//! What does this CHANGE do? — the change-time half of the git baseline.
//!
//! [`crate::git`] could already read a base tree and diff two commits, but
//! nothing in the product ever called the diff: `changed_paths` had no
//! production caller, so Yupana could answer *what a tree contains* and never
//! *what a change does*. Those are different questions, and change-time policy —
//! "what does this proposed change violate" — is only the second one. A rule
//! evaluated against tree contents silently answers the wrong question with a
//! confident yes.
//!
//! **The distinction this module exists to preserve.** `git()` returns `None` for
//! a missing git, a non-repo, and an unresolved ref alike, so `changed_paths`
//! returns an EMPTY VEC for both "nothing changed" and "I could not look". Those
//! are opposite facts: the first says a rule has nothing to judge, the second
//! says the rule was never applied. Collapsing them is the same defect the guard
//! carried for languages — an unmeasurable case that reads exactly like a clean
//! one. So every result here is a TYPE, and the unmeasurable variants carry their
//! own reason.
//!
//! **Nothing here decides policy.** It reports the changed entities and the parts
//! it could not read. What a rule does with that is the rule's business; this
//! module's contract is that it never claims a change is empty when it simply
//! could not see it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::extract::{extract_structure, language_for_extension};

/// The set of paths a change touches, or why that could not be determined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedPaths {
    /// The diff ran. An EMPTY vec here is a measurement: nothing changed.
    Diffed(Vec<PathBuf>),
    /// Not a git work tree, or `git` is not installed.
    NoRepo,
    /// One of the two endpoints does not resolve to a commit.
    UnresolvedRef(String),
}

/// One entity the change touched, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntityChange {
    /// Repo-relative path the entity lives in.
    pub file: String,
    /// Symbol name.
    pub name: String,
    /// `added` | `removed` | `modified`.
    pub kind: &'static str,
}

/// A file the change touched that could NOT be turned into entities, and why.
///
/// This is the half that must never be silent. A rule written about entities is
/// not enforced on a file that produced none because Yupana could not parse it —
/// and "produced no entities" and "was not looked at" are indistinguishable
/// unless the second is named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnreadFile {
    /// Repo-relative path.
    pub file: String,
    /// Why no entities came from it.
    pub why: String,
}

/// The answer to "what does this change do".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChangeSet {
    /// The base commit the change was diffed against.
    pub base: String,
    /// The commit (or working tree) the change was diffed to.
    pub to: String,
    /// Entities added, removed or modified.
    pub entities: Vec<EntityChange>,
    /// Files the diff named that produced no entities, each with a reason.
    /// EMPTY is the only state that means "everything in this change was read".
    pub unread: Vec<UnreadFile>,
}

impl ChangeSet {
    /// Was every touched file actually read? A caller enforcing a rule must
    /// check this before treating an empty entity list as "nothing to judge".
    #[must_use]
    pub fn fully_read(&self) -> bool {
        self.unread.is_empty()
    }

    /// One line naming what was not read, for the caller to surface. `None` when
    /// the change was read in full.
    #[must_use]
    pub fn unread_summary(&self) -> Option<String> {
        if self.unread.is_empty() {
            return None;
        }
        let names: Vec<&str> = self.unread.iter().map(|u| u.file.as_str()).collect();
        Some(format!(
            "{} of the changed file(s) produced NO entities and were NOT judged: {}",
            self.unread.len(),
            names.join(", ")
        ))
    }
}

/// Why a change could not be computed at all — distinct from a change that is
/// empty. The caller must not render these the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeError {
    /// Not a git work tree, or `git` is unavailable.
    NoRepo,
    /// A ref that does not resolve to a commit.
    UnresolvedRef(String),
}

impl std::fmt::Display for ChangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRepo => write!(
                f,
                "not a git work tree (or `git` is unavailable), so there is no \
                 base to diff against — the change was NOT evaluated"
            ),
            Self::UnresolvedRef(r) => write!(
                f,
                "`{r}` does not resolve to a commit, so there is no base to diff \
                 against — the change was NOT evaluated"
            ),
        }
    }
}

/// The paths between two refs, keeping "no changes" and "could not look" apart.
#[must_use]
pub fn changed_paths_checked(root: &Path, from: &str, to: &str) -> ChangedPaths {
    if !crate::git::is_repo(root) {
        return ChangedPaths::NoRepo;
    }
    for reference in [from, to] {
        if crate::git::resolve_commit(root, reference).is_none() {
            return ChangedPaths::UnresolvedRef(reference.to_string());
        }
    }
    ChangedPaths::Diffed(crate::git::changed_paths(root, from, to))
}

/// What does the change from `base` to `to` DO — which entities does it touch?
///
/// `to` may be a ref; pass `None` to diff the base against the WORKING TREE,
/// which is the shape change-time policy needs (an uncommitted proposal).
///
/// Files whose language this build cannot parse are NOT dropped: they land in
/// [`ChangeSet::unread`] with a reason. A caller that ignores `unread` is
/// enforcing a rule on a subset of the change while believing it covered all of
/// it — which is the failure this whole module is shaped around.
pub fn changed_entities(
    root: &Path,
    base: &str,
    to: Option<&str>,
) -> Result<ChangeSet, ChangeError> {
    if !crate::git::is_repo(root) {
        return Err(ChangeError::NoRepo);
    }
    let base_commit = crate::git::resolve_commit(root, base)
        .ok_or_else(|| ChangeError::UnresolvedRef(base.to_string()))?;
    let to_label = match to {
        Some(reference) => crate::git::resolve_commit(root, reference)
            .ok_or_else(|| ChangeError::UnresolvedRef(reference.to_string()))?,
        // The working tree has no commit id, and saying so is better than
        // printing a SHA that does not describe what was measured.
        None => "(working tree)".to_string(),
    };

    let paths = match to {
        Some(reference) => crate::git::changed_paths(root, &base_commit, reference),
        None => working_tree_changes(root, &base_commit),
    };

    let mut set = ChangeSet {
        base: base_commit.clone(),
        to: to_label,
        ..ChangeSet::default()
    };

    for path in paths {
        let rel = path.display().to_string();
        let Some(language) = path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .and_then(language_for_extension)
        else {
            set.unread.push(UnreadFile {
                file: rel,
                why: "this build has no grammar for it, so its entities are UNKNOWN".to_string(),
            });
            continue;
        };

        let before = symbols_at(root, &base_commit, &path, language);
        let after = match to {
            Some(reference) => symbols_at(root, reference, &path, language),
            None => symbols_in_working_tree(root, &path, language),
        };

        match (&before, &after) {
            // Both sides unreadable: the file is in the diff and we learned
            // nothing about it. Say so rather than emitting no entities.
            (None, None) => set.unread.push(UnreadFile {
                file: rel,
                why: "could not be read or parsed on EITHER side of the diff".to_string(),
            }),
            _ => {
                let before = before.unwrap_or_default();
                let after = after.unwrap_or_default();
                for name in after.difference(&before) {
                    set.entities.push(EntityChange {
                        file: rel.clone(),
                        name: name.clone(),
                        kind: "added",
                    });
                }
                for name in before.difference(&after) {
                    set.entities.push(EntityChange {
                        file: rel.clone(),
                        name: name.clone(),
                        kind: "removed",
                    });
                }
                // A symbol present on both sides in a file the diff named has, by
                // definition, had something change around it. Reporting it as
                // `modified` is deliberately coarse — line-level attribution is
                // the CPG tier's job — but omitting it would under-report a
                // change that edited a function's body without renaming it, which
                // is the most common change there is.
                for name in before.intersection(&after) {
                    set.entities.push(EntityChange {
                        file: rel.clone(),
                        name: name.clone(),
                        kind: "modified",
                    });
                }
            }
        }
    }

    set.entities.sort_by(|a, b| {
        (a.file.as_str(), a.name.as_str(), a.kind).cmp(&(b.file.as_str(), b.name.as_str(), b.kind))
    });
    Ok(set)
}

/// Paths differing between `base` and the WORKING TREE, including untracked
/// files — an uncommitted proposal is exactly what change-time policy judges.
fn working_tree_changes(root: &Path, base: &str) -> Vec<PathBuf> {
    let mut paths: BTreeSet<PathBuf> = crate::git::changed_paths(root, base, "HEAD")
        .into_iter()
        .collect();
    // Tracked-but-uncommitted, plus untracked-but-not-ignored.
    for args in [
        vec!["diff", "--name-only", "HEAD"],
        vec!["ls-files", "--others", "--exclude-standard"],
    ] {
        if let Some(out) = crate::git::run(root, &args) {
            paths.extend(out.lines().filter(|l| !l.is_empty()).map(PathBuf::from));
        }
    }
    paths.into_iter().collect()
}

/// Symbol names in `path` at `reference`. `None` = could not read or parse (as
/// distinct from `Some(empty)`, a file that genuinely defines nothing).
fn symbols_at(
    root: &Path,
    reference: &str,
    path: &Path,
    language: &str,
) -> Option<BTreeSet<String>> {
    let source = crate::git::read_blob_at(root, reference, path)?;
    let structure = extract_structure(&source, language).ok()?;
    Some(structure.symbols.into_iter().map(|s| s.name).collect())
}

/// Symbol names in the working-tree copy of `path`. `None` = could not read or
/// parse; a DELETED file reads as `None` here and as `Some` at the base, which
/// is what makes its symbols report as `removed`.
fn symbols_in_working_tree(root: &Path, path: &Path, language: &str) -> Option<BTreeSet<String>> {
    let source = std::fs::read_to_string(root.join(path)).ok()?;
    let structure = extract_structure(&source, language).ok()?;
    Some(structure.symbols.into_iter().map(|s| s.name).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repo with one commit on `main`, returned with its commit id.
    fn repo_with(files: &[(&str, &str)]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).unwrap();
        }
        run(&["add", "-A"]);
        run(&["commit", "-qm", "base"]);
        let base = crate::git::head_commit(dir.path()).unwrap();
        (dir, base)
    }

    #[test]
    fn an_added_symbol_in_the_working_tree_is_reported() {
        let (dir, base) = repo_with(&[("leaf.rs", "fn leaf() {}\n")]);
        std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\nfn sprout() {}\n").unwrap();

        let set = changed_entities(dir.path(), &base, None).unwrap();
        assert!(set.fully_read(), "unread: {:?}", set.unread);
        assert!(set
            .entities
            .iter()
            .any(|e| e.name == "sprout" && e.kind == "added"));
        assert_eq!(set.to, "(working tree)");
    }

    #[test]
    fn a_removed_symbol_is_reported() {
        let (dir, base) = repo_with(&[("leaf.rs", "fn leaf() {}\nfn gone() {}\n")]);
        std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();

        let set = changed_entities(dir.path(), &base, None).unwrap();
        assert!(set
            .entities
            .iter()
            .any(|e| e.name == "gone" && e.kind == "removed"));
    }

    #[test]
    fn an_untracked_file_counts_as_part_of_the_proposal() {
        // A change-time rule judges what is ABOUT to land, and a brand-new file
        // is the most obvious thing a rule wants to see.
        let (dir, base) = repo_with(&[("leaf.rs", "fn leaf() {}\n")]);
        std::fs::write(dir.path().join("new.rs"), "fn fresh() {}\n").unwrap();

        let set = changed_entities(dir.path(), &base, None).unwrap();
        assert!(set
            .entities
            .iter()
            .any(|e| e.name == "fresh" && e.file == "new.rs"));
    }

    #[test]
    fn a_change_that_touches_nothing_is_an_empty_measurement_not_a_failure() {
        let (dir, base) = repo_with(&[("leaf.rs", "fn leaf() {}\n")]);
        let set = changed_entities(dir.path(), &base, None).unwrap();
        assert!(set.entities.is_empty());
        assert!(set.fully_read(), "an empty change must still be fully read");
        assert!(set.unread_summary().is_none());
    }

    /// THE distinction this module exists for: "nothing changed" and "I could
    /// not look" must not both be an empty list.
    #[test]
    fn outside_a_repo_is_an_error_not_an_empty_change() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            changed_entities(dir.path(), "main", None).unwrap_err(),
            ChangeError::NoRepo
        );
        assert_eq!(
            changed_paths_checked(dir.path(), "a", "b"),
            ChangedPaths::NoRepo
        );
    }

    #[test]
    fn an_unresolved_ref_is_an_error_not_an_empty_change() {
        let (dir, _base) = repo_with(&[("leaf.rs", "fn leaf() {}\n")]);
        let err = changed_entities(dir.path(), "no-such-ref", None).unwrap_err();
        assert_eq!(err, ChangeError::UnresolvedRef("no-such-ref".to_string()));
        assert!(err.to_string().contains("NOT evaluated"));
    }

    /// A file the change touches whose language this build cannot parse must be
    /// NAMED, not dropped. Dropping it produces a change-set that looks complete
    /// and silently excludes part of the change from every rule.
    #[test]
    fn an_unparseable_file_is_reported_unread_not_omitted() {
        let (dir, base) = repo_with(&[("leaf.rs", "fn leaf() {}\n")]);
        std::fs::write(dir.path().join("notes.md"), "# hi\n").unwrap();

        let set = changed_entities(dir.path(), &base, None).unwrap();
        assert!(!set.fully_read());
        assert_eq!(set.unread.len(), 1);
        assert_eq!(set.unread[0].file, "notes.md");
        assert!(set.unread[0].why.contains("no grammar"));
        assert!(set.unread_summary().unwrap().contains("NOT judged"));
    }

    #[cfg(feature = "langs-extra")]
    #[test]
    fn a_python_change_is_read_like_a_rust_one() {
        let (dir, base) = repo_with(&[("leaf.py", "def leaf():\n    return 1\n")]);
        std::fs::write(
            dir.path().join("leaf.py"),
            "def leaf():\n    return 1\n\n\ndef sprout():\n    return 2\n",
        )
        .unwrap();

        let set = changed_entities(dir.path(), &base, None).unwrap();
        assert!(set.fully_read(), "unread: {:?}", set.unread);
        assert!(
            set.entities
                .iter()
                .any(|e| e.name == "sprout" && e.kind == "added"),
            "{:?}",
            set.entities
        );
    }

    #[test]
    fn two_commits_can_be_diffed_directly() {
        let (dir, base) = repo_with(&[("leaf.rs", "fn leaf() {}\n")]);
        std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\nfn second() {}\n").unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
        };
        run(&["add", "-A"]);
        run(&["commit", "-qm", "second"]);
        let head = crate::git::head_commit(dir.path()).unwrap();

        let set = changed_entities(dir.path(), &base, Some(&head)).unwrap();
        assert_eq!(set.to, head);
        assert!(set
            .entities
            .iter()
            .any(|e| e.name == "second" && e.kind == "added"));
    }
}
