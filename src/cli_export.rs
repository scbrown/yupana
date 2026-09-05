//! `yupana export` — print the referential structure as Turtle.
//!
//! Split out of `cli_cmds` because repository IDENTITY resolution lives here,
//! and identity is the part of an export that later stages cannot repair: the
//! repo name is a segment of every IRI, so it decides whether two exports of
//! the same code describe one graph or two.

use std::path::Path;

use crate::export;

impl crate::cli::Cli {
    /// The `yupana export` dispatch arm.
    ///
    /// Lives beside the exporter rather than in the match, because the decision
    /// it makes is an export decision: `export --to <quipu>` IS promotion (spec
    /// §15's other spelling), so it routes through the ONE promotion path —
    /// validate, then write — rather than a second implementation that could
    /// drift from `promote`. It is a write, so the guard is honoured first.
    pub(super) fn export_arm(
        &self,
        path: &Path,
        repo: Option<&str>,
        format: &crate::cli::ExportFormat,
        to: Option<&str>,
    ) -> anyhow::Result<()> {
        let crate::cli::ExportFormat::Turtle = format;
        match to {
            Some(_) => {
                self.load_config(path)?.write_guard("promotion")?;
                self.promote(path, "HEAD", to, repo, false, false)
            }
            None => export(path, repo),
        }
    }
}

pub(crate) fn export(path: &Path, repo: Option<&str>) -> anyhow::Result<()> {
    // Identity chain: explicit --repo, else the origin remote's repo name, else
    // the directory basename. The dir-name fallback survives ONLY here — plain
    // `export` prints locally and writes nothing — while the promote paths refuse
    // instead: a guessed identity in a WRITE fragments the shared graph.
    //
    // But "prints locally and writes nothing" under-describes where this output
    // now goes: stdout IS the producer input to a share bundle, so a guessed
    // identity reaches a write one hop later, having passed no check on the way.
    // Measured: byte-identical source under two directory names shares ZERO
    // entity IRIs, which imports as a parallel unmergeable graph rather than as
    // a conflict. Keep the fallback — reading a non-git tree is a real use — but
    // say so on stderr, so the guess cannot be silent. stdout is untouched, so
    // every existing capture keeps working.
    let mut guessed_from_dir = None;
    let repo = repo.map_or_else(
        || {
            crate::git::origin_repo_name(path).unwrap_or_else(|| {
                let name = path
                    .canonicalize()
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "repo".to_string());
                guessed_from_dir = Some(name.clone());
                name
            })
        },
        str::to_string,
    );
    // Deliberately not gated on `--quiet`: that suppresses progress chatter, and
    // an unstable identity in the data is not chatter. The promote-path refusals
    // ignore it for the same reason.
    if let Some(name) = &guessed_from_dir {
        eprintln!(
            "yupana export: no --repo and no `origin` remote at {} — attributing every \
             entity to the DIRECTORY NAME `{name}`. That name is a segment of every IRI, \
             so the same code exported from a different checkout path produces a \
             disjoint set of entities. Fine for reading; pass --repo <name> for output \
             that is captured, shared, or imported.",
            path.display()
        );
    }
    let turtle = export::to_turtle(path, &repo)?;
    print!("{turtle}");
    Ok(())
}
