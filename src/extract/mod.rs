//! Fast structural extraction via tree-sitter — the always-on breadth tier.
//!
//! This is Yupana's build-free extractor: it works on a syntactically-broken
//! buffer and produces a symbol tree with line spans and intra-file call sites,
//! tagged [`Tier::TreeSitter`]. Precise LSP facts (`lsp` feature) and
//! CPG/dataflow (`cpg` feature) layer on top in later phases.
//!
//! ## Grammar registry
//!
//! Rust is always built. Bobbin's remaining grammar set — TypeScript/TSX,
//! Python, Go, Java, C/C++ — arrives behind the `langs-extra` feature. Each
//! language lives in its own sibling module and contributes a [`GrammarSpec`]
//! (grammar + node-kind → [`SymbolKind`] mapping + call/import extraction). The
//! generic walker in this module is language-agnostic: it drives whichever spec
//! [`grammar_spec`] returns for the requested language.

use std::path::{Path, PathBuf};

use tree_sitter::{Node, Parser};

use crate::errors::{Error, Result};
use crate::types::{Symbol, SymbolKind, Tier};

#[cfg(feature = "langs-extra")]
mod cpp;
#[cfg(feature = "langs-extra")]
mod go;
#[cfg(feature = "langs-extra")]
mod java;
#[cfg(feature = "langs-extra")]
mod python;
pub mod query;
mod rust;
#[cfg(feature = "langs-extra")]
mod typescript;

/// A resolved-by-name call site: `caller` invokes `callee` at `line`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    /// Name of the enclosing function the call is made from.
    pub caller: String,
    /// Name of the invoked function/method (best-effort, by name).
    pub callee: String,
    /// 1-based line of the call.
    pub line: usize,
}

/// The structure extracted from one source file: its symbols and call sites.
#[derive(Debug, Clone, Default)]
pub struct FileStructure {
    /// Named symbols defined in the file.
    pub symbols: Vec<Symbol>,
    /// Intra-file call sites (caller/callee by name).
    pub calls: Vec<CallSite>,
    /// Candidate module-name references from `import` / `use` / `include`
    /// declarations. These are the path segments seen in imports (best-effort,
    /// by name — the tree-sitter tier); the exporter resolves them to sibling
    /// modules by matching module stems (§9.2 `bobbin:imports`,
    /// [`Tier::TreeSitter`]).
    pub import_refs: Vec<String>,
}

/// A per-language extraction recipe. Each grammar module builds one of these; the
/// generic [`walk`] below is driven entirely by these function pointers, so no
/// language-specific logic lives in the walker itself.
///
/// `Node` is `Copy`, so passing it by value costs nothing.
pub(crate) struct GrammarSpec {
    /// The tree-sitter grammar for this language.
    pub language: fn() -> tree_sitter::Language,
    /// Map a node to the kind of symbol it defines, if any.
    pub symbol_kind: fn(Node, &[u8]) -> Option<SymbolKind>,
    /// The declared name of a symbol node (for caller attribution + the symbol).
    pub symbol_name: fn(Node, &[u8]) -> Option<String>,
    /// Whether a node kind introduces a callable scope (so calls inside it are
    /// attributed to it as their caller).
    pub is_function_kind: fn(&str) -> bool,
    /// Whether a node kind is a call site.
    pub is_call_kind: fn(&str) -> bool,
    /// Best-effort name of the callee invoked by a call node.
    pub callee_name: fn(Node, &[u8]) -> Option<String>,
    /// Collect any module-name references contributed by this node.
    pub collect_imports: fn(Node, &[u8], &mut Vec<String>),
    /// The name this node contributes to the scope chain of its descendants
    /// (module/impl/trait/class/function — per language), or `None` for nodes
    /// that do not open a named scope. This is sibling-independent by design:
    /// a symbol's scope chain never changes because another symbol was added
    /// (aegis-1q14 ruled out collision-conditional qualification for exactly
    /// that reason).
    pub scope_name: fn(Node, &[u8]) -> Option<String>,
}

/// Look up the extraction recipe for `language`, or `None` if this build cannot
/// parse it. The extra grammars are gated behind `langs-extra`.
fn grammar_spec(language: &str) -> Option<GrammarSpec> {
    match language {
        "rust" => Some(rust::spec()),
        #[cfg(feature = "langs-extra")]
        "typescript" => Some(typescript::spec_typescript()),
        #[cfg(feature = "langs-extra")]
        "tsx" => Some(typescript::spec_tsx()),
        #[cfg(feature = "langs-extra")]
        "python" => Some(python::spec()),
        #[cfg(feature = "langs-extra")]
        "go" => Some(go::spec()),
        #[cfg(feature = "langs-extra")]
        "java" => Some(java::spec()),
        #[cfg(feature = "langs-extra")]
        "cpp" => Some(cpp::spec()),
        _ => None,
    }
}

