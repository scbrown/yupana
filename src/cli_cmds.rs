//! Implementations of the call-graph and dataflow CLI commands.
//!
//! These live outside `cli.rs` to keep that file small; they take the two
//! output flags they need (`json`, `quiet`) rather than the whole `Cli`.

use std::collections::BTreeSet;
use std::path::Path;

use colored::Colorize;

use crate::dataflow::{Dataflow, FlowDir};
use crate::export;
use crate::graph::{CodeGraph, Dir};
use crate::reconcile::reconcile;
use crate::render::{print_reached, reached_json};

/// `yupana callers` — direct callers and callees of a symbol.
pub(crate) fn callers(json: bool, quiet: bool, symbol: &str, path: &Path) -> anyhow::Result<()> {
    let graph = CodeGraph::build(path)?;
    if !graph.has_symbol(symbol) {
        return not_found(json, quiet, symbol, "call graph");
    }
    let callers = graph.direct(symbol, Dir::Callers);
    let callees = graph.direct(symbol, Dir::Callees);

    if json {
        let out = serde_json::json!({
            "symbol": symbol,
            "callers": reached_json(&callers),
            "callees": reached_json(&callees),
            // Provenance tier of the answer (FR-3). The call graph is tree-sitter,
            // so this is served, not left unlabelled — an empty result declares it
            // too. Matches the MCP NeighborsResponse.tier.
            "tier": "treesitter",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print_reached(&format!("callers of {symbol}"), &callers, quiet);
        print_reached(&format!("callees of {symbol}"), &callees, quiet);
    }
    Ok(())
}

/// `yupana communities` — densely-connected clusters of symbols (FR-9, Louvain).
pub(crate) fn communities(json: bool, quiet: bool, path: &Path) -> anyhow::Result<()> {
    let graph = CodeGraph::build(path)?;
    let comms = graph.communities();

    if json {
        let rows: Vec<_> = comms
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "size": c.members.len(),
                    "members": c.members.iter().map(|m| serde_json::json!({
                        "name": m.name,
                        "kind": m.kind,
                        "file": m.file,
                        "start_line": m.start_line,
                        // See the note in `cli::refs`: the wire form is
                        // "treesitter", not the derive's "tree_sitter". This
                        // document already tags "treesitter" at the top level.
                        "tier": m.tier.as_str(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let out = serde_json::json!({
            "count": comms.len(),
            "communities": rows,
            "tier": "treesitter",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else if comms.is_empty() {
        if !quiet {
            println!("no communities (empty call graph)");
        }
    } else {
        for c in &comms {
            println!(
                "{} {} ({} symbol(s))",
                "community".green().bold(),
                c.id,
                c.members.len()
            );
            for m in &c.members {
                println!(
                    "  {}:{} {} [{:?}]",
                    m.file,
                    m.start_line,
                    m.name.cyan(),
                    m.tier
                );
            }
        }
    }
    Ok(())
}

/// `yupana impact` — the blast radius (transitive callers) of a symbol,
/// optionally reconciled against a caller-supplied co-change set (FR-11).
pub(crate) fn impact(
    json: bool,
    quiet: bool,
    symbol: &str,
    path: &Path,
    hops: u32,
    cochange: Option<&Path>,
) -> anyhow::Result<()> {
    let graph = CodeGraph::build(path)?;
    if !graph.has_symbol(symbol) {
        return not_found(json, quiet, symbol, "call graph");
    }
    let reached = graph.reachable(symbol, Dir::Callers, hops);
    let structural_files: BTreeSet<String> = reached.iter().map(|r| r.file.clone()).collect();
    let cochange_set = cochange.map(read_cochange).transpose()?;

    if json {
        let mut out = serde_json::json!({
            "symbol": symbol,
            "hops": hops,
            "direction": "callers",
            "count": reached.len(),
            "reachable": reached_json(&reached),
            "structural_files": structural_files.iter().collect::<Vec<_>>(),
            // Blast radius is the trust-boundary surface (FR-25); serving it
            // unlabelled is exactly what FR-3 forbids. tree-sitter tier, tagged.
            "tier": "treesitter",
        });
        if let Some(cochange_set) = &cochange_set {
            let recon = reconcile(&structural_files, cochange_set);
            out["reconciliation"] = serde_json::json!({
                "corroborated": recon.corroborated,
                "structural_only": recon.structural_only,
                "cochange_only": recon.cochange_only,
            });
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if reached.is_empty() {
        if !quiet {
            println!("nothing calls {symbol} (blast radius empty)");
        }
        return Ok(());
    }
    println!(
        "{} {} symbol(s) affected by changing {symbol}:",
        "impact".green().bold(),
        reached.len()
    );
    for item in &reached {
        println!(
            "  {}:{} {} (hop {})",
            item.file,
            item.start_line,
            item.name.cyan(),
            item.distance
        );
    }
    if let Some(cochange_set) = &cochange_set {
        let recon = reconcile(&structural_files, cochange_set);
        println!(
            "\nreconciled with {} co-changed file(s):",
            cochange_set.len()
        );
        print_bucket("corroborated (real coupling)", &recon.corroborated, quiet);
        print_bucket(
            "structural only (new/unexercised)",
            &recon.structural_only,
            quiet,
        );
        print_bucket(
            "co-change only (refactoring smell)",
            &recon.cochange_only,
            quiet,
        );
    }
    Ok(())
}

/// Read a co-change file: a JSON array of paths or a newline-separated list.
fn read_cochange(path: &Path) -> anyhow::Result<BTreeSet<String>> {
    let text = std::fs::read_to_string(path)?;
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&text) {
        return Ok(list.into_iter().collect());
    }
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// Print one reconciliation bucket.
fn print_bucket(label: &str, files: &[String], quiet: bool) {
    if files.is_empty() {
        if !quiet {
            println!("  {label}: (none)");
        }
        return;
    }
    println!("  {}: {}", label.bold(), files.join(", "));
}

/// `yupana dataflow` — intra-procedural data dependence within a function.
pub(crate) fn dataflow(
    json: bool,
    quiet: bool,
    function: &str,
    path: &Path,
    var: Option<&str>,
    forward: bool,
    hops: u32,
) -> anyhow::Result<()> {
    let flow = Dataflow::build(path)?;
    if !flow.has_function(function) {
        return not_found(json, quiet, function, "dataflow");
    }
    let dir = if forward {
        FlowDir::FlowsInto
    } else {
        FlowDir::DependsOn
    };

    match var {
        Some(var) => {
            let steps = flow.flow(function, var, dir, hops);
            if json {
                let out = serde_json::json!({
                    "function": function,
                    "var": var,
                    "direction": dir.as_str(),
                    "count": steps.len(),
                    "flow": steps.iter().map(|s| serde_json::json!({ "name": s.name, "distance": s.distance })).collect::<Vec<_>>(),
                    "tier": "treesitter",   // FR-3: dataflow is tree-sitter-derived.
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else if steps.is_empty() {
                if !quiet {
                    println!("{var} has no {} edges in {function}", dir.as_str());
                }
            } else {
                println!("{} of {var} in {function}:", dir.as_str());
                for step in &steps {
                    println!("  {} (hop {})", step.name.cyan(), step.distance);
                }
            }
        }
        None => {
            let edges = flow.edges(function);
            if json {
                let out = serde_json::json!({
                    "function": function,
                    "count": edges.len(),
                    "edges": edges.iter().map(|e| serde_json::json!({ "dependent": e.dependent, "depends_on": e.depends_on, "line": e.line })).collect::<Vec<_>>(),
                    "tier": "treesitter",   // FR-3: dataflow is tree-sitter-derived.
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else if edges.is_empty() {
                if !quiet {
                    println!("no data-dependence edges in {function}");
                }
            } else {
                println!("data dependence in {function}:");
                for edge in edges {
                    println!(
                        "  {}:{} {} depends on {}",
                        function,
                        edge.line,
                        edge.dependent.cyan(),
                        edge.depends_on.cyan()
                    );
                }
            }
        }
    }
    Ok(())
}

/// `yupana export` — emit the referential structure as Turtle.
/// `yupana census` — same-file symbol-name collisions, the sizing input for the
/// scope-qualified IRI migration.
///
/// Walks the tree exactly like `export` and asks the extractor for each file's
/// collisions. The count MUST come from here: the exported turtle collapses
/// same-kind duplicates into identical triples and the graph keeps one kind
/// per merged node, so both understate the population by construction.
pub(crate) fn census(json: bool, quiet: bool, path: &Path) -> anyhow::Result<()> {
    use crate::extract::{extract_structure, name_collisions, source_files};

    let mut files_scanned = 0usize;
    let mut symbols_seen = 0usize;
    let mut per_file: Vec<(String, Vec<crate::extract::NameCollision>)> = Vec::new();

    for (file, language) in source_files(path) {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(structure) = extract_structure(&source, language) else {
            continue;
        };
        files_scanned += 1;
        symbols_seen += structure.symbols.len();
        let collisions = name_collisions(&structure.symbols);
        if !collisions.is_empty() {
            let rel = file
                .strip_prefix(path)
                .unwrap_or(&file)
                .to_string_lossy()
                .into_owned();
            per_file.push((rel, collisions));
        }
    }
    per_file.sort_by(|a, b| a.0.cmp(&b.0));

    let same_kind = per_file
        .iter()
        .flat_map(|(_, c)| c)
        .filter(|c| c.same_kind())
        .count();
    let cross_kind = per_file
        .iter()
        .flat_map(|(_, c)| c)
        .filter(|c| c.cross_kind())
        .count();
    let colliding_names: usize = per_file.iter().map(|(_, c)| c.len()).sum();

    if json {
        let out = serde_json::json!({
            "files": per_file.iter().map(|(file, collisions)| serde_json::json!({
                "file": file,
                "collisions": collisions.iter().map(|c| serde_json::json!({
                    "name": c.name,
                    "sites": c.sites.iter().map(|(kind, line)| serde_json::json!({
                        "kind": kind.as_str(),
                        "line": line,
                    })).collect::<Vec<_>>(),
                    "same_kind": c.same_kind(),
                    "cross_kind": c.cross_kind(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "summary": {
                "files_scanned": files_scanned,
                "symbols_seen": symbols_seen,
                "colliding_files": per_file.len(),
                "colliding_names": colliding_names,
                "same_kind": same_kind,
                "cross_kind": cross_kind,
            },
            // Same provenance story as the other tree-sitter surfaces (FR-3).
            "tier": "treesitter",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    for (file, collisions) in &per_file {
        println!("{}", file.bold());
        for c in collisions {
            let sites: Vec<String> = c
                .sites
                .iter()
                .map(|(kind, line)| format!("{}@{line}", kind.as_str()))
                .collect();
            let tag = match (c.same_kind(), c.cross_kind()) {
                (true, true) => "same-kind + cross-kind",
                (true, false) => "same-kind",
                _ => "cross-kind",
            };
            println!("  {}: {}  [{}]", c.name, sites.join(", "), tag.yellow());
        }
    }
    if !quiet {
        if per_file.is_empty() {
            println!(
                "no same-file symbol-name collisions ({files_scanned} files, {symbols_seen} symbols)"
            );
        } else {
            println!(
                "\n{} colliding name(s) in {} file(s) — {} same-kind (merge silently), {} cross-kind (shape-refusable); {} files / {} symbols scanned",
                colliding_names,
                per_file.len(),
                same_kind,
                cross_kind,
                files_scanned,
                symbols_seen,
            );
        }
    }
    Ok(())
}

pub(crate) fn export(path: &Path, repo: Option<&str>) -> anyhow::Result<()> {
    // Identity chain: explicit --repo, else the origin remote's repo name, else
    // the directory basename. The dir-name fallback survives ONLY here — plain
    // `export` prints locally and writes nothing — while the promote paths refuse
    // instead: a guessed identity in a WRITE fragments the shared graph.
    let repo = repo.map_or_else(
        || {
            crate::git::origin_repo_name(path).unwrap_or_else(|| {
                path.canonicalize()
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "repo".to_string())
            })
        },
        str::to_string,
    );
    let turtle = export::to_turtle(path, &repo)?;
    print!("{turtle}");
    Ok(())
}

/// Shared "not found" reporting for a missing symbol/function.
fn not_found(json: bool, quiet: bool, name: &str, what: &str) -> anyhow::Result<()> {
    if json {
        // The not-found result is still a served fact and still carries its tier
        // (FR-3) — this is the empty-case hole the top-level tag closes. All three
        // callers (callers/impact/dataflow) query the tree-sitter graph.
        println!(
            "{}",
            serde_json::json!({ "name": name, "found": false, "tier": "treesitter" })
        );
    } else if !quiet {
        println!("{name} not found in the {what}");
    }
    Ok(())
}

/// `yupana verify` — a verdict on a proposed edit buffer (FR-23/FR-24).
///
/// Exits non-zero when the buffer has violations, so CI and scripts can gate on
/// it. The verdict always reports the tier it was reached at and what that tier
/// could not check, so a clean result is never over-read (FR-3).
pub(crate) fn verify(json: bool, quiet: bool, file: &Path, buffer: &Path) -> anyhow::Result<()> {
    let proposed = std::fs::read_to_string(buffer)?;
    // The current contents are the baseline: violations already present before
    // the edit are not attributed to it.
    let baseline = std::fs::read_to_string(file).ok();
    let root = std::env::current_dir()?;
    let verdict = crate::verify::verify_buffer(&root, file, &proposed, baseline.as_deref())?;

    if json {
        println!("{}", serde_json::to_string_pretty(&verdict)?);
    } else if verdict.ok {
        if !quiet {
            println!(
                "{} {} [{:?}]",
                "verified".green().bold(),
                file.display(),
                verdict.tier
            );
            println!("  not checked at this tier:");
            for item in &verdict.unchecked {
                println!("    - {item}");
            }
        }
    } else {
        println!(
            "{} {} [{:?}]",
            "violations".red().bold(),
            file.display(),
            verdict.tier
        );
        for violation in &verdict.violations {
            let where_ = if violation.line == 0 {
                String::new()
            } else {
                format!(":{}", violation.line)
            };
            println!(
                "  {}{} {}",
                violation.symbol.cyan(),
                where_,
                violation.message
            );
        }
    }

    if verdict.ok {
        Ok(())
    } else {
        std::process::exit(1);
    }
}
