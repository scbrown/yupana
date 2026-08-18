//! The SPARQL the projection sends, and the ontology IRIs it names.
//!
//! Split out of [`crate::project`] purely for file size — that module was over
//! this repo's 500-line hard limit before anything in it was touched, and the
//! queries are the part with no logic in them, so moving them changes what a
//! reader has to hold in their head and nothing else.
//!
//! They stay `pub` and re-exported from `project`, so every existing path
//! (`project::TEXT_POLICY_QUERY`) still resolves: this is a move, not an API
//! change.

/// The SPARQL SELECT that pulls every `boundary:"action"`, `tree-sitter`-tier
/// structural policy out of quipu, joined to its Selector and Predicate atoms.
///
/// Only policies that carry BOTH atoms and a selector language are returned — a
/// committed-tier (SPARQL-`claim`-only) policy has no structural evidence to
/// project and is left for quipu's own write gate.
/// `aegis:appliesTo` carries the rule's PATH SCOPE, and it is the one OPTIONAL here
/// that is genuinely multi-valued: a policy scoped to three globs returns three
/// rows, which [`decode_policies`](crate::project::decode_policies) accumulates
/// rather than collapsing. Before it was projected, every quipu-defined structural
/// policy arrived with an empty `applies_to` and therefore applied to EVERY file of
/// its language — so a policy the graph scoped to one module fired repo-wide, and
/// the projection silently enforced something broader than what was declared.
/// Absent still means unscoped, which is the correct reading of a policy that
/// declares no scope.
/// The SARC constraint metadata (quipu `Q-SARC-CLASS`) is pulled as OPTIONAL,
/// deliberately. Requiring `?constraintClass` in the WHERE clause would return
/// ZERO ROWS against any quipu whose catalog has not been backfilled — the
/// both-sides-shipped-and-the-seam-went-silent failure this module's own
/// [`TEXT_POLICY_QUERY`] comment records happening once already. A projected
/// policy that predates the field decodes with `class: None` and falls back to
/// the ambient mode, which is the behaviour yupana had before the field existed.
pub const POLICY_QUERY: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?policy ?name ?language ?query ?pattern ?matchType ?gate ?effect
       ?constraintClass ?verificationPoint ?latencyBudgetMs ?backoffFormula
       ?hostedAtLayer ?appliesTo WHERE {
  ?policy a aegis:Policy ;
          aegis:boundary \"action\" ;
          aegis:selector ?sel ;
          aegis:predicate ?pred .
  ?sel aegis:evidenceSource ?query ;
       aegis:language ?language ;
       aegis:tier \"tree-sitter\" .
  ?pred aegis:evidenceSource ?pattern ;
        aegis:matchType ?matchType .
  OPTIONAL { ?pred aegis:gate ?gate }
  OPTIONAL { ?policy aegis:appliesTo ?appliesTo }
  OPTIONAL { ?policy rdfs:label ?name }
  OPTIONAL { ?policy aegis:effect ?effect }
  OPTIONAL { ?policy aegis:constraintClass ?constraintClass }
  OPTIONAL { ?policy aegis:verificationPoint ?verificationPoint }
  OPTIONAL { ?policy aegis:latencyBudgetMs ?latencyBudgetMs }
  OPTIONAL { ?policy aegis:backoffFormula ?backoffFormula }
  OPTIONAL { ?policy aegis:hostedAtLayer ?hostedAtLayer }
}";