/// The languages this BUILD can actually extract, in a stable order.
///
/// This exists to be REPORTED (`yupana status`), because the failure it guards is
/// silent by construction: a build without `langs-extra` has no grammar for five
/// of the six languages, so [`source_files`] never selects those files, the
/// extractor is never asked, and `export`/`promote` emit a well-formed document
/// that is simply missing them. Exit 0, valid Turtle, zero symbols — which is
/// indistinguishable from "that repo has no code in those languages" (aegis-ah0q1:
/// the deployed binary was Rust-only for an undated period and nothing said so).
///
/// Same principle as [`Tier::served`](crate::types::Tier::served) and the reason
/// the empty `cpg`/`lsp` features were removed (aegis-qe5z): a build must not
/// advertise a capability it does not have. The inverse is just as costly — a
/// build must not stay SILENT about a capability it is missing, because the
/// consumer cannot tell an empty answer from an unsupported one.
///
/// Gated identically to [`grammar_spec`]; `advertised_languages_all_resolve`
/// asserts the two cannot drift.
#[must_use]
pub fn languages() -> Vec<&'static str> {
    vec![
        "rust",
        #[cfg(feature = "langs-extra")]
        "typescript",
        #[cfg(feature = "langs-extra")]
        "tsx",
        #[cfg(feature = "langs-extra")]
        "python",
        #[cfg(feature = "langs-extra")]
        "go",
        #[cfg(feature = "langs-extra")]
        "java",
        #[cfg(feature = "langs-extra")]
        "cpp",
    ]
}

/// Map a source-file extension to the canonical language name understood by
/// [`extract_structure`], or `None` if this build has no grammar for it.
///
/// Only extensions whose grammar is compiled into the current build are
/// returned, so the map never promises a language the extractor would reject.
#[must_use]
pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    let language = match ext {
        "rs" => "rust",
        #[cfg(feature = "langs-extra")]
        "ts" | "mts" | "cts" | "js" | "mjs" | "cjs" => "typescript",
        #[cfg(feature = "langs-extra")]
        "tsx" | "jsx" => "tsx",
        #[cfg(feature = "langs-extra")]
        "py" | "pyi" => "python",
        #[cfg(feature = "langs-extra")]
        "go" => "go",
        #[cfg(feature = "langs-extra")]
        "java" => "java",
        #[cfg(feature = "langs-extra")]
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        _ => return None,
    };
    Some(language)
}

/// Walk `path` for every source file this build can PARSE, paired with its
/// language, honoring `.gitignore`.
///
/// The guard's graph is built from this, and it is deliberately not
/// [`rust_files`]: a blast-radius rule measured over a Rust-only graph reports a
/// misleadingly small radius for every other language — and "small" reads as
/// "safe". The pairing is returned rather than re-derived by the caller so the
/// language a file was PARSED as can never drift from the one it was SELECTED by.
#[must_use]
pub fn source_files(path: &Path) -> Vec<(PathBuf, &'static str)> {
    ignore::WalkBuilder::new(path)
        .build()
        .filter_map(std::result::Result::ok)
        .map(ignore::DirEntry::into_path)
        .filter_map(|p| {
            let language = selectable_language(&p)?;
            Some((p, language))
        })
        .collect()
}

/// The language this path should be extracted as, or `None` if it must not be
/// extracted at all.
///
/// THE ONE SELECTION RULE, shared by both projection entry points. `to_turtle`
/// walks the working tree and `to_turtle_at` lists a git commit; they were
/// documented as unable to drift "in what they emit, only in where the bytes
/// come from", but each applied its own selection, so a filter added to the
/// walker left the commit path untouched — which is precisely how a vendored
/// bundle reached the graph through `promote` while `export` was already clean.
/// Selection lives here so that cannot recur.
#[must_use]
pub fn selectable_language(path: &Path) -> Option<&'static str> {
    if is_vendored_or_minified(path) {
        return None;
    }
    let ext = path.extension().and_then(std::ffi::OsStr::to_str)?;
    language_for_extension(ext)
}

/// Should this path be ingested as a DOCUMENT (markdown)?
///
/// Same vendoring exclusion as [`selectable_language`] — a README inside
/// `node_modules/` is third-party noise for the same reason its `.js` is.
#[must_use]
pub fn is_selectable_doc(path: &Path) -> bool {
    !is_vendored_or_minified(path)
        && path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|ext| ext == "md" || ext == "markdown")
}

