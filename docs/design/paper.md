# Design: Yupana paper plan — ablation-gated admission for multi-signal retrieval

> **Implementation status (2026-08-17):** 🟡 **Planned, evidence partially in
> hand.** The measurement spine exists and reproduces (`just e2e f1` →
> `target/e2e-f1/f1-report.md`, written up in
> [`docs/briefing-retrieval-eval.md`](../briefing-retrieval-eval.md), measured
> 2026-08-15). Three things stand between that note and a submittable paper:
> a real corpus, external baselines, and a held-out calibration split. This
> document is the plan; nothing here is written yet.

## Status

- **Date:** 2026-08-17
- **Status:** Planning. The eval note is the only measured artifact.
- **Related:** [`briefing-retrieval-eval.md`](../briefing-retrieval-eval.md)
  (the numbers), [`work-scoped-governance.md`](../work-scoped-governance.md)
  (the scope ladder the briefing serves),
  [`vision.md`](../vision.md) (the three-tool stack; explicitly *not* this paper),
  quipu `docs/design/paper.md` (the house pattern this follows).

## 1. Intent and thesis

**The paper is about the gate, not about the F1.**

A retrieval system that fuses heterogeneous signals accumulates components
nobody can prove earn their place. Each one was added because it seemed
reasonable, each is defended by intuition, and removing any of them is
someone's judgment call against someone else's. Yupana makes that impossible
by construction: **a source is admitted only if its removal causes a
measured degradation, and the build fails if it doesn't.**

The thesis is that this discipline is worth adopting, and the evidence is
that running it produced four findings that contradict the design any
reasonable engineer would have shipped, including one component that was
*beating* the full system and had to be retired.

**This framing is deliberate and it is what makes the paper defensible at
this corpus size.** A claim of the form "our retrieval scores 0.954" invites
and deserves a sample-size rejection at 36 items. A claim of the form "we
forced every component to justify itself and here is what broke" is credible
at small N, because the load is carried by the explained mechanism rather
than by the margin.

### Working titles

- *Ablation-Gated Admission: Making Every Retrieval Signal Earn Its Place*
- *No Component Without a Measured Degradation*
- *Corroborated Retrieval for Work-Item Briefing, and What the Gate Removed*

First is the current favourite: it names the contribution rather than the system.

## 2. Contributions

- **C1 — Ablation-gated admission as a build discipline.** Every claimed
  source must be proven by a degradation only its removal causes; the gate is
  `core macro-F1 ≥ 0.9` **and every ablation strictly below full**. Ablations
  are feature removal on the **shipped binary** via `$YUPANA_BRIEF_ABLATE`,
  never a reimplementation, so the arm measured is the arm served. **This is
  the primary contribution.**
- **C2 — Corroboration voting over four heterogeneous sources.** Whole-label
  phrase match (2), distinctive-term probes (1 each), provenance
  co-occurrence (2), semantic similarity (2). Single-vote candidates are
  pruned *only when a corroborated candidate exists*, and the result is
  **never blanked** — the degenerate "return nothing, score perfectly on
  precision" failure is excluded by construction.
- **C3 — Four negative results the gate forced.** See §3. These are the
  paper's most interesting content and none of them would have surfaced under
  ordinary "did the number go up" evaluation.
- **C4 — A reproducible harness that scores the shipped binary** over a
  corpus organized as *problem classes*, each probe isolating one retrieval
  situation rather than averaging over an undifferentiated pool.

## 3. The four findings (the paper's spine)

| # | Finding | The measurement that forced it |
|---|---|---|
| **F1** | **No semantic veto.** A low semantic score is not evidence against a lexically corroborated candidate. | On the serving surface a term-corroborated **true** positive scored **0.352** while the term-corroborated **false** positive scored **0.384**. The model ranks the false one higher. No threshold separates them. Implemented, measured, removed. |
| **F2** | **Query-relative acceptance, not absolute floors.** | Self-similarity ranges ~0.2–0.75 per query, because entity text carries type/provenance boilerplate the bare query lacks. Two rounds of absolute cosine floors misfired in **both** directions. Shipped rule: support within 80% of the query's top non-self score, top ≥ 0.4, self excluded from setting the scale. |
| **F3** | **A component can beat the system that contains it.** Term probes retire on a semantic store. | The strictly-below gate flagged `−term-probes` *outscoring* full: on a semantic store the collisions term probes admit cost more than the recall they add. They now run only in the lexical configuration. **This finding exists only because the gate tests for improvement-on-removal, which conventional ablation reporting does not.** |
| **F4** | **Hub entities poison co-occurrence.** | Before the degree cap the hub-entity trap scored **P = 1/6**: five unrelated items retrieved because their commits touched the same justfile-shaped file. Sharing a hub evidences shared residence, not shared work. Entities above `HUB_DEGREE_CAP` distinct items now contribute nothing. |

A fifth, smaller item worth one honest paragraph: the gate also caught a
**ground-truth error**. The provenance-only probe shared a "make … faster"
sentence shape with the paraphrase probe; the model called them similar, the
eval called it a false positive, and **the model was right** — both labels
described performance work. The corpus label was fixed, not the retrieval.
Reporting this is a credibility asset, not an embarrassment.

## 4. Measured state, as of 2026-08-15

