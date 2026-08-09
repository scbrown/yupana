# Governance Plane — Policy, Workflows & Verification

Status: **design — ready to implement.** This describes a governance capability
that layers on the Yupana × Quipu × Bobbin stack. The primitives, trust model,
integration contract, and build order are settled; see
[Implementation phasing](#implementation-phasing) for the MVP and the build order,
and [Decisions and deferrals](#decisions-and-deferrals) for the calls made. Not
yet built. Scope for v1 is a single trust domain.

## Motivation

The stack today is a **knowledge plane** — Yupana (live code structure), Quipu
(governed bitemporal record), Bobbin (search / fusion / RAG). Its job is to
*know things about the code and feed context to agents*.

Missing is a **governance plane**: the layer that decides *what a fleet of
agents should be doing, and what they are allowed to do*. Today that intent
lives in skills — advisory checklists an agent may freely ignore. The goal is to
make it **load-bearing**:

- **Workflows propel** the agent — soft but pervasive, always driving toward the
  next step.
- **Policy constrains** the agent — hard but targeted, a wall at the boundaries
  that matter.

The governance plane *uses* the knowledge plane; it is not part of it. Policies
query Yupana/Quipu/Bobbin for the facts they evaluate over.

## The verification spine

"How is an artifact verified?" and "what is a policy?" are the same question. A
policy **is** a verification predicate. So the plane is not three sibling
concepts (artifact, workflow, policy) — it is **one recursive primitive,
verification**, with policy as the verifier at each node.

Verification composes bottom-up:

```text
Workflow    — verified iff its Steps are (per its order / dependencies)
  └ Step     — verified iff its required Artifacts are + its gate policy holds
     └ Artifact — verified iff its Reference, dereferenced, satisfies its Claim   ← leaf
```

Only the **leaf** touches reality — it dereferences to concrete evidence from
the knowledge plane. Everything above is composition of leaf verdicts.

## Primitives

The minimal vocabulary. `Artifact` / `Step` / `Workflow` / `Policy` are *types*
of `Entity`; the rest are base primitives.

- **Entity** — a governed, typed node in Quipu (bitemporal, provenanced).
- **Reference** — an entity's handle to a *concrete target*: a commit, a file, a
  Yupana code-symbol IRI, a URL. The entity is the governed abstraction; the
  reference points at the real thing.
- **Claim** — the verifiable assertion an entity carries (e.g. *"exists and is
  approved"*).
- **Evidence** — facts that bear on a claim, drawn from the knowledge plane
  (Yupana), the record (Quipu), or external systems (git, CI). Evidence is either
  **canonical** (read directly from a source of truth) or an **attestation** (a
  signed statement from the system that owns the fact).
- **Verifier (= Policy)** — a **pure predicate** mapping evidence → verdict
  `{satisfied | unsatisfied | unknown}`. `unknown` (no evidence yet) is distinct
  from `unsatisfied` (evidence says no). Policies are **external**: a policy
  *targets an entity type*, the way a SHACL shape targets a class. One policy
  governs all entities of that type.
- **Verdict** — the result, **stored** as a governed bitemporal fact. Carries a
  `tier` and `freshness` (see [Tiers and Freshness](../concepts/tiers-and-freshness.md)),
  is signed, is bound to the evidence it saw, and is reproducible (see
  [Verdict integrity](#verdict-integrity)).
- **Actor / Role** — who performs a step. Enables fleet-level assignment and
  separation-of-duties policies (*"the reviewer must differ from the author"*).
- **Binding / Gate** — attaches a policy to a *boundary* with an *effect*. The
  boundary is either an **action** (pre-edit) or a **transition** (step
  completion) — the same policy can be `unknown` at one and decisive at the
  other. The policy stays pure; the effect is contextual and may be **computed
  from risk × confidence** (see [Risk and confidence](#risk-and-confidence))
  rather than fixed.
- **Instance (Run)** — a definition gone live for a specific run (this bead, this
  session), holding live state + verdicts. Distinct from the definition it
  instantiates.
- **Catalog leaf** — the composable atoms policies are built from, of two kinds
  (see [Catalog](#catalog-initial)): a **selector** (evidence → a *set*, e.g.
  `changed-symbols`) and a **predicate** (element → bool, e.g. `has-test`).
  Registered, evidence-grounded, tier-tagged.
- **Parameterization** — definitions are *templates*. References and actors are
  *patterns* (`repo://{repo}/docs/design/{bead}.md`, role `reviewer`) resolved
  when an Instance binds its parameters (bead, repo, agent).
- **Temporal trigger** — a scheduled re-verification for verdicts that depend on
  *elapsed time* (SLAs, deadlines), which reactive fact-change invalidation does
  not cover.

### Composition types

- **Artifact** — an entity standing for a thing produced or required (design doc,
  test result, ADR, commit, review). Verified at the leaf.
- **Step** — one unit of work: `{ produces: [Artifact], requires: [Artifact],
  gated-by: [Policy], actor: Role }`.
- **Transition** — a *guarded* edge from a step: `{on: <outcome>, to: <step>}`. A
  step may have several — `approve → next`, `request-changes → back to
  implement`, `reject → terminal`, `escalate → higher role` — so the workflow is
  a branching, possibly looping graph, not a line. The guard is a predicate over
  the step's verdicts / decision, evaluated by the same machinery. Loops are
  bounded (an iteration cap or temporal trigger) so a review ↔ changes cycle
  cannot spin forever.
- **Workflow** — a directed graph of Steps and guarded Transitions (branches,
  loops, terminals).

## Verdict integrity

A bare "satisfied ✓" written into the record is forgeable by anyone who can
write a fact. The plane defends against the **agent** (including a compromised
one), not the human operator — the human is the trust root. A verdict is
therefore an **attestation, not a claim**, with four properties:

- **Authenticity** — every verdict is signed by a *registered* verifier
  identity. Quipu accepts a verdict-fact only if signed by a verifier registered
  for that predicate. An agent cannot mint Yupana's signature.
- **Binding** — the verdict names the *content hash* of the evidence it saw.
  Change the referent → hash changes → the stored verdict is automatically
  stale. Kills tamper-after-the-fact and replay.
- **Reproducibility** — because the predicate is pure and deterministic and the
  evidence is content-addressed, any independent verifier can re-run it over the
  same evidence and must get the same result. Verdicts are **checked, not
  trusted**. (Purity is the security property, not just clean design.) The
  verdict retains `evidence-hash + predicate-id` so third parties can re-derive.
- **Evidence provenance** — every evidence input is either **canonical** (Yupana
  reads the source of truth directly from its own live map — the agent's edit
  *is* the evidence, nothing to fake) or a **signed attestation** from the owning
  system (CI signs "green over hash H"; the approver signs the approval). The
  chain terminates at a root; nothing in it is an agent's self-report.

**Architectural constraint:** the verifier must run **out-of-band from the agent
it verifies** — a service the agent *calls*, not a library the agent *hosts* —
or the agent runs its own verifier over a doctored repo and signs anything. The
gatekeeper sits outside the gate.

**Root of trust:** this reduces "trust every verdict" to "trust the verifier
*keys* and the *registry* of which identity may attest which predicate." That
registry is a governed, human-authored definition. The scheme does not eliminate
trust; it concentrates it into a small, human-owned surface and makes everything
downstream reproducible.

## Risk and confidence

Fixed allow / deny gates are too blunt. Two derived signals let the plane
*modulate* enforcement instead of hardcoding it.

### Confidence — how sure a verdict is

Derived from the evidence behind it, reusing tier + freshness:

- **tier** — a `has-test` from a tree-sitter heuristic is lower-confidence than
  one LSP-resolved; a `tests-green` from a signed CI attestation is highest.
- **freshness** — a `stale` verdict is less trustworthy than a `fresh` one.
- **verifier reliability** — attested-and-reproduced > attested > self-reported.

Confidence composes **weakest-link**: a policy is only as confident as its
least-confident leaf (a selector's tier bounds the predicate quantified over it).
`unknown` has no confidence — it is not a soft `unsatisfied`.

### Risk — how much is at stake

An attribute of an action / change / step:

- **blast radius** (Yupana) — more dependents reached, more risk.
- **criticality** — a human-authored **risk map** (`auth/**`, `payments/**` =
  high). A governed artifact; sovereignty applies.
- **reversibility / sensitivity** — a deploy or a data migration outranks a lint.

### Risk-adaptive effect

The gate's effect becomes `effect(risk, confidence)`, not a constant:

- low risk + high confidence → **allow** silently
- low risk + low confidence → **warn**
- high risk + low confidence → **require-approval / escalate**
- high risk + high confidence → allow, but **record**

Two consequences:

- **Confidence sufficiency scales with risk.** A high-risk change can *require* a
  minimum confidence tier — "for an `auth` change, `tests-green` must be CI-tier,
  not a tree-sitter guess" (`require-confidence ≥ X when risk ≥ Y`).
- **The enforcement floor interacts with risk.** On a harness that can only
  `observe`, high-risk work must be pre-authorized or routed to a hard-gating
  harness. Risk decides which harness may run which work.

This is the market's *adaptive governance* — assisted by default, promoted or
blocked by measured gates — expressed as one function over signals the stack
already produces.

## Roles in the stack

- **Yupana — live verifier, gatekeeper, guide.** Holds the code in memory, so it
  evaluates code-grounded claims *before commit* at `tier=live`, and that live
  verdict stream *is* the propulsion: it tells the agent which claims are not yet
  green. It enforces gates (via a harness adapter) and injects workflow context.
  It also **exports its verification capabilities** into the catalog — predicates
  like `has-tests`, `blast-radius-within`, `doc-section-exists` that only the
  live map can evaluate. It runs a **co-located decision point**: Quipu pushes the
  relevant policy *bundle* and Yupana evaluates the live-tier leaves locally at the
  edit boundary — no per-tick round-trip (the OPA-bundle pattern). Runs as a
  **service**, out-of-band from the agent.
- **Quipu — the policy / workflow engine of record.** Holds the entities,
  references, and durable bitemporal verdicts; verifies the committed tier;
  enforces definition-time well-formedness via SHACL; keeps the audit trail. It
  owns **composition, storage, committed-tier evaluation, reproducibility, and
  the reactive reasoner** — generalizing SHACL (policy-as-schema is one policy
  kind) into a capability Quipu can also apply to its *own* data.
- **Bobbin — authoring intelligence + surfacing.** Powers conversational
  authoring (drafting in the vocabulary of *this* codebase) and surfaces
  governance state to agents.
- **Harness adapters — propel / constrain actuation.** Thin per-harness shims
  (Claude Code hooks; a fleet daemon; a generic governance-MCP + tool-call
  proxy). Enforcement strength is a **gradient**, tagged like a tier:
  `prevented` (harness exposes a pre-action gate) vs `observed` (detect,
  record, escalate). Never present `observed` as `prevented`.

## Entities in Quipu

The primitives are RDF classes in Quipu's governed ontology — so definition-time
verification is just SHACL over them, and the plane is *native* to Quipu, not
bolted on. Where Quipu already has a governance type, reuse it:

| Primitive | Quipu class | Notes |
|---|---|---|
| Policy | extends `Directive` | governed rule / intent, human provenance |
| Verdict / violation | `Observation` | bitemporal, signed, content-bound |
| Decision (human) | `DecisionRecord` | signed approve / reject / changes |
| Workflow / Step / Transition | new classes | the composition graph |
| Artifact / PlanStep | new classes | reference concrete targets / code symbols |
| Selector / predicate | new classes | evidence-source-bound catalog leaves |

References resolve to existing stack IRIs: a code-grounded Artifact or PlanStep
points at `bobbin:code/{repo}/{path}::{symbol}` — the same identity Yupana mints on
promotion — so intent, structure, and governance reconcile on one identifier.

Each class ships a SHACL shape; a definition that violates its shape is rejected
at write (definition-time verification). The governance ontology thus *extends*
the code-entities ontology (`CodeModule`, `CodeSymbol`, `Document`, `Section`,
`Bundle`) already in Quipu — governance facts and code facts live in one graph,
queryable together in a single SPARQL query.

## Integration contract (Yupana ↔ Quipu)

Four exchanges across the seam. Quipu signs bundles; Yupana signs verdicts; both
verify against the shared verifier registry.

```text
Quipu (engine of record)                         Yupana (co-located decision point)
  │  1. bundle pull ─────────────────────────────▶  cache live-evaluable policies
  │◀──── 2. signed live verdicts (at milestones) ──  evaluate live-tier leaves
  │◀──── 4. milestone promotion ──────────────────  on transition
  ▲                                                 3. yupana_plan_declare ◀── agent/harness
```

1. **Bundle pull (Quipu → Yupana).** On instance bind — and reactively on any
   policy change — Quipu compiles the *live-evaluable* subset of the workflow's
   policies into a **signed bundle**: the catalog leaves Yupana can evaluate, their
   gates + effect matrix, the relevant risk-map slice, and the registry entries.
   A re-pushed bundle invalidates the affected cached verdicts.
2. **Live verdict envelope (Yupana → Quipu).** Yupana emits signed verdicts —
   `{predicate-id, target-ref, outcome, evidence-hash, tier, freshness,
   confidence, verifier, sig}` — cached live in Yupana and **promoted to Quipu only
   at milestones** (the live / settled split).
3. **`yupana_plan_declare` (agent / harness → Yupana).** `{instance, targets:
   [symbol-ref], rationale}`; Yupana binds targets to symbols, reconciles against
   the inferred plan, and returns the plan + an immediate coverage verdict.
   Companion `yupana_plan_status` returns the plan and per-`PlanStep` verdicts.
4. **Milestone promotion (Yupana / plane → Quipu).** On a transition,
   `{instance, step, transition, verdicts[], committed-evidence, actor,
   valid-time}` is written as bitemporal facts, waking the reactive reasoner.

Tool surface: `yupana_policy_check`, `yupana_plan_declare`, `yupana_plan_status` join
Yupana's existing `yupana_*` MCP tools (and REST mirror); harness adapters call these
plus the pre-edit hook. The `quipu` Cargo feature already carries Yupana's
promotion path; policy bundles ride the same channel.

## Authoring

Two distinct "creations" — do not conflate:

- **Definition** — authoring a workflow / policy *template*. Governed,
  human-driven, written to Quipu. This is what "authoring" means below.
- **Instance** — a definition going live for a run. Automatic, runtime, held in
  Yupana's overlay + Quipu state. Not authored.

### Creation is composition over a catalog

Underneath authoring sits a **catalog** of atomic, pre-verified,
evidence-grounded **selectors and predicates** (see [Catalog](#catalog-initial)).
Authoring is composition three layers deep, bottoming out in the catalog:

```text
Workflow  = directed graph of Steps + guarded Transitions
  Step    = { produces, requires, gated-by: [Policy], actor: Role }
  Policy  = { targets: EntityType, claim: <composed catalog leaves> }   (external)
  Catalog leaf = selector | predicate, bound to an evidence source      (leaf)
```

Nobody writes raw predicates to build a workflow — they **assemble named
pieces**. That is what makes authoring both user-oriented and safe (cf. a
Semgrep rule registry, Terraform modules). The authoring *modalities* are
interchangeable front-ends over this same composition:

- **Conversational** — describe the process to a builder agent; it decomposes to
  a workflow + inferred policies, renders a draft, iterates. Powered by Bobbin
  retrieval.
- **Visual** — phases as nodes, gates as policy-chips; edit rules via a guided
  builder. Fits Quipu's embeddable web components.
- **Guided rule builder** — WHO / WHAT / WHEN (condition over facts) / THEN
  (effect). Never raw predicate syntax.
- **Mined** — the plane observes fleet behavior in Quipu's bitemporal log and
  *proposes* a codification of a recurring sequence; a human approves.

### The composition grammar

The grammar over the catalog is small: a **selector** yields a set, a
**predicate** tests an element, and **combinators** (`and` / `or` / `not`) plus
**quantifiers** (`all` / `any`) bind them. A composed policy — a fitness
function — reads:

```text
∀ s ∈ changed-public-symbols(run) : has-test(s)
      └─ selector (Yupana)             └─ predicate (Yupana)
targets Step[implement].exit-gate  ·  effect: block
```

Definitions are **templates**: references and actors are patterns
(`repo://{repo}/docs/design/{bead}.md`, role `reviewer`) that an Instance
resolves when it binds its parameters (bead, repo, agent). Authoring is over
roles and patterns; runtime resolves them.

### Dry-run against history

Because predicates are pure and reproducible, a draft policy is replayed over
past evidence *before* promotion — *"this would have blocked 3 of the last 20
merges"* — via Quipu's counterfactual `speculate()` over the bitemporal log. You
tune against reality, then promote; the same replay validates a **mined**
proposal. No governance rule goes live blind.

### Creation is itself verified

A definition is an Entity with the Claim *"well-formed"* — acyclic step graph,
every referenced policy exists in the catalog, every `produces` / `requires`
resolves. That claim is checked at write time by **SHACL** (definition-time
verification), the same spine one altitude up:

- **Definition-time** — *is this workflow well-formed?* (SHACL, at write, Quipu)
- **Run-time** — *is this workflow satisfied by reality?* (stored verdicts, at
  execution, Yupana + Quipu)

### Catalog growth and sovereignty

- **Catalog + escape hatch.** 90% is composition of named predicates; an expert
  may author a *raw* predicate (a graph query / Yupana capability) and **register
  it back** into the catalog as a new named primitive. A newly registered
  predicate is itself an entity that must be verified/reviewed before it enters
  the catalog — creation-is-verified, recursively.
- **Sovereignty.** Three producers — human composes, agent proposes, system mines
  — all emit a candidate that becomes governing only after definition-time
  verification **and** human promotion. The thing that governs the agents cannot
  be silently rewritten by an agent.

## Catalog (initial)

A starting set — deliberately small, meant to be refined. Every entry is
evidence-grounded and tier-tagged; the escape hatch grows it. `run` is the bound
Instance context.

### Selectors (evidence → set)

- `changed-files(run)` — files touched — *git / Yupana overlay*
- `changed-symbols(run)` — symbols touched — *Yupana*
- `changed-public-symbols(run)` — exported subset of the above — *Yupana*
- `blast-radius(change)` — dependents / frontier of a change — *Yupana*
- `callers(symbol)` / `callees(symbol)` — *Yupana*
- `sinks-reached(source)` — taint / dataflow reachable sinks — *Yupana (cpg)*
- `referencing-docs(symbol)` / `referenced-symbols(section)` — doc↔code — *Yupana*
- `required-artifacts(step)` / `prior-steps(step)` — *Quipu (definition)*

### Predicates (element → bool)

Code-grounded, live tier (*Yupana*):

- `has-test(symbol)` · `symbol-exists(ref)` · `doc-section-exists(ref)`
- `blast-radius-within(change, scope)` · `in-scope(action, scope)`
- `sanitized-before(source, sink-class)` — no unsanitized taint path

Record-grounded, committed tier (*Quipu*):

- `approval-exists(entity, by-role)` · `decision-record-exists(change)`
- `different-actor-than(step-a, step-b)` · `artifact-verified(artifact)`
- `within-duration(event, dur)` — depends on a temporal trigger

Attested by external owners:

- `tests-green(artifact)` · `build-succeeds(commit)` — *CI (signed)*
- `commit-exists(ref)` — *git* · `no-secrets(change)` — *scanner (signed)*

### Combinators

- boolean: `and` · `or` · `not`
- quantifiers over a selector: `all` · `any`

## Execution model

Per run, the plane drives the loop through the harness adapter:

```text
turn
  ├─ propel     inject "phase, done, next step, unmet claims"  (derived: definition × live verdicts)
  ├─ constrain  each action → policy gate → allow / ask / deny  (PreToolUse-equivalent)
  ├─ advance    observe completion → verify → advance state machine
  └─ gate       on attempted stop → exit-criteria verdicts met?
                  ├─ no  → block, return remaining steps → agent continues
                  └─ yes → transition / complete → promote run to Quipu (audit)
```

The injected guide content is **not authored separately** — it is *derived* from
`definition × current verdicts`. Verdicts are derived facts: when a referent
changes, the verdict goes `stale` and staleness propagates up the artifact →
step → workflow chain (Quipu's reactive reasoner + Yupana freshness).

### State: live vs settled

The state machine obeys the same tier split as the rest of the stack — so the
answer to *"do we write every step to Quipu?"* is **no**.

- **Live tier (Yupana).** The hot loop runs in Yupana's session runtime over the
  overlay: live-tier gates, the current-step pointer, plan progress, propulsion.
  Volatile, per-session, re-derived on demand — *never* written to Quipu
  tick-by-tick. It is ephemeral precisely because it is re-derivable, the same
  reason overlays are.
- **Settled tier (Quipu).** Only **transitions and committed verdicts** are
  promoted — step entered/exited, gate passed, run completed — each a sparse,
  audit-worthy bitemporal fact. Quipu's reactive reasoner drives the *settled*
  transitions (cross-agent handoffs, temporal triggers); Yupana drives the *live*
  ones.

The **Instance** is a Quipu entity (identity, workflow, parameters, last durable
milestone); Yupana holds a live *projection* of it. Agents coordinate through the
Quipu record, not shared memory — a handoff is: agent A promotes a milestone →
the reasoner enqueues the next step for agent B's role → agent B's Yupana
rehydrates from that milestone.

### Resumability

Resume is not a context reload — it is `last-promoted-milestone (Quipu) +
re-derived live verdicts (Yupana)`. Durable progress is never lost; the volatile
delta since the last milestone is cheap to re-derive by re-verifying the current
working tree. Because verdicts are content-bound, anything the world changed
while paused surfaces as **stale**, so resume is precise about what shifted —
which a silent checkpoint cannot be. State lives in the plane, not the context
window, so a different agent (or harness) can resume the same Instance.

## Intent map (tactical plan)

Yupana maps what the code *is*; the intent map adds what the agent *intends to do
to it*, and binds the two. It is the middle layer between the strategic workflow
(Quipu) and the raw code graph (Yupana) — and the layer that makes tactical resume
and intent-conformance possible.

A **`PlanStep`** entity references a code symbol (`add-sanitizer → parseInput`)
and belongs to the running Step / Instance. Only Yupana can bind intent to
structure live, holding both the code graph and the edit overlay at once.

### Sourcing the plan

Yupana **infers** a draft plan from the harness's plan / todo output and the
agent's opening edits, binding each item to symbols. The agent may **declare** a
plan to override or correct the inference (`yupana_plan_declare`), and a declared
plan wins. Inference is the low-friction default; declaration is the authority.
Requiring a plan before edits is itself a policy (`plan-declared-before-edit`).

### What it unlocks — all on the existing spine

- **Plan completeness** — `∀ s ∈ blast-radius(change) : s ∈ plan.targets`
  (`plan-covers-blast-radius`): flags symbols in the blast radius the plan omits.
  Verification of the *plan's* coherence with the code, not the code's.
- **Tactical resume** — verdicts on PlanSteps (done / pending / drifted) give
  resume granularity down to the individual edit.
- **Intent-conformance** — `∀ edit ∈ run : edit.target ∈ plan.targets` → else
  scope-creep. **Soft by default** (`warn` / `require-revision`, since
  re-planning is healthy); **hard only when composed** with a scope policy that
  already matters (drift *into* a protected module → `deny`).
- **Targeted context** — Yupana and Bobbin inject exactly the symbol, its blast
  radius, and its tests for the next planned edit instead of dumping context.

Plan drift is expected: the plan is a living artifact, re-verified as it changes.

## Human in the loop

Human review needs **no new machinery** — a human decision is just another
*attested evidence source*, the same shape as a CI attestation. A
`require-approval` gate (which risk-adaptive effect already produces for
high-risk / low-confidence) sits `unknown` until a human **signs** a decision,
which lands as a fact and satisfies the gate. *Obtaining* the decision is a Step
assigned to a human **Actor**; *checking* it is a policy. The decision is a
signed Artifact — `{instance, gate, approve | reject | changes, by, rationale,
valid-time}`.

### Asynchronous by default

A fleet of persistent agents cannot block on a synchronous human, so the flow
suspends and resumes:

```text
gate → require-approval → verdict unknown
  → Instance suspends (milestone promoted to Quipu — durable)
  → DecisionRequest dispatched to the human's queue (adapter / notification)
  → the agent is freed
  ⟳ … human responds whenever …
  → signed Decision lands as a fact
  → reactive reasoner re-evaluates → resume / branch / terminate,
     re-dispatching to whatever agent fills the role
```

The decision **wakes the workflow, not the agent** — human latency is decoupled
from agent lifetime, because state lives in the plane (this is
[Resumability](#resumability) earning its rent). The operator becomes an **async
approver over a governance inbox** (part of Quipu's web surface), each request
pre-loaded with the context the plane already computed — diff, blast radius,
failing verdict, risk score. The same DecisionRequest routes sync (harness-inline
when present) or async (inbox) by availability.

### Async-specific guarantees

- **Approvals are content-bound.** The human approves *hash H*; if the artifact
  changes while the request waits, the approval is **stale** and does not
  auto-apply — re-request. This kills approve-then-sneak-in-changes, and is just
  verdict content-binding reused.
- **Approvals are role-restricted signed attestations** under the verifier
  registry. An agent can no more forge or self-issue an approval than forge a
  Yupana verdict; separation-of-duties (`different-actor-than`) prevents
  self-approval. Sovereignty enforced cryptographically.
- **Waits are bounded by temporal triggers.** No decision within the SLA →
  escalate to another role / auto-reject / notify.

### The spectrum

One mechanism, different triggers:

- **Approval gate** — sign off before a transition.
- **Review with changes** — approve / reject / **request-changes**, the last
  branching the workflow back to a prior step.
- **Ambiguity escalation** — triggered by *low confidence* (not high risk): the
  plane asks a human a call it cannot make.
- **Human-authored step** — the work is inherently human; a policy verifies the
  produced artifact.
- **Break-glass override** — the human is the trust root and may overrule a
  `deny`, but *only* via a signed, recorded override fact — never a silent
  bypass. Gates are overridable by default; a small `non-overridable` set (e.g.
  legal / compliance) requires **N-of-M** human sign-off instead.

## Worked example

One `feature-sdlc` run, threading every primitive. Bead `KUE-42` — "add
rate-limiting to the login handler," repo `yupana`, actor `agent-fix-3`. Risk map:
`auth/**` = high.

**Definitions (Quipu).** `feature-sdlc` = Steps `design → implement → review →
merge`, guarded transitions between them. The `implement` exit gate binds:

```text
plan-present               plan-declared-before-edit          (entry gate)
plan-covers-blast-radius   ∀ s ∈ blast-radius(change) : s ∈ plan.targets
all-public-tested          ∀ s ∈ changed-public-symbols(run) : has-test(s)
tests-green(artifact)                                          [CI-attested]
```

`review` binds `require-approval` + `different-actor-than(implement)`.

**1 — Bind.** Instance created for `(feature-sdlc, bead=KUE-42, repo=yupana,
actor=agent-fix-3)`; `design-doc` pattern resolves to `docs/design/KUE-42.md`.
Milestone `instance started, step=design` → Quipu. Quipu pushes the `implement`
bundle to Yupana.

**2 — Design.** Agent writes the design doc; `design-artifact-approved` is an
async `require-approval` gate → **suspends**; a human signs a `DecisionRecord`
bound to the doc's hash → transition fires → `step=implement`.

**3 — Plan (intent map).** On entry, Yupana *infers* a plan from the harness todo
→ `targets = {login_handler, RateLimiter}`, satisfying `plan-present`. Agent
edits both, tries to finish.

**4 — Live gate (Yupana PDP).** `blast-radius(change)` at `tier=lsp` (high
confidence) → `{login_handler, RateLimiter, session_store, auth_middleware}`.
`plan-covers-blast-radius` → **unsatisfied** (two omitted). Risk = high (`auth`).
`effect(high, high, unsatisfied)` → **deny at the transition** + inject the two
missing symbols. (A docs change would `warn` — same policy, different effect.)

**5 — Adapt.** Agent declares a plan override (`yupana_plan_declare`) covering all
four, edits them, adds tests. Re-eval: `plan-covers-blast-radius` ✓,
`all-public-tested` ✓ (lsp). `tests-green` → **`unknown`** (no CI yet) → still
cannot complete; propulsion: "CI pending."

**6 — Commit → milestone (Quipu).** Agent commits; Yupana promotes committed
structural facts; CI **signs** `tests-green over hash H'`. Quipu's engine
evaluates the gate over committed evidence → all ✓, high confidence. Milestone
`step=implement completed` → reactive reasoner fires the `approve`-guarded
transition target... but `review` first needs a human, and
`different-actor-than` forbids `agent-fix-3`, so it enqueues `review` for
`agent-review-2`.

**7 — Review (HITL).** `agent-review-2` runs review checks; `require-approval`
**suspends** to the governance inbox. The human picks `request-changes` → the
guarded transition branches **back** to `implement` (bounded loop). Second pass:
approve → `merge`.

**8 — Resume (if `agent-fix-3` had died at step 5).** A fresh agent rehydrates
from the last milestone (`implement in progress`); Yupana re-derives live verdicts;
the diff shows `auth_middleware` still untested → precisely the remaining work.
No context reload.

Every step is a stored, bitemporal, signed fact — the run is fully auditable and
reproducible after the fact.

## Decisions and deferrals

Settled:

- **Home** — the policy / workflow **engine lives in Quipu** (widely applicable,
  usable by Quipu on its own data); Yupana is a co-located decision point; no new
  repo. Working name for the capability: *the loom*.
- **Enforcement floor** — the default floor is `observed` (never block real work
  because a harness can't gate), but a **high-risk action requires a `prevented`
  boundary** — a hard-gating harness or explicit pre-authorization. Risk sets the
  required floor (see [Risk and confidence](#risk-and-confidence)).
- **Task → workflow binding** — a **router** selects by task type / label /
  files-touched, with a **declared override** and a **repo-default** fallback.
- **Catalog authority** — an agent may *propose* a catalog predicate, but it
  becomes registered only on **human (keeper) promotion** after passing
  reproducibility review — creation-is-verified applied to the catalog itself.
- **Plan-before-edit** — code-editing steps carry a `plan-present` entry gate
  (satisfiable by inference); the coverage gate is meaningless without it.

Deferred (v1):

- **Compensation** — runs with irreversible side effects use **halt-and-escalate**
  rather than defined Saga-style undo.
- **Capability naming** — *the loom* is a working name, not final.

## Implementation phasing

Build order across the three repos. Each phase that wires a Cargo feature adds it
to the CI matrix in the same change (don't ship dark), and every verdict it
produces is tagged `tier` + `freshness` + `confidence`.

- **Phase 0 — Foundations (cross-cutting).** Verifier registry + signing keys
  (the root of trust); content-hash evidence binding; the human-authored risk-map
  format. Everything downstream depends on these.
- **Phase 1 — Quipu: ontology + engine.** Governance classes + SHACL shapes;
  definition-time validation; bitemporal verdict storage; committed-tier
  evaluation over the Datalog reasoner; retain `evidence-hash + predicate-id` for
  reproducibility. Extends the existing `shacl` + reasoner machinery.
- **Phase 2 — Yupana: policy module + live verdicts.** `policy/{model,eval}.rs`
  over `serve/{refs,impact,dataflow,callgraph}`; bundle pull + cache; signed live
  verdict emission; `yupana_policy_check` (MCP + REST); wire the pre-edit hook to
  `deny`. Initial Yupana-grounded catalog leaves. Behind a `policy` feature.
- **Phase 3 — Yupana: intent map.** `PlanStep` binding; inference + `yupana_plan_declare`
  / `yupana_plan_status`; `plan-present`, `plan-covers-blast-radius`,
  intent-conformance.
- **Phase 4 — Harness adapters.** Claude Code (`SessionStart` / `UserPromptSubmit`
  inject, `PreToolUse` gate, `PostToolUse` advance, `Stop` gate); the fleet
  daemon; the enforcement gradient (`prevented` / `observed`).
- **Phase 5 — Orchestration + HITL.** Guarded transitions, instance lifecycle,
  milestone promotion, reactive handoffs; `DecisionRecord` flow, the governance
  inbox (Quipu web components), temporal triggers, break-glass; risk-adaptive
  effects fully wired.
- **Phase 6 — Authoring.** Catalog composition + escape hatch; dry-run via
  `speculate`; conversational (Bobbin), visual (Quipu web), and mined front-ends.

**MVP** is the [worked example](#worked-example)'s spine: Phases 0–2 (minimal) +
one workflow (`feature-sdlc`), one live gate (`plan-covers-blast-radius` +
`has-test`), and the Claude Code adapter's pre-edit `deny` + `Stop` gate. That
proves propel + constrain + verify end-to-end on one harness.

**Scope boundary (v1):** a **single trust domain**. Enforceable multi-tenant
isolation is a known, decided-deferred gap in Quipu (`group_id` is provenance
only); multi-operator separation is a prerequisite to lift before the plane
governs across trust boundaries, not a v1 deliverable.
