//! Intra-procedural data-dependence — the Rust-native start of the CPG tier.
//!
//! Per `docs/yupana-spec.md` §14.1, Yupana takes the Rust-native path rather than
//! embedding Joern: we reimplement the traversals we need. This module builds a
//! per-function variable data-dependence graph from tree-sitter (a binding
//! depends on the locals used in its initializer) and answers flow queries:
//! `DependsOn` (what a variable is derived from) and `FlowsInto` (what is
//! derived from it). It is an approximation tagged at the tree-sitter tier —
//! precise inter-procedural taint arrives with a full CPG.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::errors::{Error, Result};
use crate::extract::rust_files;

/// Direction of a dataflow query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowDir {
    /// What the variable is (transitively) derived from.
    DependsOn,
    /// What is (transitively) derived from the variable.
    FlowsInto,
}

impl FlowDir {
    /// Lowercase wire form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FlowDir::DependsOn => "depends_on",
            FlowDir::FlowsInto => "flows_into",
        }
    }
}

/// A reached variable in a flow query.
#[derive(Debug, Clone)]
pub struct FlowStep {
    /// Variable name.
    pub name: String,
    /// Hop distance from the queried variable.
    pub distance: u32,
}

/// A single data-dependence edge: `dependent` is derived from `depends_on`.
#[derive(Debug, Clone)]
pub struct DepEdge {
    /// The assigned/bound variable.
    pub dependent: String,
    /// A local used in its initializer.
    pub depends_on: String,
    /// 1-based line of the binding/assignment.
    pub line: usize,
}

#[derive(Default)]
struct FnAccum {
    locals: HashSet<String>,
    edges: Vec<DepEdge>,
}

struct FnData {
    depends: HashMap<String, Vec<String>>,
    flows_into: HashMap<String, Vec<String>>,
    edges: Vec<DepEdge>,
}

/// Per-function intra-procedural data-dependence over a subtree.
pub struct Dataflow {
    functions: HashMap<String, FnData>,
}

impl Dataflow {
    /// Build the dataflow model for the Rust files under `root`.
    pub fn build(root: &Path) -> Result<Self> {
        let mut acc: HashMap<String, FnAccum> = HashMap::new();
        for file in rust_files(root) {
            if let Ok(source) = std::fs::read_to_string(&file) {
                // Per-file parse errors are non-fatal: skip and continue.
                let _ = collect_file(&source, &mut acc);
            }
        }

        let mut functions = HashMap::new();
        for (name, accum) in acc {
            let mut depends: HashMap<String, Vec<String>> = HashMap::new();
            let mut flows_into: HashMap<String, Vec<String>> = HashMap::new();
            let mut edges = Vec::new();
            for edge in accum.edges {
                if edge.dependent != edge.depends_on && accum.locals.contains(&edge.depends_on) {
                    depends
                        .entry(edge.dependent.clone())
                        .or_default()
                        .push(edge.depends_on.clone());
                    flows_into
                        .entry(edge.depends_on.clone())
                        .or_default()
                        .push(edge.dependent.clone());
                    edges.push(edge);
                }
            }
            functions.insert(
                name,
                FnData {
                    depends,
                    flows_into,
                    edges,
                },
            );
        }
        Ok(Self { functions })
    }

    /// Whether `function` is present in the model.
    #[must_use]
    pub fn has_function(&self, function: &str) -> bool {
        self.functions.contains_key(function)
    }

    /// The raw dependence edges of `function`.
    #[must_use]
    pub fn edges(&self, function: &str) -> &[DepEdge] {
        self.functions.get(function).map_or(&[], |f| &f.edges)
    }

    /// Variables reachable from `var` in `function`, within `max_hops`.
    #[must_use]
    pub fn flow(&self, function: &str, var: &str, dir: FlowDir, max_hops: u32) -> Vec<FlowStep> {
        let Some(data) = self.functions.get(function) else {
            return Vec::new();
        };
        let adjacency = match dir {
            FlowDir::DependsOn => &data.depends,
            FlowDir::FlowsInto => &data.flows_into,
        };

        let mut visited: HashSet<String> = HashSet::from([var.to_string()]);
        let mut frontier = vec![var.to_string()];
        let mut reached = Vec::new();
        let mut hop = 0;

        while hop < max_hops && !frontier.is_empty() {
            hop += 1;
            let mut next = Vec::new();
            for node in &frontier {
                if let Some(neighbors) = adjacency.get(node) {
                    for neighbor in neighbors {
                        if visited.insert(neighbor.clone()) {
                            reached.push(FlowStep {
                                name: neighbor.clone(),
                                distance: hop,
                            });
                            next.push(neighbor.clone());
                        }
                    }
                }
            }
            frontier = next;
        }
        reached
    }
}

