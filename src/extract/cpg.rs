//! Rust-native control dependence and bounded, call-site-matched value flow.
//!
//! This is a conservative source model, not a proof that a program is safe.
//! Locals are flow-insensitive; unsupported constructs are reported explicitly.

mod cfg;
mod flow;
mod parse;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use crate::dataflow::FlowDir;
use crate::errors::{Error, Result};
use crate::types::Tier;

/// A source-anchored control-dependence fact (dependent → predicate).
#[derive(Debug, Clone, Serialize)]
pub struct ControlEdge {
    /// Source ID of the controlled statement.
    pub dependent: String,
    /// Source ID of the controlling predicate.
    pub depends_on: String,
    /// One-based source line.
    pub line: usize,
    /// One-based predicate line.
    pub condition_line: usize,
    /// Code ontology predicate.
    pub relation: &'static str,
    /// Extractor provenance, always CPG here.
    pub tier: Tier,
}

/// A source-anchored value, qualified by file, function and declaration.
#[derive(Debug, Clone, Serialize)]
pub struct Value {
    /// Stable within this source snapshot: file, function and byte offset.
    pub id: String,
    /// Qualified function ID.
    pub function: String,
    /// Local name, or a synthetic $return/$call value.
    pub name: String,
    /// One-based source line.
    pub line: usize,
    /// Extractor provenance, always CPG here.
    pub tier: Tier,
}

#[derive(Clone)]
struct Link {
    from: String,
    to: String,
    boundary: Boundary,
}

#[derive(Clone, Copy)]
enum Boundary {
    Local,
    Call(usize),
    Return(usize),
}

struct Function {
    id: String,
    name: String,
    scope: Vec<String>,
    file: String,
    parameters: Vec<String>,
    result: String,
}

struct Call {
    caller: String,
    target: String,
    arguments: Vec<Vec<String>>,
    result: String,
    line: usize,
}

/// The extraction and query result. `truncated` includes hop/state budgets;
/// diagnostics and approximation remain relevant even when it is false.
#[derive(Debug, Serialize)]
pub struct Report {
    /// Qualified function ID.
    pub function: String,
    /// Whether the function selector resolved uniquely.
    pub found: bool,
    /// Requested variable selector, if any.
    pub var: Option<String>,
    /// Traversal direction.
    pub direction: String,
    /// Extractor provenance, always CPG here.
    pub tier: Tier,
    /// Limits applying even when no diagnostic was emitted.
    pub approximation: &'static str,
    /// CFG-derived control facts for the selected function.
    pub control_edges: Vec<ControlEdge>,
    /// Reached values with witness paths.
    pub flow: Vec<Step>,
    /// Unsupported syntax, unresolved calls or selection failures.
    pub diagnostics: Vec<String>,
    /// Whether the requested search exceeded a hop/state budget.
    pub truncated: bool,
}

/// One reached value, with an actual witness path through the modeled graph.
#[derive(Debug, Serialize)]
pub struct Step {
    #[serde(flatten)]
    /// Reached source value.
    pub value: Value,
    /// Number of edges in the witness.
    pub distance: u32,
    /// Value IDs from the source to this value.
    pub path: Vec<String>,
}

/// An on-demand CPG of the selected Rust sources.
#[derive(Default)]
pub struct Cpg {
    functions: BTreeMap<String, Function>,
    values: BTreeMap<String, Value>,
    links: Vec<Link>,
    calls: Vec<Call>,
    controls: Vec<ControlEdge>,
    diagnostics: BTreeSet<String>,
}

impl Cpg {
    /// Parse Rust sources. Read/parse failures are not silently treated as empty.
    pub fn build(root: &Path) -> Result<Self> {
        let mut model = Self::default();
        let mut files = super::source_files(root);
        files.sort_by(|a, b| a.0.cmp(&b.0));
        for (file, language) in files {
            if language != "rust" {
                continue;
            }
            let source = std::fs::read_to_string(&file)
                .map_err(|e| Error::Parse(format!("{}: {e}", file.display())))?;
            let relative = file.strip_prefix(root).unwrap_or(&file);
            let relative = if relative.as_os_str().is_empty() {
                file.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else {
                relative.to_string_lossy().into_owned()
            };
            parse::file(&mut model, &relative, &source)?;
        }
        model.connect_calls();
        Ok(model)
    }

    /// Query by an exact qualified function ID or a unique simple name.
    /// Ambiguity is returned as a diagnostic, never merged across functions.
    pub fn query(&self, function: &str, var: Option<&str>, dir: FlowDir, hops: u32) -> Report {
        let candidates: Vec<_> = self
            .functions
            .values()
            .filter(|f| f.id == function || f.name == function)
            .collect();
        let mut report = Report {
            function: function.into(), found: candidates.len() == 1,
            var: var.map(str::to_string), direction: dir.as_str().into(), tier: Tier::Cpg,
            approximation: "Rust source may-flow; flow-insensitive locals, matched call sites; no alias, sanitizer or path-feasibility proof",
            control_edges: Vec::new(), flow: Vec::new(),
            diagnostics: self.diagnostics.iter().cloned().collect(), truncated: false,
        };
        if candidates.len() != 1 {
            report.diagnostics.push(format!(
                "function {function}: {} candidates; use an exact ID: {}",
                candidates.len(),
                candidates
                    .iter()
                    .map(|f| f.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            return report;
        }
        let id = &candidates[0].id;
        report.function.clone_from(id);
        report.control_edges = self
            .controls
            .iter()
            .filter(|e| e.dependent.starts_with(&format!("{id}#")))
            .cloned()
            .collect();
        if let Some(var) = var {
            let starts: Vec<_> = self
                .values
                .values()
                .filter(|v| &v.function == id && (v.name == var || v.id == var))
                .map(|v| v.id.clone())
                .collect();
            if starts.is_empty() {
                report
                    .diagnostics
                    .push(format!("variable {var} not found in {id}"));
            }
            let (steps, truncated) = self.traverse(starts, dir, hops.min(256));
            report.flow = steps;
            report.truncated = truncated || hops > 256;
        }
        report
    }

    fn value(&mut self, function: &str, name: &str, byte: usize, line: usize) -> String {
        let id = format!("{function}#{byte}:{name}");
        self.values.entry(id.clone()).or_insert_with(|| Value {
            id: id.clone(),
            function: function.into(),
            name: name.into(),
            line,
            tier: Tier::Cpg,
        });
        id
    }

    fn link(&mut self, from: String, to: String, boundary: Boundary) {
        if from != to || !matches!(boundary, Boundary::Local) {
            self.links.push(Link { from, to, boundary });
        }
    }
}

#[cfg(test)]
mod tests;
