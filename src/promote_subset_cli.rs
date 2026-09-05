//! The `yupana promote --subset` command path (aegis-8o7r10).
//!
//! [`crate::promote_subset`] is the DOMAIN half — it decides which file owns
//! which fact and what a subset promote would write. This module is the CLI
//! half: preconditions, ordering, the write loop and what an operator is told.
//! They are apart because the domain half is pure and heavily tested against
//! fixtures and a real projection, while this one talks to git, the network and
//! the process exit code.

use std::path::Path;
use std::process::Command;

/// Is `ancestor` reachable from `descendant`?
///
/// `Some(true)`/`Some(false)` is the ANSWER; `None` means the question could not
/// be ASKED — no git, not a repo, an unresolvable ref. Kept apart because a
/// caller refusing on "not an ancestor" must not refuse identically on "I could
/// not look": the second is a broken environment, not a bad argument.
///
/// Reads the exit status directly because `merge-base --is-ancestor` answers
/// through it (0 yes, 1 no) with no stdout, which the shared `git::git` helper —
/// mapping every non-zero exit to `None` — cannot express.
#[must_use]
pub(crate) fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Option<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .ok()?;
    match output.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        // 128 (and anything else) is "could not look": a bad ref, not a repo.
        _ => None,
    }
}

/// The `--subset` preconditions, answered BEFORE the tree is read.
///
/// Placed early deliberately: a misconfiguration should cost a message, not
/// a full projection — the same rule the branch-model refusal in
/// [`Self::promote`] follows. Returns the repo-relative paths to write.
pub(crate) fn subset_preflight(
    path: &Path,
    base: &str,
    commit: &str,
) -> anyhow::Result<Vec<String>> {
    use crate::change::ChangedPaths;

    // A base that is not an ANCESTOR of what is being promoted means the two
    // refs diverged, and `base..commit` then silently omits everything on the
    // base's own side. Under `--replace-snapshot` a wrong base costs nothing —
    // the whole snapshot is rewritten either way — so this class of mistake
    // has never had a consequence here and has never had a check.
    //
    // It does NOT catch the marker hazard `--base`'s help describes (a
    // last-SEEN marker that ran ahead IS an ancestor), and it must not be read
    // as covering it. It catches the other one: a base on a different line of
    // history entirely.
    match is_ancestor(path, base, commit) {
        Some(true) => {}
        Some(false) => anyhow::bail!(
            "--subset base `{base}` is not an ancestor of `{commit}`: the delta between \
             them would omit everything on the base's own side, and a per-file snapshot \
             write cannot be undone by the next run.\n  \
             Promote a commit descended from the base, or do a full \
             `--replace-snapshot` resync to re-establish one."
        ),
        None => anyhow::bail!(
            "could not determine whether `{base}` is an ancestor of `{commit}` at {}. \
             Refusing: an unanswerable precondition is not a satisfied one.",
            path.display()
        ),
    }

    // "Nothing changed" and "I could not look" are OPPOSITE facts, and the
    // diff collapses them into an empty vec unless the typed form is used.
    // Reading the second as the first would promote nothing and report
    // success — the shape that lets a scheduler advance its marker over a
    // commit that was never promoted (aegis-ucoh).
    match crate::change::changed_paths_checked(path, base, commit) {
        ChangedPaths::Diffed(paths) => Ok(paths.iter().map(|p| p.display().to_string()).collect()),
        ChangedPaths::NoRepo => anyhow::bail!(
            "--subset needs a git work tree at {} to diff against `{base}`; \
             refusing rather than promoting an unknown file set.",
            path.display()
        ),
        ChangedPaths::UnresolvedRef(r) => anyhow::bail!(
            "--subset base `{r}` does not resolve to a commit at {}; \
             refusing rather than promoting an unknown file set.",
            path.display()
        ),
    }
}

