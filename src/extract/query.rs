//! Tree-sitter query evaluation — the matcher behind structural rules.
//!
//! The extractor (the parent module) walks trees by hand to build the symbol
//! graph. A *policy* needs the opposite: to MATCH nodes declaratively, so an
//! operator can write `(line_comment) @c` and have Yupana find every comment. This
//! is the crate's only use of the tree-sitter query language (`.scm`), kept
//! behind one function so [`crate::rules`] carries no grammar detail.
//!
//! It is tree-sitter-tier, and honest about it: a query runs against the same
//! best-effort parse the extractor uses, so a capture is only as precise as the
//! grammar. A malformed query is an [`Error`], never an empty result — a selector
//! that does not compile must be surfaced, not silently match nothing (the same
//! discipline [`crate::policy::Scope::glob_errors`] applies to path globs).

use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

use super::grammar_spec;
use crate::errors::{Error, Result};

/// One node captured by a query: the `@name` it was bound to, the node's source
/// text, and its 1-based line span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// The capture name the query bound this node to (without the `@`).
    pub name: String,
    /// The node's source text.
    pub text: String,
    /// 1-based first line of the node.
    pub start_line: usize,
    /// 1-based last line of the node.
    pub end_line: usize,
}

/// Compile `query_src` against `language`'s grammar and return every capture it
/// makes over `source`.
///
/// Errors are distinguished so a caller can report *why* a rule could not be
/// evaluated rather than treating it as "nothing matched":
/// - [`Error::UnsupportedLanguage`] — this build has no grammar for `language`.
/// - [`Error::Parse`] — the query itself does not compile, or the source could
///   not be parsed.
pub fn run_query(source: &str, language: &str, query_src: &str) -> Result<Vec<Capture>> {
    let spec =
        grammar_spec(language).ok_or_else(|| Error::UnsupportedLanguage(language.to_string()))?;
    let lang = (spec.language)();

    let query = Query::new(&lang, query_src)
        .map_err(|e| Error::Parse(format!("tree-sitter query: {e}")))?;

    let mut parser = Parser::new();
    parser
        .set_language(&lang)
        .map_err(|e| Error::Parse(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| Error::Parse("tree-sitter produced no tree".to_string()))?;

    let bytes = source.as_bytes();
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut captures = Vec::new();
    let mut matches = cursor.matches(&query, tree.root_node(), bytes);
    while let Some(matched) = matches.next() {
        for capture in matched.captures {
            let text = capture
                .node
                .utf8_text(bytes)
                .unwrap_or_default()
                .to_string();
            captures.push(Capture {
                name: names
                    .get(capture.index as usize)
                    .copied()
                    .unwrap_or_default()
                    .to_string(),
                text,
                start_line: capture.node.start_position().row + 1,
                end_line: capture.node.end_position().row + 1,
            });
        }
    }
    Ok(captures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_line_comments_with_their_text() {
        let source = "\
// TODO: wire this up
fn f() {}
// see ABC-123
";
        let captures = run_query(source, "rust", "(line_comment) @c").unwrap();
        let texts: Vec<&str> = captures.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"// TODO: wire this up"));
        assert!(texts.contains(&"// see ABC-123"));
        // The capture name and line span travel with the node.
        let todo = captures.iter().find(|c| c.text.contains("TODO")).unwrap();
        assert_eq!(todo.name, "c");
        assert_eq!(todo.start_line, 1);
    }

    #[test]
    fn a_malformed_query_is_an_error_not_an_empty_match() {
        // A selector that does not compile must be surfaced, never read as "nothing
        // matched" — the same honesty the blast-radius Sizing type enforces.
        let err = run_query("fn f() {}", "rust", "(nonexistent_node) @x").unwrap_err();
        assert!(matches!(err, Error::Parse(_)));
    }

    #[test]
    fn an_unsupported_language_is_distinguished() {
        let err = run_query("", "cobol", "(comment) @c").unwrap_err();
        assert!(matches!(err, Error::UnsupportedLanguage(_)));
    }

    #[test]
    fn no_captures_is_an_empty_vec_not_an_error() {
        let captures = run_query("fn f() {}", "rust", "(line_comment) @c").unwrap();
        assert!(captures.is_empty());
    }
}