/// Is this a VENDORED or MINIFIED artefact — third-party or machine-generated
/// code that happens to sit in the tree?
///
/// These are not authored source and must never reach the graph. Minified
/// bundles in particular are catastrophic rather than merely noisy: every
/// mangled short name (`xGe`, `Art`) becomes a `CodeSymbol`, and the resulting
/// entities are both meaningless to a reader and numerous enough to bury the
/// real ones.
///
/// MEASURED on the quipu repo when `langs-extra` was first deployed: two files —
/// `ui/vendor/three.module.min.js` and `docs/book/mermaid.min.js` — produced
/// 574k of ~590k symbol references, 97% of the projection. That inflated the
/// promotion from 4.3 MB in 5 chunks to 65 MB in 71, and the write was REFUSED
/// server-side. So this filter is not a tidiness preference; without it the
/// extra grammars cannot be promoted at all.
///
/// Deliberately name/path-based rather than content-sniffing (e.g. "very long
/// lines"): a path rule is predictable, cheap, and a reader can tell from the
/// filename alone whether a file is included. `.gitignore` does NOT cover this —
/// vendored bundles are usually COMMITTED, which is the whole point of vendoring.
fn is_vendored_or_minified(path: &Path) -> bool {
    // `foo.min.js`, `foo.min.css`, `bundle.min.mjs` — the conventional marker
    // for machine-minified output, checked on the file STEM so it catches any
    // extension.
    if let Some(stem) = path.file_stem().and_then(std::ffi::OsStr::to_str) {
        if stem.ends_with(".min") || stem.ends_with("-min") {
            return true;
        }
    }
    // Directories that by convention hold third-party or generated code. Matched
    // on a full path COMPONENT so `myvendor/` or `src/distance.rs` do not trip it.
    path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("vendor" | "vendored" | "node_modules" | "third_party" | "thirdparty")
        )
    })
}

/// Like [`source_files`], but restricted to the languages named in `allowed`.
///
/// This is where the `languages` config key becomes real (aegis-ltjo). It was
/// documented as "languages to extract" and read by nothing: a user who set
/// `languages = ["rust"]` to RESTRICT analysis got no restriction, because every
/// walk yielded every compiled grammar. Now `yupana analyze` passes the configured
/// set here and the count reflects it. `allowed` holds language NAMES (the
/// canonical `language_for_extension` output — "rust", "python", …), so a name
/// this build cannot parse simply matches nothing rather than erroring.
#[must_use]
pub fn source_files_in(path: &Path, allowed: &[String]) -> Vec<(PathBuf, &'static str)> {
    source_files(path)
        .into_iter()
        .filter(|(_, language)| allowed.iter().any(|a| a == language))
        .collect()
}

/// Walk `path` for Rust source files, honoring `.gitignore`.
#[must_use]
pub fn rust_files(path: &Path) -> Vec<PathBuf> {
    ignore::WalkBuilder::new(path)
        .build()
        .filter_map(std::result::Result::ok)
        .map(ignore::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|ext| ext == "rs"))
        .collect()
}

/// Extract the named symbols from `source`, written in `language`.
///
/// Returns [`Error::UnsupportedLanguage`] for a language this build cannot
/// parse.
pub fn extract_symbols(source: &str, language: &str) -> Result<Vec<Symbol>> {
    Ok(extract_structure(source, language)?.symbols)
}

/// Extract symbols and call sites from `source`, written in `language`.
///
/// Returns [`Error::UnsupportedLanguage`] for a language this build cannot
/// parse.
pub fn extract_structure(source: &str, language: &str) -> Result<FileStructure> {
    let spec =
        grammar_spec(language).ok_or_else(|| Error::UnsupportedLanguage(language.to_string()))?;

    let mut parser = Parser::new();
    parser
        .set_language(&(spec.language)())
        .map_err(|e| Error::Parse(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| Error::Parse("tree-sitter produced no tree".to_string()))?;

    Ok(walk(&spec, tree.root_node(), source.as_bytes()))
}

/// Symbols in one file that share a NAME — and therefore share the
/// unqualified symbol IRI `<module>::<name>`, merging into a single graph
/// node on promotion.
///
/// This is the census surface for the scope-qualified IRI migration: the
/// collision population is countable ONLY here, at the extractor, where the
/// per-file symbol list still carries start lines. Every downstream surface
/// is blind by construction — the exported turtle collapses same-kind
/// duplicates into byte-identical triples, and the live graph keeps one kind
/// per merged node (only cross-kind collisions survive there, as shape
/// violations).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NameCollision {
    /// The shared symbol name.
    pub name: String,
    /// Each definition site: `(kind, start_line)`, in file order.
    pub sites: Vec<(SymbolKind, usize)>,
}