/// Write one snapshot per CHANGED FILE instead of one for the repository.
///
/// Reached only from [`Self::promote`], and only after the FULL projection
/// has been built: the read is deliberately unchanged, because a projection
/// extracted from the changed files alone does not resolve their references
/// into unchanged files and snapshot replacement would then retract edges
/// that are still true (aegis-8o7r10).
///
/// ## Every failure here refuses; none writes half a graph
///
/// The writes are separate transactions — quipu has no multi-key atomic
/// replace — so a mid-run failure leaves earlier files written and later ones
/// not. That is a PARTIAL promote, and it is reported as one with the keys
/// that landed named, rather than as a failure that implies nothing changed.
/// The next run over the same base is idempotent and completes it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn promote_subset(
    path: &Path,
    base: &str,
    commit: &str,
    repo: &str,
    changed: &[String],
    turtle: &str,
    source: &str,
    endpoint: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<()> {
    // An EMPTY diff is a measurement, not an error — and it must not be
    // dressed up as one. Nothing to write is the correct outcome for a
    // re-promote of an unchanged tree, which is exactly the case this whole
    // feature exists to make cheap.
    if changed.is_empty() {
        println!(
            "yupana promote --subset: no files changed between {base} and {commit} — \
             nothing to write."
        );
        // The marker still advances here, and correctly: the graph is current
        // as of `commit` precisely because there was nothing to write.
        println!(
            "promoted-commit: {}",
            crate::git::resolve_commit(path, commit).unwrap_or_else(|| commit.to_string())
        );
        return Ok(());
    }

    let plan = crate::promote_subset::plan(repo, turtle, changed)?;
    let files = plan.writes.iter().filter(|w| w.file.is_some()).count();
    println!(
        "yupana promote --subset: {files} changed file(s), {} retraction(s), \
         {} triple(s) across {} key(s); {} file(s) in the projection untouched.",
        plan.retractions(),
        plan.triples(),
        plan.writes.len(),
        plan.unchanged_files
    );

    if dry_run {
        // A dry run validates the SAME documents that would be written, one
        // per key, rather than the concatenation — a payload that validates
        // whole can still contain a partition that does not stand alone.
        for w in &plan.writes {
            let outcome = crate::promote::dry_run(endpoint, &w.turtle, source)?;
            let mut out = std::io::stdout();
            print!("  {} ({} triples): ", w.key, w.triples);
            if !outcome.report(&mut out)? {
                std::process::exit(2);
            }
        }
        return Ok(());
    }

    let Some(ep) = endpoint else {
        anyhow::bail!(
            "no Quipu endpoint for a write: pass --to <url>, or --dry-run to validate only."
        );
    };

    let mut wrote = 0usize;
    let mut triples = 0usize;
    for w in &plan.writes {
        match crate::promote::promote_snapshot(ep, &w.turtle, source, &w.key) {
            Ok(outcome) => {
                let mut sink = std::io::sink();
                if !outcome.report(&mut sink)? {
                    // A refusal is not a crash, and a PARTIAL promote must
                    // never exit 0: a marker-disciplined caller would book
                    // the whole range as done.
                    eprintln!(
                        "yupana promote --subset: REFUSED at key {} after {wrote} key(s) \
                         already written. This is a PARTIAL promote — re-run with the same \
                         --base to complete it (each key is replaced, so the re-run is \
                         idempotent).",
                        w.key
                    );
                    std::process::exit(2);
                }
                wrote += 1;
                triples += w.triples;
            }
            Err(e) => {
                eprintln!(
                    "yupana promote --subset: FAILED at key {} after {wrote} key(s) already \
                     written: {e}\n  This is a PARTIAL promote, NOT a no-op. Re-run with the \
                     same --base to complete it.",
                    w.key
                );
                std::process::exit(2);
            }
        }
    }
    println!("yupana promote --subset: wrote {wrote} key(s), {triples} triple(s).");
    // THE MARKER CONTRACT (malcolm, aegis-8o7r10). A caller tracking what it
    // has promoted must advance ONLY on this line, and only to this sha —
    // never to whatever it happened to see when it polled. Printed only on
    // the all-keys-written path, so a partial promote (which exits 2 above)
    // cannot be mistaken for one that completed.
    println!(
        "promoted-commit: {}",
        crate::git::resolve_commit(path, commit).unwrap_or_else(|| commit.to_string())
    );
    Ok(())
}