Per arm, macro-averaged:

| arm | core F1 | hard F1 | overall F1 |
|---|---|---|---|
| full (semantic) | 0.954 | 0.952 | **0.954** |
| − provenance | 0.667 | 0.800 | 0.703 |
| − /context pipeline | 0.438 | 0.222 | 0.379 |
| − semantic (= lexical config) | 0.979 | 0.489 | 0.845 |

The lexical configuration is a **valid deployment**, not a crippled arm —
stores without an embedding provider run it — and is separately
ablation-proven: term probes carry the term-overlap class, and the
corroboration threshold carries both false-positive classes (0.87 → 0.94
macro when voting landed).

Remaining sub-1.0 scores are **structural, not noise**: the crowded cluster
measures the briefing's deliberate 5-item cap, and the mixed composite
measures the F3 term-probe trade.

## 5. Research questions

- **RQ1 — Does every shipped source earn its place?** Answered, affirmatively,
  by the ablation table. *Evidence: in hand.*
- **RQ2 — Does corroboration voting beat the obvious alternatives?**
  🔴 **Unanswered. This is the biggest hole.** The ablations are internal
  only; nothing establishes that voting beats BM25 alone, embedding top-k
  alone, or reciprocal rank fusion. *Evidence: does not exist.*
- **RQ3 — Is a low semantic score evidence against a corroborated candidate?**
  Answered, negatively (F1). *Evidence: in hand, one surface, one model.*
- **RQ4 — Do absolute similarity floors work on this surface?** Answered,
  negatively (F2). *Evidence: in hand.*
- **RQ5 — Does the discipline survive contact with a real corpus?**
  🔴 **Unanswered.** *Evidence: does not exist.*

## 6. Build order

Strictly ordered. Each step is worthless before the one above it.

1. **🔴 Real corpus.** The eval note's own stated next step: re-run the
   harness over a snapshot of real tracker history, and **derive relevance
   labels from the verdict spool — items whose briefed ground the agent then
   edited are confirmed positives.** That label-derivation trick is itself
   worth a paragraph, because it produces ground truth from observed agent
   behaviour rather than from human annotation. Highest value work in the plan.
2. **🔴 External baselines** (RQ2). At minimum BM25, embedding top-k, and RRF
   over the same candidate pool. Without these a reviewer cannot tell whether
   voting beats the obvious thing.
3. **🔴 Held-out calibration split.** Thresholds are currently calibrated on
   the corpus they are scored on. Fix it, or the honest disclosure in
   §"Threats" becomes the first thing a reviewer attacks. Fixing is better.
4. **🟡 Second embedding model.** The no-veto finding (F1) is currently a
   claim about all-MiniLM-L6-v2 on one serving surface. A second model either
   generalizes it or bounds it. Either outcome is publishable; the current
   state is neither.
5. **🟡 Related work.** None written. See §8.
6. **🟢 Draft.** Only after 1–3.

## 7. Paper outline

1. Introduction — the unjustified-component problem in multi-signal retrieval
2. The briefing task and why it is a retrieval problem
3. Corroboration voting (C2)
4. **Ablation-gated admission (C1)** — the gate, and why strictly-below matters
5. Harness and corpus
6. Results, including baselines (RQ2)
7. **What the gate forced** (F1–F4) — the heart of the paper
8. Threats to validity
9. Related work
10. Conclusion

§7 should be longer than §6. That ordering is the whole editorial argument.

## 8. Related work (to be written)

⚠️ **The retrieval task itself is well-studied and the paper must say so
early.** Surfacing prior work items similar to a new one is duplicate bug
report detection under another name, with a literature going back two
decades. **Novelty is claimed for the gate and the findings, not the task.**
Pretending otherwise invites the worst kind of review.

Areas to cover: duplicate/similar issue retrieval; hybrid lexical+dense
retrieval and rank fusion (RRF); ensemble and voting retrieval; continuous
evaluation and CI discipline for ML systems; code search. The RRF literature
matters most, because RRF is the obvious alternative to corroboration voting
and is the baseline a reviewer will ask for by name.

## 9. Scope boundaries (honest)

- **Not** a code-search paper and **not** the three-tool stack paper. Yupana's
  structural-extraction mission ([`vision.md`](../vision.md)) is out of scope;
  this paper is about one retrieval surface and one discipline.
- One serving surface, one embedding model, one tracker shape.
- The corpus, until step 1 lands, is **synthetic with designed rather than
  sampled collisions**. Say so in the abstract, not just in §8.
- F1 (no semantic veto) is a claim about *this surface*, expressly **not**
  about semantic models in general.

## 10. Patent interaction

The mechanisms here — corroborated retrieval, hub-degree cap, ablation-gated
admission, assignment-time briefing, observed-scope enforcement — were first
disclosed 2026-08-15 and are within **provisional D, `64/135,436`**
(see `resume/provisionals.md` and quipu `docs/patents/disclosure-timeline.md`).

**Publishing costs nothing.** D is planned to lapse on 2027-08-17, US
priority is already locked at the 2026-08-17 filing date, and non-US rights
on this subject matter were foreclosed by the public repository disclosures
regardless. Publication additionally converts these mechanisms into prior art
that no one else can patent, which is the outcome actually wanted here.
