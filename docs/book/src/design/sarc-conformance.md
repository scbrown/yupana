# SARC Conformance — what yupana × quipu still needs

Status: **Phases 1–6 landed.** Constraint metadata
and placement; the Σ-derived trace record and signed verdict emission; a real
Post-Action Auditor with `throttle`; an Escalation Router with a bounded
reversibility window; the `T ⊨ Σ` checker, the dispatch-graph inventory and the
replay harness; authority intersection over named graphs and the attribution
tuple.

**Read the "as built" sections, not this line.** Each ends with what its phase
did *not* close, and four of those matter: there is **no Action-Time Monitor at
all** (G6, untouched); the escalation queue has no server, so §5.3's
`W_q < τ_rev` is unmeasured; θ is calibratable but not calibrated, because replay
counts blocks and cannot label false positives; and no trust predicate evaluates
imported content — the boundary is declared and reported, not closed. See [Build
order](#build-order) for the phases as originally scoped.

## Why this document

Besanson [\[SARC\]](#sources) proposes that constraints become a first-class
specification object alongside state, action space and reward (§3.1):

```text
c = ⟨src, class, pred, verif, resp⟩  + a declared operating point θ
```

compiled into four named enforcement points in the agent loop (§4.1) — a
**Pre-Action Gate** (PAG), an **Action-Time Monitor** (ATM), a **Post-Action
Auditor** (PAA), and an **Escalation Router** (ER) — under eight runtime
invariants I1–I8 (§3.5) whose joint effect is *specification-trace
correspondence* (Definition 2, §3.6): given a specification Σ and a trace T, an
auditor can mechanically decide `T ⊨ Σ` in `O(|T|·|C|)` without access to the
model, its prompts, or its developers.

The stack is already most of the way there, and on the *specification* side it
is ahead of the paper's own prototype: quipu's `aegis:` governance ontology is
SHACL-validated and bitemporal, and verdicts are ed25519-signed against a
human-owned `VerifierRegistration` root of trust — SARC's reference artifact is
a JSON spec file and a Python checker (§3.6, §13.4).

What is missing is not the substrate. It is a specific, enumerable set of holes:
the constraint objects are under-declared, three of the four enforcement points
are absent or advisory-only, the trace is not derived from the specification,
and nothing checks correspondence. This document names them and orders the work.

## Where the stack stands

### Already in place

| SARC concept | Stack today | Where |
|---|---|---|
| Constraint specification object | `aegis:Policy` + `aegis:Selector` / `aegis:Predicate` atoms | `quipu/shapes/governance.ttl` |
| `pred` | `aegis:claim` (SPARQL ASK), or selector `.scm` + predicate regex | `quipu/shapes/policies/treesitter.ttl` |
| Pre-Action Gate | `yupana hook pre-edit`, `Mode::{Off,Advise,Enforce}` | `src/hook/pre_edit.rs`, `src/policy.rs` |
| Policy-layer reference monitor | quipu pre-commit write gate | `quipu/src/governance/guard.rs` |
| Verdict as attestation, not claim | ed25519-signed, evidence-hash-bound `aegis:Verdict` | `src/verdict.rs`, `quipu/src/signing.rs` |
| Root of trust | `aegis:VerifierRegistration`, human-authored | `quipu/shapes/governance.ttl` |
| Latency budget ([SARC] §5.1) | `policy.deadline_ms`, fail-open on expiry | `src/policy.rs` |
| One-directional policy projection | quipu canonical → yupana read cache | `src/project.rs`, `src/hook/rule_planes.rs` |
| Confidence inputs | `tier ∈ {live,lsp,tree-sitter,committed,attested}` + `freshness` | shapes, FR-3 |
| Layer discipline (SARC I6) | no rule lives in the prompt; declared, not yet verified against reality | [Governance Plane](governance-plane.md) |
| Post-Action Auditor | `yupana hook post-edit`, soft class + `throttle` | `src/hook/paa.rs`, `src/throttle.rs` |
| Constraint class + placement | `aegis:constraintClass` / `verificationPoint`, checked at write | `quipu/src/governance/placement.rs` |
| Σ-derived trace record | `constraints[]` with outcome, response and placement | `src/trace.rs` |
| Signed verdicts, both sides | yupana spools at the gate and the PAA; quipu persists the write gate's | `src/verdict_spool.rs`, `quipu/src/governance/verdict_facts.rs` |

[Governance Plane](governance-plane.md) independently anticipates much of SARC —
risk × confidence adaptive effect, verdict integrity, the out-of-band verifier,
the `prevented`/`observed` enforcement gradient. SARC's marginal contribution on
top of it is **placement discipline** ([SARC] §4.2, Table 3: which constraint
class belongs at which enforcement point) and **checkable correspondence**
([SARC] §3.6: a decidable audit). SARC is explicit that this is a *specification
discipline* layered over a policy-as-code substrate rather than a replacement
for one (§2.1) — which is exactly the relationship quipu's write gate already
has to yupana's projection.

### The gaps

**G1 — Constraint objects are incomplete.** Violates I2 ([SARC] §3.5: "a
constraint missing any field is not a constraint; it is a comment").

`aegis:Policy` carries `targets`, `claim`, `boundary ∈ {action,transition}` and
`effect ∈ {allow,warn,require-approval,deny,escalate,record}`. It does not carry:

- **`class ∈ {hard, soft, escalation}`.** `effect` conflates class with
  response, so "what kind of constraint is this" is not declarable and the
  class→placement rules ([SARC] Table 3) cannot be checked.
- **an operating point θ** — no false-positive / false-negative tolerance.
- **a reversibility window τ_rev** and on-timeout behaviour (needed by I4).
- **a per-constraint latency budget** at its verification point.
- **a hosting layer** (`orchestration|tool|policy`), needed to check I6.
- **`src.type`** — `aegis:Directive` supplies optional `authority`/`issuedBy`,
  and the shipped catalog sets neither.

**G2 — The verdict path is built but not wired.** Violates I3 and I8 ([SARC]
§3.5). **Closed in Phase 2**, both halves.

`src/verdict.rs` implemented signing and `promote_verdict` correctly — mirroring
quipu's scheme so a yupana-signed verdict verifies under quipu's root of trust —
and had **no caller** outside `yupana verdict-key`. A pre-edit guard decision, the
exact moment a constraint fires, never became a governed fact. Symmetrically,
quipu's own write-gate decision was not persisted (`Q-VERDICT-PERSIST`).

Both now record. yupana signs at the gate and at the PAA and spools locally,
drained by `yupana verdicts`; quipu stages its write-gate verdicts and flushes
them *after* the savepoint resolves, so a denial's verdict survives the rollback
that denial caused.

**G3 — The trace is not derived from Σ.** Violates I3 ([SARC] §3.5: "the trace
is generated; it is not reconstructed"), and I8 by consequence. **Closed for the
constraint set in Phase 2**; the attribution half stays open under G7. See
[Phase 2, as built](#phase-2-as-built).

`src/metrics.rs` emitted `{kind, ts, agent, tenant, item, …}` per event. There
was no pre/post state, no `constraints_evaluated` set with outcomes, no
attribution tuple. `docs/work-scoped-governance.md` names this precisely — "records and
rules share one vocabulary" — and phases it. Its phase 1 *is* SARC's I3. It is
designed, not built.

**G4 — The Post-Action Auditor is advisory context, not a constraint site.**
**Closed in Phase 3.**

`src/hook/post_edit.rs` injects blast-radius context after an edit. It evaluates
no constraint, emits no verdict, and cannot prevent the *next* action — which is
what a PAA is for ([SARC] §4.1). SARC's soft class `C_s` has nowhere to live,
and `throttle` — the declared PAA response responsible for the paper's entire
89.5% soft-overage reduction ([SARC] §10.3, Table 6) — is not in quipu's
`effect` enum.

**G5 — There is no Escalation Router.** Violates I4 ([SARC] §3.5, §5.3).

`require-approval` and `escalate` currently fail closed at the quipu write gate
*with no channel to grant approval* (`guard.rs::effect_blocks`, and the design
doc says so plainly). `aegis:Decision` and `aegis:assignsWorkflow` shapes exist;
there is no runtime router, no operator group, no queue, no τ_rev, no
default-deny-on-timeout, no capacity model. [SARC] I4: "escalation without a
bound is not human oversight; it is deferred autonomy." The queueing model that
makes `W_q < τ_rev` measurable rather than asserted is §5.3.

**G6 — There is no Action-Time Monitor.**

Nothing observes an action mid-flight. Long-running Bash, MCP tool calls and
sub-agent runs have no cumulative-budget monitor and no interrupt.
`src/hook/pre_bash.rs` is deliberately record-only and prints nothing.

**G7 — No attribution tuple, no authority intersection.** Violates I5 ([SARC]
§9.3, §9.6).

SARC's `α = ⟨P, planner, executor, tool, auth, C_eval⟩` has no counterpart. The
spool carries one flat `agent` + `tenant` + `item`. There is no
principal-and-agent chain, no authority composition (`all-of` / `any-of`), no
monotonic narrowing under delegation — and the trace is a **sequence, not a
tree**, so orchestrator/worker runs are exposed to exactly the
constraint-laundering and attribution-dilution failure modes of [SARC] §9.5.
quipu's `group_id` is documented as provenance-only, and
[Governance Plane](governance-plane.md) scopes v1 to a single trust domain. This is the deepest gap and the one with a
real prerequisite.

> **Correction.** The "real prerequisite" was described as quipu multi-tenancy,
> and that was wrong: named graphs are already a storage-enforced isolation
> substrate, and the actual gap was authorization over them. Mostly closed — see
> [Phase 6, as built](#phase-6-as-built) for what landed and what is still open
> (the trace is still a sequence, not a tree).

**G8 — Enforcement completeness is unmeasured.** Violates I7 ([SARC] §3.5).

`docs/work-scoped-governance.md` §"What this cannot reach" is an honest,
explicit list of bypass surfaces: CI pipelines, cron, the far side of a remote
shell, a sibling session's VCS index, a hostile agent. I7 is a property of the
*dispatch graph*, verified by inspection against Σ — and there is no inventory
of governed vs ungoverned tool-call classes, so "which actions traverse an
enforcement point" is not answerable mechanically.

**G9 — The `aegis:` vocabulary is not described, only constrained.**

Found while building Phase 1, not in the original analysis. `shapes/*.ttl`
contained **zero** `rdfs:domain`, `rdfs:range`, `rdf:Property` or `owl:*Property`
declarations: every `aegis:` term was defined implicitly, by the SHACL shapes
that constrain it. A shape says what is *valid*; it does not say what a term
*means* or what it *relates*. So a reader could learn that `constraintClass` is
one of three strings and could not learn how it differs from `effect`.

That matters for two consumers the stack already plans on. The authoring surface
in [Governance Plane](governance-plane.md) §Authoring is composition over a
catalog — an agent drafting a policy has nothing to read. And aligning `aegis:`
against an external governance vocabulary (the ontology-matching problem
[Agent-OM] addresses) needs property descriptions, which is precisely what was
missing.

It also starved machinery already built: `src/owl.rs` parses `rdfs:domain` /
`rdfs:range` / `rdfs:subPropertyOf`, and the reasoner materialises domain/range
inference — fed nothing. Partly closed in Phase 1 by
`shapes/aegis-properties.ttl` for the SARC fields; the rest of the vocabulary is
`Q-SARC-VOCAB`.

**G10 — No audit checker, no replay, no calibration.**

Nothing computes `T ⊨ Σ`. There is no spool reader, no replay harness, no
measured false-positive rate — so the advise→enforce promotion ladder in
`work-scoped-governance.md` §Evals cannot be walked, and θ (G1) would be
undeclarable-in-practice even once the field exists.

### Position on the adoption ladder ([SARC] §13.3)

As first written, and where each rung now stands:

- **Level 1** (PAG, hard constraints at the tool/policy layer, structured trace
  emission) — was "substantially met; I3 and I8 outstanding". **I3 met** by the
  Σ-derived trace record; I8 waits on the checker (Phase 5).
- **Level 2** (PAA, soft constraints with calibrated operating points) — was "not
  started". **PAA and `throttle` built**, and the replay harness now measures the
  promotion gates. But θ is calibratable rather than calibrated: replay counts
  blocks and cannot label false positives, and bounds no false negatives at all.
  A declared θ that no measurement backs is a number, not a calibration, and the
  rung should not be claimed on one.
- **Level 3** (ATM, ER with declared τ_rev) — **ER built** with a declared window
  and default-deny past it. **No ATM**: nothing observes an action mid-flight
  (G6), and the escalation queue has no server, so §5.3's `W_q < τ_rev` is
  unmeasured. This rung is half met.
- **Level 4** (multi-agent) — was "blocked on quipu multi-tenancy", which
  misdescribed the blocker. Authority intersection, the attribution tuple, the
  reconstructed dispatch tree, constraint inheritance with a decidability rescue,
  and trust-boundary declarations are all built. Two things keep this rung from
  being claimed outright: the trace is emitted as a sequence, so sibling
  dispatches of the same principal are indistinguishable, and nothing evaluates
  imported content.

## Decisions

- **Vocabulary**: extend `aegis:` in place in `quipu/shapes/governance.ttl`
  rather than layering a separate `sarc:` overlay. One vocabulary; the existing
  SHACL validation and projection decode carry the new fields for free. Cost:
  the shipped `treesitter.ttl` catalog needs backfilling in the same change.
- **Escalation Router owner**: **quipu**. The engine of record already models
  `Decision`, `assignsWorkflow` and the bitemporal audit trail, and
  `require-approval` already fails closed there. Yupana gets a thin client. This
  matches the settled "the engine lives in Quipu; yupana never originates policy"
  rule.
- **MVP scope**: Phases 1–3 — SARC-conformant for a single agent in a single
  trust domain, which is exactly the v1 boundary
  [Governance Plane](governance-plane.md) already declared.

## Build order

### Phase 1 — Complete the constraint object

*Closes G1. Prerequisite for everything else: you cannot place a constraint by
class before class exists.*

**quipu — `shapes/governance.ttl`:**

- `aegis:constraintClass`, `sh:in ("hard" "soft" "escalation")`, required on
  `boundary "action"` policies.
- `aegis:verificationPoint`, `sh:in ("PAG" "ATM" "PAA" "tool_layer" "policy_layer")`.
  This replaces nothing — `boundary` stays as the coarse action/transition
  split; `verificationPoint` is the fine placement SARC needs.
  `tool_layer`/`policy_layer` already appear in yupana's projected policies.
- `aegis:hostedAtLayer`, `sh:in ("orchestration" "tool" "policy")`. Deliberately
  **no `"prompt"` value**. See [Phase 1, as built](#phase-1-as-built) for why the
  omission is defence in depth rather than the guarantee it first looked like.
- `aegis:OperatingPoint` node shape + `aegis:operatingPoint` on `Policy`:
  `falsePositiveTolerance`, `falseNegativeTolerance`, `threshold`,
  `calibrationBasis`.
- `aegis:reversibilityWindowSeconds` and `aegis:onTimeout` — the latter
  `sh:in ("deny")`, one value only, and re-checked by value in `placement.rs`
  because the shape alone does not bind on every write path. Required on
  escalation-class policies.
- `aegis:latencyBudgetMs` on `Policy`.
- `"throttle"` added to the `aegis:effect` enum, plus `aegis:backoffFormula`.
- `aegis:sourceType`, `sh:in ("regulatory" "contractual" "ethical" "operational")`,
  and `aegis:authority` required on action-boundary policies.

**quipu — `src/governance/placement.rs` (new):** a class↔placement conformance
pass, run at definition time alongside SHACL. `hard ⇒ verificationPoint ∈ {PAG,
ATM, tool_layer, policy_layer}`; `soft ⇒ {ATM, PAA}`; `escalation ⇒ {PAG, PAA}`
and must declare τ_rev. This is [SARC] Table 3 made mechanical, and it is what
"placement discipline" means in practice. Backfill `shapes/policies/treesitter.ttl`
(`no-ticket-in-comment` → hard/PAG, `todo-needs-ticket` → soft/PAA).

**yupana:**

- `project_queries::POLICY_QUERY` gains `?constraintClass ?verificationPoint
  ?latencyBudgetMs ?fpTolerance ?fnTolerance ?reversibilityWindowSeconds` as
  **OPTIONAL**s. Not required — a projection that hard-required a field quipu
  had not yet backfilled would return zero rows, which is the exact
  both-sides-shipped-and-the-seam-returned-nothing failure `project_queries.rs`
  already documents in its own comments.
- `project::ProjectedPolicy` gains those fields alongside `effect`;
  `decode_policies` reads them through the existing `required`/`optional`
  closures. An unrecognised `constraintClass` is an `Error::Projection`,
  matching how an unknown `matchType` is handled — never a silent drop.
- `rules::Rule` gains `class` and `verification_point`, so a locally-configured
  `[[yupana.policy.rules]]` and a projected policy stay one type.
- `hook/rule_planes.rs::governed_check` selects its response by declared class
  (hard ⇒ block under `Enforce`; soft ⇒ never block; escalation ⇒ route) rather
  than by `project::effect_blocks` alone. `Mode::Advise` keeps its ceiling: it
  never blocks, whatever the class.

### Phase 1, as built

Shipped in quipu (`shapes/governance.ttl`, `src/governance/placement.rs`) and
yupana (`src/constraint.rs`, `src/project_decode.rs`,
`src/hook/rule_planes.rs`). Three things came out differently from the plan
above, each because building it surfaced something the analysis had not:

**Two fields are refused on the write path, and omitted from the vocabulary as
defence in depth.** `aegis:hostedAtLayer` has no `"prompt"` value and
`aegis:onTimeout` has only `"deny"`.

This was first described here as making the unsafe settings *unrepresentable*,
which was an overstatement worth correcting rather than quietly fixing. A
`sh:in` enum only binds when SHACL runs, and SHACL runs under
`shacl.validate_on_write` — which **defaults to false** and validates episode
ingest rather than `Store::transact` generally. A policy written through `/knot`
or a direct transact could carry `onTimeout "allow"` and nothing would object.
"The shape rejects it" and "the store cannot hold it" are different claims, and
shipping the second while only the first was true is the exact failure this
document catalogues elsewhere.

Both values are therefore re-checked in `placement.rs`, on the path that is
actually on — before the action-boundary exemption, since a bad `onTimeout`
fails just as silently on a transition-boundary policy. The vocabulary omission
sits behind that as a second layer, not above it as a guarantee. Even so, the
honest ceiling is **"refused on quipu's write path"**: a raw SQL write, or a
process that opens the store directly, bypasses every check quipu has.

`hostedAtLayer` was **declared but otherwise unconsumed** at this point — I6 was
checked for *well-formedness* and not for *truth*, so a policy could claim `tool`
while being enforced only in yupana's orchestration-layer hook. [Spec
B](#spec-b--h-sarc-i6-check-the-hosting-layer-against-reality) is now built; see
[H-SARC-I6, as built](#h-sarc-i6-as-built).

**Multi-valued fields are refused, not resolved.** Asserting
`constraintClass "hard"` over an existing `"soft"` leaves *both* facts active —
assertion is not replacement. The first implementation read the last SPARQL row
and silently picked one, so a re-class would have landed while the old placement
still validated. This was caught by the write-path test, not by the unit tests
over the rule table, which is the argument for having both. A policy with two
classes is now refused as ambiguous, with the retract-in-the-same-transaction
remedy in the message, and `a_clean_re_placement_retracting_the_old_value_lands`
is the recoverability half — refusing ambiguity is only safe if there is a way
to legitimately move a policy.

**The projection decoder got the same collapse the text decoder already had.**
`POLICY_QUERY` gained three OPTIONALs, and SPARQL returns the cross product of
them: a policy carrying two `rdfs:label`s comes back as two rows and became two
identical rules. `decode_text_rules` already carried a comment recording this
exact failure on the live catalogue — 7 entities projecting as 11 rules, 4
duplicates, each reported twice to the model with conflicting rationales.
`decode_policies` was exposed to it the whole time and the new fields made it
likelier, so it now collapses on the policy IRI and refuses rows that disagree
on a required field. Identity is the IRI, not the label: an unlabelled policy
falls back to a row-indexed name, so keying on the name would give every row a
distinct identity and collapse nothing.

**The vocabulary had no property declarations at all.** Not a SARC gap — a gap
SARC's fields made visible. `shapes/aegis-properties.ttl` now declares the SARC
properties with `rdfs:domain` / `rdfs:range` / `rdfs:comment`, and a test checks
the two files against each other so a field added to the shape without a
description fails rather than drifting. Domains are declared **one property at a
time**, not swept in: `rdfs:domain` is an inference rule the reasoner
materialises, so declaring it on a generically-named field like `aegis:kind`
would silently type the first unrelated thing in the estate that used the name.
The `OperatingPoint` fields deliberately carry range and comment only.

Two behaviour changes worth knowing about when reading a verdict:

- **The declared class outranks the governed effect.** A `soft` policy never
  blocks, even with `effect "deny"` — that combination is contradictory, quipu's
  placement check now refuses to define it, and honouring what the author
  declared it to *be* is the only reading that is not a guess. A policy with no
  class (projected from a catalog predating the field) still behaves exactly as
  before: the effect decides.
- **A policy declared at the PAA does not fire at the pre-edit gate.** It is
  skipped, not evaluated-and-ignored. Evaluating it there would tell the model
  to fix something its author scoped to after the fact — and until Phase 3 lands
  the post-edit auditor, such a policy is *not evaluated at all*. That is the
  honest state, and it is visible in the projection rather than hidden.

`Mode::Advise` remains a ceiling over all of it: an advise-mode deployment never
blocks, whatever class a projected constraint declares. That is what makes
staging a new hard constraint safe before anyone has measured its
false-positive rate.

### Two gaps Phase 1 exposed — and the spec to close them

Both were found by building, not by analysis, and neither is a SARC gap as such:
SARC assumes a described vocabulary and takes I6 as a design rule rather than a
checkable property. They are gaps in *this stack's* ability to back the claims
it makes.

#### Spec A — `Q-SARC-VOCAB`: describe the rest of the `aegis:` vocabulary

**The gap.** G9. Outside `shapes/aegis-properties.ttl` (SARC fields only), no
`aegis:` term carries `rdfs:domain`, `rdfs:range` or `rdfs:comment`. Terms are
defined by the shapes that constrain them, which states validity and not meaning.

**What to produce.** One declaration per property in the `aegis:` namespace,
in `shapes/aegis-properties.ttl`, each carrying:

- `a owl:DatatypeProperty` or `a owl:ObjectProperty` — literal-valued vs
  IRI-valued. The distinction is load-bearing for the reasoner, not cosmetic.
- `rdfs:label` — a human-readable name.
- `rdfs:range` — the datatype or class. Inert for the reasoner on datatype
  properties (it types only IRI-valued objects), so this is for readers and
  matchers.
- `rdfs:comment` — **what the term means and how it differs from its nearest
  neighbour.** A comment restating the label is worse than none: it passes the
  presence check while telling a reader nothing.
- `rdfs:domain` — **only where the subject class is unambiguous.**

**The domain rule, stated as a rule because it is a trap.** `rdfs:domain` is an
inference the reasoner *materialises*: declaring it asserts `rdf:type` on every
subject carrying the property. A generically-named term (`aegis:kind`,
`aegis:threshold`, `aegis:name`) will eventually be used by something else in
the estate, and a domain would silently retype it. Declare a domain when the
property's name is specific enough that no other class could reasonably carry
it; otherwise range and comment only, and say so in a comment on the omission.

**Sequencing.** Property-at-a-time, in shape-file order, not one sweep. Each
batch runs the reasoner over a store holding the shipped catalog and asserts the
inferred types are the intended ones — the materialisation is the risk, so it is
what the test has to exercise.

**Acceptance.** `every_sarc_property_the_shape_constrains_is_also_described`
generalises from the SARC field list to every `aegis:` property the shape graph
mentions; adding a constrained property without a declaration fails the build.
A second test asserts no declaration carries a `rdfs:comment` that is merely its
`rdfs:label` restated.

**As built.** `governance_plane_properties_are_all_described` in
`quipu/src/governance_tests.rs` does exactly that, over every `sh:path` in
`governance.ttl`, with a non-vacuity floor — an extractor that silently found
nothing would otherwise pass over an empty set.
`every_description_says_more_than_its_own_label` is the second test; seven
declarations were rewritten to pass it honestly rather than by lowering the bar.

Two things came out differently. **`aegis:gate` carries two meanings** in this
vocabulary — a Predicate's applicability condition, and the gate that produced an
`aegis:Decision` — and the declaration records that rather than picking one, with
a test asserting it still does. A reader who meets only one of the two will write
code assuming it is the only one.

And the scope is the **governance plane only**. The ~100 estate properties in
`aegis-ontology.shapes.ttl` (`hostname`, `rig`, `park`, `plexId`) are excluded by
name: their intended subjects are not all obvious from their shapes, and
asserting domains by guess *materialises wrong `rdf:type`s* rather than merely
documenting badly. `materialising_the_declarations_types_the_shipped_catalog_correctly`
runs the reasoner over the shipped catalog and asserts no Selector or Predicate
became a Policy — verified by mutation, since the materialisation is the risk and
so is what the test has to exercise.

**Why it earns its place.** Two consumers this stack has already committed to.
The authoring surface ([Governance Plane](governance-plane.md) §Authoring) is
composition over a catalog, and an agent drafting a policy currently has nothing
to read about the terms it is composing. And aligning `aegis:` against an
external governance vocabulary — the ontology-matching problem
[Agent-OM] addresses, with property descriptions as its primary signal — is not
attemptable against a vocabulary that has none.

#### Spec B — `H-SARC-I6`: check the hosting layer against reality

**The gap.** `aegis:hostedAtLayer` is declared and unconsumed. Nothing compares
it against where a constraint is *actually* evaluated, so a policy may claim
`"tool"` — the layer an agent cannot route around — while being enforced solely
by yupana's orchestration-layer pre-edit hook, which an agent bypasses by writing
the file another way. I6 is currently checked for well-formedness and not for
truth, and a false `tool` claim is worse than an honest `orchestration` one
because it stops people looking.

**What to produce.** A layer-truth check at the projection seam, where both
halves are known at once:

1. Yupana knows what it is. A rule evaluated by `yupana hook pre-edit` is hosted at
   the **orchestration** layer, always — that is what the hook is. This is a
   constant in `rule_planes.rs`, not a configurable.
2. On projection, compare each policy's declared `hostedAtLayer` to the layer
   that will actually evaluate it. A policy declaring `tool` or `policy` while
   yupana is its only evaluator is a **mismatch**.
3. The response is a loud fail-open, not a block: yupana refusing to project a
   policy because its metadata overclaims would disable a rule that does still
   work, trading a documentation error for an enforcement gap. Report the
   mismatch, project the rule, evaluate it at the layer that is real.
4. Record the layer actually used on the verdict and the trace record
   (`hosted_at` on `ConstraintEvaluation`), so the audit checker of Phase 5 can
   verify claim against record rather than taking the claim.

**The asymmetry that makes it worth doing.** Declaring a *weaker* layer than the
truth is harmless — a `tool`-enforced constraint described as `orchestration`
understates its own robustness. Declaring a *stronger* one is the failure. The
check is therefore one-directional: flag when `declared` is more robust than
`actual`, stay silent otherwise.

**Acceptance.** A projected policy declaring `hostedAtLayer "tool"` and
evaluated by yupana produces a mismatch notice naming the policy, the claimed
layer and the real one; the rule still evaluates and still blocks if it is hard.
A policy declaring `"orchestration"` produces silence. The negative case — no
declaration at all — also produces silence, since an absent claim overclaims
nothing.

#### `H-SARC-I6`, as built

`yupana/src/hosting.rs`, `quipu/src/governance/audit/passes.rs::hosting`. The spec
above held up; three things are worth recording about how it landed.

**The check runs twice, on purpose, and they are different checks.** Yupana checks
at the **projection seam** — once per refresh, not per edit, because a metadata
defect repeated on every guard line is a notice people learn to scroll past. That
one compares the catalog's claim against `YUPANA_HOSTS_AT`, a constant rather than
a configurable: a hook in the agent's loop *is* the orchestration layer, and
making it settable would let a deployment declare itself into a robustness it
does not have. Quipu checks at **audit time**, comparing the claim in Σ against
the layer the trace says actually ran the constraint. The second is the one that
cannot be fooled by yupana being wrong about itself.

**The record must not echo the claim.** `ConstraintEvaluation::hosted_at` is
stamped from the evaluating code's own constant, never copied from the policy's
`aegis:hostedAtLayer`. A field that repeated the declaration would let an
overclaim survive the audit by being asserted twice, which is the failure mode
the whole check exists to close.

**An unknown layer is a projection error, not a `None`.** The one value quipu's
vocabulary deliberately omits is `"prompt"`, and silently decoding it to "no
claim" would turn the single claim I6 forbids into no claim at all. It fails the
projection the way an unknown `constraintClass` does. On the audit side the same
value is an *incompleteness* rather than a violation, because there the checker
may simply be older than the runtime.

### Phase 2 — Wire the verdict, derive the trace

*Closes G2 and G3 — the two gaps that make I8 impossible.*

1. Call `verdict::promote_verdict` from the pre-edit decision path. Every fired
   constraint emits one signed, evidence-hash-bound `aegis:Verdict`.
2. Carry the projection's real freshness into `aegis:freshness`.
   `rule_verdict_message` already threads it into the model-facing text;
   `verdict_turtle` currently hardcodes `"fresh"`, which is precisely the
   silent-fresh-tag the tier discipline exists to prevent.
3. Batch and buffer. Promotion must not sit on the edit's critical path —
   `deadline_ms` is 100 ms and a `/knot` round-trip is not. Spool locally with
   the existing fail-silent discipline; drain from the resident daemon.
4. Persist quipu's write-gate verdict too (`Q-VERDICT-PERSIST`), so the
   policy-layer monitor is as auditable as the orchestration-layer one.
5. Restructure the spool record to be **derived from Σ**:

   ```text
   { pre_state_ref, action, post_state_ref,
     constraints_evaluated: [{ id, class, verification_point, outcome,
                               response_taken }],
     attribution, reward_components }
   ```

   This is `work-scoped-governance.md` phase 1 with [SARC] §3.6's `E_i` and
   §9.6's `α_i`
   named explicitly. Records and policies then share one vocabulary — the
   precondition for derive/test/explain in that document *and* for the checker
   in Phase 5.

### Phase 2, as built

The constraint set landed first, because the verdict has nothing honest to say
until the record can hold it.

`src/trace.rs` defines `ConstraintEvaluation` — SARC's `E_i` element — carrying
the four things the audit checker's passes need per constraint: **which** one,
**where** it was evaluated, what it **concluded**, and what was **done** about
it. `outcome` and `response` are separate fields on purpose: pass (iii) is "does
the recorded response match the one the policy declared", and a single collapsed
field makes it unwritable. `Outcome::Unknown` stays distinct from `Unsatisfied`
for the same reason it does in SARC — collapsing them makes an unevaluated check
indistinguishable from a passing one.

`Response::NoAction` is representable and distinct from `Logged`. A constraint
that fired and drew no response is a real state — a soft rule under a runtime
with nowhere to put it, which is exactly where the stack sits before Phase 3 —
and rounding it to `Logged` would read as a deliberate choice.

**What this replaced, and what it recovered.** `Decision` carried a `+`-joined
string of rule ids. It answered "what fired" and could not answer "was each
evaluated at a point compatible with its class", so two rules firing identically
at different points produced byte-identical records. Worse, the *governed*
plane never carried names at all: structural violations reached the spool as the
literal string `"governed-structural"` plus a count, so an operator could see
that three governed rules fired and not which three. The names existed only
inside the composed model-facing message. That is the unattributable-record
shape the audit field was added to prevent, surviving inside the very field
added to prevent it. `ProjectedViolation` now carries its id, class and point,
and the record names every rule.

The old `rule` field is **derived** from the constraint set rather than removed:
live dashboards group on it, and dropping it would silently empty every panel
built on it. That migration is a separate change from the one adding structure.

**A testing note worth keeping.** Driving the real spool from a test needs
`std::env::set_var`, which now requires `unsafe` — and this crate sets
`unsafe_code = "deny"`. Rather than weaken that, `guard` was split into
`guard_recorded` (decide + compose the record) and a two-line `guard` (emit +
return). The whole record composition is now under test through the real
decision path, and exactly one line — the `emit` call, which `metrics.rs` covers
directly — sits outside it.

**Verdict emission (G2) closed.** `src/verdict_spool.rs` signs one verdict per
evaluated constraint at the moment the constraint fires, and appends it locally;
`yupana verdicts` promotes the spool to quipu. It is a spool and not a direct
call because the guard runs inside `PreToolUse` under a `deadline_ms` that
defaults to 100 ms, and a `/knot` round-trip is not that — the projection path
already records what an unbounded quipu call does here (a wedged quipu held the
guard for two minutes). Promotion on the edit path would make every agent's
edit latency a function of quipu's availability, to record a fact nobody needs
until an audit. Signing is microseconds; the verdict is durable at the moment
of decision and stays bound to the evidence hash it was signed over, so the
delay costs nothing.

Four judgement calls in it worth naming:

- **The guard never mints a signing key.** `verdict::load_or_generate` creates
  one when absent, and a keypair materialising as a side effect of an agent's
  edit should not happen quietly. On the hook path only an *existing* key signs;
  `yupana verifier` is the deliberate act that creates one.
- **The verdict records what the PREDICATE concluded, not what the guard did.**
  A constraint can be unsatisfied while the mode declined to block. Conflating
  them would make an advise-mode fleet indistinguishable from a compliant one in
  the governed record; the response lives in the trace record beside it.
- **`unknown` spools nothing.** An unknown verdict asserts "there was no
  evidence", and a constraint yupana evaluated had evidence by construction.
  Minting satisfied or unsatisfied for it would be a signed claim about
  something that concluded neither.
- **A rejected verdict is retained, never dropped.** The drain truncates only
  when every line was accepted. A rejection is a fact about the verdict — a
  shape violation, an unregistered verifier — and truncating past it would erase
  exactly the record worth investigating.

**Freshness stopped being a lie in two places.** `verdict_turtle` hardcoded
`aegis:freshness "fresh"`, so every verdict yupana could have promoted would have
claimed currency it never checked. It now takes the real value, and `Decision`
carries the currency of the policy set that produced it — a parameter rather
than a defaulted field, because a default would be `Fresh` and every caller who
forgot it would rebuild the same defect. The trace record carries it as
`policy_freshness`, which is what stops a soak window counting verdicts computed
against a stale projection as evidence about current policy.

`Recomputing` maps to `stale` on the verdict and stays itself on the trace
record: `aegis:freshness` admits only fresh/stale and the conservative reading is
the only one that cannot overstate, while a trace record is diagnostic and the
distinction is real. Two audiences, two mappings, each stated where it applies.

**The attribution tuple, since closed.** Phase 2 shipped `α` partial: yupana
supplied `tool`, `executor` and `C_eval`, and left the principal chain `P` and
the intersected authority `auth` absent rather than filled with the single agent
id — a one-element chain asserted where a real chain belongs reads as "this
action had one principal" to exactly the auditor the field exists for. The
stated blocker was multi-tenancy quipu did not have. **That reading was wrong
about quipu and is now wrong about yupana too**; see [Phase 6](#phase-6-as-built)
for what named graphs already provided, what authority intersection added, and
`src/attribution.rs` for the five elements yupana now records. `auth` is still not
one of them, and that is deliberate rather than pending: it is the intersection
of grants quipu owns, so yupana records `P` and the checker derives `auth` from the
authoritative source.

### Phase 3 — Make the PAA a real enforcement point

*Closes G4.*

Give `hook/post_edit.rs` a constraint-evaluation path alongside its advisory
context: evaluate `verificationPoint "PAA"` policies against
`(pre, post, action, obs)`, emit verdicts, and implement the `throttle`
response — a declared backoff applied to *subsequent* actions once a soft window
is crossed. This is the single highest-leverage mechanism in the [SARC]
evaluation
and the stack has no equivalent.

Soft constraints stay non-blocking by construction. The PAA's "prevents the
next, not the just-completed" semantics must be explicit in the module doc,
because presenting it otherwise is the false-`prevented` claim the enforcement
gradient exists to stop.

### Phase 3, as built

`src/hook/paa.rs` evaluates the constraints that DECLARE
`verificationPoint "PAA"` against the completed file, records them in the same
trace vocabulary the gate uses, and spools their verdicts.

**The two points partition the rule set; they do not overlap.** A rule declaring
`PAA` is skipped at the gate and judged here; one declaring `PAG`, or declaring
nothing at all, is judged at the gate exactly as before the field existed. So a
constraint is evaluated once and its verdict says where — auditing at both would
record two verdicts for one action and make the coverage pass ambiguous.

**A satisfied constraint is recorded, not skipped.** "The rule ran and held" and
"the rule never ran" are different facts, and an absent evaluation reads as a
constraint nobody applied.

**The auditor never blocks, under any mode.** `Enforce` and `Advise` behave
identically here; only `Off` disarms it. That is not an oversight — it is what
makes the soft class mean something, and quipu's placement check already refuses
to let a *hard* constraint declare itself at this point.

**`throttle`, and what it honestly is.** A declared `backoffFormula` records an
expiring, repo-scoped backoff (`src/throttle.rs`) that the next edit's advisory
surfaces. It is `observed`, not `prevented`: an agent that ignores it proceeds.
There is no timer and no sleeping — a throttle is state with an expiry, read by
whatever hook runs next — so it is deliberately not described as rate limiting,
which it cannot enforce. What it does buy is that a crossing is bounded,
recorded, and visible to the next action instead of dissolving into a log line.

Three judgement calls worth naming:

- **An unparsed backoff formula applies no throttle and says so**, recording
  `Response::NoAction`. A default would be a backoff nobody declared, under a
  constraint whose entire point is that its cost *was* declared — and silently
  warning instead would report the crossing as handled.
- **Throttles are scoped to the repo root.** State is one file per user, not per
  checkout. The first implementation was unscoped, and the test suite caught it
  the way it deserved to be caught: tests passed alone and failed together,
  because a throttle written by one leaked into another. In production the same
  bug means a window crossed in one repo advises an agent editing an unrelated
  one — an advisory that is true about something and false about the work in
  front of it, which is how agents learn to ignore advisories.
- **An unscoped record matches nothing rather than everything.** The permissive
  reading of a legacy line would advise every repo on the host.

**The audit runs independently of the blast-radius advisory.** `advisory_for`
bails early on a non-Rust file, a file with no symbols, or an edit landing
outside every symbol body. The auditor must not inherit those exits, or a
constraint's coverage would depend on a property of the file with nothing to do
with the rule.

### Phase 4 — Escalation Router (quipu, yupana client)

*Closes G5. Turns `require-approval` from fail-closed-with-no-channel into
bounded human oversight.*

- `aegis:OperatorGroup` shape: capacity model (`M/M/c` — `c`, `mean_service_s`),
  hours, after-hours mode, `fallback_if_unavailable "deny"`.
- Router service in quipu: accept a suspended action, dispatch a
  `DecisionRequest` to the group's queue, hold until τ_rev, **default-deny** on
  timeout. A ruling that *modifies* the action must re-enter the gate (SARC
  Algorithm 1's `goto PagCheck`) — re-validation, not trust.
- Decisions stay content-bound to the evidence hash (already the shape's
  contract), so approve-then-change goes stale automatically.
- Yupana gets a thin ER client at the pre-edit seam. When the router is
  unreachable the escalation-class response is deny, and the fail-open notice
  says so loudly.
- Emit queue-depth / wait / utilisation metrics, because the operative claim of
  [SARC] §5.3 is that `W_q < τ_rev` is a *measurable* property of an M/M/c
  queue, not an assertion.

### Phase 4, as built

`quipu/src/governance/router.rs`, plus `aegis:OperatorGroupShape` and
`aegis:DecisionRequestShape`. Three things came out differently from the sketch
above, each because the sketch described something that could not be true.

**The router does not hold the transaction open.** A write gate is synchronous;
"hold until τ_rev" would convert an approval gate into a lock on the store, and
a store you cannot write to while a human is at lunch is an outage wearing
governance's clothes. What actually happens: the refused attempt **mints a
`DecisionRequest`** naming the policy, the target, the group that can rule, an
evidence hash and an `expiresAt` derived from the constraint's reversibility
window; a human signs an `aegis:Decision` bound to the same hash; the **next
attempt** finds it and proceeds. The hold is the agent retrying, not the engine
waiting. §5.3's window still governs — past `expiresAt` an unserviced request is
`Expired`, which is a **denial**, and `Ruling::permits` returns true for
`Approved` alone so neither `Pending` nor `Expired` can read as a pass.

**A rejection outranks an approval.** When both are bound to the same evidence,
two humans have disagreed, and resolving that by row order would make the
outcome depend on storage layout. The safe reading of a disagreement about
whether to permit something is no.

**The request is staged, not written in place.** The gate runs inside the
savepoint the refusal is about to roll back, so a request written there would
vanish with it — leaving an operator a refusal and no request to act on, exactly
the state the router exists to end. Requests stage on the `Store` and flush after
the savepoint resolves *either way*, the same mechanism verdicts use rather than
a second one invented for decisions.

**What it is not:** there is no scheduler and no notification. `routedTo` records
*which* group should rule; delivering the request to them is a consumer of the
record. The queue-depth and utilisation metrics of §5.3 need a queue with a
server attached, and claiming `W_q < τ_rev` from a store that only records
requests would be the dashboard anti-pattern with extra steps. That measurement
stays open, and it is named here rather than quietly dropped.

### Phase 5 — Audit checker and enforcement inventory

*Closes G8 and G10 — the "auditable by construction" claim itself.*

- `quipu_audit_check(Σ, T)`: four passes per [SARC] Definition 2 (§3.6) —
  coverage,
  class-placement compatibility, outcome consistency, attribution completeness —
  returning a structured discrepancy report. Deterministic,
  predicate-language-agnostic, and explicitly **not** an LLM call: [SARC] §5.1's
  design rule, which is the same `O(ℓ_tool)` budget discipline yupana already
  applies to its own guard.
- A dispatch-graph inventory for I7: enumerate every tool-call class the harness
  exposes, mark governed/ungoverned, fail the check when an executable class
  traverses no compatible enforcement point. Seed it from
  `work-scoped-governance.md` §"What this cannot reach", so the known-unreachable
  surfaces become **data** rather than prose.
- The replay/eval harness from `work-scoped-governance.md` §Evals — liveness,
  both-outcomes, non-vacuity, recoverability, replay. This is what makes the
  Phase-1 operating point θ an honest number rather than a declared one, and it
  gates advise→enforce promotion per rule.

*All three built; see [Phase 5, as built](#phase-5-as-built), including what
replay measures and what it provably cannot.*

### Phase 5, as built

`quipu/src/governance/audit.rs` (+ `audit/passes.rs`, `audit_spec.rs`),
`inventory.rs`, `replay.rs`, and `quipu audit <trace>|inventory|replay <trace>`.
All three pieces landed. What is worth recording is where each one stops.

#### The checker

Four passes, each a comparison between two declared values and never a model
call. Coverage, class-placement compatibility, outcome consistency, attribution
completeness — plus the I6 claim-versus-record check described above.

**Two severities, kept apart deliberately.** A **violation** is the trace
contradicting Σ: a soft constraint that blocked, a declared `deny` that only
warned under `enforce`, a record whose declared chain disagrees with the process
that ran. An **incompleteness** is the trace not saying *enough* to decide: no
principal chain, no declared class, a constraint Σ declares that this window
never exercised. Collapsing them would break the checker in the direction that
matters — report everything as a violation and an operator learns to ignore the
output; report everything as an incompleteness and a soft constraint blocking an
edit reads as a formatting note. Only violations change the exit code.

**The outcome pass is mode-aware**, and has to be. `advise` has a declared
ceiling, so a hard `deny` constraint that only warned is *correct* under `advise`
and a violation under `enforce`. A check that ignored the mode would have to pick
one of those two records to be wrong about.

**The placement pass reuses `placement::points_for`** rather than carrying its
own copy of Table 3. Two copies would eventually disagree, and the disagreement
would be between the definition-time check and the audit-time one — the two
places that must not.

**What it cannot check, stated in the module doc rather than papered over.**
SARC's coverage pass asks whether every constraint that *applied* to an action
was evaluated. Deciding applicability means re-running the selector against the
file as it stood, and quipu has neither the file nor the parser. What is
checkable is the converse — nothing is cited that Σ does not define — plus
vacuity. A checker reporting "coverage: pass" while testing something weaker
would be the more dangerous artifact.

Unreadable lines are **counted, never skipped**, and the count is in every
summary even at zero. Conformance over a window that was only partly read is not
conformance.

#### The dispatch inventory (I7)

`shapes/dispatch-inventory.ttl` turns
`docs/work-scoped-governance.md` §"What this cannot reach" from prose into
`aegis:ToolClass` facts. Prose goes stale the first time a harness adds a tool;
nothing recomputes, and the list quietly becomes a description of last year's
deployment.

The distinction the whole thing exists for: an executable class traversing
nothing with **no stated reason** is an unknown hole (violation); the same class
with an `aegis:ungovernedReason` is an **acknowledged bypass surface**
(incompleteness). Neither is "governed", and the checker never reports one as the
other — without that split an operator cannot tell a decision from an oversight.
Every acknowledged surface is reported on every run, because one an operator has
stopped seeing is one they have stopped weighing.

Plus the cross-check the other direction: a constraint placed at a point no
executable class traverses **can never fire**. It reads as governance in the
catalog and is inert in the deployment — the failure hardest to see from either
side alone. An empty inventory is an incompleteness, never a pass: an unwritten
dispatch graph is not an empty one.

The seed file says in its own comment that nothing derives it from the harness's
tool registry, so it can drift the way the prose did. The difference is that a
drifted declaration is a wrong answer to a question something asks, rather than a
paragraph nobody re-reads.

#### The replay harness

Five gates per rule — liveness, both-outcomes, in-spec, recoverability, and
new-blocks (not a gate; the number an operator is actually deciding about).
Recoverability walks the trace in **order**: a target cleared *before* its
refusal proves nothing about whether anyone got past the rule, and counting it
would make every rule that ever allowed anything look recoverable.

Nothing is re-evaluated. The predicate needed the file as it stood and that file
is gone, so replay is deterministic arithmetic over records rather than a
simulation.

**Three limits ride in the summary rather than in a footnote**, because a
promotion number read without them is read as a safety claim:

1. It measures only traffic that happened. A rule that would block a kind of edit
   nobody attempted shows zero new blocks and is not therefore safe.
2. It counts false-positive **candidates**, never false positives. A block is
   wrong only if the action was legitimate, and no record carries that judgement
   — it needs a human. Reporting `new_blocks` as an FP rate would be the
   number-shaped fiction the operating point exists to replace.
3. It bounds **no false negatives at all**. Actions a rule let through without
   firing look exactly like actions it correctly approved.

So θ is now *measurable in one direction and still undeclared in the other*, and
the doc should not be read as saying the operating points are calibrated. They
are calibratable.

#### Still open in Phase 5

There is no scheduler behind the Escalation Router, so §5.3's `W_q < τ_rev`
remains unmeasured: the queue-depth and utilisation metrics need a queue with a
server attached, and claiming the inequality from a store that only records
requests would be the dashboard anti-pattern with extra steps.

### Phase 6 — Multi-agent

*Closes G7. Originally written as "gated on quipu enforceable multi-tenancy — do
not start before it". That gate was misdescribed; see [Phase 6, as
built](#phase-6-as-built).*

- Attribution tuple α on every trace record; the trace stored as a **tree**,
  worker subtrees attached to their dispatch node, never summarised.
- Authority intersection along the call chain, monotonically non-increasing;
  an empty intersection fails safe.
- Constraint inheritance with **decidability rescue**: evaluate an inherited
  constraint at the deepest layer where it remains decidable, or escalate —
  never silently drop it, which is the constraint-laundering path.
- Trust-boundary tagging on imported state (Bobbin retrieval results, MCP tool
  output, sub-agent responses) with a PAA trust predicate. The zero-trust agent
  gateway of [SARC] §9.5, expressed in the existing constraint vocabulary rather
  than as a separate perimeter layer.

### Phase 6, as built

#### The gate was described wrongly

"Blocked on quipu multi-tenancy" was the framing above, and in G7, and in the
Phase 2 note about the attribution tuple. It was wrong, and it was wrong in a way
that cost real work: it made a whole phase look unreachable when the substrate
was already there.

Quipu's named graphs are not a provenance label. `graphs` is a registry with an
enforced `committed|overlay` class and a bind-once `parent_branch`, and writes,
retractions and idempotency are **all graph-scoped in SQL**
(`store/overlays.rs`, `store/ops.rs`). Partitioning was never the missing piece.
What was missing is **authorization**: `http_auth::authorize` is one global
bearer token, all-or-nothing, and nothing checked whether a principal may write
to graph *N*. "Multi-tenancy is unbuilt" and "multi-tenancy has no access-control
layer" are different claims, and only the second was true. `quipu`'s
`docs/design/group-isolation.md` carried the older framing and has been corrected
in place, because a doc that is being read as a blocker is not a neutral
inaccuracy.

#### Authority intersection — `quipu/src/governance/authority.rs`

§9.3's rule, `auth_k = ⋂ authority(p_i)` for `i in 0..=k`, as code:

- `Authority::intersect` is monotonically non-increasing, so adding a delegate
  can only narrow. That is the defence against **authority escalation via tool
  capability** (§9.5) — a sub-agent whose own credentials are broader cannot use
  them, because the effective authority is the intersection and not the
  executor's own.
- **The wildcard is the identity, not a widening.** A principal may hold `*`,
  which is how a single-tenant deployment keeps working unchanged; `*`
  intersected with a narrow authority is the narrow one, so a wildcard-holding
  orchestrator delegating to a scoped worker yields the *worker's* scope.
- **An empty intersection fails safe.** A chain narrowed to nothing cannot act,
  and that is a refusal rather than a fallback to the principal's authority —
  the fallback would be precisely the escalation the rule exists to stop.
- **An empty chain is `none()`, not `any()`.** "Nobody said who is acting" must
  not mean "anybody may act".
- **An undeclared principal holds nothing.** Reading an absent grant as
  permission is how an access-control layer becomes decorative.

Enforcement runs in `Store::enforce_graph_authority`, called **before** the
savepoint in `transact_to_graph` — the real write path, not a helper a caller
can route around. It is gated by `enforce_authority` (default off) and is inert
without a principal chain, so every existing caller is untouched: the flag makes
a *supplied* chain binding rather than making attribution a hard requirement
beneath a running deployment. The refusal names the chain, the graph, and what
the chain actually holds, because a refusal that says only "denied" leaves an
operator guessing which link narrowed it.

#### The attribution tuple — `yupana/src/attribution.rs`

`α = ⟨P, planner, executor, tool, auth, C_eval⟩`, on the gate record and the PAA
record alike. Five of the six:

- **`P`** from `YUPANA_PRINCIPAL_CHAIN`, comma-separated and caller-first. A
  dispatcher that spawns a sub-agent appends itself and exports the extended
  chain. **Absent when undeclared** — an undeclared chain is not a one-link
  chain, and this is the same distinction that kept the field out of the record
  in Phase 2.
- **`planner`** from `YUPANA_PLANNER`, declared and never derived from the chain's
  head. Which link deliberated and which executed is a fact about the dispatch;
  reading it off list position would be an inference wearing a record's clothes.
- **`executor`** from `$SHANTY_AGENT` — the identity of the process that actually
  ran, which is ground truth rather than a claim.
- **`tool`** from the hook payload.
- **`C_eval`** is the `constraints` array Phase 2 already emits.

`auth` is deliberately **not** recorded by yupana, and this is a settled decision
rather than a pending one: the effective authority is the intersection of grants
that live in quipu, yupana cannot read them inside a 100 ms pre-edit budget, and a
locally-guessed value would put a number in the field the grant store never
agreed to. Recording `P` is what lets the checker derive `auth` from the
authoritative source. The tuple is completed by the audit, not faked by the hook.

**The conflict flag.** `YUPANA_PRINCIPAL_CHAIN` is a declaration; `$SHANTY_AGENT`
is what is running. When a chain is declared and its tail disagrees with the
executor, the record says `attribution_conflict` rather than silently preferring
one. That disagreement is the observable signature of a laundered chain — an
agent acting under a dispatch record naming somebody else — and a record that
resolved it by precedence would delete the only evidence of it. It is emitted
only when true: a `false` on every line trains a reader to skip the field, which
is the opposite of what it is for.

#### The attribution tree — `quipu/src/governance/tree.rs`

`quipu audit tree <trace.jsonl>` reassembles the dispatch forest from the
principal chains. The trace is still emitted as a **sequence**, so this is
reconstruction rather than structure, and the output says so in three places
rather than presenting a tree as a guarantee:

- **Unattributed records are not placed.** Attaching one to whichever root
  happened to be first would invent an answer to the question the tree exists to
  answer.
- **Implied dispatch nodes are flagged.** A chain `[orchestrator, worker]` proves
  an orchestrator exists; it does not prove the orchestrator's own actions are in
  the window. "Did nothing" and "was not recorded" are different facts.
- **Collapsed nodes get a note.** Two dispatches of the same worker by the same
  caller produce the same chain and land on one node — attribution dilution
  reappearing at the reader instead of at the record.

#### Constraint inheritance — `quipu/src/governance/inheritance.rs`

`aegis:inheritedByDelegates` declares that a constraint binds the whole subtree
under a dispatch. `aegis:onUndecidable` admits only `"escalate"` — the same shape
as `onTimeout` admitting only `"deny"` — which is the **decidability rescue**:
evaluate at the deepest layer where the constraint still decides, or hand it to a
human. A constraint that silently stops applying where it cannot be checked is
one an agent escapes by dispatching into a context where the evidence is absent.

Two findings, at deliberately different severities because one has evidence and
the other has a question. A constraint that **decided on a target** and is absent
from a deeper action on the *same* target is a **violation** — it proved it could
decide there. A constraint evaluated at a dispatch node and never in its subtree
is an **incompleteness**: that might be laundering and might be a selector that
matched nothing deeper, and deciding needs the files as they stood. Reporting
both as violations would make the strong one unfindable.

`is_below` is a strict prefix test. A sibling branch is not a delegation of this
one, and a shallower action after a deeper one is *returning*, not delegating —
either read as laundering would flag every parallel run.

#### Trust boundaries — declared on the `ToolClass`

`aegis:importsUntrustedState` and `aegis:untrustedOrigin`, on the dispatch
inventory rather than on a policy, because the tool class is the thing that
actually imports. Reported by `quipu audit inventory` whether or not the class is
governed: `governedAt` says a class's own *actions* traverse a point and says
nothing about what it *returned*. A class that imports and declares no origin is
a violation — an import channel nobody can describe is one nobody can weigh.

`shapes/dispatch-inventory.ttl` declares this stack's three real channels: a
sub-agent's response text, MCP tool output, and retrieved documents.

#### Still open in Phase 6

**There is no trust predicate.** The vocabulary now names the channels and the
inventory reports them on every run, but nothing *evaluates* imported content.
The PAA judges the file after an edit, not the material the edit was based on,
and building a predicate over sub-agent responses needs a producer that records
them — which no part of this stack does today. So the boundary is declared and
open, not closed; the honest claim is that it is now visible on every run rather
than absent from the model.

**The trace is emitted as a sequence.** Making it structurally a tree means the
harness emitting a dispatch id per spawn, which is a change in the harness, not
in yupana. Until then, sibling dispatches of the same principal are
indistinguishable — and `quipu audit tree` says so per node instead of leaving a
reader to assume otherwise.

## Files this touches

**quipu**

- `shapes/governance.ttl` — the constraint-object extensions (Phase 1)
- `shapes/policies/treesitter.ttl` — backfill the shipped catalog (Phase 1)
- `src/governance/guard.rs` — class-aware effects, verdict persistence
- `src/governance/placement.rs` *(new)* — class↔placement conformance pass
- `src/governance/router.rs` *(new)* — escalation router (Phase 4)
- `src/governance/audit.rs` *(new)* — the `T ⊨ Σ` checker (Phase 5)
- `src/signing.rs` — reused unchanged; it is already the root of trust
- `docs/design/policy-edit-hooks.md` — the SARC gaps land in its backlog table

**yupana**

- `src/project_queries.rs`, `src/project.rs` — projection of the new fields
- `src/rules.rs` — `Rule` gains class + verification point
- `src/hook/rule_planes.rs` — response selection by declared class
- `src/hook/pre_edit.rs` — verdict emission on decision
- `src/hook/post_edit.rs` — PAA constraint evaluation + throttle (Phase 3)
- `src/metrics.rs`, `src/audit.rs` — Σ-derived trace record (Phase 2)
- `src/verdict.rs` — real freshness and batching; the signing scheme is correct
- `src/daemon/` — verdict drain, ER client (Phase 4)
- `docs/work-scoped-governance.md` — reconcile its phasing with this one

## Verification

Beyond each repo's normal gate:

- **quipu** — `cargo test governance`: the extended catalog must still conform;
  new tests that a class↔placement mismatch is *rejected at write* (the
  definition-time half of the discipline), and that an escalation-class policy
  without τ_rev fails validation.
- **yupana** — `just check && just test`, plus two-sided fixtures through the real
  hook binary per `work-scoped-governance.md` §Evals: a RED case and a GREEN
  case per new constraint class, and a non-vacuity mutation check.
- **End to end** — a policy authored in quipu, projected into yupana, fired at
  pre-edit, promoted back as a signed verdict, and accepted by
  `quipu_audit_check` against the same Σ. That round trip *is* the
  decidable-audit property of [SARC] Property 1, and it is the acceptance test
  for Phases 1–2.

**One caveat travels with any number this produces:** replay measures false
positives only on traffic that actually happened, and measures no false
negatives at all. A seeded adversarial corpus gives coverage against known
attacks and none against novel ones.

## Out of scope — and why

Five of the six sources this analysis was drawn from bear on how Σ gets
*authored* and kept aligned with its sources, not on how it is enforced at
runtime. That distinction matters: [SARC] §6 is explicit that the arrow from
obligation to predicate is an **institutional process, not a technical step**,
and that the framework "presupposes rather than resolves" it. So the authoring
work is real, and it is a different piece of work.

Where each one lands, should we pick that thread up:

- **[Raji & Bashir]** surveys the governance requirements Σ has to encode, and
  is the clearest statement of the principal-agent problem behind G7 — who is
  the principal, and what is the agent authorized to do on their behalf. Its
  Singapore MGF summary ("assess and bound the risks upfront", "make humans
  meaningfully accountable") maps onto the risk map and the ER respectively.
- **[Informatica/Deloitte]** argues the semantic-layer case Quipu already
  embodies. Useful as external corroboration for why the governed graph is the
  right home for policy, not as a source of requirements.
- **[Agent-OM]** is directly applicable to keeping `aegis:` aligned with
  external vocabularies as they drift — the maintenance half of the translation
  layer, and the thing that stops predicates silently decaying away from their
  source obligations ([SARC] §6, "the translation layer as a governed control
  surface").
- **[Peshevski et al.]** is the agent-driven ontology-construction pattern
  behind the "mined" authoring modality in
  [Governance Plane](governance-plane.md) §Authoring.
- **[Olivares-Alarcos et al.]** grounds *explanation* generation in an ontology
  while keeping the reasoning sound — relevant to making a refusal legible, which
  is a stated requirement of the recoverability eval in
  `docs/work-scoped-governance.md` §Evals ("every refusal names the command that
  satisfies it").

None of them changes the runtime gap list above, which is why they are named
here rather than folded into it.

## Sources

- **[SARC]** — Besanson, G. (2026). *SARC: A Governance-by-Architecture
  Framework for Agentic AI Systems: Compiling Regulatory Obligations into
  Runtime Constraints*. Working paper, Universidad Torcuato Di Tella.
  [arXiv:2605.07728v1](https://arxiv.org/abs/2605.07728) [cs.SE].
  Reference artifacts: <https://github.com/besanson/sarc-governance>.
  All section, table, definition and invariant references in this document are
  to this paper.
- **[Raji & Bashir]** — Raji, M. & Bashir, M. (2026). *Towards Agentic AI
  Governance: A Preliminary Assessment*. AIR-RES 2026, Springer Nature.
  [arXiv:2607.07612v1](https://arxiv.org/abs/2607.07612).
- **[Informatica/Deloitte]** — Beierschoder, M., Andrensek, J. & Rebele, T.
  *Building the Semantic Data Layer for Agentic AI*. Informatica / Deloitte
  whitepaper (5340en).
- **[Agent-OM]** — Qiang, Z., Wang, W. & Taylor, K. (2024). *Agent-OM:
  Leveraging LLM Agents for Ontology Matching*. PVLDB 18(3), 516–529.
  [doi:10.14778/3712221.3712222](https://doi.org/10.14778/3712221.3712222).
- **[Peshevski et al.]** — Peshevski, D., Stojanov, R. & Trajanov, D. (2025).
  *AI Agent-Driven Framework for Automated Product Knowledge Graph Construction
  in E-Commerce*. [arXiv:2511.11017v1](https://arxiv.org/abs/2511.11017)
  [cs.AI].
- **[Olivares-Alarcos et al.]** — Olivares-Alarcos, A., Ahsan, M., Sanjaya, S.,
  Lin, H.-I. & Alenyà, G. *Ontological grounding for sound and natural robot
  explanations via large language models*.
  [arXiv:2602.13800v1](https://arxiv.org/abs/2602.13800).

Internal design documents this analysis builds on, all in-tree:

- [Governance Plane](governance-plane.md) — the verification spine, verdict
  integrity, risk × confidence, the Yupana↔Quipu integration contract.
- [Policy edit hooks](policy-edit-hooks.md) — evidence locality, the quipu
  pre-commit gate, the yupana projection, and the `Q-*` / `H-*` backlog the
  `Q-SARC-*` beads extend.
- [Tiers and Freshness](../concepts/tiers-and-freshness.md) — the confidence
  inputs SARC's operating point composes over.
- `docs/work-scoped-governance.md` — the trace taxonomy, the five eval
  properties, and the per-rule promotion ladder. Out of the book by design;
  cited by path.
