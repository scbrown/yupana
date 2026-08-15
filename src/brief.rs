//! The work-item briefing — the CONTEXT consumer of the scope ladder
//! (docs/work-scoped-governance.md: one resolution, three consumers).
//!
//! The pre-edit guard is the last line of defense; this is the first. When a
//! work item is assigned (the tracker publishes the plate), the agent is
//! handed what the suite already knows — the item's ground (paths prior work
//! touched) and its structural neighborhood, SIMILAR work items with their
//! outcomes so successful work gets reused rather than rediscovered, the
//! CENTRAL entities the neighborhood hangs off, related in-flight work, and
//! the governed rules in force — BEFORE the first mistake, not after it.
//!
//! Served two ways: `yupana hook session-start` (a Claude Code `SessionStart`
//! hook whose stdout injects the briefing into the agent's context) and the
//! same adapter fed by hand (`echo '{}' | yupana hook session-start`).
//! Sources live in [`crate::brief_sources`] — each one a reused surface of
//! the suite (quipu `/context`, `/project` pagerank, SPARQL, the shared
//! policy projection), never a reimplementation.
//!
//! DIVISION OF LABOR, so the suite's tools never duplicate each other:
//! Bobbin's own hooks (`bobbin hook install`: `inject-context` on
//! UserPromptSubmit, `session-context`/`prime-context` on SessionStart) own
//! SEMANTIC CODE context — query-driven, embedding/index-backed. This
//! briefing owns the GOVERNANCE/WORK-ITEM context — plate-driven,
//! graph-backed. A deployment wires both hooks side by side; this module
//! deliberately does not shell out to bobbin, because nesting one tool's
//! injection inside another's would inject the same context twice.

use crate::config::YupanaConfig;

pub use crate::brief_sources::gather;

/// Everything the briefing says, gathered before rendering so the compose is
/// pure and testable.
#[derive(Debug, Default)]
pub struct Brief {
    /// The tracked work item id (from the plate).
    pub item: String,
    /// The item's label in the graph, when it has one.
    pub label: Option<String>,
    /// The observed ground: paths prior work on the item touched, each with
    /// the symbols defined there and the files their callers live in.
    pub ground: Vec<GroundPath>,
    /// Items whose commits touched the same entities — likely coordination.
    pub related: Vec<String>,
    /// Semantically similar work items, with outcome and ground — successful
    /// ones are the reuse signal.
    pub similar: Vec<SimilarItem>,
    /// The central entities around the item's ground (personalized pagerank),
    /// as `(iri-or-label, score)`.
    pub central: Vec<(String, f64)>,
    /// Governed rule names in force at the edit boundary.
    pub rules: Vec<String>,
    /// The scope posture line (what happens outside the ground).
    pub posture: String,
    /// `Some(age)` when the projection was served from the durable cache.
    pub cache_age: Option<u64>,
}

/// One path of the observed ground, with its structural neighborhood.
#[derive(Debug, Default)]
pub struct GroundPath {
    /// Repo-relative path.
    pub path: String,
    /// Symbols defined in the file (capped).
    pub symbols: Vec<String>,
    /// Files outside the ground that call into those symbols (capped) — where
    /// an edit here will be felt.
    pub felt_from: Vec<String>,
}

/// A work item similar to the tracked one, per quipu's context pipeline.
#[derive(Debug, Default)]
pub struct SimilarItem {
    /// Tracker id.
    pub id: String,
    /// Label, when the graph has one.
    pub label: Option<String>,
    /// Declared outcome (`done` = successful, worth reusing); `None` = open.
    pub outcome: Option<String>,
    /// The similarity score the pipeline assigned.
    pub score: f64,
    /// The paths that item's work touched — its reusable ground.
    pub ground: Vec<String>,
}

/// The governed rules in force, named for the briefing.
#[must_use]
pub fn rules_in_force(registry: &crate::project::ProjectionRegistry) -> Vec<String> {
    let mut rules: Vec<String> = registry
        .grounded_rules()
        .iter()
        .map(|r| {
            let match_type = match r.match_type {
                crate::grounding::GroundMatch::MustGround => "must-ground",
                crate::grounding::GroundMatch::MustNotGround => "must-not-ground",
            };
            format!("{} ({match_type}, grounded)", r.name)
        })
        .collect();
    rules.extend(
        registry
            .policies()
            .iter()
            .map(|p| format!("{} ({}, structural)", p.rule.name, p.effect)),
    );
    let text_rules = registry.text_rules().len();
    if text_rules > 0 {
        rules.push(format!("{text_rules} governed text rule(s)"));
    }
    rules
}

/// The scope posture line: what happens to an edit outside the ground.
#[must_use]
pub fn posture_line(config: &YupanaConfig) -> String {
    let rung = if config
        .policy
        .work_item_scope
        .is_lower_than(config.policy.mode)
    {
        config.policy.work_item_scope
    } else {
        config.policy.mode
    };
    match rung {
        crate::policy::Mode::Enforce => {
            "work_item_scope = enforce: edits outside the ground above will be DENIED \
             until you update your tracked item or an operator widens your declared scope."
                .to_string()
        }
        crate::policy::Mode::Advise => {
            "work_item_scope = advise: edits outside the ground above will draw an \
             advisory naming this item."
                .to_string()
        }
        crate::policy::Mode::Off => {
            "work_item_scope = off: the ground above is informational only.".to_string()
        }
    }
}

