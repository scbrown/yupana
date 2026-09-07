//! Source identity, lexical bindings and value edges for Rust free functions.
use super::{cfg, Boundary, Call, Cpg, Function};
use crate::errors::{Error, Result};
use std::collections::BTreeMap;
use tree_sitter::{Node, Parser};

type Env = BTreeMap<String, String>;

pub(super) fn text<'a>(node: Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

pub(super) fn children(node: Node<'_>) -> Vec<Node<'_>> {
    node.named_children(&mut node.walk()).collect()
}

pub(super) fn file(model: &mut Cpg, file: &str, source: &str) -> Result<()> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|e| Error::Parse(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| Error::Parse(format!("{file}: no tree")))?;
    if tree.root_node().has_error() {
        model
            .diagnostics
            .insert(format!("{file}: syntax errors; file omitted"));
        return Ok(());
    }
    let mut scope: Vec<String> = std::path::Path::new(file)
        .with_extension("")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if scope.first().is_some_and(|s| s == "src") {
        scope.remove(0);
    }
    if scope
        .last()
        .is_some_and(|s| matches!(s.as_str(), "lib" | "main" | "mod"))
    {
        scope.pop();
    }
    let mut stack = vec![(tree.root_node(), scope)];
    while let Some((node, scope)) = stack.pop() {
        if node.kind() == "impl_item" || node.kind() == "trait_item" {
            model.diagnostics.insert(format!(
                "{file}:{}: methods/traits not resolved",
                node.start_position().row + 1
            ));
            continue;
        }
        if node.kind() == "function_item" {
            function(model, file, source, node, scope);
            continue;
        }
        let mut scope = scope;
        if node.kind() == "mod_item" {
            if let Some(name) = node.child_by_field_name("name") {
                scope.push(text(name, source).into());
            }
        }
        for child in children(node).into_iter().rev() {
            stack.push((child, scope.clone()));
        }
    }
    Ok(())
}

fn function(model: &mut Cpg, file: &str, source: &str, node: Node, scope: Vec<String>) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let Some(name) = node.child_by_field_name("name") else {
        return;
    };
    let name = text(name, source).to_string();
    let mut path = scope.clone();
    path.push(name.clone());
    let id = format!("{file}::{}@{}", path.join("::"), node.start_byte());
    let result = model.value(
        &id,
        "$return",
        node.start_byte(),
        node.start_position().row + 1,
    );
    let mut parameters = Vec::new();
    let mut env = Env::new();
    if let Some(params) = node.child_by_field_name("parameters") {
        for param in children(params) {
            let Some(pattern) = param.child_by_field_name("pattern") else {
                continue;
            };
            if pattern.kind() != "identifier" {
                model.diagnostics.insert(format!(
                    "{id}: unsupported parameter pattern; function omitted"
                ));
                return;
            }
            let name = text(pattern, source);
            let value = model.value(
                &id,
                name,
                pattern.start_byte(),
                pattern.start_position().row + 1,
            );
            env.insert(name.into(), value.clone());
            parameters.push(value);
        }
    }
    cfg::extract(model, &id, source, body);
    model.functions.insert(
        id.clone(),
        Function {
            id: id.clone(),
            name,
            scope,
            file: file.into(),
            parameters,
            result: result.clone(),
        },
    );
    let mut builder = Builder {
        model,
        source,
        function: id,
        result,
    };
    let tail = builder.expr(body, &mut env);
    for from in tail {
        builder
            .model
            .link(from, builder.result.clone(), Boundary::Local);
    }
}

struct Builder<'a> {
    model: &'a mut Cpg,
    source: &'a str,
    function: String,
    result: String,
}

