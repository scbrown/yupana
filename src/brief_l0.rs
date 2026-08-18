//! The L0 briefing — what an agent is handed at assignment time, and an honest
//! statement of everything held back.
//!
//! Split from [`crate::brief`] under the file-size ratchet (yupana #83). The
//! seam is the natural one: `brief` owns the shape of what the suite knows,
//! this owns how much of it is spent on the agent's context up front.

use crate::brief::Brief;

/// One withheld section of the briefing: how much there is, and the call that
/// reveals it.
///
/// The census is the whole difference between a briefing that is SMALL and one
/// that is INCOMPLETE. A short briefing with no counts is indistinguishable
/// from a graph that knew nothing — the same ambiguity quipu's `EmbeddingStatus`
/// and `graph_view`'s `truncated` exist to remove, and the same answer: the
/// fact about the answer travels with the answer.
///
/// The `expand` string names a REAL `quipu_ask` rung
/// (`quipu/src/mcp/named_query.rs`), which is what makes it redeemable rather
/// than a suggestion. A census that named a call the catalog does not register
/// would be a dead end the agent finds only by trying it.
struct Section {
    /// What the section is called in the briefing.
    label: &'static str,
    /// How many items it holds. Zero sections are omitted entirely — a heading
    /// with nothing under it implies we looked and found nothing, which is a
    /// claim the briefing has not always earned.
    count: usize,
    /// The `quipu_ask` invocation that returns it.
    expand: String,
}

/// The sections L0 withholds, in the order the briefing lists them.
fn sections(brief: &Brief) -> Vec<Section> {
    let item = &brief.item;
    vec![
        Section {
            label: "ground paths with their symbols and callers",
            count: brief.ground.len(),
            expand: format!("quipu_ask brief_ground {{item: \"{item}\"}}"),
        },
        Section {
            label: "similar past work items",
            count: brief.similar.len(),
            expand: format!("quipu_context {{query: \"<this item's subject>\"}}"),
        },
        Section {
            label: "central entities around the ground",
            count: brief.central.len(),
            expand: format!("quipu_project {{algorithm: \"ppr\", seeds: [<ground entities>]}}"),
        },
        Section {
            label: "related in-flight work items",
            count: brief.related.len(),
            expand: format!("quipu_ask brief_related {{item: \"{item}\"}}"),
        },
        Section {
            label: "governed rules at the edit boundary",
            count: brief.rules.len(),
            expand: "yupana status rules".to_string(),
        },
    ]
}