impl NameCollision {
    /// Two sites with the SAME kind — invisible everywhere downstream: they
    /// merge silently into one node, unioning both symbols' edges.
    #[must_use]
    pub fn same_kind(&self) -> bool {
        self.sites
            .iter()
            .enumerate()
            .any(|(i, (k, _))| self.sites[i + 1..].iter().any(|(k2, _)| k == k2))
    }

    /// Sites with DIFFERENT kinds — the variant a `symbolKind maxCount 1`
    /// shape can refuse (the merged node carries two kinds).
    #[must_use]
    pub fn cross_kind(&self) -> bool {
        self.sites.iter().any(|(k, _)| *k != self.sites[0].0)
    }
}

/// Group `symbols` by name and return every name defined more than once.
///
/// Sites are deduplicated on `(kind, start_line)` first: an extractor emitting
/// the same definition twice is not a collision. Order within a collision is
/// file order (by start line).
#[must_use]
pub fn name_collisions(symbols: &[Symbol]) -> Vec<NameCollision> {
    let mut by_name: std::collections::BTreeMap<&str, Vec<(SymbolKind, usize)>> =
        std::collections::BTreeMap::new();
    for s in symbols {
        let sites = by_name.entry(&s.name).or_default();
        if !sites.contains(&(s.kind, s.start_line)) {
            sites.push((s.kind, s.start_line));
        }
    }
    by_name
        .into_iter()
        .filter(|(_, sites)| sites.len() > 1)
        .map(|(name, mut sites)| {
            sites.sort_by_key(|(_, line)| *line);
            NameCollision {
                name: name.to_string(),
                sites,
            }
        })
        .collect()
}

/// Language-agnostic traversal: collect symbols, intra-file calls, and import
/// references by asking `spec` about each node. Iterative so a deeply-nested
/// tree can't overflow the stack.
fn walk(spec: &GrammarSpec, root: Node, bytes: &[u8]) -> FileStructure {
    let mut symbols = Vec::new();
    let mut calls = Vec::new();
    let mut import_refs = Vec::new();
    // Each frame carries the name of the nearest enclosing function (so a call
    // site can be attributed to its caller) and the chain of named enclosing
    // scopes (so a symbol's identity survives a same-named sibling elsewhere in
    // the file — aegis-1q14).
    let mut stack: Vec<(Node, Option<String>, Vec<String>)> = vec![(root, None, Vec::new())];

    while let Some((node, enclosing, scope)) = stack.pop() {
        let mut inner = enclosing.clone();

        if let Some(kind) = (spec.symbol_kind)(node, bytes) {
            if let Some(name) = (spec.symbol_name)(node, bytes) {
                symbols.push(Symbol {
                    name: name.clone(),
                    scope: scope.clone(),
                    kind,
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    tier: Tier::TreeSitter,
                });
                if (spec.is_function_kind)(node.kind()) {
                    inner = Some(name);
                }
            }
        }

        if (spec.is_call_kind)(node.kind()) {
            if let (Some(caller), Some(callee)) = (&enclosing, (spec.callee_name)(node, bytes)) {
                calls.push(CallSite {
                    caller: caller.clone(),
                    callee,
                    line: node.start_position().row + 1,
                });
            }
        }

        (spec.collect_imports)(node, bytes, &mut import_refs);

        // Children inherit this node's scope, extended when the node opens one.
        let child_scope = match (spec.scope_name)(node, bytes) {
            Some(name) => {
                let mut s = scope.clone();
                s.push(name);
                s
            }
            None => scope,
        };
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push((child, inner.clone(), child_scope.clone()));
        }
    }

    symbols.sort_by_key(|symbol| symbol.start_line);
    import_refs.sort();
    import_refs.dedup();
    FileStructure {
        symbols,
        calls,
        import_refs,
    }
}

/// The text of a node's `name` field — the common case for symbol naming across
/// grammars. Languages whose symbol name is nested (e.g. C/C++ declarators)
/// supply their own `symbol_name`.
pub(crate) fn field_name(node: Node, bytes: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(bytes).ok())
        .map(str::to_string)
}

/// Collect every `identifier` under `node` into `out`, dropping any of the given
/// path `anchors` (`crate` / `self` / `super` for Rust). Shared by grammars
/// whose imports are dotted identifier paths.
pub(crate) fn collect_path_idents(
    node: Node,
    bytes: &[u8],
    anchors: &[&str],
    out: &mut Vec<String>,
) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier" {
            if let Ok(text) = n.utf8_text(bytes) {
                if !anchors.contains(&text) {
                    out.push(text.to_string());
                }
            }
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
}

#[cfg(test)]
#[path = "extract_test.rs"]
mod extract_test;
