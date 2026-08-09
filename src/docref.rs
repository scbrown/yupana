//! Doc→code reference extraction — the markdown side of the referential graph
//! (spec §5.10 / FR-33).
//!
//! This is Yupana's build-free markdown scanner. It splits a document into headed
//! [`DocSection`]s and, within each, harvests candidate *code-symbol mentions*:
//! backtick code spans (`` `foo` ``, `` `graph::reachable` ``), fenced code
//! blocks, and markdown link targets (`[..](mod::sym)`). Each mention is a
//! *name* (plus an optional `qualifier::` hint) — resolution to a concrete
//! `CodeSymbol` IRI happens in the exporter ([`crate::export`]) against the code
//! graph, so this module never fabricates a symbol identity: it only proposes
//! names to look up.
//!
//! Deliberately dependency-free (a line scanner, not tree-sitter-md): the mention
//! surface is small and the "start permissive" guidance (§9.2) means recall over
//! a heavy grammar. Unresolved mentions are simply dropped downstream.

use std::path::{Path, PathBuf};

/// A candidate reference to a code symbol, harvested from doc prose or code.
///
/// `qualifier` carries the segment before `::` in a `qualifier::symbol` mention
/// (a module stem or receiver type, best-effort); the exporter uses it to narrow
/// an ambiguous name to the right module when it can.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mention {
    /// The bare symbol name to resolve (last `::` segment, sans call parens).
    pub symbol: String,
    /// Optional `qualifier::` hint (module stem or receiver type).
    pub qualifier: Option<String>,
}

/// A headed section of a document plus the code mentions found under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocSection {
    /// The heading text (ATX `#…` line, trimmed).
    pub heading: String,
    /// Heading depth (number of leading `#`, 1 = top level).
    pub depth: usize,
    /// GitHub-style anchor slug, unique within the document.
    pub slug: String,
    /// Deduplicated, sorted symbol mentions under this heading.
    pub mentions: Vec<Mention>,
}

/// Walk `path` for markdown files, honoring `.gitignore`.
#[must_use]
pub fn doc_files(path: &Path) -> Vec<PathBuf> {
    ignore::WalkBuilder::new(path)
        .build()
        .filter_map(std::result::Result::ok)
        .map(ignore::DirEntry::into_path)
        // One selection rule, shared with the commit-tree path — a README under
        // `node_modules/` is third-party noise for the same reason its `.js` is.
        .filter(|p| crate::extract::is_selectable_doc(p))
        .collect()
}

/// Split `source` into headed sections, harvesting code-symbol mentions in each.
///
/// Mentions appearing before the first heading are dropped: a `Section` in the
/// ontology requires a heading, and real docs open with a title. Sections with
/// no mentions are still returned (the exporter decides what to emit).
#[must_use]
pub fn extract_doc_sections(source: &str) -> Vec<DocSection> {
    let mut sections: Vec<DocSection> = Vec::new();
    let mut slugs_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut current: Option<DocSection> = None;
    let mut in_fence = false;
    let mut fence_marker: Option<(char, usize)> = None;

    for line in source.lines() {
        let trimmed = line.trim_start();

        // Fence toggling: a run of ``` or ~~~ opens/closes a code block. The
        // closing marker must match the opening character and be at least as long.
        if let Some((ch, len)) = fence_run(trimmed) {
            match fence_marker {
                None => {
                    in_fence = true;
                    fence_marker = Some((ch, len));
                }
                Some((open_ch, open_len)) if ch == open_ch && len >= open_len => {
                    in_fence = false;
                    fence_marker = None;
                }
                _ => {}
            }
            continue; // fence delimiter lines carry no mentions
        }

        if in_fence {
            if let Some(sec) = current.as_mut() {
                scan_scoped_idents(line, &mut sec.mentions);
            }
            continue;
        }

        if let Some((depth, heading)) = atx_heading(trimmed) {
            if let Some(done) = current.take() {
                sections.push(finish(done));
            }
            let slug = unique_slug(&heading, &mut slugs_seen);
            current = Some(DocSection {
                heading,
                depth,
                slug,
                mentions: Vec::new(),
            });
            continue;
        }

        // Prose line: only backtick spans and link targets are treated as
        // mentions — scanning every word would flood the graph with prose.
        if let Some(sec) = current.as_mut() {
            let mut spans: Vec<String> = Vec::new();
            backtick_spans(line, &mut spans);
            link_targets(line, &mut spans);
            for span in spans {
                if let Some(m) = parse_mention(&span) {
                    sec.mentions.push(m);
                }
            }
        }
    }

    if let Some(done) = current.take() {
        sections.push(finish(done));
    }
    sections
}

/// Dedup and sort a section's mentions.
fn finish(mut sec: DocSection) -> DocSection {
    sec.mentions.sort();
    sec.mentions.dedup();
    sec
}

/// If `line` is an ATX heading (`#`…`######` + space + text), return its depth
/// and trimmed text (trailing closing `#` stripped).
fn atx_heading(line: &str) -> Option<(usize, String)> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    // A real heading needs a space (or nothing) after the hashes; `#foo` is not
    // a heading, it is a fragment.
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some((hashes, text))
}

