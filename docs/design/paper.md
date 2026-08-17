# Design: Yupana paper plan — evidence-local constraint enforcement for agents

> **Implementation status (2026-08-17):** 🟡 **Planned.** Substantial evidence
> already exists in the repo and in production traces; the gap is assembly, not
> discovery. **Supersedes rev 1 of this document**, which framed the paper around
> the briefing-retrieval eval. That was the wrong axis: retrieval quality is
> bobbin's territory, and it is not what yupana is for.

## Status

- **Date:** 2026-08-17 (rev 2)
- **Status:** Planning. Evidence largely in hand; nothing drafted.
- **Related:** [`../book/src/design/sarc-conformance.md`](../book/src/design/sarc-conformance.md)
  (the spine), [`../book/src/design/policy-edit-hooks.md`](../book/src/design/policy-edit-hooks.md)
  (evidence locality), [`../book/src/reference/policy-guard.md`](../book/src/reference/policy-guard.md)
  (latency + fail-open + the measured degradation),
  [`../book/src/reference/enforcement-trace.md`](../book/src/reference/enforcement-trace.md)
  (the record), [`../book/src/concepts/game-state.md`](../book/src/concepts/game-state.md)
  (the second domain), [`../work-scoped-governance.md`](../work-scoped-governance.md)
  (incidents + eval discipline), [`../patents/filing-record.md`](../patents/filing-record.md).

## 1. Intent and thesis

**Agent policy should be evaluated at the earliest boundary where its evidence is
already hot — and the hard part is not deciding, it is refusing to let silence
read as success.**

An autonomous agent that produces an invalid artifact usually finds out late: the
linter fails, CI goes red, the game engine rejects the order. Late feedback is
expensive in tokens, in wall-clock, and in the agent's ability to attribute the
failure to the choice that caused it. The obvious fix — check earlier — runs
straight into a harder problem, which is that an early gate that cannot be
trusted is worse than no gate. Every failure that motivated this work was a
control that *appeared* to be enforcing and was not.

So the paper is about placement and about honesty, in that order.

**Placement** is answered by **evidence locality**: evaluate a policy where its
evidence is already hot. Governed-fact policies reason over the committed
graph, so **quipu** evaluates them at its pre-commit gate. Structural-evidence
policies reason over the call graph, reachability, blast radius, or the board, so
**yupana** evaluates them at the action boundary against a hot, one-directional
projection of quipu's canonical policies. **A policy is never *defined* in
yupana.** Authoring, SHACL validation, verdict signing and the root of trust stay
in quipu; yupana holds a read cache and says so in every verdict.

**Honesty** is answered by a taxonomy of non-answers, and that is the paper's
real contribution. See §4.

### Working titles

- *Evidence-Local Constraint Enforcement for Autonomous Agents*
- *Silence Is Not Success: Conformance Engineering for Agent Governance*
- *Where Should the Gate Live? Evidence Locality in Agent Policy Enforcement*

## 2. The frame: the action-side counterpart to the quipu paper

**This is the single most important structural decision, and it is constrained by
a paper that is already submitted.** The work implements an externally published
framework rather than inventing a vocabulary:

