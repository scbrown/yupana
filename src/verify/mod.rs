//! Monitor-guided edit verification — a verdict on a *proposed* buffer.
//!
//! FR-23: given an edited buffer, re-run analysis on it against the base graph
//! Yupana already holds and return a boolean verdict plus violations. FR-24: this
//! is single-signal and boolean, served **directly** to agents — Bobbin may
//! consume verdicts like any other Yupana fact, but verification does not live
//! there.
//!
//! ## What this tier can and cannot decide
//!
//! Every fact Yupana serves carries a tier (FR-3), and a verdict is no exception.
//! At the tree-sitter tier there is no type information and no name resolution,
//! so the verifier checks only what is decidable *syntactically* and reports the
//! rest as unchecked rather than implying a clean bill of health:
//!
//! | Violation (FR-23) | At this tier |
//! |---|---|
//! | `identifier-does-not-exist` | free calls only, and only ones the edit introduces |
//! | `wrong-arity` | free calls resolving to exactly one known definition |
//! | `unresolved-import` | bodiless `mod foo;` with no sibling file |
//! | `type-violation` | **not checked** — needs the LSP tier |
//!
//! The bias throughout is against false positives. A verdict can gate an agent's
//! edit, so a wrong "this is broken" costs more than a missed problem.

mod buffer;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use crate::errors::Result;
use crate::graph::CodeGraph;
use crate::types::Tier;
use buffer::{BufferFacts, CallForm};

/// The kinds of violation FR-23 enumerates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViolationKind {
    /// A call to a name that resolves to no definition.
    IdentifierDoesNotExist,
    /// A call passing the wrong number of arguments.
    WrongArity,
    /// A type error. Never produced at the tree-sitter tier.
    TypeViolation,
    /// A `mod foo;` with no corresponding file.
    UnresolvedImport,
}

/// One problem found in the proposed buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    /// Which kind of problem.
    pub kind: ViolationKind,
    /// The offending name.
    pub symbol: String,
    /// 1-based line in the proposed buffer.
    pub line: usize,
    /// Human- and model-readable explanation.
    pub message: String,
}

/// The verdict on a proposed buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    /// The boolean FR-24 asks for: no violations found.
    pub ok: bool,
    /// The tier this verdict was reached at — never present tree-sitter as LSP.
    pub tier: Tier,
    /// What was found.
    pub violations: Vec<Violation>,
    /// What this tier did **not** check, so `ok: true` is not over-read.
    pub unchecked: Vec<String>,
}

