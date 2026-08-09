# Work-scoped agent governance: one scope, three consumers

**Status:** design. Nothing here is implemented yet.
**Scope:** how yupana, Quipu and a work-item tracker combine to govern, trace, and
contextualise what an autonomous agent does.

## The problem this solves

A fleet of autonomous coding agents shares a set of repositories and hosts. Today the
policy surface is *content-shaped*: rules match on the text an edit introduces ("this
string must not enter a public repo"). That is a real and useful control, but it is
blind to the two questions that actually matter when an agent acts:

- **Was this agent supposed to be touching this at all?**
- **Which piece of work caused this to happen?**

Three failure classes motivate the design. Notably, **none of them is an authorization
failure** — in each case the actor was permitted to do what it did:

1. **Unattributable action.** Several agents reach a host through one shared credential.
   Three binaries were replaced there inside 25 seconds and no log could say by whom.
   The subsequent investigation cost hours and nearly invented a second actor that did
   not exist, because a *method* discriminator (which script left a backup file) was
   misread as an *actor* discriminator.
2. **Unattributable message.** An agent injected text into another agent's input box
   through a raw terminal-multiplexer send. No record existed anywhere. The
   investigation eventually found the sender was the coordinator that had also filed the
   incident report — its own message, unrecognised 40 minutes later.
3. **A control that could not fail.** A CI gate configured to continue on error ran for
   126 seconds and had never once failed a build, while its existence was read as
   coverage.

The lesson the three share: **the expensive failures were failures of provenance and of
verification, not of permission.** So the trace is the product; blocking is the follow-on.

## The model

**The work item is the capability.** An agent is always working *some* tracked item. That
item, projected into the graph, declares the entities it may touch:

```turtle
item:1234  a  gov:WorkItem ;
    gov:touchesRepo     repo:service-a ;
    gov:touchesHost     host:build-01 ;
    gov:touchesService  svc:metrics ;
    gov:grantsAction    "edit", "deploy" .    # not "restart", not "destructive-git"
```

Scope carries its **provenance**, because it will not be hand-authored for hundreds of
open items:

| provenance | source | trust |
|---|---|---|
| `declared` | explicit edges on the item | highest |
| `derived` | labels, parent epic, named repos/hosts, yupana's structural graph | medium |
| `observed` | what previous sessions on this item actually touched | grows with use |

**Unknown scope advises; it never blocks.** A guard that strands an operator with no way
forward is a worse outage than the thing it prevents — we have one on record that refused
an operator for having *already* done what it demanded, unrecoverable without a restart.

**Scope governs mutation, not observation.** Investigative work legitimately reads far
outside any item's scope; advisories on reads only teach people to ignore advisories.

## One resolution, three consumers

This is the part worth building. Resolving "what is this agent working on, and what does
that touch" is a single computation, and **three different subsystems want the answer**:

```text
                  ┌──────────────────────────────┐
   work item ───► │  scope resolution            │
   + agent id     │  (item → entities, + prov.)  │
                  └──────────┬───────────────────┘
                             │
        ┌────────────────────┼────────────────────┐
        ▼                    ▼                    ▼
   1. POLICY            2. TRACE             3. CONTEXT
   may this action      record what          pre-fetch what the
   proceed?             happened, tagged     agent will need
                        with the work item
```

### 1. Policy

The decision point. Deliberately honest about reach: an agent-side hook sits in front of
the agent's *own* tool calls and nothing else. See [Non-goals](#what-this-cannot-reach).

### 2. Trace

Every consulted action emits one record:

```text
(actor, work_item, verb, target, target_class, blast_radius,
 verdict, rule_id, scope_provenance, outcome, ts)
```

**Records and rules share one vocabulary.** A policy is then a saved query over records
plus an effect, which buys three properties at once:

- **derive** a rule by generalising observed records
- **test** a rule by replaying records — a measured false-positive rate on real traffic
  *before* anyone is affected
- **explain** a refusal by pointing at the records that justified the rule

yupana already ships a fail-silent JSONL telemetry spool with per-event agent and tenant
labels. It is currently *edit-shaped*: it records a file extension where a target entity
belongs, carries no work item, and its command events hold a raw command string that no
policy can be written against. Closing those three gaps is the whole of phase 1. Note the
spool is deliberately **not** served by the resident daemon, so its numbers do not vanish
when the daemon does — the moment they matter most.

### 3. Context (the symmetry worth exploiting)

If the graph can predict what an agent may *access*, it can predict what that agent will
*need to read* — and those are nearly the same query.

Semantic-search context injection already exists as a prompt-submit hook, but it is
**work-item-blind**: it sees the prompt text and nothing about what the agent is actually
assigned to. The scope subgraph is, almost exactly, a context bundle definition — the same
repos, services and symbols. So:

- **scope → policy**: these are the entities you may touch
- **scope → context**: these are the entities you should have already read

And the `observed` provenance tier improves *both* from the same signal. What previous
sessions on this item actually touched is simultaneously the policy prior and the
retrieval prior. One feedback loop, two payoffs.

This also inverts a chronic problem: agents rediscover the same context repeatedly because
retrieval is per-prompt and stateless. Keyed on the work item, retrieval becomes
cumulative — the second session on an item starts where the first finished.

## Architecture

**Ownership seam.** The tracker is the authority on `(agent → work item)`. yupana is the
decision engine. The resident daemon is a cache with **push-invalidation from the
authority** — the tracker pushes the binding when it changes (dispatch, re-anchor), rather
than yupana polling or shelling out per action.

**Why the daemon is load-bearing rather than an optimisation.** The guard already costs
157–322 ms per edit *before* any scope resolution. Adding a subprocess plus a graph
round-trip approaches a second on every shell call. The daemon holds the projected policy
subgraph, the code graph, and a rolling audit window; the hook becomes a thin client.

**Enforcement depends on the daemon only in a late phase.** Observe-only ships without it.
When that dependency does arrive, a daemon-down fail-open must be **loud**: silently
disabling policy while everyone believes it is on is precisely failure class 3 above.

## Evals: the gate for observe → enforce

Every control in the motivating list failed the same way: **it could not fail, and was
believed to be passing.** The eval harness exists to make that state unrepresentable, and
its discipline is borrowed from a both-outcomes rule-testing pattern already proven
elsewhere in the estate rather than invented here.

Five properties, commonly conflated, each tested separately:

1. **Liveness — the guard is actually invoked** through the real harness path. This is the
   property nobody tests and the one most likely to be silently false. Test shape is
   two-sided: a control (direct invocation fires) plus the identical input through the
   real tool (must also fire), plus a negative. **Absence of output is never evidence —
   the control is what makes the test valid.**
2. **Both-outcomes** — every rule needs a RED case (fires on the violation) *and* a GREEN
   case (silent on the near-miss). The GREEN case is the one that gets skipped, and it is
   where false positives live. A rule lacking one is flagged FP-unverified.
3. **Non-vacuity** — deliberately break the rule and confirm the eval goes red. A green
   eval that cannot go red is asserting nothing.
4. **Recoverability** — break the handler and assert the action still proceeds; assert
   every refusal names *the command that satisfies it*, not merely "denied".
5. **Replay** — run a candidate rule over recorded action records: *would this have
   blocked anything that actually happened?* This is the measured FP rate that gates
   promotion, and it turns the advise→block decision from a waiting game into a
   computation.

**Harness requirements.** The runner ships in the same change as the fixtures — an eval
suite with no runner is the same artifact as no eval suite, and we have a live instance of
exactly that. Fixtures exercise the real binary through the real hook path, because the
failure mode is wiring, not logic, and unit stubs cannot see wiring. The runner logs which
universe it swept: a runner covering one of two rule sets while reporting green converts
"nobody checks" into "CI says it is fine", which is strictly worse than no runner.

**Per-rule promotion ladder.** A rule's enforcement tier becomes a function of its eval
evidence rather than a judgement call: RED case, GREEN case, passing mutation check,
replay FP = 0 over N days, a remediation-naming message, and a registered verifier. The
tier lives in the graph, so promotion is a data write.

**Honest limits.** Replay measures false positives only on traffic that *happened* — it
cannot see work nobody has done yet, and it measures **no** false negatives at all. A
seeded adversarial corpus gives FN coverage against known attacks and none against novel
ones. That caveat travels with the numbers or the numbers mislead.

## What this cannot reach

Stated plainly, because an enforcement claim that is not true is worse than no
enforcement: it stops people looking.

| surface | why unreachable from an agent-side hook | where it must be enforced |
|---|---|---|
| CI pipelines | the runner executes the workflow — no agent, no session | repo-side checks, branch protection |
| cron, timers, scheduled jobs | no agent session in the loop | unit level, or the invoked script |
| the far side of a remote shell | the guard sees one command string, not the N it triggers | remote forced-command, or a lock |
| a sibling session's VCS index | another process touches your staging area | a VCS hook |
| a hostile or buggy agent | it can edit its own guard | per-agent credentials at the destination |

Origin-side actor stamping is **complementary to** destination-side credentials, never a
substitute. Described any other way it becomes another false "handled".

## Phasing

1. **Record-only trace.** Add work item, target entity and outcome to the spool taxonomy.
   No enforcement; cannot strand anyone; immediately answers the attribution questions.
2. **Eval harness, runner, and spool reader.** Ship the degradation-rate consumer with it.
3. **Extend the matcher surface** to tool-call classes currently ungoverned.
4. **Scope vocabulary in the graph**, behind the schema-proposal process.
5. **Pre-action consultation**, advise-only, rules pinned to warn, a few agents first.
6. **Daemon thin-client cutover.** Enforcement begins to depend on the daemon.
7. **Flip to enforce, per rule, on the promotion ladder.**

**A prerequisite spanning 4–7:** yupana's text-rule projection is bound to a single graph
type in compiled Rust, so a new rule class projects zero rows and is *silently absent*.
Widening it to a supertype — with the existing type as a subclass, so every current rule
keeps projecting — is a code change plus a shapes update, not a data write. The projection
module already carries a comment recording the last time this exact seam shipped on both
sides and returned zero rows.
