# The Enforcement Trace

What an action tells an auditor afterwards, and the two commands that carry it
into quipu.

This is the interface `quipu audit` consumes. If you are wiring the audit
checker, this page is the contract; if you are reading a spool by hand, it is the
field list. The reasoning behind the shape is in [SARC
Conformance](../design/sarc-conformance.md).

## Why the record has this shape

SARC's invariant I3 asks that a trace be **derived from the specification**
rather than reconstructed from logs afterwards. The audit checker needs four
things per action — which constraints applied, where each was evaluated, what
outcome it produced, and what response was taken — and none of them can be
recovered from a record that says only which rules fired.

Before this, the spool's `rule` field was a `+`-joined string of names. It
answers "what fired" and cannot answer "was this evaluated at a point compatible
with its class", which is exactly the placement pass of the checker. Two rules
that fired identically at different points produced byte-identical records.

## Where the spool lives

`$YUPANA_METRICS_PATH`, else `$XDG_STATE_HOME/yupana/metrics.jsonl`, else
`~/.local/state/yupana/metrics.jsonl`. One JSON object per line, appended, rotated
to `.jsonl.old` at 64 MiB.

Writes are **absolutely fail-silent** — stricter than the guard's own fail-open.
A metrics write must never change a guard outcome, never block an edit, never
print. A bookkeeping layer that can take enforcement down with it measures
negative value.

## Record kinds

| `kind` | emitted by | meaning |
|---|---|---|
| `guard` | `yupana hook pre-edit` | a Pre-Action Gate decision, one per edit |
| `audit` | `yupana hook post-edit` | a Post-Action Auditor crossing, with `point: "PAA"` |
| `governed` | the pre-edit guard | which governed rules spoke, by name |
| `fail_open` | anywhere on the guard path | the guard degraded, and why-kind |
| `hosting_overclaim` | the projection refresh | policies claiming a stronger layer than the one evaluating them |
| `command` | the CLI | deliberate tool use — the leverage signal |

Every line also carries `ts` (unix seconds), and `agent` / `tenant` from
`$SHANTY_AGENT` and `$BOBBIN_ROLE` when set.

## The constraint set — `constraints[]`

One array rather than sibling keys, so a reader never has to reassemble which
class went with which id by position:

```json
{
  "kind": "guard", "mode": "enforce", "result": "deny",
  "policy_freshness": "fresh",
  "constraints": [
    {
      "id": "no-ticket-in-comment",
      "class": "hard",
      "verification_point": "PAG",
      "hosted_at": "orchestration",
      "outcome": "unsatisfied",
      "response": "blocked"
    }
  ]
}
```

| field | values | notes |
|---|---|---|
| `id` | string | the constraint's stable id — the same name its verdict cites |
| `class` | `hard` \| `soft` \| `escalation` | **omitted** when undeclared, never guessed |
| `verification_point` | `PAG` \| `ATM` \| `PAA` \| `tool_layer` \| `policy_layer` | omitted when undeclared |
| `hosted_at` | `orchestration` \| `tool` \| `policy` | the layer that **actually** evaluated it |
| `outcome` | `satisfied` \| `unsatisfied` \| `unknown` | what the predicate concluded |
| `response` | `blocked` \| `warned` \| `logged` \| `escalated` \| `no-action` | what the runtime did |

Three of those distinctions carry weight:

- **`unknown` is not a soft `unsatisfied`.** A constraint that could not be
  evaluated has told you nothing, and collapsing the two is how an unevaluated
  check reads as a passing one.
- **`outcome` and `response` are separate** because they answer different audit
  questions — what the predicate concluded, versus what the runtime did with it —
  and the checker's outcome pass is precisely "does the recorded response match
  the one the policy declared".
- **`hosted_at` is never the layer the policy claimed.** It is stamped from the
  evaluating code's own constant. A field that echoed the declaration would let
  an overclaim survive the audit by being asserted twice (SARC I6).

`policy_freshness` rides alongside: how current the *policy set* behind those
evaluations was. It is what stops a soak window counting verdicts computed
against a stale projection as evidence about current policy.

The legacy `rule` field is still emitted, derived from the same set, because live
dashboards group on it. Dropping it would silently empty every panel built on it.

## The attribution tuple — who is answerable

SARC §9.6's `α = ⟨P, planner, executor, tool, auth, C_eval⟩`, on the gate record
and the PAA record alike. Recorded on **allow as well as deny**: a field that
appears only on refusals cannot answer "which chain has been acting here", which
is the question it exists for.