/// What the tree-sitter tier cannot decide, stated plainly in every verdict.
fn unchecked_at_treesitter() -> Vec<String> {
    [
        "type-violation (needs the LSP tier)",
        "method calls `x.f()` (needs the receiver's type)",
        "path-qualified calls `a::b::f()` (may resolve in another crate)",
        "`use` paths (only bodiless `mod` declarations are resolved)",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// Verify `proposed` as the new contents of `file`, against the graph at `root`.
///
/// Only calls the edit *introduces* are reported: a name already broken (or
/// already resolving through machinery this tier cannot see) is pre-existing and
/// not this edit's business. `baseline` is the current contents of `file`, or
/// `None` for a new file.
pub fn verify_buffer(
    root: &Path,
    file: &Path,
    proposed: &str,
    baseline: Option<&str>,
) -> Result<Verdict> {
    let facts = buffer::analyze(proposed)?;
    let baseline_facts = baseline.and_then(|text| buffer::analyze(text).ok());
    let mut existing: BTreeMap<(String, usize), usize> = BTreeMap::new();
    if let Some(baseline) = &baseline_facts {
        for call in baseline.calls.iter().filter(|c| c.form == CallForm::Free) {
            *existing.entry((call.name.clone(), call.arity)).or_insert(0) += 1;
        }
    }

    // The base graph supplies names defined elsewhere in the tree.
    let graph = CodeGraph::build(root).ok();
    let defined_here: BTreeSet<&str> = facts.functions.iter().map(|f| f.name.as_str()).collect();
    let removed_here: BTreeSet<&str> = baseline_facts
        .iter()
        .flat_map(|b| &b.functions)
        .map(|f| f.name.as_str())
        .filter(|name| !defined_here.contains(name))
        .collect();
    let current_file = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");

    let mut violations = Vec::new();
    check_calls(
        &facts,
        &mut existing,
        &defined_here,
        &removed_here,
        &current_file,
        graph.as_ref(),
        &mut violations,
    );
    check_file_modules(root, file, &facts, &mut violations);

    violations.sort_by_key(|v| (v.line, v.symbol.clone()));
    Ok(Verdict {
        ok: violations.is_empty(),
        tier: Tier::TreeSitter,
        violations,
        unchecked: unchecked_at_treesitter(),
    })
}

/// Check the call sites the edit introduces.
fn check_calls(
    facts: &BufferFacts,
    existing: &mut BTreeMap<(String, usize), usize>,
    defined_here: &BTreeSet<&str>,
    removed_here: &BTreeSet<&str>,
    current_file: &str,
    graph: Option<&CodeGraph>,
    out: &mut Vec<Violation>,
) {
    for call in &facts.calls {
        // Only free calls are resolvable by name alone (see `unchecked`).
        if call.form != CallForm::Free {
            continue;
        }
        // Unchanged call sites are not this edit's problem.
        if !removed_here.contains(call.name.as_str()) {
            if let Some(remaining) = existing.get_mut(&(call.name.clone(), call.arity)) {
                if *remaining > 0 {
                    *remaining -= 1;
                    continue;
                }
            }
        }
        // Imports, locals, closures, and fn-typed parameters are all in scope.
        if facts.bound_names.contains(&call.name) {
            continue;
        }

        let in_buffer = defined_here.contains(call.name.as_str());
        let in_graph = graph.is_some_and(|g| {
            g.definitions(&call.name)
                .iter()
                .any(|definition| definition.file != current_file)
        });

        if !in_buffer && !in_graph {
            out.push(Violation {
                kind: ViolationKind::IdentifierDoesNotExist,
                symbol: call.name.clone(),
                line: call.line,
                message: format!(
                    "`{}` is called here but is defined nowhere in this buffer or the \
                     project graph, and is not brought into scope by a `use`. \
                     [tree-sitter tier]",
                    call.name
                ),
            });
            continue;
        }

        // Arity is only decidable against exactly one known, non-method
        // definition in this buffer. Overloads across the tree, trait impls, and
        // methods all make a name ambiguous, so those are left alone.
        let mut matches = facts.functions.iter().filter(|f| f.name == call.name);
        let (Some(def), None) = (matches.next(), matches.next()) else {
            continue;
        };
        if def.takes_self || def.params == call.arity {
            continue;
        }
        out.push(Violation {
            kind: ViolationKind::WrongArity,
            symbol: call.name.clone(),
            line: call.line,
            message: format!(
                "`{}` is called with {} argument(s) but is defined at line {} taking {}. \
                 [tree-sitter tier]",
                call.name, call.arity, def.start_line, def.params
            ),
        });
    }
}

/// Check that each bodiless `mod foo;` has a file behind it.
fn check_file_modules(root: &Path, file: &Path, facts: &BufferFacts, out: &mut Vec<Violation>) {
    if facts.file_modules.is_empty() {
        return;
    }
    let dir = file
        .parent()
        .map_or_else(|| root.to_path_buf(), Path::to_path_buf);
    for name in &facts.file_modules {
        // `mod foo;` resolves to `foo.rs` or `foo/mod.rs` beside the declaring
        // file. (A `#[path]` attribute can redirect it — hence the hedge in the
        // message rather than a flat assertion.)
        if dir.join(format!("{name}.rs")).exists() || dir.join(name).join("mod.rs").exists() {
            continue;
        }
        out.push(Violation {
            kind: ViolationKind::UnresolvedImport,
            symbol: name.clone(),
            line: 0,
            message: format!(
                "`mod {name};` declares a file module, but neither `{name}.rs` nor \
                 `{name}/mod.rs` exists beside this file. [tree-sitter tier]"
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("helpers.rs"), "fn helper() {}\n").unwrap();
        dir
    }

    fn verify(dir: &Path, proposed: &str, baseline: Option<&str>) -> Verdict {
        verify_buffer(dir, &dir.join("a.rs"), proposed, baseline).unwrap()
    }

    #[test]
    fn a_clean_buffer_passes() {
        let dir = project();
        let verdict = verify(dir.path(), "fn f() { helper(); }\n", None);
        assert!(verdict.ok, "unexpected: {:?}", verdict.violations);
        assert_eq!(verdict.tier, Tier::TreeSitter);
        // `ok` is never bare: the verdict always says what it did not check.
        assert!(!verdict.unchecked.is_empty());
    }

    #[test]
    fn an_unknown_identifier_is_caught() {
        let dir = project();
        let verdict = verify(dir.path(), "fn f() { no_such_fn(); }\n", None);
        assert!(!verdict.ok);
        assert_eq!(verdict.violations.len(), 1);
        assert_eq!(
            verdict.violations[0].kind,
            ViolationKind::IdentifierDoesNotExist
        );
        assert_eq!(verdict.violations[0].symbol, "no_such_fn");
    }

    #[test]
    fn wrong_arity_is_caught_against_a_definition_in_the_buffer() {
        let dir = project();
        let verdict = verify(
            dir.path(),
            "fn takes_two(a: u8, b: u8) {}\nfn f() { takes_two(1); }\n",
            None,
        );
        assert!(!verdict.ok);
        let violation = &verdict.violations[0];
        assert_eq!(violation.kind, ViolationKind::WrongArity);
        // The message must carry both numbers to be actionable.
        assert!(violation.message.contains("1 argument(s)"));
        assert!(violation.message.contains("taking 2"));
    }

    #[test]
    fn a_missing_file_module_is_an_unresolved_import() {
        let dir = project();
        let verdict = verify(dir.path(), "mod nope;\n", None);
        assert!(!verdict.ok);
        assert_eq!(verdict.violations[0].kind, ViolationKind::UnresolvedImport);

        // ...and a module that does exist is fine.
        let ok = verify(dir.path(), "mod helpers;\n", None);
        assert!(ok.ok, "unexpected: {:?}", ok.violations);
    }

    #[test]
    fn pre_existing_breakage_is_not_attributed_to_this_edit() {
        let dir = project();
        let baseline = "fn f() { already_broken(); }\n";
        let proposed = "fn f() { already_broken(); helper(); }\n";
        // The edit added a *valid* call; the pre-existing bad one is not its
        // business, so the verdict must stay clean.
        let verdict = verify(dir.path(), proposed, Some(baseline));
        assert!(verdict.ok, "unexpected: {:?}", verdict.violations);
    }

    #[test]
    fn a_newly_introduced_break_is_attributed_to_this_edit() {
        let dir = project();
        let baseline = "fn f() { helper(); }\n";
        let proposed = "fn f() { helper(); newly_bad(); }\n";
        let verdict = verify(dir.path(), proposed, Some(baseline));
        assert!(!verdict.ok);
        assert_eq!(verdict.violations[0].symbol, "newly_bad");
    }

    #[test]
    fn an_additional_occurrence_of_pre_existing_breakage_is_new() {
        let dir = project();
        let baseline = "fn f() { already_broken(); }\n";
        let proposed = "fn f() { already_broken(); already_broken(); }\n";

        let verdict = verify(dir.path(), proposed, Some(baseline));

        assert!(!verdict.ok, "the added occurrence was silently exempted");
        assert_eq!(verdict.violations.len(), 1);
        assert_eq!(verdict.violations[0].symbol, "already_broken");
        assert_eq!(
            verdict.violations[0].kind,
            ViolationKind::IdentifierDoesNotExist
        );
    }

    #[test]
    fn a_pre_existing_method_does_not_exempt_a_new_free_call() {
        let dir = project();
        let baseline = "fn f(x: Thing) { x.ghost(); }\n";
        let proposed = "fn f(x: Thing) { x.ghost(); ghost(); }\n";

        let verdict = verify(dir.path(), proposed, Some(baseline));

        assert!(!verdict.ok, "the method call exempted a new bare call");
        assert_eq!(verdict.violations.len(), 1);
        assert_eq!(verdict.violations[0].symbol, "ghost");
        assert_eq!(
            verdict.violations[0].kind,
            ViolationKind::IdentifierDoesNotExist
        );
    }

    #[test]
    fn removing_a_local_definition_breaks_its_retained_call() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = "fn local_only() {}\nfn f() { local_only(); }\n";
        std::fs::write(dir.path().join("a.rs"), baseline).unwrap();
        let proposed = "fn f() { local_only(); }\n";

        let verdict = verify(dir.path(), proposed, Some(baseline));

        assert!(
            !verdict.ok,
            "the deleted definition survived through the base graph"
        );
        assert_eq!(verdict.violations.len(), 1);
        assert_eq!(verdict.violations[0].symbol, "local_only");
    }

    #[test]
    fn removing_one_definition_is_safe_when_another_file_defines_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = "fn shared() {}\nfn f() { shared(); }\n";
        std::fs::write(dir.path().join("a.rs"), baseline).unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn shared() {}\n").unwrap();
        let proposed = "fn f() { shared(); }\n";

        let verdict = verify(dir.path(), proposed, Some(baseline));

        assert!(
            verdict.ok,
            "the other file still defines shared: {verdict:?}"
        );
    }

    #[test]
    fn imports_locals_and_callbacks_are_not_false_positives() {
        let dir = project();
        // Each of these is a bare call to a name with no local `fn` definition;
        // a naive check would flag all three.
        let verdict = verify(
            dir.path(),
            "use std::mem::swap;\n\
             fn f(callback: fn()) {\n\
                 let closure = || {};\n\
                 closure();\n\
                 callback();\n\
                 swap(&mut 1, &mut 2);\n\
             }\n",
            None,
        );
        assert!(verdict.ok, "false positives: {:?}", verdict.violations);
    }

    #[test]
    fn methods_and_path_calls_are_left_alone() {
        let dir = project();
        // Unresolvable at this tier, so they must be silent rather than wrong.
        let verdict = verify(
            dir.path(),
            "fn f(x: Thing) { x.whatever(); other_crate::thing(); }\n",
            None,
        );
        assert!(verdict.ok, "false positives: {:?}", verdict.violations);
    }

    #[test]
    fn a_method_is_not_arity_checked_for_its_self_receiver() {
        let dir = project();
        // `m` takes (&self, x) = 2 params; the free-call form passes 1. Counting
        // naively would report a bogus arity error.
        let verdict = verify(
            dir.path(),
            "struct S;\nimpl S { fn m(&self, x: u8) {} }\nfn f() { m(1); }\n",
            None,
        );
        assert!(verdict.ok, "false positives: {:?}", verdict.violations);
    }

    #[test]
    fn a_symbol_defined_elsewhere_in_the_tree_resolves() {
        let dir = project();
        // `helper` lives in helpers.rs, not in this buffer — the base graph is
        // what makes this resolvable.
        let verdict = verify(dir.path(), "fn f() { helper(); }\n", None);
        assert!(verdict.ok, "unexpected: {:?}", verdict.violations);
    }
}
