//! Export the referential structure as RDF Turtle in the `bobbin:` code ontology.
//!
//! This is the governed projection of Yupana's live graph — the substrate under
//! Phase-4 promotion (`--to quipu`). It emits *precise, typed referential
//! structure* (modules, symbols, `definedIn` / `calls` / `imports` edges),
//! **not** the embedding-oriented chunking Bobbin produces. Facts validate
//! against Quipu's
//! `shapes/code-entities.ttl` (`CodeModule`, `CodeSymbol`), whose namespace this
//! mirrors. Document/`Section` nodes and `Section → references → CodeSymbol`
//! edges fold in as the markdown extractor lands (spec §5.10).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::docref::{doc_files, extract_doc_sections, Mention};
use crate::errors::Result;
use crate::extract::{extract_structure, source_files};

/// The code-ontology namespace (matches `shapes/code-entities.ttl`).
pub(crate) const ONTO: &str = "http://aegis.gastown.local/ontology/";

/// Emit the referential structure of every source file this build can PARSE
/// under `root` as Turtle, attributing entities to repository `repo`.
///
/// EVERY LANGUAGE, not just Rust (the aegis-81t2 class, found again here): the
/// walk is [`source_files`] — the same drift-proof (path, language) pairing the
/// guard's graph uses — so a Python or TypeScript repo promotes its real
/// structure instead of a green empty write. Name/stem resolution is scoped
/// PER LANGUAGE: a global name map would mint cross-language call edges from
/// simple collisions (`main`, `run`, `init` exist everywhere), and a lying
/// edge in the knowledge graph is worse than a missing one.
pub fn to_turtle(root: &Path, repo: &str) -> Result<String> {
    // Working-tree source stream: the drift-proof (path, language) walk, each
    // file read from disk.
    let sources: Vec<(String, String, &'static str)> = source_files(root)
        .into_iter()
        .filter_map(|(file, language)| {
            let source = std::fs::read_to_string(&file).ok()?;
            Some((rel_path(&file, root), source, language))
        })
        .collect();
    let docs: Vec<(String, String)> = doc_files(root)
        .into_iter()
        .filter_map(|file| {
            let source = std::fs::read_to_string(&file).ok()?;
            Some((rel_path(&file, root), source))
        })
        .collect();
    to_turtle_from(repo, &sources, &docs)
}

/// Promote-time export at a COMMITTED ref (FR-22): read source and doc content
/// from the git tree at `reference`, never the working tree — so uncommitted
/// churn (an in-flight overlay edit, an unsaved buffer) can NEVER be promoted.
/// The IRIs are identical to [`to_turtle`]'s (repo + repo-relative path), so a
/// re-promotion of the same commit supersedes rather than forking the graph.
pub fn to_turtle_at(root: &Path, repo: &str, reference: &str) -> Result<String> {
    let files = crate::git::list_files_at(root, reference);
    let sources: Vec<(String, String, &'static str)> = files
        .iter()
        .filter_map(|path| {
            // Selection is `extract`'s call, NOT a second copy of the rule here
            // — the copy is what let a vendored bundle through this path while
            // the working-tree path already excluded it.
            let language = crate::extract::selectable_language(path)?;
            let source = crate::git::read_blob_at(root, reference, path)?;
            Some((path.display().to_string(), source, language))
        })
        .collect();
    let docs: Vec<(String, String)> = files
        .iter()
        .filter(|p| crate::extract::is_selectable_doc(p))
        .filter_map(|path| {
            let source = crate::git::read_blob_at(root, reference, path)?;
            Some((path.display().to_string(), source))
        })
        .collect();
    to_turtle_from(repo, &sources, &docs)
}

/// Build the Turtle projection from pre-materialized `(rel, source, language)`
/// source triples and `(rel, source)` doc pairs — the single body both
/// [`to_turtle`] (working tree) and [`to_turtle_at`] (committed tree) share, so
/// the two entry points cannot drift in what they emit, only in where the bytes
/// come from.
fn to_turtle_from(
    repo: &str,
    sources: &[(String, String, &'static str)],
    doc_sources: &[(String, String)],
) -> Result<String> {
    let mut modules: Vec<(String, String, &'static str)> = Vec::new();
    let mut symbols: Vec<SymbolTriple> = Vec::new();
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    let mut raw_calls: Vec<(String, String)> = Vec::new();
    // module IRI → its import-name references; and module stem → module IRI(s),
    // for resolving `use`/`mod` references to sibling modules.
    let mut raw_imports: Vec<(String, Vec<String>)> = Vec::new();
    let mut by_stem: HashMap<String, Vec<String>> = HashMap::new();
    // Symbol IRI → its owning module's stem, for narrowing a `qualifier::symbol`
    // doc mention to the right module (FR-33 resolution).
    let mut stem_by_sym: HashMap<String, String> = HashMap::new();

    for (rel, source, language) in sources {
        let (rel, language) = (rel.as_str(), *language);
        let Ok(structure) = extract_structure(source, language) else {
            continue;
        };
        let module = module_iri(repo, rel);
        // Language-scoped keys: `use foo` in Rust must never resolve to a
        // Python module named foo, and a call to `run` must stay within the
        // language whose parser saw it.
        let stem = format!("{language}\u{0}{}", module_stem(rel));
        modules.push((module.clone(), rel.to_string(), language));
        by_stem
            .entry(stem.clone())
            .or_default()
            .push(module.clone());
        for symbol in &structure.symbols {
            let iri = symbol_iri(&module, &symbol.scope, &symbol.name);
            symbols.push(SymbolTriple {
                iri: iri.clone(),
                name: symbol.name.clone(),
                kind: symbol.kind.as_str().to_string(),
                module: module.clone(),
            });
            stem_by_sym.insert(iri.clone(), stem.clone());
            by_name
                .entry(format!("{language}\u{0}{}", symbol.name))
                .or_default()
                .push(iri);
        }
        for call in &structure.calls {
            raw_calls.push((
                format!("{language}\u{0}{}", call.caller),
                format!("{language}\u{0}{}", call.callee),
            ));
        }
        raw_imports.push((
            module.clone(),
            structure
                .import_refs
                .iter()
                .map(|r| format!("{language}\u{0}{r}"))
                .collect(),
        ));
    }

    let mut call_edges: BTreeSet<(String, String)> = BTreeSet::new();
    for (caller, callee) in raw_calls {
        if let (Some(from), Some(to)) = (by_name.get(&caller), by_name.get(&callee)) {
            for a in from {
                for b in to {
                    if a != b {
                        call_edges.insert((a.clone(), b.clone()));
                    }
                }
            }
        }
    }

    // Resolve each import reference to a sibling module by matching its stem.
    let mut import_edges: BTreeSet<(String, String)> = BTreeSet::new();
    for (from, refs) in raw_imports {
        for name in refs {
            if let Some(targets) = by_stem.get(&name) {
                for to in targets {
                    if *to != from {
                        import_edges.insert((from.clone(), to.clone()));
                    }
                }
            }
        }
    }

    // Doc→code references (FR-33): parse each markdown file into headed
    // sections, resolve every symbol mention against the code graph built above,
    // and emit `Section --references--> CodeSymbol`. Only documents/sections that
    // resolve at least one reference are materialized — the graph is referential
    // structure, not every heading in the repo.
    let mut docs: Vec<DocTriple> = Vec::new();
    let mut sections: Vec<SectionTriple> = Vec::new();
    let mut ref_edges: BTreeSet<(String, String)> = BTreeSet::new();
    for (rel, source) in doc_sources {
        let rel = rel.clone();
        let doc = document_iri(repo, &rel);
        let source = source.as_str();
        let mut doc_used = false;
        for section in extract_doc_sections(source) {
            let sec_iri = format!("{doc}#{}", section.slug);
            let mut sec_used = false;
            for mention in &section.mentions {
                for target in resolve(mention, &by_name, &stem_by_sym) {
                    if ref_edges.insert((sec_iri.clone(), target)) {
                        sec_used = true;
                    }
                }
            }
            if sec_used {
                sections.push(SectionTriple {
                    iri: sec_iri,
                    heading: section.heading,
                    depth: section.depth,
                    document: doc.clone(),
                });
                doc_used = true;
            }
        }
        if doc_used {
            docs.push(DocTriple {
                iri: doc,
                path: rel,
            });
        }
    }

    let (symbols, collapsed) = dedupe_by_iri(symbols);

    Ok(render(
        repo,
        &modules,
        &symbols,
        &call_edges,
        &import_edges,
        &docs,
        &sections,
        &ref_edges,
        &collapsed,
    ))
}

/// Collapse symbols sharing an IRI to ONE, deterministically, and say which.
///
/// Two mutually-exclusive `#[cfg]` declarations of one name are BOTH seen by
/// the extractor — tree-sitter parses, it does not evaluate `cfg` — so they
/// mint a single IRI carrying two `symbolKind` values. `code-entities.ttl` puts
/// `sh:maxCount 1` on `symbolKind`, so SHACL refuses the ENTIRE promotion and
/// the repo's structure freezes at its last good commit. yupana's own
/// `src/mcp/state_tools.rs` did exactly this and froze yupana's own code graph
/// for a day (aegis-4ba2e).
///
/// FIRST DECLARATION WINS, in file order. Without evaluating `cfg` there is no
/// correct choice between the two, so the requirement here is determinism, not
/// cleverness: the same tree must always project the same bytes, or a
/// re-promotion forks the graph instead of superseding it. `git ls-tree` output
/// is sorted and parse order within a file is stable, so first-wins is stable.
///
/// Only a CROSS-KIND collision is recorded. Two sites with the same kind emit
/// byte-identical `symbolKind` triples, and RDF is a set — they were never a
/// maxCount violation. (`yupana collisions` reports those separately; they are
/// the aegis-1q14 class, and this function is not the place to relitigate it.)
///
/// **The drop is not silent.** Dropping a declaration quietly would trade a
/// loud freeze for a quiet wrong answer, and would hide a GENUINE IRI collision
/// just as effectively as a `cfg` pair — which is why this landed only after a
/// refusal learned to name its focus node (aegis-o8rq8). Each collapse is
/// written into the payload as a Turtle comment, so it travels with the
/// artifact that gets promoted and survives in the one that gets refused.
fn dedupe_by_iri(symbols: Vec<SymbolTriple>) -> (Vec<SymbolTriple>, Vec<String>) {
    let mut first_at: HashMap<String, usize> = HashMap::new();
    let mut kept: Vec<SymbolTriple> = Vec::new();
    // BTreeMap: the notes are part of the emitted bytes, so their order has to
    // be stable too, not just the symbols'.
    let mut collapsed: std::collections::BTreeMap<String, (String, BTreeSet<String>)> =
        std::collections::BTreeMap::new();

    for symbol in symbols {
        if let Some(&i) = first_at.get(&symbol.iri) {
            if kept[i].kind != symbol.kind {
                collapsed
                    .entry(symbol.iri.clone())
                    .or_insert_with(|| (kept[i].kind.clone(), BTreeSet::new()))
                    .1
                    .insert(symbol.kind);
            }
            continue;
        }
        first_at.insert(symbol.iri.clone(), kept.len());
        kept.push(symbol);
    }

    let notes = collapsed
        .into_iter()
        .map(|(iri, (winner, dropped))| {
            format!(
                "# yupana: <{iri}> was declared with more than one symbolKind — kept \
                 \"{winner}\" (first in file order), dropped {}. Mutually-exclusive \
                 #[cfg] declarations of one name look like this; so does a genuine \
                 IRI collision. See aegis-4ba2e.",
                dropped
                    .iter()
                    .map(|k| format!("\"{k}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect();

    (kept, notes)
}

/// Resolve a doc [`Mention`] to concrete `CodeSymbol` IRIs, never inventing one:
/// a mention contributes edges only if its name matches an extracted symbol. A
/// `qualifier::` hint narrows the match to symbols in the module of that stem
/// when any qualify; otherwise every same-named symbol is returned (recall over
/// precision, per the "start permissive" guidance).
fn resolve(
    mention: &Mention,
    by_name: &HashMap<String, Vec<String>>,
    stem_by_sym: &HashMap<String, String>,
) -> Vec<String> {
    // Docs are PROSE: a mention has no language, so it searches ACROSS the
    // language-scoped keys (`{language}\0{name}`) that keep call/import
    // resolution honest. A doc citing `reachable` may legitimately mean the
    // Rust one or the Python one — recall over precision, as before.
    let candidates: Vec<String> = by_name
        .iter()
        .filter(|(key, _)| {
            key.rsplit_once('\u{0}')
                .map_or(key.as_str(), |(_, name)| name)
                == mention.symbol
        })
        .flat_map(|(_, iris)| iris.iter().cloned())
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    if let Some(qualifier) = &mention.qualifier {
        let narrowed: Vec<String> = candidates
            .iter()
            .filter(|iri| {
                stem_by_sym.get(*iri).is_some_and(|stem| {
                    stem.rsplit_once('\u{0}')
                        .map_or(stem.as_str(), |(_, bare)| bare)
                        .eq_ignore_ascii_case(qualifier)
                })
            })
            .cloned()
            .collect();
        if !narrowed.is_empty() {
            return narrowed;
        }
    }
    candidates
}

/// A `CodeSymbol` ready to emit.
struct SymbolTriple {
    iri: String,
    name: String,
    kind: String,
    module: String,
}

/// A `Document` ready to emit.
struct DocTriple {
    iri: String,
    path: String,
}

/// A `Section` ready to emit (only materialized when it has ≥1 reference).
struct SectionTriple {
    iri: String,
    heading: String,
    depth: usize,
    document: String,
}

/// Render the collected structure as a Turtle document.
#[allow(clippy::too_many_arguments)]
fn render(
    repo: &str,
    modules: &[(String, String, &'static str)],
    symbols: &[SymbolTriple],
    call_edges: &BTreeSet<(String, String)>,
    import_edges: &BTreeSet<(String, String)>,
    docs: &[DocTriple],
    sections: &[SectionTriple],
    ref_edges: &BTreeSet<(String, String)>,
    collapsed: &[String],
) -> String {
    // Every entity carries rdfs:label (aegis-lsv6t). This exporter emitted none,
    // and it is the sole producer of the live code plane: 11,928 of its 12,438
    // nodes had no label, so `?s rdfs:label ?l` — the query a person runs to find
    // a thing BY NAME, and the one /ask's labeled_like is built on — returned
    // nothing for any of them. They were not unnamed; the name was on a domain
    // predicate (bobbin:heading, bobbin:name, bobbin:filePath) that only someone
    // who already knew the schema would ask for.
    //
    // Nothing failed and nothing warned, because SHACL only refuses what a shape
    // requires and code-entities.ttl's shapes require the domain predicates, not
    // the label. Quipu's own walker for these same classes
    // (quipu:scripts/ingest-repos.py) has always emitted rdfs:label alongside
    // them — so the values below are not a new convention, they are the sibling
    // emitter's, which is why no fresh judgement about "what the label should be"
    // was needed: heading for a Section, symbol name for a CodeSymbol, file
    // basename for a Document or CodeModule.
    let mut out = String::new();
    out.push_str("@prefix bobbin: <http://aegis.gastown.local/ontology/> .\n");
    out.push_str("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n");
    out.push_str("@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\n");

    // Emitted BEFORE the data, so it is the first thing a reader of a refused
    // payload sees rather than something buried mid-file (aegis-4ba2e).
    if !collapsed.is_empty() {
        for note in collapsed {
            out.push_str(note);
            out.push('\n');
        }
        out.push('\n');
    }

    for (iri, rel, language) in modules {
        out.push_str(&format!(
            "<{iri}> a bobbin:CodeModule ;\n    rdfs:label \"{}\" ;\n    \
             bobbin:filePath \"{}\" ;\n    \
             bobbin:repo \"{}\" ;\n    bobbin:language \"{}\" .\n\n",
            esc(basename(rel)),
            esc(rel),
            esc(repo),
            esc(language),
        ));
    }

    for symbol in symbols {
        out.push_str(&format!(
            "<{}> a bobbin:CodeSymbol ;\n    rdfs:label \"{}\" ;\n    \
             bobbin:name \"{}\" ;\n    \
             bobbin:symbolKind \"{}\" ;\n    bobbin:definedIn <{}> .\n",
            symbol.iri,
            esc(&symbol.name),
            esc(&symbol.name),
            esc(&symbol.kind),
            symbol.module,
        ));
    }

    if !call_edges.is_empty() {
        out.push('\n');
        for (from, to) in call_edges {
            out.push_str(&format!("<{from}> bobbin:calls <{to}> .\n"));
        }
    }

    if !import_edges.is_empty() {
        out.push('\n');
        for (from, to) in import_edges {
            out.push_str(&format!("<{from}> bobbin:imports <{to}> .\n"));
        }
    }

    // ── Documents / Sections / references (FR-33) ──
    if !docs.is_empty() {
        out.push('\n');
        for doc in docs {
            out.push_str(&format!(
                "<{}> a bobbin:Document ;\n    rdfs:label \"{}\" ;\n    \
                 bobbin:filePath \"{}\" ;\n    \
                 bobbin:repo \"{}\" .\n\n",
                doc.iri,
                esc(basename(&doc.path)),
                esc(&doc.path),
                esc(repo),
            ));
        }
    }

    for section in sections {
        out.push_str(&format!(
            "<{}> a bobbin:Section ;\n    rdfs:label \"{}\" ;\n    \
             bobbin:heading \"{}\" ;\n    \
             bobbin:headingDepth {} ;\n    bobbin:inDocument <{}> .\n",
            section.iri,
            esc(&section.heading),
            esc(&section.heading),
            section.depth,
            section.document,
        ));
    }

    if !ref_edges.is_empty() {
        out.push('\n');
        for (from, to) in ref_edges {
            out.push_str(&format!("<{from}> bobbin:references <{to}> .\n"));
        }
    }
    out
}

/// The module-name stem used to resolve `use`/`mod` references to a module: the
/// file stem, except a `mod.rs` takes its parent directory's name (Rust's
/// directory-module convention). Root modules (`lib`/`main`) keep their stem;
/// they are valid import *sources* but rarely import targets.
fn module_stem(rel: &str) -> String {
    let path = Path::new(rel);
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    // Directory-module conventions, one per ecosystem, same shape: the file
    // that IS its directory. Rust's mod.rs, Python's __init__.py, JS/TS's
    // index.*. Each takes the parent directory's name so `import pkg` and
    // `use pkg` resolve to the module that defines pkg.
    if stem == "mod" || stem == "__init__" || stem == "index" {
        return path
            .parent()
            .and_then(Path::file_name)
            .map_or(stem, |n| n.to_string_lossy().into_owned());
    }
    stem
}

/// Mint a `CodeModule` IRI: `{ONTO}code/{repo}/{path}` with `/` percent-encoded
/// in the path segment (mirrors Quipu's `namespace.rs` construction).
pub(crate) fn module_iri(repo: &str, rel: &str) -> String {
    format!("{ONTO}code/{repo}/{}", rel.replace('/', "%2F"))
}

/// Mint a `CodeSymbol` IRI: `{module}::{scope1}::{scope2}::{name}` — the scope
/// chain is what keeps two same-named symbols in one file on distinct IRIs
/// (aegis-1q14: without it, 42 same-kind collisions across bobbin/yupana/quipu
/// silently merged, unioning different symbols' call edges). Scope segments are
/// raw source text (impl types can be `Foo<T>` or `dyn Trait`), so IRI-hostile
/// characters are percent-encoded; `::` between segments is the one separator.
fn symbol_iri(module: &str, scope: &[String], name: &str) -> String {
    let mut iri = String::from(module);
    for seg in scope {
        iri.push_str("::");
        iri.push_str(&iri_segment(seg));
    }
    iri.push_str("::");
    iri.push_str(&iri_segment(name));
    iri
}

/// Percent-encode the characters that are illegal or structural in an IRI
/// reference (space and angle brackets from generic types, quotes, and `%`
/// itself first so encoding is injective — two different raw segments can never
/// encode to the same IRI text).
fn iri_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '%' => out.push_str("%25"),
            ' ' => out.push_str("%20"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '"' => out.push_str("%22"),
            '{' => out.push_str("%7B"),
            '}' => out.push_str("%7D"),
            '|' => out.push_str("%7C"),
            '\\' => out.push_str("%5C"),
            '^' => out.push_str("%5E"),
            '`' => out.push_str("%60"),
            // `[` and `]` are gen-delims reserved for IPv6 literals in the
            // authority and are ILLEGAL in a path segment, so a raw one makes the
            // whole document unparseable — not just its own triple.
            //
            // MEASURED (aegis-r5xta): the hourly quipu code-promote failed on
            // 55 runs over 12 days with `Invalid IRI code point '['`, leaving
            // CodeSymbol/CodeModule frozen at their 2026-07-23 state for quipu and
            // yupana. Every offender was a JavaScript computed method name —
            // `[Symbol.iterator]` in vendored three.module.min.js and
            // mermaid.min.js — not the Rust slice types one would guess.
            '[' => out.push_str("%5B"),
            ']' => out.push_str("%5D"),
            '\n' | '\t' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Mint a `Document` IRI: `{ONTO}doc/{repo}/{path}` with `/` percent-encoded in
/// the path segment (mirrors Quipu's `document_iri`; the section anchor is
/// appended as `#{slug}` by the caller).
fn document_iri(repo: &str, rel: &str) -> String {
    format!("{ONTO}doc/{repo}/{}", rel.replace('/', "%2F"))
}

/// Path relative to `root`, falling back to the file name when the root is the
/// file itself.
fn rel_path(file: &Path, root: &Path) -> String {
    match file.strip_prefix(root) {
        Ok(p) if !p.as_os_str().is_empty() => p.display().to_string(),
        _ => file.file_name().map_or_else(
            || file.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
    }
}

/// Escape a Turtle string literal.
pub(crate) fn esc(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The last path segment, for the human-readable half of a path-shaped label.
/// The full path stays on `bobbin:filePath`, which is what disambiguates two
/// same-named files; the label is what a person types into a name search.
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
#[path = "export_test.rs"]
mod export_test;