/// The SPARQL SELECT that pulls the governed TEXT-rule catalogue
/// (`aegis:InternalIdentifierPattern`, aegis-mqnl) out of quipu.
///
/// This is the vocabulary the first real governed rule actually shipped in —
/// measured against the live graph, not designed at a whiteboard: per-pattern
/// regex, `enforcementTier` (block|warn), optional `exemptPathRegex`, class and
/// rationale. It is deliberately a SECOND projection query rather than a
/// reshaping of [`POLICY_QUERY`]: a text rule has no Selector (no language, no
/// tree-sitter tier), so forcing it through the structural vocabulary would
/// either invent fake Selector atoms in the graph or silently drop the
/// catalogue — which is exactly what happened: both sides shipped, and the seam
/// returned 0 rows.
///
/// TARGETS THE SUPERTYPE `aegis:TextRule`, NOT `aegis:InternalIdentifierPattern`
/// (aegis-368cu.4). Bound to the concrete class, this catalogue was exactly one
/// class wide: any NEW rule kind — a CI-gate rule, an action rule, a scope rule —
/// projected ZERO ROWS and was SILENTLY ABSENT while the graph listed it and
/// everyone stopped looking. That is the same seam whose scar is described above,
/// which is the reason to fix the shape rather than mint new rules under a name
/// that would lie about what they are.
///
/// ORDERING IS LOAD-BEARING, AND IT IS THE GRAPH FIRST. The subtype edge must
/// exist before a concrete text-rule class can project. Quipu deliberately
/// withholds implicit subclass inference for a plain `a TextRule` query, so the
/// query traverses `rdfs:subClassOf*` explicitly rather than relying on a server
/// inference default. Without either the edge or the explicit traversal, the
/// catalogue silently empties — the failure this comment exists to prevent.
pub const TEXT_POLICY_QUERY: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?s ?label ?regex ?class ?tier ?exempt ?rationale WHERE {
  ?s a/rdfs:subClassOf* aegis:TextRule ;
     aegis:regex ?regex ;
     aegis:enforcementTier ?tier .
  OPTIONAL { ?s rdfs:label ?label }
  OPTIONAL { ?s aegis:identifierClass ?class }
  OPTIONAL { ?s aegis:exemptPathRegex ?exempt }
  OPTIONAL { ?s rdfs:comment ?rationale }
}";

/// The governed policy whose claim decides repo exposure — mqnl's rule #1,
/// live in the graph. The IRI is data about the deployment's ontology, like
/// the `aegis:` prefix in the queries above: one namespace, one policy plane.
pub const EXPOSURE_POLICY_IRI: &str =
    "http://aegis.gastown.local/ontology/policy_no-internal-ids-in-public-repos";

/// The SPARQL SELECT that pulls the entity-grounded predicate catalogue
/// (bobbin-tvn; quipu semantic-grounded-edit-policies Design A). A predicate
/// qualifies by declaring `aegis:candidateSource "token"` — rev 2's no-regex
/// contract: candidates are the tokens of the introduced text, truth is
/// membership against the grounding query's projected id set.
pub const GROUNDED_POLICY_QUERY: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?pred ?name ?label ?matchType ?tier ?rationale WHERE {
  ?pred a aegis:Predicate ;
        aegis:candidateSource \"token\" ;
        aegis:groundingQuery ?groundingQuery ;
        aegis:matchType ?matchType .
  OPTIONAL { ?pred aegis:name ?name }
  OPTIONAL { ?pred rdfs:label ?label }
  OPTIONAL { ?pred aegis:enforcementTier ?tier }
  OPTIONAL { ?pred rdfs:comment ?rationale }
}";

/// The OBSERVED work-item scope map (work-scoped-governance ladder, bottom
/// rung): which paths prior work on each item actually touched, via the
/// deterministic provenance chain quipu already aggregates
/// (`Bead <-aegis:implements- Commit -aegis:modifies-> entity`, quipu
/// shapes/provenance.ttl). Zero rows on a store with no promoted provenance —
/// an empty map, not an error, like every other catalogue here.
pub const WORK_ITEM_SCOPE_QUERY: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>
SELECT ?id ?path WHERE {
  ?c aegis:implements ?w ;
     aegis:modifies ?e .
  ?w aegis:identifier ?id .
  ?e aegis:filePath ?path .
}";

/// The DERIVED rung's source: which work item each item hangs under.
///
/// `aegis:contains` is quipu's existing epic/bead hierarchy (an epic is just a
/// Bead with children — `shapes/provenance.ttl`), so this rung, like the
/// observed one below it, needs no new graph vocabulary.
///
/// WHY A PARENT'S GROUND IS A SCOPE AND NOT A GUESS: a child of an epic whose
/// prior work has all landed in one subtree is, on the balance of evidence,
/// scoped to that subtree. That is an INFERENCE about what work MAY touch,
/// where the observed rung is a RECORD of what it HAS touched — which is
/// exactly why `derived` sits above `observed` in trust order and why it never
/// hard-denies (see `scope_arm`).
pub const WORK_ITEM_PARENT_QUERY: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>
SELECT ?id ?parent WHERE {
  ?p aegis:contains ?w .
  ?w aegis:identifier ?id .
  ?p aegis:identifier ?parent .
}";

/// The grounding query itself — the authoritative work-item id set, projected
/// into the hot plane at refresh time so evaluation is O(1) membership per
/// token, never SPARQL per keystroke.
pub const GROUNDING_SET_QUERY: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>
SELECT ?id WHERE { ?w a aegis:WorkItem ; aegis:identifier ?id }";
