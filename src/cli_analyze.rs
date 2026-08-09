//! `analyze`, `refs` and `changed` — the commands that BUILD structure and
//! report it, split out of `cli` for size (yupana #83).
//!
//! A child module of `cli`, not a sibling: these stay `impl Cli` methods reading
//! the global output flags (`--json`, `--quiet`) and `load_config` straight off
//! `self`, exactly as before, which a sibling could not do without widening
//! `Cli`'s private fields. Only the entry points are `pub(super)`.

use super::*;

impl Cli {
    /// Build the base graph for `path` and print a summary. With `at`, source
    /// the summary from the git tree at that ref (the FR-13 base) rather than
    /// the working tree.
    pub(super) fn analyze(&self, path: &Path, at: Option<&str>) -> anyhow::Result<()> {
        // The `languages` key becomes real here (aegis-ltjo): analysis is
        // restricted to the configured set instead of always extracting every
        // compiled grammar. Discovery roots at the analyze target.
        let languages = self.load_config(path)?.languages;
        let (files, symbols) = match at {
            Some(reference) => Self::analyze_at_ref(path, reference, &languages)?,
            None => Self::analyze_working_tree(path, &languages)?,
        };

        if self.json {
            let mut out =
                serde_json::json!({ "files": files, "symbols": symbols, "tier": "treesitter" });
            if let Some(reference) = at {
                out["at"] = serde_json::json!(reference);
            }
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else if !self.quiet {
            let at_note = at.map_or_else(String::new, |r| format!(" @ {r}"));
            println!(
                "{} {files} file(s), {symbols} symbol(s) [tree-sitter]{at_note}",
                "analyzed".green().bold()
            );
        }
        Ok(())
    }

    /// Count files and symbols across the working tree under `path`, restricted
    /// to the configured `languages` (aegis-ltjo).
    fn analyze_working_tree(path: &Path, languages: &[String]) -> anyhow::Result<(usize, usize)> {
        let mut files = 0usize;
        let mut symbols = 0usize;
        for (file, language) in crate::extract::source_files_in(path, languages) {
            let source = std::fs::read_to_string(&file)?;
            files += 1;
            symbols += extract_symbols(&source, language)?.len();
        }
        Ok((files, symbols))
    }

    /// Count files and symbols in the git tree at `reference` (the FR-13 base).
    fn analyze_at_ref(
        path: &Path,
        reference: &str,
        languages: &[String],
    ) -> anyhow::Result<(usize, usize)> {
        let root = std::env::current_dir()?;
        // REFUSE rather than report an empty baseline. `analyze --at no-such-ref`
        // printed "0 file(s), 0 symbol(s)" and exited 0, which is what a ref
        // holding no parseable files looks like — so a typo in a ref name read as
        // a real, empty measurement.
        if !crate::git::is_repo(&root) {
            anyhow::bail!(
                "not a git work tree (or `git` is unavailable), so NO BASELINE was \
                 built at `{reference}` — this is not an empty baseline"
            );
        }
        if crate::git::resolve_commit(&root, reference).is_none() {
            anyhow::bail!(
                "`{reference}` does not resolve to a commit, so NO BASELINE was \
                 built — this is not an empty baseline"
            );
        }
        let prefix = path.strip_prefix(".").unwrap_or(path);
        let mut files = 0usize;
        let mut symbols = 0usize;
        for file in crate::git::list_files_at(&root, reference) {
            if !file.starts_with(prefix) {
                continue;
            }
            // Honour the configured languages instead of hardcoding Rust: a file
            // whose extension maps to no compiled grammar, or to one the config
            // excludes, is skipped (aegis-ltjo).
            let Some(language) = file
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .and_then(crate::extract::language_for_extension)
                .filter(|lang| languages.iter().any(|a| a == lang))
            else {
                continue;
            };
            let Some(source) = crate::git::read_blob_at(&root, reference, &file) else {
                continue;
            };
            files += 1;
            symbols += extract_symbols(&source, language)?.len();
        }
        Ok((files, symbols))
    }

    /// Find definitions of `symbol` by name under `path`.
    ///
    /// Reads the SAME graph `callers`/`impact` read, deliberately. This walked
    /// `rust_files` and parsed every hit as `"rust"`, so on a Python (or Go,
    /// or TypeScript) tree it scanned ZERO files and printed "no definition
    /// found" — while `yupana callers` on the same symbol in the same tree
    /// answered from the multi-language graph and listed call sites (yupana #76).
    /// That is the `from_sources` "parse each file as the language it IS" bug
    /// surviving in the one command whose name advertises symbol lookup, and it
    /// failed in the worst direction: a confident "this symbol does not exist"
    /// rather than an error.
    pub(super) fn refs(
        &self,
        symbol: Option<&str>,
        path: &Path,
        at: Option<&str>,
    ) -> anyhow::Result<()> {
        // With `--at`, the lone positional is the search PATH, not a symbol: a
        // name is redundant once a position names the symbol, and reading it as
        // one would reject the natural `yupana refs --at a.rs:3 .` as a conflict.
        let root = match (at, symbol) {
            (Some(_), Some(positional)) => PathBuf::from(positional),
            _ => path.to_path_buf(),
        };
        let graph = crate::graph::CodeGraph::build(&root)?;
        let (nodes, _) = graph.stats();

        // `--at FILE:LINE` names the symbol by POSITION (FR-4, yupana #8), and
        // answers with THAT symbol — not with every symbol sharing its name.
        // Resolving the position to a name and then looking the name up would
        // hand back all twelve `build`s again, which is the exact
        // over-connection the position form exists to cut through.
        let (symbol, hits) = match (symbol, at) {
            (_, Some(at)) => match self.resolve_at(&graph, at, nodes)? {
                Some(node) => (node.name.clone(), vec![node]),
                None => return Ok(()),
            },
            (Some(name), None) => (name.to_string(), graph.definitions(name)),
            (None, None) => {
                anyhow::bail!("give a symbol name or --at FILE:LINE");
            }
        };
        let symbol = symbol.as_str();

        if self.json {
            let rows: Vec<_> = hits
                .iter()
                .map(|sym| {
                    serde_json::json!({
                        "file": sym.file,
                        "name": sym.name,
                        "kind": sym.kind,
                        "start_line": sym.start_line,
                        "end_line": sym.end_line,
                        // `as_str()`, not the raw serde form: `Tier`'s derive
                        // renames to snake_case ("tree_sitter") while every
                        // other served surface — MCP, the daemon wire,
                        // `not_found` — spells it "treesitter" (the documented
                        // wire/ontology form). Emitting both spellings in ONE
                        // document made a consumer's tier check position-
                        // dependent.
                        "tier": sym.tier.as_str(),
                    })
                })
                .collect();
            // The empty answer carries its tier too (FR-3) — the hole
            // `cli_cmds::not_found` closed for callers/impact/dataflow, closed
            // here. `searched` is the honest half of a zero result: 0 symbols
            // searched means NOTHING here was parseable, which is a different
            // fact from "the name is absent from a graph that has 4000 symbols"
            // and must not be reported as the same one.
            let out = serde_json::json!({
                "symbol": symbol,
                "count": rows.len(),
                "definitions": rows,
                "searched_symbols": nodes,
                "tier": "treesitter",
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else if hits.is_empty() {
            if !self.quiet {
                if nodes == 0 {
                    println!(
                        "no definition found for {symbol} \
                         (nothing parseable under {} — the graph is empty, \
                         so this is not evidence the symbol is absent)",
                        path.display()
                    );
                } else {
                    println!("no definition found for {symbol} (searched {nodes} symbol(s))");
                }
            }
        } else {
            for sym in &hits {
                println!(
                    "{}:{} {} ({}) [{:?}]",
                    sym.file,
                    sym.start_line,
                    sym.name.cyan(),
                    sym.kind,
                    sym.tier
                );
            }
        }
        Ok(())
    }

    /// Print the entities a change touches — and, separately, the files it
    /// could NOT read.
    ///
    /// The two lists are printed apart on purpose. A rule enforced on the first
    /// list while the second is non-empty has judged a SUBSET of the change and
    /// will report a clean result for it; the operator has to be able to see
    /// that from the output, not infer it. Exit 2 when anything was unread, for
    /// the same reason: a caller that only checks the exit code still learns
    /// that the answer was partial.
    pub(super) fn changed(&self, base: Option<&str>, to: Option<&str>) -> anyhow::Result<()> {
        let root = std::env::current_dir()?;
        let config = YupanaConfig::load(&root)?;
        let base = base.unwrap_or(&config.base_ref);

        let set = match crate::change::changed_entities(&root, base, to) {
            Ok(set) => set,
            Err(e) => {
                // NOT an empty change. Say which, and fail — a caller that read
                // "0 entities" here would treat an unevaluated change as a clean
                // one, which is the premise this command exists to protect.
                if self.json {
                    println!(
                        "{}",
                        serde_json::json!({ "error": e.to_string(), "evaluated": false })
                    );
                } else {
                    eprintln!("yupana: {e}");
                }
                std::process::exit(2);
            }
        };

        if self.json {
            println!("{}", serde_json::to_string_pretty(&set)?);
        } else {
            println!("{}", "yupana changed".bold());
            println!("  base : {}", set.base);
            println!("  to   : {}", set.to);
            if set.entities.is_empty() {
                println!("  entities: none — this change touches no known entities");
            } else {
                println!("  entities: {}", set.entities.len());
                for e in &set.entities {
                    println!("    {:<9} {} :: {}", e.kind, e.file, e.name);
                }
            }
            if let Some(summary) = set.unread_summary() {
                println!();
                println!("  ⚠ {summary}");
                for u in &set.unread {
                    println!("    {} — {}", u.file, u.why);
                }
                println!("    A rule judged on the entities above has NOT been applied to these.");
            }
        }
        if !set.fully_read() {
            std::process::exit(2);
        }
        Ok(())
    }

    /// Resolve `--at FILE:LINE` to the name of the symbol enclosing that line.
    ///
    /// `Ok(None)` means "said why, nothing to report" — the caller returns
    /// quietly. Every miss is EXPLAINED rather than silently answered as an
    /// absent symbol, because a position that resolves to nothing is exactly the
    /// confident-wrong-answer shape yupana #76 was about: "no definitions" reads
    /// as "this does not exist" when the truth is "I could not find what you
    /// pointed at".
    fn resolve_at<'g>(
        &self,
        graph: &'g crate::graph::CodeGraph,
        at: &str,
        nodes: usize,
    ) -> anyhow::Result<Option<&'g crate::graph::SymbolNode>> {
        // Split from the RIGHT, but count first: `rsplit_once(':')` alone reads
        // `a.rs:3:9` as file `a.rs:3` line `9`, silently accepting a column by
        // folding it into the filename — the failure mode the refusal below
        // exists to prevent, hidden in the parser.
        let parts: Vec<&str> = at.rsplitn(3, ':').collect();
        let (file, line_part) = match parts.as_slice() {
            // A column is REFUSED, not ignored. The extractor records lines, so
            // resolving `file:12:7` as line 12 would serve line precision under
            // a column-precise request — the FR-3 rule against presenting an
            // approximation as the finer tier.
            [col, line_part, file] if col.chars().all(|c| c.is_ascii_digit()) => {
                anyhow::bail!(
                    "--at takes FILE:LINE, not FILE:LINE:COL (got column `{col}`). The \
                     tree-sitter tier resolves to the innermost symbol on a LINE; \
                     column-precise resolution needs the LSP tier (FR-2), which is not \
                     built. Retry as `{file}:{line_part}`."
                );
            }
            [line_part, file] => (*file, *line_part),
            _ => anyhow::bail!("--at wants FILE:LINE, got `{at}`"),
        };
        let line: usize = line_part
            .parse()
            .map_err(|_| anyhow::anyhow!("--at wants a line number, got `{line_part}`"))?;

        if let Some(node) = graph.symbol_at(file, line) {
            return Ok(Some(node));
        }

        // Nothing encloses that line. Separate the three reasons, because they
        // call for different fixes.
        if !self.quiet {
            let in_file = graph.file_symbols(file);
            if nodes == 0 {
                println!(
                    "nothing parseable under this path — the graph is empty, so `{at}` \
                     resolves to nothing for that reason, not because the line is blank"
                );
            } else if in_file.is_empty() {
                println!(
                    "no symbols in the graph for `{file}` (is the path relative to the \
                     search root, and does this build have a grammar for it?) — searched \
                     {nodes} symbol(s)"
                );
            } else {
                let near: Vec<String> = in_file
                    .iter()
                    .map(|s| format!("{}:{}-{}", s.name, s.start_line, s.end_line))
                    .collect();
                println!(
                    "no symbol encloses {file}:{line} — it falls between definitions \
                     (a blank line, an import, a top-level comment). `{file}` defines: {}",
                    near.join(", ")
                );
            }
        }
        Ok(None)
    }
}
