//! `yupana promote` — the Phase-4 Quipu promotion command, split out of `cli` for
//! size (yupana #83). See `cli_analyze` for why this is a child module.
//!
//! Two `cfg` forms of one method: the real path under `quipu`, and a stub that
//! prints the phase notice without it.

use super::*;

use crate::promote_trigger::{decide, Decision, Trigger};

/// The arguments to `yupana promote`.
///
/// Flattened out of the `Commands` enum so promotion's surface lives beside
/// promotion's code: every one of these is an authorization or an identity the
/// caller must state, and their reasoning belongs next to the checks that
/// enforce it rather than in the dispatcher.
#[derive(clap::Args, Debug)]
pub struct PromoteArgs {
    /// Commit-ish to promote.
    #[arg(long, default_value = "HEAD")]
    pub commit: String,
    /// Quipu base URL to promote into (e.g. `http://localhost:8080`).
    /// REQUIRED for a write, and it is the ONLY thing that authorizes one: a
    /// discovered `[yupana.quipu] endpoint` is deliberately NOT enough, because
    /// that key is set host-wide so the pre-edit guard can READ the rule
    /// catalogue. Without `--to`, promotion refuses and names the endpoint it
    /// found. `--dry-run` needs no target.
    #[arg(long)]
    pub to: Option<String>,
    /// Extract and SHACL-validate the projection, then STOP — write nothing.
    /// Answers "would this promotion conform?" without touching the graph.
    #[arg(long)]
    pub dry_run: bool,
    /// Replace the complete per-repository code snapshot atomically. This
    /// is explicit because it authorizes absence (including an empty tree)
    /// to retract facts from the prior snapshot.
    #[arg(long)]
    pub replace_snapshot: bool,
    /// Repository name to attribute promoted entities to. Defaults to the
    /// `origin` remote's repo name; with no origin, promotion refuses rather
    /// than deriving identity from the directory name (a worktree's dir name
    /// mints wrong IRIs and fragments the graph).
    #[arg(long)]
    pub repo: Option<String>,
    /// The event that invoked this promotion, for a git hook or CI step to
    /// declare. `[yupana.quipu] promote_on` decides whether that event
    /// promotes (FR-19); the default `manual` always does.
    #[arg(long, value_enum, default_value_t = crate::promote_trigger::Trigger::Manual)]
    pub trigger: crate::promote_trigger::Trigger,
    /// Write only the files changed since `--base`, each under its own
    /// per-file producer key, instead of replacing the repo-wide snapshot.
    /// The tree is still extracted in FULL, so cross-file references resolve
    /// as they always do; what shrinks is the WRITE. Needs `--base`, and
    /// conflicts with `--replace-snapshot` — this IS that, per file.
    #[arg(long, requires = "base", conflicts_with = "replace_snapshot")]
    pub subset: bool,
    /// Base commit-ish for `--subset`. MUST be the last SUCCESSFULLY
    /// PROMOTED commit, never the last commit SEEN: a marker advanced on a
    /// poll that skipped the promote leaves every commit in between silently
    /// unwritten forever. Advance your marker only on exit 0, and only to
    /// the `promoted-commit:` sha this prints. See `cli_promote.rs`.
    #[arg(long)]
    pub base: Option<String>,
    /// Directory to promote (defaults to current dir).
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

impl Cli {
    /// Does `[yupana.quipu] promote_on` admit a promotion invoked by `trigger`?
    ///
    /// FR-19's trigger, and the reader `promote_on` did not have. Ungated by
    /// feature on purpose: the policy answer must be the same whether or not
    /// this binary can promote, so a `--trigger`-driven caller sees "skipped by
    /// policy" rather than "built without the feature" when both are true.
    ///
    /// A decline returns `false` and the caller exits **0**. That is the one
    /// place this repo's "a non-promotion must never read as a promotion" rule
    /// is deliberately not applied, and the reason is that here the two are the
    /// same thing: `promote_on = "merge"` skipping a plain commit IS the
    /// configuration doing its job, and a `post-commit` hook that exited
    /// non-zero on every ordinary commit would be turned off within a day. The
    /// line printed says WROTE NOTHING in as many words so a log reader is never
    /// left inferring it, which is the protection the exit code was carrying.
    pub(super) fn trigger_admits(
        &self,
        path: &Path,
        commit: &str,
        trigger: Trigger,
    ) -> anyhow::Result<bool> {
        let promote_on = self.load_config(path)?.quipu.promote_on;
        // Only asked when it can change the answer — a manual invocation never
        // needs git, so a promotion outside a repo is unaffected by this gate.
        let is_merge = match trigger {
            Trigger::Manual => false,
            _ => crate::git::is_merge_commit(path, commit),
        };
        match decide(&promote_on, trigger, is_merge)? {
            Decision::Promote => Ok(true),
            Decision::Declined(why) => {
                if !self.quiet {
                    println!("yupana promote: {why}");
                }
                Ok(false)
            }
        }
    }
    /// Promote a tree's structural facts into Quipu: emit Turtle, SHACL-validate it
    /// in-process, and write it iff it conforms (FR-19/20/21).
    #[cfg(feature = "quipu")]
    #[allow(clippy::unused_self)]
    // method form for call-site symmetry with the stub arm
    // Eight, and each one is a distinct authorization the caller must state
    // explicitly: the target, the identity, the ref, and three separate
    // opt-ins to a write. Bundling them into a struct would let a caller
    // build a default and get a live graph write it never named.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn promote(
        &self,
        path: &Path,
        commit: &str,
        to: Option<&str>,
        repo: Option<&str>,
        dry_run: bool,
        replace_snapshot: bool,
        subset_base: Option<&str>,
    ) -> anyhow::Result<()> {
        // `--to` IS THE AUTHORIZATION, and it is the only one (aegis-o2h97).
        //
        // This used to fall back to a discovered `[yupana.quipu] endpoint`, which
        // reads as reasonable and is not: that key is set in the HOST-WIDE
        // `~/.config/bobbin/config.toml` so the pre-edit guard can READ quipu's
        // rule catalogue (aegis-m9ln), so on every agent machine in this fleet a
        // bare `yupana promote` in any checkout wrote to the live graph. It was
        // found by someone running it expecting a dry run and getting a real
        // 25k-triple promotion; the write happened to be the wanted one that
        // time. A config set up to authorize READS must not silently authorize
        // WRITES, and the flag's own help promised a refusal that never fired.
        //
        // Nothing measured depended on the fallback: the one scheduled promoter
        // in this deployment passes `--to "$QUIPU_URL"` explicitly and refuses to
        // default it. The MCP tool keeps its own discovery — there the
        // endpoint comes from a server an operator deliberately started, not from
        // whatever config is ambient in an agent's shell.
        let cfg = self.load_config(path)?;
        // §9.4 branch modeling (GH #4), refused HERE — before any tree is read —
        // when the configured model is the named-graph one nothing implements.
        // A misconfiguration should cost a message, not a full projection.
        let branch_model = crate::promote_branch::parse(&cfg.quipu.branch_model)?;
        crate::promote_branch::ensure_implemented(branch_model)?;
        let discovered = Some(cfg.quipu.endpoint.clone()).filter(|e| !e.is_empty());
        let endpoint: Option<String> = match to {
            // An empty `--to` is not a target; it would post to `/knot` on the
            // empty host. Treated as absent so it lands on a refusal, not a URL.
            Some(t) if !t.is_empty() => Some(t.to_string()),
            // A dry run writes nothing, so a discovered endpoint is safe to use —
            // and naming it is the useful half: it tells the operator which graph
            // a real run would have hit.
            _ if dry_run => discovered,
            _ => match discovered {
                Some(found) => anyhow::bail!(
                    "refusing to promote into a DISCOVERED endpoint: {found}\n  \
                     `[yupana.quipu] endpoint` is configured so the pre-edit guard can READ the \
                     rule catalogue. It does not authorize a write, and a promotion is a live \
                     graph write with no undo.\n  \
                     To write there, say so:        yupana promote --to {found}\n  \
                     To check it without writing:   yupana promote --dry-run"
                ),
                None => anyhow::bail!(
                    "no Quipu endpoint: pass --to <url> for a real promotion, or --dry-run to \
                     validate without writing. Refusing rather than guessing a graph to write into."
                ),
            },
        };
        // Repo identity is DATA IDENTITY: it is a segment of every promoted IRI.
        // Explicit --repo wins; otherwise the origin remote names the repository.
        // With neither, REFUSE — the old fallback (directory basename) minted
        // `code/<worktree-dir>/…` islands from agent worktrees and CI workspaces,
        // structurally fragmenting one repo into unmergeable parallel graphs.
        let repo = match repo {
            Some(r) => r.to_string(),
            None => crate::git::origin_repo_name(path).ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot determine repository identity: no `origin` remote at {}. \
                     Pass --repo <name>. Refusing rather than deriving identity from \
                     the directory name, which fragments the graph.",
                    path.display()
                )
            })?,
        };
        // Promotion reads the COMMITTED tree at `commit` (FR-22): uncommitted
        // working-tree churn — an in-flight overlay edit, an unsaved buffer —
        // must never reach Quipu. Checked here, after the config preconditions
        // (endpoint/identity) and just before the read, so it refuses a phantom
        // ref rather than promoting against one.
        if crate::git::resolve_commit(path, commit).is_none() {
            anyhow::bail!(
                "cannot promote `{commit}`: it does not resolve to a commit at {}. \
                 Refusing rather than promoting a phantom ref.",
                path.display()
            );
        }
        // `--subset` preconditions answered here, BEFORE the tree is read: a bad
        // base should cost a message, not a full projection.
        let subset_changed = match subset_base {
            Some(base) => Some(crate::promote_subset_cli::subset_preflight(
                path, base, commit,
            )?),
            None => None,
        };
        let mut turtle = crate::export::to_turtle_at(path, &repo, commit)?;
        // A promotion that extracted NOTHING is not a promotion — it is a green
        // empty write (measured: a Python repo "promoted: 0 triples" with a
        // SUCCESS exit while analyze saw 1647 symbols, and the scheduler wrote
        // its done-marker over the void). Say so and refuse (exit 2, the
        // could-not-promote code), so a marker-disciplined caller retries
        // instead of booking emptiness as done.
        // A subset promote replaces per-file snapshots, so like `--replace-snapshot`
        // it legitimately authorizes absence — a deleted file's partition IS empty.
        // The refusal below is for the OTHER shape: a promote that meant to assert
        // a tree and extracted nothing.
        if !replace_snapshot && subset_base.is_none() && !turtle.contains("bobbin:CodeModule") {
            eprintln!(
                "yupana promote: extracted NOTHING from {} — no parseable source                  files under this tree for the grammars in this build. Refusing                  to promote an empty graph as success. (Is the language behind                  the `langs-extra` feature? Is the path right?)",
                path.display()
            );
            std::process::exit(2);
        }
        // §9.7 (GH #5): the `commit → touched entities` provenance edge, produced
        // HERE because this is where both halves are already in hand — the
        // commit being promoted and the entities just projected. Appended
        // BEFORE the branch qualifier so the commit node is qualified too: the
        // commit is on that branch as much as anything it touched.
        if let Some(prov) = crate::promote_provenance::commit_turtle(path, &repo, commit, &turtle) {
            turtle.push_str(&prov);
        }
        // §9.4's qualifier fallback: tag every promoted entity with the branch
        // its facts came from, so branch-scoped queries are answerable with no
        // Quipu change. A branch that cannot be determined is OMITTED, never
        // guessed — the same absent-beats-invented rule FR-3 freshness follows.
        let branch = crate::git::branch_for(path, commit);
        if branch.is_none() && !self.quiet {
            eprintln!(
                "yupana promote: no branch resolves for `{commit}` — promoting WITHOUT a \
                 `bobbin:onBranch` qualifier rather than guessing one. Promote from an \
                 attached checkout, or pass a branch name to --commit, to qualify these facts."
            );
        }
        let turtle = crate::promote_branch::qualify(branch_model, &turtle, branch.as_deref())?;
        // Provenance carries the RESOLVED commit SHA, not the ref spelling —
        // `--commit main` and `--commit <sha>` promoting the same tree record
        // the same source (partial FR-21; full bitemporal fields are a #15
        // follow-up).
        let resolved =
            crate::git::resolve_commit(path, commit).unwrap_or_else(|| commit.to_string());
        let source = format!("yupana promote {repo}@{resolved} (cli)");
        // SUBSET: write only the changed files' partitions, each under its own
        // producer key. Diverges here, AFTER the projection is complete, because
        // the whole point is that the READ is unchanged — see `promote_subset`.
        if let (Some(base), Some(changed)) = (subset_base, subset_changed.as_ref()) {
            return crate::promote_subset_cli::promote_subset(
                path,
                base,
                commit,
                &repo,
                changed,
                &turtle,
                &source,
                endpoint.as_deref(),
                dry_run,
            );
        }
        let outcome = match (dry_run, &endpoint) {
            (true, ep) => crate::promote::dry_run(ep.as_deref(), &turtle, &source)?,
            (false, Some(ep)) if replace_snapshot => {
                crate::promote::promote_snapshot(ep, &turtle, &source, &format!("code:{repo}"))?
            }
            (false, Some(ep)) => crate::promote::promote(ep, &turtle, &source)?,
            // Unreachable: the resolution above bails on a write with no target.
            // Spelled as a refusal rather than an `expect` so that if that match
            // is ever edited, a missing target degrades to a refusal, never a panic
            // and never a guessed graph.
            (false, None) => anyhow::bail!(
                "no Quipu endpoint for a write: pass --to <url>, or --dry-run to validate only."
            ),
        };
        let mut out = std::io::stdout();
        let wrote = outcome.report(&mut out)?;
        // A refusal is a could-not-promote, not a success: exit non-zero so a script
        // cannot read a rejected promotion as a landed one.
        if !wrote {
            std::process::exit(2);
        }
        Ok(())
    }

    /// Without the `quipu` feature, promotion is unbuilt — say so honestly,
    /// and exit non-zero: the same rule as the feature arm's refusal path. A
    /// stub that exits 0 reads as a landed promotion to any script — measured
    /// 2026-07-23 (aegis-ucoh): the quipu-ingest cron ran a feature-less
    /// binary, took exit 0 as success, and advanced its promote marker past a
    /// commit that was never promoted.
    #[cfg(not(feature = "quipu"))]
    // Same eight arguments as the feature arm, and for the same reason: the two
    // signatures must match exactly or the call site stops compiling in one
    // build and not the other. The default-feature clippy arm is the ONLY one
    // that sees this function — `--features quipu` compiles the other half — so
    // a lint satisfied only there is a false green.
    #[allow(clippy::unused_self, clippy::too_many_arguments)]
    pub(super) fn promote(
        &self,
        _path: &Path,
        _commit: &str,
        _to: Option<&str>,
        _repo: Option<&str>,
        _dry_run: bool,
        _replace_snapshot: bool,
        _subset_base: Option<&str>,
    ) -> anyhow::Result<()> {
        self.planned(
            "promote",
            4,
            "Quipu promotion needs `--features quipu` (this binary was built without it)",
        );
        std::process::exit(2);
    }
}