/// Accumulate per-function locals and dependence edges from one source file.
fn collect_file(source: &str, acc: &mut HashMap<String, FnAccum>) -> Result<()> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|e| Error::Parse(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| Error::Parse("tree-sitter produced no tree".to_string()))?;
    let bytes = source.as_bytes();

    let mut stack: Vec<(Node, Option<String>)> = vec![(tree.root_node(), None)];
    while let Some((node, enclosing)) = stack.pop() {
        let mut inner = enclosing.clone();
        match node.kind() {
            "function_item" => {
                if let Some(name) = node
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(bytes).ok())
                {
                    inner = Some(name.to_string());
                    let mut params = Vec::new();
                    if let Some(parameters) = node.child_by_field_name("parameters") {
                        collect_idents(parameters, bytes, &mut |id| params.push(id));
                    }
                    let entry = acc.entry(name.to_string()).or_default();
                    for id in params {
                        entry.locals.insert(id);
                    }
                }
            }
            "let_declaration" => add_edges(node, "pattern", "value", &enclosing, bytes, acc, true),
            "assignment_expression" => {
                add_edges(node, "left", "right", &enclosing, bytes, acc, false);
            }
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push((child, inner.clone()));
        }
    }
    Ok(())
}

/// Record dependence edges from a binding/assignment node's target to the
/// locals used in its value expression.
fn add_edges(
    node: Node,
    target_field: &str,
    value_field: &str,
    enclosing: &Option<String>,
    bytes: &[u8],
    acc: &mut HashMap<String, FnAccum>,
    declares: bool,
) {
    let Some(function) = enclosing else {
        return;
    };
    let Some(target) = node
        .child_by_field_name(target_field)
        .and_then(|n| ident_text(n, bytes))
    else {
        return;
    };
    let line = node.start_position().row + 1;
    let mut used = Vec::new();
    if let Some(value) = node.child_by_field_name(value_field) {
        collect_idents(value, bytes, &mut |id| used.push(id));
    }

    let entry = acc.entry(function.clone()).or_default();
    if declares {
        entry.locals.insert(target.clone());
    }
    for id in used {
        entry.edges.push(DepEdge {
            dependent: target.clone(),
            depends_on: id,
            line,
        });
    }
}

/// Call `f` for every `identifier` token in `node`'s subtree.
fn collect_idents(node: Node, bytes: &[u8], f: &mut dyn FnMut(String)) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind() == "identifier" {
            if let Ok(text) = current.utf8_text(bytes) {
                f(text.to_string());
            }
        }
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// The first `identifier` token in `node` (handles `mut x`, refs, etc.).
fn ident_text(node: Node, bytes: &[u8]) -> Option<String> {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind() == "identifier" {
            return current.utf8_text(bytes).ok().map(str::to_string);
        }
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(src: &str) -> Dataflow {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        Dataflow::build(dir.path()).unwrap()
    }

    #[test]
    fn tracks_transitive_dependence() {
        let flow = build(
            "\
fn f(a: i32) -> i32 {
    let b = a + 1;
    let c = b * 2;
    c
}
",
        );
        assert!(flow.has_function("f"));
        // c depends on b (hop 1) and a (hop 2).
        let deps = flow.flow("f", "c", FlowDir::DependsOn, 5);
        let names: Vec<&str> = deps.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"b"));
        assert!(names.contains(&"a"));
    }

    #[test]
    fn tracks_forward_flow() {
        let flow = build(
            "\
fn f(a: i32) -> i32 {
    let b = a + 1;
    let c = b * 2;
    c
}
",
        );
        // a flows into b then c.
        let into = flow.flow("f", "a", FlowDir::FlowsInto, 5);
        let names: Vec<&str> = into.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
    }

    #[test]
    fn non_locals_are_filtered() {
        let flow = build("fn f() { let x = helper(); }");
        // `helper` is not a local, so x has no dependence edges.
        assert!(flow.flow("f", "x", FlowDir::DependsOn, 5).is_empty());
    }
}
