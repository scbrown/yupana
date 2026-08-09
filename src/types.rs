//! The typed fact model Yupana serves.
//!
//! Every served fact carries a [`Tier`] (how it was derived) — the confidence
//! tag FR-3 requires so a consumer never mistakes a tree-sitter approximation
//! for an LSP-precise fact.
//!
//! [`Freshness`] (how current a fact is) is FR-3's OTHER half, and it is **not
//! yet served**. Freshness state (fresh/stale/recomputing) is a property of the
//! Phase-3 resident graph + file-watcher (FR-16/17); the current serve path
//! rebuilds the graph on demand per request from the working copy, where there
//! is no cached fact that could be stale. So [`Freshness`] and [`Fact`] (the
//! subject–edge–object carrier that pairs a value with both tags) are defined
//! here as the Phase-3 carrier but have no caller on the served path yet.
//!
//! This docstring previously asserted that every served fact carried a freshness
//! tag. It did not — freshness was served nowhere (aegis-8yrn). Correcting the
//! claim, rather than stamping a hardcoded `fresh` that would imply a tracking
//! system that does not exist, is the FR-3 honesty rule applied to the spec
//! itself: a fact that cannot state its own freshness says so, it does not
//! default to looking current.

use serde::{Deserialize, Serialize};

/// Provenance/precision of a fact — which extractor produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Fast, build-free, approximate (tree-sitter).
    TreeSitter,
    /// Precise defs/refs/types where a build resolves (LSP).
    Lsp,
    /// Control/data dependence from the code property graph.
    Cpg,
    /// A fact an engine ADAPTER stated, not one Yupana derived from source
    /// (FR-35). Its provenance is an adapter id + turn + faction, never a
    /// `file:line` — see [`crate::state`].
    ///
    /// It is a peer of the code tiers, not a rung above or below them: a board
    /// fact and a code fact must be equally impossible to mistake for each
    /// other, in BOTH directions. Nothing here is span-anchored, so a consumer
    /// that reads an `engine-state` fact as if it pointed at source is wrong in
    /// the same way FR-3 exists to prevent.
    ///
    /// Renamed explicitly: the enum's `snake_case` rule would serialize this as
    /// `engine_state`, and the addendum fixes the spelling as `engine-state`.
    #[serde(rename = "engine-state")]
    EngineState,
}

/// How current a served fact is relative to the tenant's working copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    /// Reflects the latest observed edit.
    Fresh,
    /// Known to be behind a pending edit.
    Stale,
    /// A recompute is in flight.
    Recomputing,
}

impl Freshness {
    /// The lowercase string form used on the wire and in the trace record.
    ///
    /// `Recomputing` renders as itself here rather than collapsing to `stale`:
    /// a trace record is diagnostic and the distinction is real. The VERDICT
    /// path collapses it (see `crate::verdict`), because `aegis:freshness`
    /// admits only fresh/stale and the conservative reading is the only one
    /// that cannot overstate — two different audiences, two different mappings,
    /// each stated where it applies.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Freshness::Fresh => "fresh",
            Freshness::Stale => "stale",
            Freshness::Recomputing => "recomputing",
        }
    }
}

impl Tier {
    /// The lowercase string form used on the wire and in the ontology.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::TreeSitter => "treesitter",
            Tier::Lsp => "lsp",
            Tier::Cpg => "cpg",
            Tier::EngineState => "engine-state",
        }
    }

    /// The tiers this build can ACTUALLY serve — the single source of truth for
    /// `yupana status` / `yupana_status` (aegis-qe5z).
    ///
    /// It reports a tier only when that tier has a registered extractor. Today
    /// that is tree-sitter ALONE: the LSP tier (FR-2) and the CPG tier (FR-7) are
    /// unimplemented, so they are NOT advertised. `status` used to push `lsp`/`cpg`
    /// on `cfg!(feature = "lsp"/"cpg")`, but those were EMPTY Cargo features that
    /// gated no code — so `--features lsp` produced a binary that advertised a
    /// precision tier while every fact it served was still `TreeSitter`. That is
    /// the exact "present an approximation as precise" failure FR-3 and AGENTS.md
    /// forbid, one level up at the feature flag (sibling of aegis-8yrn).
    ///
    /// When a tier gains a real implementation, add it HERE, gated on that
    /// implementation (a `Vec` push behind the module that provides it) — never on
    /// a bare feature flag, which can be enabled without the code existing.
    ///
    /// `engine-state` (FR-35) obeys that rule rather than bending it: the
    /// `game-state` feature gates [`crate::state`], which is the ingestion
    /// engine itself, so the flag and the implementation are the same thing
    /// here — unlike the removed `lsp`/`cpg` flags, which gated nothing. A build
    /// without `game-state` has no `/ingest` to state a fact through and so does
    /// not advertise the tier.
    #[must_use]
    pub fn served() -> Vec<String> {
        // `mut` is used only on the `game-state` arm; without the allow, the
        // default build fails `-D warnings` on an unused `mut`.
        #[allow(unused_mut)]
        let mut tiers = vec![Tier::TreeSitter.as_str().to_string()];
        #[cfg(feature = "game-state")]
        tiers.push(Tier::EngineState.as_str().to_string());
        tiers
    }
}

