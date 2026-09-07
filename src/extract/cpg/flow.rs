//! Call resolution and bounded pushdown traversal of value edges.
use super::{Boundary, Cpg, Step};
use crate::dataflow::FlowDir;
use std::collections::{BTreeMap, HashSet, VecDeque};

impl Cpg {
    pub(super) fn connect_calls(&mut self) {
        for index in 0..self.calls.len() {
            let call = &self.calls[index];
            let caller = &self.functions[&call.caller];
            // Resolve only lexical bare calls, or a unique crate-qualified path.
            // Never connect every same-named function (or infer method dispatch).
            let mut candidates = Vec::new();
            if !call.target.contains("::") {
                for depth in (0..=caller.scope.len()).rev() {
                    candidates = self
                        .functions
                        .values()
                        .filter(|f| {
                            f.file == caller.file
                                && f.scope == caller.scope[..depth]
                                && f.name == call.target
                        })
                        .collect();
                    if !candidates.is_empty() {
                        break;
                    }
                }
            }
            if candidates.is_empty() && call.target.starts_with("crate::") {
                let target = call.target.trim_start_matches("crate::");
                candidates = self
                    .functions
                    .values()
                    .filter(|f| {
                        let mut path = f.scope.clone();
                        path.push(f.name.clone());
                        path.join("::") == target
                    })
                    .collect();
            }
            if candidates.len() != 1 || candidates[0].parameters.len() != call.arguments.len() {
                self.diagnostics.insert(format!(
                    "{}:{}: unresolved/ambiguous call {} ({} candidates); no invented return flow",
                    call.caller,
                    call.line,
                    call.target,
                    candidates.len()
                ));
                continue;
            }
            let callee = candidates[0];
            let mut links = Vec::new();
            for (args, param) in call.arguments.iter().zip(&callee.parameters) {
                for arg in args {
                    links.push((arg.clone(), param.clone(), Boundary::Call(index)));
                }
            }
            links.push((
                callee.result.clone(),
                call.result.clone(),
                Boundary::Return(index),
            ));
            for (from, to, boundary) in links {
                self.link(from, to, boundary);
            }
        }
    }

    pub(super) fn traverse(
        &self,
        starts: Vec<String>,
        dir: FlowDir,
        hops: u32,
    ) -> (Vec<Step>, bool) {
        let forward = dir == FlowDir::FlowsInto;
        let mut adjacency = BTreeMap::<&str, Vec<(&str, Boundary)>>::new();
        for link in &self.links {
            let (from, to, boundary) = if forward {
                (&link.from, &link.to, link.boundary)
            } else {
                (
                    &link.to,
                    &link.from,
                    match link.boundary {
                        Boundary::Call(c) => Boundary::Return(c),
                        Boundary::Return(c) => Boundary::Call(c),
                        Boundary::Local => Boundary::Local,
                    },
                )
            };
            adjacency.entry(from).or_default().push((to, boundary));
        }
        let start_set: HashSet<_> = starts.iter().cloned().collect();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        for start in starts {
            seen.insert((start.clone(), Vec::<usize>::new()));
            queue.push_back((start.clone(), Vec::<usize>::new(), vec![start]));
        }
        let mut reached = BTreeMap::new();
        let mut truncated = false;
        while let Some((node, stack, path)) = queue.pop_front() {
            for (next, boundary) in adjacency.get(node.as_str()).into_iter().flatten() {
                let mut context = stack.clone();
                match boundary {
                    Boundary::Local => {}
                    Boundary::Call(c) => context.push(*c),
                    Boundary::Return(c) => {
                        if let Some(expected) = context.pop() {
                            if expected != *c {
                                continue;
                            }
                        }
                        // A source inside a callee has unknown incoming context;
                        // its return may flow to any of its actual callers.
                    }
                }
                let key = (next.to_string(), context.clone());
                if seen.contains(&key) {
                    continue;
                }
                if path.len() > hops as usize || seen.len() >= 50_000 {
                    truncated = true;
                    continue;
                }
                seen.insert(key);
                let mut witness = path.clone();
                witness.push(next.to_string());
                if !start_set.contains(*next) {
                    reached.entry(next.to_string()).or_insert_with(|| Step {
                        value: self.values[*next].clone(),
                        distance: (witness.len() - 1) as u32,
                        path: witness.clone(),
                    });
                }
                queue.push_back((next.to_string(), context, witness));
            }
        }
        let mut steps: Vec<_> = reached.into_values().collect();
        steps.sort_by(|a, b| (a.distance, &a.value.id).cmp(&(b.distance, &b.value.id)));
        (steps, truncated)
    }
}
