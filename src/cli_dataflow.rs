//! Intra-procedural and CPG dataflow CLI rendering.
use crate::cli_cmds::not_found;
use crate::dataflow::{Dataflow, FlowDir};
use colored::Colorize;
use std::path::Path;

/// `yupana dataflow` — intra-procedural data dependence within a function.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dataflow(
    json: bool,
    quiet: bool,
    function: &str,
    path: &Path,
    var: Option<&str>,
    dir: FlowDir,
    hops: u32,
    interprocedural: bool,
) -> anyhow::Result<()> {
    if interprocedural {
        #[cfg(feature = "cpg")]
        {
            let report = crate::extract::cpg::Cpg::build(path)?.query(function, var, dir, hops);
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(());
        }
        #[cfg(not(feature = "cpg"))]
        anyhow::bail!("interprocedural dataflow requires a build with the cpg feature");
    }
    let flow = Dataflow::build(path)?;
    if !flow.has_function(function) {
        return not_found(json, quiet, function, "dataflow");
    }

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