/// The kind of a named code symbol. Values mirror the enumeration in Quipu's
/// `shapes/code-entities.ttl` (`bobbin:symbolKind`) so promoted facts validate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    /// A free function.
    Function,
    /// A method associated with a type.
    Method,
    /// A class.
    Class,
    /// An interface / trait.
    Interface,
    /// An enum.
    Enum,
    /// A struct.
    Struct,
    /// A variable binding.
    Variable,
    /// A constant.
    Constant,
    /// A module.
    Module,
    /// A property.
    Property,
    /// A field.
    Field,
    /// A constructor.
    Constructor,
    /// A type alias.
    TypeAlias,
}

impl SymbolKind {
    /// The lowercase string form used on the wire and in the ontology
    /// (`bobbin:symbolKind`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Enum => "enum",
            SymbolKind::Struct => "struct",
            SymbolKind::Variable => "variable",
            SymbolKind::Constant => "constant",
            SymbolKind::Module => "module",
            SymbolKind::Property => "property",
            SymbolKind::Field => "field",
            SymbolKind::Constructor => "constructor",
            SymbolKind::TypeAlias => "type_alias",
        }
    }
}

/// A structural edge between two symbols or modules. These become predicates in
/// the `bobbin:` code ontology on promotion (see `docs/yupana-spec.md` §9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Caller invokes callee.
    Calls,
    /// A use site of a definition.
    References,
    /// A symbol is defined in a module.
    DefinedIn,
    /// A module depends on another module.
    Imports,
    /// A data-dependence edge (CPG).
    DataDependsOn,
    /// A control-dependence edge (CPG).
    ControlDependsOn,
}

/// A named symbol extracted from a source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    /// The symbol's name.
    pub name: String,
    /// The chain of named enclosing scopes (module, impl, trait, class,
    /// function — per language), outermost first; empty for a top-level symbol.
    /// This is what makes the promoted IRI unique when one file defines two
    /// same-named symbols in different scopes (aegis-1q14: 42 of 43 measured
    /// collisions silently merged into one node before this existed).
    #[serde(default)]
    pub scope: Vec<String>,
    /// What kind of symbol it is.
    pub kind: SymbolKind,
    /// 1-based line where the symbol begins.
    pub start_line: usize,
    /// 1-based line where the symbol ends.
    pub end_line: usize,
    /// How this symbol was derived.
    pub tier: Tier,
}

/// A single structural fact: `subject —edge→ object`, tagged with provenance.
///
/// The Phase-3 freshness carrier (see the module docs): it pairs a value with
/// both a [`Tier`] and a [`Freshness`], but nothing on the served path constructs
/// one yet — freshness lands with the resident graph + watcher (FR-16/17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    /// The subject identifier.
    pub subject: String,
    /// The relationship.
    pub edge: EdgeKind,
    /// The object identifier.
    pub object: String,
    /// How this fact was derived.
    pub tier: Tier,
    /// How current the fact is.
    pub freshness: Freshness,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn served_tiers_are_only_implemented_ones() {
        // Status must advertise a tier only when it is real. The
        // extractor assigns TreeSitter, so that is always claimed — never
        // `lsp`/`cpg`, which have no implementation. A push of an unimplemented
        // tier here (or a re-introduced empty feature) fails this.
        assert!(Tier::served().contains(&"treesitter".to_string()));
        assert!(!Tier::served().contains(&"lsp".to_string()));
        assert!(!Tier::served().contains(&"cpg".to_string()));
    }

    /// The `engine-state` tier is advertised EXACTLY when the engine that can
    /// produce one is compiled in — asserted in both directions, because the
    /// failure this guards is symmetric. Advertising it without `game-state`
    /// repeats the empty-feature lie; omitting it WITH `game-state` sends a
    /// consumer looking for a tier the build actually serves.
    #[test]
    fn engine_state_is_advertised_exactly_when_its_engine_is_built() {
        let advertised = Tier::served().contains(&"engine-state".to_string());
        assert_eq!(advertised, cfg!(feature = "game-state"));
    }

    /// A board fact and a code fact must never render alike (FR-3, FR-35): the
    /// wire strings are what a consumer discriminates on, so a collision here
    /// would make the two indistinguishable downstream.
    #[test]
    fn every_tier_has_a_distinct_wire_string() {
        let all = [Tier::TreeSitter, Tier::Lsp, Tier::Cpg, Tier::EngineState];
        let distinct: std::collections::BTreeSet<&str> = all.iter().map(|t| t.as_str()).collect();
        assert_eq!(distinct.len(), all.len());
        assert_eq!(Tier::EngineState.as_str(), "engine-state");
    }

    /// The tier round-trips through serde under the name the addendum fixes.
    /// `rename_all = "snake_case"` would render this variant `engine_state`, so
    /// the explicit rename is load-bearing and a silent change to it would move
    /// the wire contract without touching any prose.
    #[test]
    fn engine_state_serializes_as_the_addendum_spells_it() {
        let json = serde_json::to_string(&Tier::EngineState).unwrap();
        assert_eq!(json, "\"engine-state\"");
        let back: Tier = serde_json::from_str("\"engine-state\"").unwrap();
        assert_eq!(back, Tier::EngineState);
    }
}