| field | source | notes |
|---|---|---|
| `principal_chain` | `$YUPANA_PRINCIPAL_CHAIN` | comma-separated, caller-first |
| `planner` | `$YUPANA_PLANNER` | declared, never derived from the chain's head |
| `executor` | `$SHANTY_AGENT` | the identity of the process that actually ran |
| `tool` | the hook payload | `Edit`, `Write`, `MultiEdit` |
| `attribution_conflict` | computed | emitted **only when true** — see below |

A dispatcher that spawns a sub-agent appends itself and exports the extended
chain; the sub-agent's hook then records the real chain rather than its own leaf
identity:

```bash
YUPANA_PRINCIPAL_CHAIN="orchestrator,worker" \
YUPANA_PLANNER="orchestrator" \
SHANTY_AGENT="worker" \
  claude --agent worker
```

**Every element is omitted when undeclared.** Filling `principal_chain` with
`[$SHANTY_AGENT]` would assert "this action had exactly one principal" to
precisely the auditor the field exists for, and an undeclared chain is not a
one-link chain.

**`planner` is not derived from the chain's head.** Which link deliberated and
which executed is a fact about the dispatch; reading it off list position would
be an inference wearing a record's clothes.

**`auth` is deliberately absent.** The effective authority is the intersection of
every link's grant, those grants live in quipu, and yupana cannot read them inside
a 100 ms pre-edit budget. Recording `principal_chain` is what lets quipu's
checker *derive* `auth` from the authoritative source; a locally-guessed value
would put a number in the field the grant store never agreed to.

### `attribution_conflict`

`YUPANA_PRINCIPAL_CHAIN` is a declaration; `$SHANTY_AGENT` is what is running. When
a chain is declared and its last link disagrees with the executor, the record
says so rather than silently preferring one. That disagreement is the observable
signature of a **laundered chain** — an agent acting under a dispatch record that
names somebody else — and a record that resolved it by precedence would delete
the only evidence of it.

It is emitted only when true. A `false` on every line trains a reader to skip the
field, which is the opposite of what it is for.

## Verdicts — `yupana verifier` and `yupana verdicts`

Both need the `quipu` feature.

A trace record is diagnostic. A **verdict** is an attestation: ed25519-signed,
bound to an evidence hash, and verifiable against a `aegis:VerifierRegistration`
that a human authored.

```bash
# Show the public key to register in quipu as this verifier's aegis:publicKey.
# This is the DELIBERATE key-creation act — it mints the key if absent.
yupana verifier --key-path yupana-signing.pk8

# Drain the local spool into quipu.
yupana verdicts --to http://localhost:7878
```

The guard **signs at the moment a constraint fires and appends locally**; it
never promotes on the edit path. A `/knot` round-trip does not fit inside the
guard's 100 ms deadline, and putting one there would make every agent's edit
latency a function of quipu's availability — a transiently wedged quipu once held
a guard for the full two minutes a caller was willing to wait.

The hook path uses an **existing key only** and never mints one. A signing
identity materialising from an agent's edit is not something that should happen
quietly; `yupana verifier` is where that decision is made.

The spool is truncated only when **every** verdict was accepted. A partial drain
leaves the file intact rather than losing the remainder.

## The Post-Action Auditor and `throttle`

`yupana hook post-edit` evaluates constraints declaring `verificationPoint "PAA"`
against the file *as it now stands* — the completed-action state, which is the
whole reason the point exists. The gate saw only the fragment an edit proposed.

It **cannot prevent the action it just watched**, and saying so plainly matters:
a PAA presented as prevention is the false-`prevented` claim the enforcement
gradient exists to stop. What it can do is change what happens next.

`throttle` is that response. A crossing records an expiring backoff, and the
*next* edit's advisory surfaces it. The fold is purely additive by construction —
it only ever turns an allow into a notice or appends to an existing one, and it
cannot reach a deny. A soft constraint must not become a block by this route.

A satisfied PAA constraint is **recorded, not skipped**. "The rule ran and held"
and "the rule never ran" are different facts, and the checker's coverage pass
needs to tell them apart — an absent evaluation reads as a constraint nobody
applied.

## Reading a trace with quipu

```bash
quipu audit ~/.local/state/yupana/metrics.jsonl --db ops.db   # T ⊨ Σ
quipu audit replay ~/.local/state/yupana/metrics.jsonl --db ops.db
quipu audit tree ~/.local/state/yupana/metrics.jsonl
quipu audit inheritance ~/.local/state/yupana/metrics.jsonl --db ops.db
```

`quipu audit` exits non-zero when the trace **contradicts** the specification; an
incompleteness (no principal chain, no declared class, a constraint the window
never exercised) is reported and does not fail the gate. See quipu's [CLI
reference](https://scbrown.github.io/quipu/reference/cli.html) for the passes and
what each one provably cannot decide.
