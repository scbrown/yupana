//! Shared syntax evidence for command policies. Never executes or expands input.
//!
//! The grammar identifies command positions, so quoted prose and heredoc bodies
//! cannot masquerade as commands. The lexical word decoder removes quotes only;
//! expansions remain syntax, not claimed runtime values.

use tree_sitter::{Node, Parser};

#[derive(Debug, Clone)]
pub(crate) struct Command {
    pub text: String,
    pub words: Vec<String>,
    pub dynamic_words: Vec<String>,
    /// Nested execution environments; directory changes do not escape them.
    pub scope: Vec<usize>,
}

#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub commands: Vec<Command>,
    /// A partial tree is evidence of uncertainty, never a successful full parse.
    pub incomplete: bool,
    /// Execution indirection exceeded the literal resolver; keep its context.
    pub unresolved: Vec<Command>,
}

pub(crate) fn parse(source: &str) -> Parsed {
    parse_at_depth(source, 0)
}

fn parse_at_depth(source: &str, depth: usize) -> Parsed {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        return Parsed {
            incomplete: true,
            ..Parsed::default()
        };
    }
    let Some(tree) = parser.parse(source, None) else {
        return Parsed {
            incomplete: true,
            ..Parsed::default()
        };
    };
    let mut result = Parsed {
        incomplete: tree.root_node().has_error(),
        ..Parsed::default()
    };
    visit(tree.root_node(), source.as_bytes(), &[], &mut result);
    wrappers::expand(&mut result, depth);
    result
}

fn visit(node: Node<'_>, bytes: &[u8], scope: &[usize], result: &mut Parsed) {
    let mut nested = scope.to_vec();
    if matches!(
        node.kind(),
        "subshell" | "command_substitution" | "process_substitution" | "function_definition"
    ) {
        nested.push(node.start_byte());
    }
    if node.kind() == "command" {
        let mut words = Vec::new();
        if let Some(name) = node.child_by_field_name("name") {
            decode_word(name, bytes, &mut words, &mut result.incomplete);
        }
        let mut cursor = node.walk();
        for arg in node.children_by_field_name("argument", &mut cursor) {
            decode_word(arg, bytes, &mut words, &mut result.incomplete);
        }
        let mut cursor = node.walk();
        let dynamic_words = node
            .child_by_field_name("name")
            .into_iter()
            .chain(node.children_by_field_name("argument", &mut cursor))
            .filter(|node| dynamic(*node))
            .flat_map(|node| {
                let text = node.utf8_text(bytes).unwrap_or_default();
                match shell_words::split(text) {
                    Ok(words) if words.len() == 1 => words,
                    _ => vec![text.to_string()],
                }
            })
            .collect();
        let command = Command {
            text: node.utf8_text(bytes).unwrap_or_default().to_string(),
            words,
            dynamic_words,
            scope: nested.clone(),
        };
        // Keep dynamic names in execution order as opaque words. Consumers may
        // recognize evidence without claiming to know their expanded value.
        result.commands.push(command);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        // Each side of a pipeline has its own environment. Treating its `cd`
        // as a change to the caller's cwd would fabricate repository evidence.
        let mut child_scope = nested.clone();
        if node.kind() == "pipeline" {
            child_scope.push(child.start_byte());
        }
        visit(child, bytes, &child_scope, result);
    }
}

fn decode_word(node: Node<'_>, bytes: &[u8], words: &mut Vec<String>, incomplete: &mut bool) {
    let text = node.utf8_text(bytes).unwrap_or_default();
    if dynamic(node) {
        *incomplete = true;
    }
    match shell_words::split(text) {
        Ok(decoded) if decoded.len() == 1 => words.extend(decoded),
        _ => {
            // Keep opaque syntax in its original argument position. Dropping it
            // could turn its next operand into a flag, repository or ref.
            words.push(text.to_string());
            *incomplete = true;
        }
    }
}

fn dynamic(node: Node<'_>) -> bool {
    if matches!(
        node.kind(),
        "simple_expansion"
            | "expansion"
            | "arithmetic_expansion"
            | "command_substitution"
            | "process_substitution"
            | "extglob_pattern"
            | "brace_expression"
    ) {
        return true;
    }
    let mut cursor = node.walk();
    let found = node.named_children(&mut cursor).any(dynamic);
    found
}

#[cfg(test)]
#[path = "shell_command_test.rs"]
mod tests;

#[path = "shell_command_wrappers.rs"]
mod wrappers;