/// Render the briefing as the context block an agent reads.
#[must_use]
pub fn render(brief: &Brief) -> String {
    let mut out = String::new();
    let title = brief.label.as_deref().unwrap_or("(no label in the graph)");
    out.push_str(&format!(
        "## Work-item briefing (yupana)\n\nTracked work item: `{}` — {title}\n",
        brief.item
    ));
    match brief.cache_age {
        Some(u64::MAX) => out.push_str(
            "\nNOTE: quipu could not be projected and no cache was servable — the \
             governed sections below are EMPTY because they could not be read, \
             not because nothing governs you.\n",
        ),
        Some(age) => out.push_str(&format!(
            "\n(projected policy served from cache, {age}s old)\n"
        )),
        None => {}
    }
    if brief.ground.is_empty() {
        out.push_str(
            "\nGround: no observed paths yet — no prior commit implements this \
             item. Your first landed commit starts its ground.\n",
        );
    } else {
        out.push_str("\nGround — paths prior work on this item touched (your observed scope):\n");
        for gp in &brief.ground {
            out.push_str(&format!("- `{}`", gp.path));
            if !gp.symbols.is_empty() {
                out.push_str(&format!(" — defines {}", gp.symbols.join(", ")));
            }
            out.push('\n');
            if !gp.felt_from.is_empty() {
                out.push_str(&format!(
                    "  edits here are felt from: {}\n",
                    gp.felt_from.join(", ")
                ));
            }
        }
    }
    out.push_str(&format!("\nScope posture: {}\n", brief.posture));
    if !brief.central.is_empty() {
        out.push_str(
            "\nCentral entities around your ground (graph pagerank — what this \
             neighborhood hangs off; treat changes to these with care):\n",
        );
        for (entity, score) in &brief.central {
            out.push_str(&format!("- {entity} (score {score:.3})\n"));
        }
    }
    if !brief.similar.is_empty() {
        out.push_str("\nSimilar past work (quipu context pipeline):\n");
        for s in &brief.similar {
            let label = s.label.as_deref().unwrap_or("");
            out.push_str(&format!("- `{}` {label} (score {:.2})", s.id, s.score));
            match s.outcome.as_deref() {
                Some("done") => {
                    out.push_str(
                        " — outcome: done. SUCCESSFUL prior work: study \
                                  and reuse its approach before inventing one.",
                    );
                    if !s.ground.is_empty() {
                        out.push_str(&format!(" It touched: {}.", s.ground.join(", ")));
                    }
                }
                Some(other) => out.push_str(&format!(" — outcome: {other}.")),
                None => out.push_str(" — still open; coordinate, don't duplicate."),
            }
            out.push('\n');
        }
    }
    if !brief.related.is_empty() {
        out.push_str(&format!(
            "\nRelated work items (touched the same entities — coordinate before \
             overlapping): {}\n",
            brief
                .related
                .iter()
                .map(|r| format!("`{r}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !brief.rules.is_empty() {
        out.push_str("\nGoverned rules at the edit boundary:\n");
        for rule in &brief.rules {
            out.push_str(&format!("- {rule}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_names_item_ground_posture_and_rules() {
        let brief = Brief {
            item: "aegis-1".into(),
            label: Some("fix the loom".into()),
            ground: vec![GroundPath {
                path: "src/a.rs".into(),
                symbols: vec!["tally".into()],
                felt_from: vec!["src/b.rs".into()],
            }],
            related: vec!["aegis-2".into()],
            rules: vec!["edit-cites-work-item (must-ground, grounded)".into()],
            posture:
                "work_item_scope = enforce: edits outside the ground above will be DENIED \
                      until you update your tracked item or an operator widens your declared scope."
                    .into(),
            ..Brief::default()
        };
        let text = render(&brief);
        for needle in [
            "aegis-1",
            "fix the loom",
            "src/a.rs",
            "tally",
            "felt from: src/b.rs",
            "aegis-2",
            "edit-cites-work-item",
            "DENIED",
        ] {
            assert!(
                text.contains(needle),
                "briefing missing `{needle}`:\n{text}"
            );
        }
    }

    #[test]
    fn successful_similar_work_is_flagged_for_reuse_with_its_ground() {
        let brief = Brief {
            item: "aegis-1".into(),
            similar: vec![
                SimilarItem {
                    id: "aegis-9".into(),
                    label: Some("weave the warp".into()),
                    outcome: Some("done".into()),
                    score: 0.83,
                    ground: vec!["src/weave.rs".into()],
                },
                SimilarItem {
                    id: "aegis-8".into(),
                    outcome: None,
                    ..SimilarItem::default()
                },
            ],
            central: vec![("http://kg/ent_loom".into(), 0.412)],
            ..Brief::default()
        };
        let text = render(&brief);
        assert!(
            text.contains("SUCCESSFUL prior work"),
            "reuse flag:\n{text}"
        );
        assert!(
            text.contains("It touched: src/weave.rs"),
            "reusable ground:\n{text}"
        );
        assert!(
            text.contains("still open; coordinate"),
            "open items say so:\n{text}"
        );
        assert!(
            text.contains("ent_loom (score 0.412)"),
            "central entity named:\n{text}"
        );
    }

    #[test]
    fn an_unprojectable_store_is_named_not_omitted() {
        let brief = Brief {
            item: "aegis-1".into(),
            cache_age: Some(u64::MAX),
            ..Brief::default()
        };
        let text = render(&brief);
        assert!(
            text.contains("could not be read, not because nothing governs you"),
            "empty-by-failure must be distinguishable from empty:\n{text}"
        );
    }

    #[test]
    fn an_empty_ground_reads_as_a_fresh_start_not_a_wide_scope() {
        let brief = Brief {
            item: "aegis-1".into(),
            ..Brief::default()
        };
        assert!(render(&brief).contains("no observed paths yet"));
    }
}