/// If `line` is a fence delimiter, return its `(char, run-length)`.
fn fence_run(line: &str) -> Option<(char, usize)> {
    for ch in ['`', '~'] {
        let len = line.chars().take_while(|&c| c == ch).count();
        if len >= 3 {
            return Some((ch, len));
        }
    }
    None
}

/// Push the contents of every backtick code span on `line` into `out`. Handles
/// runs of N backticks closed by a matching run of N.
fn backtick_spans(line: &str, out: &mut Vec<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        let run_start = i;
        while i < chars.len() && chars[i] == '`' {
            i += 1;
        }
        let run = i - run_start;
        let content_start = i;
        // Find a closing run of exactly `run` backticks.
        let mut j = i;
        let mut closed: Option<(usize, usize)> = None;
        while j < chars.len() {
            if chars[j] == '`' {
                let rs = j;
                while j < chars.len() && chars[j] == '`' {
                    j += 1;
                }
                if j - rs == run {
                    closed = Some((rs, j));
                    break;
                }
            } else {
                j += 1;
            }
        }
        match closed {
            Some((content_end, after)) => {
                let content: String = chars[content_start..content_end].iter().collect();
                out.push(content.trim().to_string());
                i = after;
            }
            None => break, // unterminated span; give up on the rest of the line
        }
    }
}

/// Push the target of every `[text](target)` markdown link on `line` into `out`.
fn link_targets(line: &str, out: &mut Vec<String>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b'(' {
            let start = i + 2;
            if let Some(rel) = line[start..].find(')') {
                let target = &line[start..start + rel];
                // Drop a trailing " \"title\"" and any URL fragment/query.
                let target = target.split_whitespace().next().unwrap_or(target);
                out.push(target.to_string());
                i = start + rel + 1;
                continue;
            }
        }
        i += 1;
    }
}

/// Parse one backtick-span / link-target string into a [`Mention`], or `None`
/// if it names no plausible symbol (a bare path, a flag, prose, …).
///
/// Takes the leading run of identifier/`:` characters, so `reachable()` →
/// `reachable`, `graph::reachable<T>` → qualifier `graph` + `reachable`,
/// `src/graph.rs` → `src` (a stem that simply won't resolve), `--json` → `None`.
fn parse_mention(raw: &str) -> Option<Mention> {
    let raw = raw.trim();
    let end = raw
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
        .unwrap_or(raw.len());
    let head = &raw[..end];
    let segs: Vec<&str> = head.split("::").filter(|s| !s.is_empty()).collect();
    let symbol = (*segs.last()?).to_string();
    if !is_ident(&symbol) {
        return None;
    }
    let qualifier = if segs.len() >= 2 {
        normalize_qualifier(segs[segs.len() - 2])
    } else {
        None
    };
    Some(Mention { symbol, qualifier })
}

/// The qualifier is the segment immediately before the symbol, unless it is a
/// path anchor (`crate`/`self`/`super`/`std`/`core`), which carries no module
/// identity.
fn normalize_qualifier(prev: &str) -> Option<String> {
    if matches!(prev, "crate" | "self" | "super" | "std" | "core") {
        return None;
    }
    Some(prev.to_string())
}

/// Scan a code line for `ident(::ident)*` sequences, pushing each as a mention.
/// Used inside fenced blocks, where any token may be a symbol reference.
fn scan_scoped_idents(text: &str, out: &mut Vec<Mention>) {
    let chars: Vec<char> = text.chars().collect();
    let is_start = |c: char| c.is_alphabetic() || c == '_';
    let is_cont = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = 0;
    while i < chars.len() {
        if !is_start(chars[i]) {
            i += 1;
            continue;
        }
        let mut segments: Vec<String> = Vec::new();
        loop {
            let seg_start = i;
            while i < chars.len() && is_cont(chars[i]) {
                i += 1;
            }
            segments.push(chars[seg_start..i].iter().collect());
            if i + 1 < chars.len() && chars[i] == ':' && chars[i + 1] == ':' {
                i += 2;
                if i < chars.len() && is_start(chars[i]) {
                    continue;
                }
            }
            break;
        }
        if let Some(symbol) = segments.last().cloned() {
            let qualifier = if segments.len() >= 2 {
                let prev = segments[segments.len() - 2].as_str();
                if matches!(prev, "crate" | "self" | "super" | "std" | "core") {
                    None
                } else {
                    Some(prev.to_string())
                }
            } else {
                None
            };
            out.push(Mention { symbol, qualifier });
        }
    }
}