> **[SARC]** Besanson, G. (2026). *SARC: A Governance-by-Architecture Framework
> for Agentic AI Systems: Compiling Regulatory Obligations into Runtime
> Constraints.* Working paper, Universidad Torcuato Di Tella.
> [arXiv:2605.07728v1](https://arxiv.org/abs/2605.07728) [cs.SE].

SARC proposes constraints as first-class specification objects,
`c = ⟨src, class, pred, verif, resp⟩` plus a declared operating point θ, compiled
into four enforcement points in the agent loop — **Pre-Action Gate**,
**Action-Time Monitor**, **Post-Action Auditor**, **Escalation Router** — under
eight runtime invariants I1–I8 whose joint effect is specification-trace
conformance.

yupana × quipu implements **phases 1–6**, closing G1–G5, G7, G8 and G10.
[`sarc-conformance.md`](../book/src/design/sarc-conformance.md) tracks every gap
against a numbered invariant.

### 🔴 The quipu paper already claimed the store. Do not re-claim it.

**The Quipu paper is submitted (`arXiv:submit/7961151`) and its position is
fixed:**

> *"SARC stops at the loop … The knowledge the agents act on sits in an ungoverned
> store underneath. **Our position is that the store is the right compilation
> target** for exactly the machinery SARC specifies."*

It relocates SARC into the store — Σ as facts, the gate as the write path,
verdicts as signed bitemporal facts — and **measures agreement with SARC's own
reference checker, verdict-for-verdict.** That evaluation is spent. This paper
cannot repeat it and must not imply it.

**What it leaves open is exactly this paper.** Quipu's related-work section names
the gap itself, describing point-of-action gating as *"the loop-side counterpart
of our write gate; \sys is the specialisation to the store."*

So the thesis is the complement, and it is sharper than a general conformance
claim:

> **Quipu compiles SARC into the store. Yupana compiles it into the action.**
> The Pre-Action Gate cannot live in the store, because its evidence — the
> proposed edit, the buffer, the call graph, the speculative post-order board —
> **never reaches the store by definition.** Anything that reached the store
> already committed, and a gate that fires after the commit is an auditor.

That framing does three things at once: it makes the paper complementary rather
than competing, it gives a principled reason the split exists rather than an
architectural preference, and it makes **evidence locality (C1) the load-bearing
idea** instead of a design note.

**Cite the Quipu paper as the companion, and be explicit that this is the
action-side half of one program.**

**Why the SARC frame is worth keeping.** A paper claiming "we built a good agent
guard" competes with everyone's guard and has no shared yardstick. A paper
reporting **what it cost to place a published framework's Pre-Action Gate, and
which invariants resisted implementation there** has an external standard, an
obvious related-work anchor, and a contribution that survives disagreement about
our design taste. It makes the honest gaps into results rather than apologies.

## 3. Contributions

- **C1 — Evidence locality as a placement rule for SARC's constraint classes.**
  SARC says a constraint has a class and must be placed at a compatible point; it
  does not say how to choose. We answer: place it where its evidence is already
  resident. The governed/structural split falls out, and with it the
  one-directional projection (quipu canonical → yupana cache, never the reverse,
  because a diverged cache would let yupana allow what quipu would deny).
- **C2 — One constraint engine over two unrelated evidence domains.** Code edits
  (tree-sitter selectors, `boundary:"action"`) and *Alpha Centauri* game orders
  (`graph-pattern` selectors, `boundary:"order"`, `engine-state` tier) run the
  same engine and share `match_type`, `gate`, `effect`, `claim`, `targets`,
  `label`. **`MatchType` is literally the same Rust type**, so `must-match` cannot
  come to mean one thing over an AST and another over a board. SARC asserts
  domain-neutrality; this demonstrates it on a domain with no source files at all.
- **C3 — Measured degradation of the enforcement point itself.** See §5. To our
  knowledge nobody reports how often their agent guard silently stopped guarding.
- **C4 — A taxonomy of non-answers.** See §4. **The paper's core.**
- **C5 — An evaluation discipline that makes "a control that could not fail"
  unrepresentable.** See §6.

## 4. The core: silence must never read as success

Every distinction below is a category that conventional guards collapse, and each
one was forced by an observed failure rather than derived from principle. **This
is the section to write first and the one to defend.**

| Distinction | Collapsing it produces |
|---|---|
| `served_from_cache` ≠ `fail_open` | "Collapsing them is what made a soak count unguarded edits as clean ones." One is the guard enforcing last-known policy; the other is the guard not running. |
| `vacuous` ≠ pass | A selector that has rotted away from the adapter's vocabulary matches zero nodes and produces zero violations, **which reads exactly like a clean board**. |
| `unevaluated` ≠ skipped | `selector_lang: "sparql"` is refused at compile time and **named in the report**. "A policy silently not evaluated reports a clean board it never looked at." |
| `unknown` ≠ `unsatisfied` | No evidence yet is not evidence of compliance. `unknown` has no confidence; it is not a soft `unsatisfied`. |
| empty board **refused** ≠ zero findings | "Zero findings over a board that was never loaded is a green light over a dead backend." Both `guard` and `whatif` return 409 rather than a clean report. |
| STALE-with-AGE ≠ stale ≠ fresh | "'Stale' alone cannot distinguish a slow quipu from a week-old catalogue, and those warrant opposite reactions." Past TTL the cache is **refused**: "a retired rule that keeps firing from disk is worse than no rule, because it is unfalsifiable from the outside." |
| `pre_existing` breach blames no order | A violation that already held before the action attributes to no actor. |
| `attribution_conflict`, emitted only when true | A declared chain whose last link disagrees with the executor is **the observable signature of a laundered chain**. "A record that resolved it by precedence would delete the only evidence of it." A `false` on every line trains readers to skip the field. |
| `auth` **deliberately absent** | Effective authority is the intersection of every link's grant; those grants live in quipu and cannot be read inside a 100 ms budget. "A locally-guessed value would put a number in the field the grant store never agreed to." |
| omitted ≠ empty | "An undeclared chain is not a one-link chain." Every attribution element is omitted when undeclared rather than defaulted. |

Two further instances of the same idea outside the guard: dataset coverage
reported as `empty | none | partial | full` rather than a bare count, and the
installer proving each refusal gate with deliberately invalid probes that each
omit exactly one required property, so the arms discriminate.

## 5. Measured results

### 5.1 The guard degrades exactly when it matters

Measured **2026-08-04** on the production fleet, before the durable cache landed:

- **5.2% of all pre-edit invocations** failed open on projection timeouts alone.
- **19% of one day's** invocations.

The mechanism is self-interference and it is the most interesting result in the
paper: the governed plane projects its rule catalogue from quipu over HTTP, the
hook is a short-lived process **per edit**, and **quipu serves `/query`
effectively one at a time**. Heavy graph work is therefore exactly what starves
the guard that reads the graph. **The guard was least available precisely when it
mattered most.**

The fix is a durable projection cache under the staleness contract in §4. Report
the post-fix rate; if it has not been re-measured since, **re-measuring it is
build step 1.**

### 5.2 Latency

The hook is synchronous in the agent's loop. Budget is `deadline_ms`, default
**100 ms**; on expiry the analysis is abandoned and the edit **allowed**. The
guard already costs **157–322 ms per edit** before any scope resolution, so on
large trees it exceeds budget and fails open by design until the resident daemon
lands. **State this plainly**: the paper's central claim is early feedback, and
the current implementation buys it only where the graph is already hot.

### 5.3 🎯 The NeuralAmplifier evaluation — the second domain, measured

**This is the highest-value experiment in the plan**, because every number in
§5.1 and §5.2 is code-side. A game-side measurement is what turns C2 from
*asserted* (same Rust type) into *demonstrated* (same engine, two domains, both
measured).

**Measure the guard, not the game.** "The LLM beat the built-in AI" is a
capability result about the brain — provisional C territory, decision delegation
— and it does not evidence that a pre-action gate works. It is also
methodologically fragile: SMAC's built-in AI is weak and cheats at higher
difficulties, outcomes are high-variance, and faction, map and seed all confound.
A reviewer will correctly shred "we won 7 of 10". Keep it as a secondary,
clearly-labelled result; do **not** let the paper rest on it.

**The right experiment is the one the engine already makes possible.** Thinker
replays a save **deterministically** — this is the method `evals/runs/na-s4e`
used to prove `facility_maint_total > 0`, running two builds over one identical
save and pairing rows on a strict state fingerprint (surface, faction, base id,
name, turn, `call_seq`, plus four independent aggregates), comparing only pairs
agreeing on all of it. **Reuse that method exactly.** It gives a controlled A/B
that a tournament never could.

Three arms over one pinned save:

| arm | `work_item_scope` / policy mode | what it isolates |
|---|---|---|
| **off** | guard inert | baseline: orders go to the engine, the engine rejects the bad ones — **late feedback** |
| **advise** | violations reported, orders still submitted | the guard's *judgement* without its *effect*; yields the false-positive rate |
| **enforce** | violations blocked pre-submission | feedback moved to the order boundary |

Primary metrics, all of which speak to this paper's thesis rather than to the
brain's skill:

- **Engine-rejection rate per order.** The direct measure of the user-facing
  claim: feedback arriving before the engine says no. This is the headline number.
- **Detection lag.** Turns between a policy breach becoming true and something
  surfacing it: order boundary (turn N) versus engine or downstream consequence
  (turn N+k).
- **Guard cost per decision**, wall-clock and tokens. §5.2's latency claim,
  re-measured in a domain with no source files.
- **Taxonomy counts in the wild** — `vacuous`, `unevaluated`, `pre_existing`,
  `served_from_cache` vs `fail_open`. §4 currently argues these categories are
  necessary; observed frequencies in a second domain make the case empirically.
- **False-positive rate from the advise arm**, feeding the §6 promotion ladder.
  "A false deny removes a legal, possibly correct move," so this number gates
  whether `enforce` is defensible at all.

⚠️ **Host feasibility — the constraint to resolve first.** The proven path is
`thinker.dll` plus the retail game, which is Windows. GLSMAC is cross-platform
and has `just glsmac build`, but NeuralAmplifier marks it long-term and the whole
project pre-alpha, and yupana's game-state harness sits behind the `game-state`
Cargo feature. **Running this on the Mac host means either GLSMAC (less mature
path) or Windows/Wine for Thinker.** Settle that before designing the arms; it
determines whether the deterministic-replay method is available at all, and that
method is the reason this experiment is worth running.

🟢 Note that `just eval score` **needs no model, no game and no tokens** — prompts
and answers are committed, so any number the paper quotes can be recomputed by a
skeptical reader on a fresh clone. Say so in the paper; it is a reproducibility
asset most systems papers cannot offer.

### 5.4 Conformance

Phases 1–6 landed; G1–G5, G7, G8, G10 closed. **Honestly open, and each is a
result:** there is **no Action-Time Monitor at all** (G6, untouched); the
escalation queue has no server, so SARC §5.3's `W_q < τ_rev` is **unmeasured**; θ
is calibratable but **not calibrated**, because replay counts blocks and cannot
label false positives; and no trust predicate evaluates imported content — the
boundary is declared and reported, not closed.

## 6. The evaluation discipline (C5)

Every control in the motivating incidents failed the same way: **it could not
fail, and was believed to be passing.** Five properties, commonly conflated,
tested separately:

1. **Liveness** — the guard is actually invoked through the real harness path.
   Two-sided: a control (direct invocation fires) plus identical input through the
   real tool, plus a negative. **"Absence of output is never evidence — the
   control is what makes the test valid."** This is the property nobody tests and
   the one most likely to be silently false.
2. **Both-outcomes** — every rule needs a RED case and a GREEN case. The GREEN
   case is the one that gets skipped, and it is where false positives live. A rule
   lacking one is flagged **FP-unverified**.
3. **Non-vacuity** — deliberately break the rule and confirm the eval goes red. "A
   green eval that cannot go red is asserting nothing."
4. **Recoverability** — break the handler and assert the action still proceeds;
   assert every refusal **names the command that satisfies it**, not merely
   "denied".
5. **Replay** — run a candidate rule over recorded action records: would this have
   blocked anything that actually happened? A measured FP rate that gates
   promotion, turning advise→block from a waiting game into a computation.

**Per-rule promotion ladder:** enforcement tier is a function of eval evidence
rather than a judgement call, and the tier lives in the graph, so promotion is a
data write.

## 7. Motivation: three incidents, none of them authorization failures

1. **Unattributable action.** Several agents reached a host through one shared
   credential. Three binaries were replaced inside 25 seconds and no log could say
   by whom. The investigation nearly invented a second actor that did not exist,
   because a *method* discriminator was misread as an *actor* discriminator.
2. **Unattributable message.** An agent injected text into another agent's input
   box through a raw terminal-multiplexer send. No record existed anywhere. The
   sender turned out to be the coordinator that had also filed the incident
   report — its own message, unrecognised 40 minutes later.
3. **A control that could not fail.** A CI gate configured to continue on error
   ran for 126 seconds, had never once failed a build, and its existence was read
   as coverage.

**"The expensive failures were failures of provenance and of verification, not of
permission."** That sentence is the paper's opening.

## 8. Paper outline

1. Introduction — late feedback, and why checking earlier is the easy half
2. Three incidents (§7)
3. SARC in brief, and what conformance demands (§2)
4. Evidence locality and the governed/structural split (C1)
5. Architecture: canonical policy in quipu, one-directional projection, action-boundary evaluation
6. **Silence is not success** (C4) — *the longest section*
7. Two domains, one engine: code edits and game orders (C2)
8. Measured degradation and latency (C3, §5)
9. Evaluation discipline and the promotion ladder (C5)
10. Conformance report: what landed, what resisted (§5.3)
11. What this cannot reach (§9)
12. Related work
13. Conclusion

## 9. Scope boundaries (honest)

Reproduce the **"what this cannot reach"** table verbatim, because "an
enforcement claim that is not true is worse than no enforcement: it stops people
looking": CI pipelines, cron and scheduled jobs, the far side of a remote shell,
a sibling session's VCS index, and a hostile or buggy agent that can edit its own
guard. Origin-side actor stamping is **complementary to** destination-side
credentials, never a substitute.

Also in scope to state plainly:

- The governance plane (workflows, risk × confidence, `effect(risk, confidence)`)
  is **design, not built**. Present it as future work or cut it. Do not imply it runs.
- Replay measures false positives **only on traffic that happened**, and measures
  **no false negatives at all**.
- The game guard sees an **approximated post-order board** — orders carry declared
  effects and yupana applies exactly those. The engine remains sole authority on
  legality; the guard can only subtract from moves already legal.
- v1 is a single trust domain.

## 10. Build order

0. **🔴 Settle the host question** for the NeuralAmplifier arm (§5.3): GLSMAC on
   macOS, or Windows/Wine for Thinker. Everything in step 3 depends on it, and the
   deterministic-replay method is only available on the Thinker path.
1. **🔴 Re-measure the fail-open rate** post-cache. The 5.2%/19% figures predate
   the fix and the paper needs the after number. Highest value, lowest effort.
2. **🔴 Measure guard latency distribution** at the action boundary, with and
   without the resident daemon. The early-feedback claim is quantitative and
   currently rests on a 157–322 ms range and a 100 ms budget.
3. **🔴 Run the three-arm game eval** (§5.3) over one pinned save, pairing on the
   na-s4e state fingerprint. This is what makes C2 a demonstrated result.
4. **🟡 A worked cross-domain example** — one constraint expressed over code and
   the analogous one over a board, side by side, showing the shared type. Cheap,
   and it sells C2 in prose even before step 3 lands.
4. **🟡 Related work.** Much of it is already assembled in the Quipu paper's
   `sections/09-related.tex` — **reuse that bibliography rather than rebuilding
   it.** Anchors: SARC; the Quipu paper as companion; **SARC-DQ**, which gates
   evidence quality *at the point of action* and is therefore the closest
   neighbour to this paper specifically, not to Quipu's; **Green SARC** as
   evidence the frame specialises per domain; **Tardygrada**, which converges
   independently on three-valued verdicts and weakest-link aggregation and so
   corroborates §4's `unknown` ≠ `unsatisfied` and the least-confident-leaf rule.
   Still to add: **Monitor-Guided Decoding** (constraining an LM to type-valid,
   arity-correct output is the nearest neighbour to the whole early-feedback
   thesis), agent sandboxing and guardrails, policy-as-code and **admission
   control** (Kubernetes admission webhooks are a very close structural analogue,
   including the fail-open/fail-closed argument and the timeout-means-admit
   default), and runtime verification with monitor placement.
5. **🟢 Draft.** After 1–2.

## 11. What this paper is not

Not the retrieval paper. [`../briefing-retrieval-eval.md`](../briefing-retrieval-eval.md)
measures corroborated similar-work retrieval and is good work, but retrieval
quality belongs to bobbin, and folding it in would produce a paper with two
theses and no spine. If it is published, publish it separately.

Not the three-tool stack paper ([`../vision.md`](../vision.md)). Yupana's
structural-extraction mandate is background here, not subject.

## 12. Patent interaction

Mechanisms in this paper sit in **provisional D, `64/135,436`**, filed 2026-08-17.
Full detail in [`../patents/filing-record.md`](../patents/filing-record.md).
**Publishing costs nothing:** US priority is locked at the filing date, D is
planned to lapse 2027-08-17, and non-US rights were foreclosed by the public
repository disclosures regardless. **No patent reason to delay, redact, or
embargo.**
