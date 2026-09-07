//! Structured CFG and fixed-point postdominators (normal exits only).
use super::{
    parse::{children, text},
    ControlEdge, Cpg,
};
use crate::types::Tier;
use std::collections::BTreeSet;
use tree_sitter::Node;

struct Block {
    byte: usize,
    line: usize,
    next: Vec<usize>,
}
struct Graph {
    blocks: Vec<Block>,
}

pub(super) fn extract(model: &mut Cpg, function: &str, source: &str, body: Node) {
    // Do not issue seemingly complete control facts for constructs whose CFG
    // we have not modeled. Data flow can still report its own partial results.
    let mut pending = vec![body];
    while let Some(node) = pending.pop() {
        if matches!(
            node.kind(),
            "match_expression"
                | "for_expression"
                | "try_expression"
                | "await_expression"
                | "closure_expression"
                | "async_block"
                | "macro_invocation"
                | "function_item"
                | "label"
        ) || (node.kind() == "binary_expression"
            && node
                .child_by_field_name("operator")
                .is_some_and(|op| matches!(text(op, source), "&&" | "||")))
        {
            model.diagnostics.insert(format!(
                "{function}:{}: control CFG omitted for {}",
                node.start_position().row + 1,
                node.kind()
            ));
            return;
        }
        pending.extend(children(node));
    }
    if body.descendant_count() > 10_000 {
        model
            .diagnostics
            .insert(format!("{function}: control CFG size budget exceeded"));
        return;
    }
    let mut graph = Graph {
        blocks: vec![Block {
            byte: 0,
            line: 0,
            next: Vec::new(),
        }],
    };
    let entry = graph.build(body, 0, None);
    if graph.blocks.len() > 2048 {
        model
            .diagnostics
            .insert(format!("{function}: control CFG block budget exceeded"));
        return;
    }
    let reachable = graph.reachable(entry);
    let mut terminating = BTreeSet::from([0]);
    loop {
        let before = terminating.len();
        for (i, block) in graph.blocks.iter().enumerate() {
            if block.next.iter().any(|n| terminating.contains(n)) {
                terminating.insert(i);
            }
        }
        if terminating.len() == before {
            break;
        }
    }
    if !reachable.is_subset(&terminating) {
        model.diagnostics.insert(format!(
            "{function}: nonterminating CFG region; control dependence omitted"
        ));
        return;
    }
    let mut post: Vec<_> = graph.blocks.iter().map(|_| reachable.clone()).collect();
    post[0] = BTreeSet::from([0]);
    loop {
        let mut changed = false;
        for &i in &reachable {
            if i == 0 {
                continue;
            }
            let mut set = reachable.clone();
            for next in &graph.blocks[i].next {
                set = set.intersection(&post[*next]).copied().collect();
            }
            set.insert(i);
            if set != post[i] {
                post[i] = set;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut edges = BTreeSet::new();
    for &controller in &reachable {
        for next in &graph.blocks[controller].next {
            for &dependent in post[*next].difference(&post[controller]) {
                if dependent != 0 {
                    edges.insert((dependent, controller));
                }
            }
        }
    }
    for (dependent, controller) in edges {
        let d = &graph.blocks[dependent];
        let c = &graph.blocks[controller];
        model.controls.push(ControlEdge {
            dependent: format!("{function}#stmt:{}", d.byte),
            depends_on: format!("{function}#stmt:{}", c.byte),
            line: d.line,
            condition_line: c.line,
            relation: "bobbin:controlDependsOn",
            tier: Tier::Cpg,
        });
    }
    model
        .controls
        .sort_by(|a, b| (&a.dependent, &a.depends_on).cmp(&(&b.dependent, &b.depends_on)));
}

impl Graph {
    fn add(&mut self, node: Node, next: Vec<usize>) -> usize {
        let id = self.blocks.len();
        self.blocks.push(Block {
            byte: node.start_byte(),
            line: node.start_position().row + 1,
            next,
        });
        id
    }

    // Construct backwards from the continuation. Early returns bypass it;
    // break/continue target the nearest loop exit/header rather than fallthrough.
    fn build(&mut self, node: Node, next: usize, loop_targets: Option<(usize, usize)>) -> usize {
        match node.kind() {
            "block" => children(node)
                .into_iter()
                .rev()
                .fold(next, |next, child| self.build(child, next, loop_targets)),
            "expression_statement" | "else_clause" => children(node)
                .first()
                .map_or(next, |c| self.build(*c, next, loop_targets)),
            "if_expression" => {
                let yes = node
                    .child_by_field_name("consequence")
                    .map_or(next, |n| self.build(n, next, loop_targets));
                let no = node
                    .child_by_field_name("alternative")
                    .map_or(next, |n| self.build(n, next, loop_targets));
                let condition = node.child_by_field_name("condition").unwrap_or(node);
                self.add(condition, vec![yes, no])
            }
            "while_expression" | "loop_expression" => {
                let condition = node.child_by_field_name("condition").unwrap_or(node);
                let header = self.add(condition, Vec::new());
                let body = node
                    .child_by_field_name("body")
                    .map_or(header, |n| self.build(n, header, Some((next, header))));
                self.blocks[header].next = if node.kind() == "while_expression" {
                    vec![body, next]
                } else {
                    vec![body]
                };
                header
            }
            "return_expression" => self.add(node, vec![0]),
            "break_expression" => self.add(node, vec![loop_targets.map_or(0, |(exit, _)| exit)]),
            "continue_expression" => {
                self.add(node, vec![loop_targets.map_or(0, |(_, header)| header)])
            }
            _ => {
                // Expression-level branches (e.g. let x = if ...) must contribute
                // their own control nodes before the enclosing binding executes.
                let statement = self.add(node, vec![next]);
                self.embedded(node, statement, loop_targets)
            }
        }
    }

    fn embedded(&mut self, node: Node, next: usize, targets: Option<(usize, usize)>) -> usize {
        children(node).into_iter().rev().fold(next, |next, child| {
            if matches!(
                child.kind(),
                "if_expression"
                    | "block"
                    | "while_expression"
                    | "loop_expression"
                    | "return_expression"
                    | "break_expression"
                    | "continue_expression"
            ) {
                self.build(child, next, targets)
            } else {
                self.embedded(child, next, targets)
            }
        })
    }

    fn reachable(&self, entry: usize) -> BTreeSet<usize> {
        let mut seen = BTreeSet::new();
        let mut pending = vec![entry];
        while let Some(i) = pending.pop() {
            if seen.insert(i) {
                pending.extend(&self.blocks[i].next);
            }
        }
        seen
    }
}