/// A valid identifier: leading letter/`_`, then letters/digits/`_`.
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// GitHub-style anchor slug: lowercase, spaces/underscores→`-`, punctuation
/// dropped, runs of `-` collapsed.
fn slugify(heading: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true; // leading dashes are trimmed by starting "true"
    for c in heading.chars() {
        if c.is_alphanumeric() {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            last_dash = false;
        } else if (c == ' ' || c == '-' || c == '_') && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Slug for `heading`, suffixed `-1`, `-2`, … if already used in this document.
fn unique_slug(heading: &str, seen: &mut std::collections::HashMap<String, usize>) -> String {
    let base = slugify(heading);
    let base = if base.is_empty() {
        "section".to_string()
    } else {
        base
    };
    let count = seen.entry(base.clone()).or_insert(0);
    let slug = if *count == 0 {
        base.clone()
    } else {
        format!("{base}-{count}")
    };
    *count += 1;
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(sym: &str) -> Mention {
        Mention {
            symbol: sym.to_string(),
            qualifier: None,
        }
    }

    fn mq(qual: &str, sym: &str) -> Mention {
        Mention {
            symbol: sym.to_string(),
            qualifier: Some(qual.to_string()),
        }
    }

    #[test]
    fn splits_sections_by_heading() {
        let md = "# Title\n\nprose\n\n## Sub\n\nmore\n";
        let secs = extract_doc_sections(md);
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[0].heading, "Title");
        assert_eq!(secs[0].depth, 1);
        assert_eq!(secs[1].heading, "Sub");
        assert_eq!(secs[1].depth, 2);
        assert_eq!(secs[1].slug, "sub");
    }

    #[test]
    fn harvests_backtick_mentions() {
        let md = "# H\n\nCall `reachable` and `graph::update` here.\n";
        let secs = extract_doc_sections(md);
        assert!(secs[0].mentions.contains(&m("reachable")));
        assert!(secs[0].mentions.contains(&mq("graph", "update")));
    }

    #[test]
    fn strips_call_parens_and_generics() {
        let md = "# H\n\nUse `reachable()` and `collect::<Vec<_>>()`.\n";
        let secs = extract_doc_sections(md);
        assert!(secs[0].mentions.contains(&m("reachable")));
        assert!(secs[0].mentions.contains(&m("collect")));
    }

    #[test]
    fn drops_path_anchors_as_qualifier() {
        let md = "# H\n\nSee `crate::graph::reachable`.\n";
        let secs = extract_doc_sections(md);
        // qualifier is the segment before the symbol: `graph`, not `crate`.
        assert!(secs[0].mentions.contains(&mq("graph", "reachable")));
    }

    #[test]
    fn ignores_flags_and_plain_paths_as_symbols() {
        let md = "# H\n\nRun `--json` on `docs/readme` first.\n";
        let secs = extract_doc_sections(md);
        // `--json` yields nothing; `docs/readme` yields only the stem `docs`.
        assert!(!secs[0].mentions.iter().any(|x| x.symbol == "json"));
        assert!(secs[0].mentions.iter().all(|x| x.symbol != "readme"));
    }

    #[test]
    fn harvests_fenced_code_identifiers() {
        let md = "# H\n\n```rust\nfn f() { reachable(); other::thing(); }\n```\n";
        let secs = extract_doc_sections(md);
        assert!(secs[0].mentions.contains(&m("reachable")));
        assert!(secs[0].mentions.contains(&mq("other", "thing")));
    }

    #[test]
    fn harvests_link_targets() {
        let md = "# H\n\nSee [the fn](graph::reachable) for details.\n";
        let secs = extract_doc_sections(md);
        assert!(secs[0].mentions.contains(&mq("graph", "reachable")));
    }

    #[test]
    fn fence_delimiters_are_not_mentions() {
        let md = "# H\n\n~~~\nfoo\n~~~\n";
        let secs = extract_doc_sections(md);
        assert!(secs[0].mentions.contains(&m("foo")));
        // the tildes themselves produced no bogus mention
        assert_eq!(secs[0].mentions.len(), 1);
    }

    #[test]
    fn mentions_before_first_heading_are_dropped() {
        let md = "prose with `orphan` before any heading\n\n# H\n\n`kept`\n";
        let secs = extract_doc_sections(md);
        assert_eq!(secs.len(), 1);
        assert!(secs[0].mentions.contains(&m("kept")));
        assert!(!secs[0].mentions.iter().any(|x| x.symbol == "orphan"));
    }

    #[test]
    fn duplicate_headings_get_unique_slugs() {
        let md = "# Setup\n\na\n\n# Setup\n\nb\n";
        let secs = extract_doc_sections(md);
        assert_eq!(secs[0].slug, "setup");
        assert_eq!(secs[1].slug, "setup-1");
    }

    #[test]
    fn slugify_matches_github_style() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  Multi   Space  "), "multi-space");
        assert_eq!(slugify("snake_case name"), "snake-case-name");
    }

    #[test]
    fn atx_heading_requires_space() {
        assert!(atx_heading("#notaheading").is_none());
        assert_eq!(atx_heading("# yes").unwrap(), (1, "yes".to_string()));
        assert_eq!(
            atx_heading("### deep ###").unwrap(),
            (3, "deep".to_string())
        );
        assert!(atx_heading("####### too-deep").is_none());
    }
}