impl Builder<'_> {
    fn warn(&mut self, node: Node, detail: &str) {
        self.model.diagnostics.insert(format!(
            "{}:{}: {detail}",
            self.function,
            node.start_position().row + 1
        ));
    }

    fn expr(&mut self, node: Node, env: &mut Env) -> Vec<String> {
        match node.kind() {
            "identifier" => env
                .get(text(node, self.source))
                .cloned()
                .into_iter()
                .collect(),
            "scoped_identifier" => {
                self.warn(
                    node,
                    "qualified non-call value unresolved; not a local binding",
                );
                Vec::new()
            }
            "block" => {
                let mut local = env.clone();
                let mut tail = Vec::new();
                for child in children(node) {
                    tail = self.expr(child, &mut local);
                    // Statements with a semicolon do not produce a tail value.
                    if matches!(child.kind(), "expression_statement" | "let_declaration") {
                        tail.clear();
                    }
                }
                tail
            }
            "let_declaration" => {
                let used = node
                    .child_by_field_name("value")
                    .map_or_else(Vec::new, |v| self.expr(v, env));
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    if pattern.kind() == "identifier" {
                        let name = text(pattern, self.source);
                        let target = self.model.value(
                            &self.function,
                            name,
                            pattern.start_byte(),
                            pattern.start_position().row + 1,
                        );
                        for from in used {
                            self.model.link(from, target.clone(), Boundary::Local);
                        }
                        env.insert(name.into(), target);
                    } else {
                        self.warn(node, "destructuring binding omitted");
                    }
                }
                Vec::new()
            }
            "assignment_expression" | "compound_assignment_expr" => {
                let used = node
                    .child_by_field_name("right")
                    .map_or_else(Vec::new, |v| self.expr(v, env));
                if let Some(left) = node.child_by_field_name("left") {
                    if left.kind() == "identifier" {
                        if let Some(target) = env.get(text(left, self.source)) {
                            for from in used {
                                self.model.link(from, target.clone(), Boundary::Local);
                            }
                        }
                    } else {
                        self.warn(node, "heap/field assignment omitted (no alias model)");
                    }
                }
                Vec::new()
            }
            "return_expression" => {
                for child in children(node) {
                    for from in self.expr(child, env) {
                        self.model.link(from, self.result.clone(), Boundary::Local);
                    }
                }
                Vec::new()
            }
            "call_expression" => self.call(node, env),
            "if_expression" => {
                if let Some(condition) = node.child_by_field_name("condition") {
                    self.expr(condition, env);
                }
                let mut values = Vec::new();
                for field in ["consequence", "alternative"] {
                    if let Some(branch) = node.child_by_field_name(field) {
                        values.extend(self.expr(branch, &mut env.clone()));
                    }
                }
                values
            }
            "while_expression" | "loop_expression" => {
                for field in ["condition", "body"] {
                    if let Some(part) = node.child_by_field_name(field) {
                        self.expr(part, &mut env.clone());
                    }
                }
                Vec::new()
            }
            "closure_expression" | "async_block" | "function_item" | "macro_invocation"
            | "match_expression" | "for_expression" | "try_expression" | "await_expression" => {
                self.warn(node, &format!("{} omitted from value flow", node.kind()));
                Vec::new()
            }
            "reference_expression"
            | "unary_expression"
            | "field_expression"
            | "index_expression" => {
                self.warn(node, "reference/field/index operation is value-only; mutation through aliases is not modeled");
                children(node)
                    .into_iter()
                    .flat_map(|c| self.expr(c, env))
                    .collect()
            }
            _ => children(node)
                .into_iter()
                .flat_map(|c| self.expr(c, env))
                .collect(),
        }
    }

    fn call(&mut self, node: Node, env: &mut Env) -> Vec<String> {
        let Some(callee) = node.child_by_field_name("function") else {
            return Vec::new();
        };
        let target = text(callee, self.source).to_string();
        let result = self.model.value(
            &self.function,
            &format!("$call:{target}"),
            node.start_byte(),
            node.start_position().row + 1,
        );
        let arguments = node
            .child_by_field_name("arguments")
            .map_or_else(Vec::new, |a| {
                children(a)
                    .into_iter()
                    .map(|arg| self.expr(arg, env))
                    .collect()
            });
        if !matches!(callee.kind(), "identifier" | "scoped_identifier") || env.contains_key(&target)
        {
            self.warn(node, "indirect/method call omitted; no guessed return flow");
        } else {
            self.model.calls.push(Call {
                caller: self.function.clone(),
                target,
                arguments,
                result: result.clone(),
                line: node.start_position().row + 1,
            });
        }
        vec![result]
    }
}