/// Render the L0 briefing: what the agent needs before its first edit, plus an
/// honest statement of everything held back and how to ask for it.
///
/// [`render`] prints every section in full. That is the right shape for an
/// operator reading a briefing by hand and the wrong shape for a session-start
/// injection, which pays for every line in the agent's context whether or not
/// it is read — and which, for an item with a wide ground, is most of a page
/// before the agent has done anything.
///
/// L0 keeps what is cheap and load-bearing — the item, its ground PATHS, the
/// scope posture — and replaces each remaining section with one line naming its
/// size and its expansion. The three honesty behaviours [`render`] holds are
/// held here too, verbatim, because they are the difference between "small" and
/// "quietly missing": an unprojectable store says the sections are empty
/// because they could not be READ, an empty ground reads as a fresh start
/// rather than an unbounded scope, and a cache-served projection states its age.
#[must_use]
pub fn render_l0(brief: &Brief) -> String {
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
            out.push_str(&format!("- `{}`\n", gp.path));
        }
    }
    out.push_str(&format!("\nScope posture: {}\n", brief.posture));

    let held: Vec<Section> = sections(brief)
        .into_iter()
        .filter(|s| s.count > 0)
        .collect();
    if held.is_empty() {
        return out;
    }
    out.push_str(
        "\nHeld back — the suite knows more about this item than is printed above. \
         Ask for a section when you need it; the counts are exact:\n",
    );
    for s in &held {
        out.push_str(&format!("- {} {} — `{}`\n", s.count, s.label, s.expand));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brief::{GroundPath, SimilarItem};

    fn wide_brief() -> Brief {
        Brief {
            item: "aegis-1".into(),
            label: Some("fix the loom".into()),
            ground: vec![GroundPath {
                path: "src/a.rs".into(),
                symbols: vec!["tally".into()],
                felt_from: vec!["src/b.rs".into()],
            }],
            related: vec!["aegis-2".into()],
            similar: vec![SimilarItem {
                id: "aegis-9".into(),
                ..SimilarItem::default()
            }],
            central: vec![("http://kg/ent_loom".into(), 0.41)],
            rules: vec!["edit-cites-work-item (must-ground, grounded)".into()],
            posture: "work_item_scope = advise: edits outside the ground above will draw an \
                      advisory naming this item."
                .into(),
            cache_age: None,
        }
    }

    /// L0 keeps what the agent needs before its first edit — the item, its
    /// ground paths, the posture — and spends nothing on the rest.
    #[test]
    fn l0_keeps_the_ground_and_the_posture() {
        let text = render_l0(&wide_brief());
        for needle in [
            "aegis-1",
            "fix the loom",
            "src/a.rs",
            "advisory naming this item",
        ] {
            assert!(text.contains(needle), "L0 dropped `{needle}`:\n{text}");
        }
    }

    /// RED, and the point of the whole thing. A section L0 does not print must
    /// be COUNTED and its expansion NAMED. A short briefing with no census is
    /// indistinguishable from a graph that knew nothing.
    #[test]
    fn every_withheld_section_is_counted_and_names_its_expansion() {
        let text = render_l0(&wide_brief());
        assert!(text.contains("Held back"), "no census at all:\n{text}");
        for needle in [
            "1 similar past work items",
            "1 central entities around the ground",
            "1 related in-flight work items",
            "1 governed rules at the edit boundary",
            "quipu_ask brief_related",
        ] {
            assert!(text.contains(needle), "census missing `{needle}`:\n{text}");
        }
    }

    /// GREEN, and the control the test above needs. With nothing held back
    /// there must be NO census — a "held back: 0" line would train an agent to
    /// skip the section on the occasions it matters.
    #[test]
    fn a_briefing_with_nothing_held_back_prints_no_census() {
        let brief = Brief {
            item: "aegis-1".into(),
            posture: "work_item_scope = off".into(),
            ..Brief::default()
        };
        let text = render_l0(&brief);
        assert!(
            !text.contains("Held back"),
            "nothing was withheld, so nothing should claim to be:\n{text}"
        );
    }

    /// The three honesty behaviours the full render holds must survive the cut,
    /// because they are exactly the difference between "small" and "quietly
    /// missing".
    #[test]
    fn an_unprojectable_store_is_still_named_not_omitted() {
        let brief = Brief {
            item: "aegis-1".into(),
            cache_age: Some(u64::MAX),
            ..Brief::default()
        };
        assert!(render_l0(&brief).contains("could not be read, not because nothing governs you"));
    }

    #[test]
    fn an_empty_ground_still_reads_as_a_fresh_start_not_a_wide_scope() {
        let brief = Brief {
            item: "aegis-1".into(),
            ..Brief::default()
        };
        assert!(render_l0(&brief).contains("no observed paths yet"));
    }

    #[test]
    fn a_cache_served_projection_still_states_its_age() {
        let brief = Brief {
            item: "aegis-1".into(),
            cache_age: Some(42),
            ..Brief::default()
        };
        assert!(render_l0(&brief).contains("cache, 42s old"));
    }

    /// L0 must actually be SMALLER than the full render, or the split bought
    /// nothing and the census is pure addition.
    #[test]
    fn l0_is_smaller_than_the_full_render() {
        let brief = wide_brief();
        let full = crate::brief::render(&brief).lines().count();
        let l0 = render_l0(&brief).lines().count();
        assert!(
            l0 < full,
            "L0 ({l0} lines) is not smaller than full ({full})"
        );
    }
}
