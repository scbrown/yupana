# Governed session trajectory advice

The post-edit hook can advise when a selected command was attempted earlier in
the same session. For example, a work-item creation command followed by more
edits can prompt the agent to check ownership before continuing. The graph owns
the command vocabulary, event ordering, notification frequency and explanation.
Yupana owns the observation and evaluation mechanism.

The command observation runs in `pre-bash`. It proves an **attempt**, not a
successful command or a newly created work item. Deep investigation without a
matching command stays silent. Missing session identity cannot establish an
ordering and stays silent as well.

## Policy data

This is a separate `trajectory_policies` channel in the existing projection,
using the existing `Policy`, `Selector` and `Predicate` classes. A Policy with
`targets = "session-trajectory"` supplies:

| Field | Meaning |
| --- | --- |
| `selector / evidenceSource` | JSON object with nonempty `programs` and `verbs` arrays of bare command tokens |
| `predicate / evidenceSource` | `command-before-edit`, the supported ordering |
| `enforcementTier` | `warn`, the supported advisory tier |
| `oncePer` | `session` for one notice per rule/session, or `edit` for every subsequent edit |
| `effect` / `verificationPoint` | `warn` at `PAA`, the supported response and placement |
| `rdfs:comment` | The full explanation delivered to the agent |

The example declaration is `policies/delegate-line.ttl` in the repository.
It is imported deliberately; the binary never seeds a fallback rule.

The normal Policy and Selector/Predicate shape requirements still apply,
including `claim` and atom `name` fields. The trajectory decoder validates its
additional fields, rejects unknown keys in the trigger JSON, rejects conflicting
rows for one identity, and treats a missing required field as an error. Policies
with incomplete fields remain visible to the query so they cannot disappear
through a required SPARQL join.

A work-item selector can use:

```json
{"programs":["br","bd"],"verbs":["create"]}
```

Its rationale must preserve the discriminator: comments, updates and closes are
ordinary traffic on an existing item, not artifact creation. The advice should
ask whether edits belong to the agent's assigned work; it must permit continuing
that work. The parser retains the previous narrow behavior, including the known
miss when a value-less long flag precedes the verb (`br --json create`). That
produces a missed observation, never a claim of successful creation.

## Tier and placement

This slice supports **advice only**. A trajectory policy marked `block` is
**refused during projection**, with an error naming the missing pre-edit
enforcement point. The post-edit hook cannot prevent an edit that already
happened. A future pre-edit gate is a separate placement change requiring its
own verification and promotion; accepting a block tier here would falsely
advertise enforcement.

The deployment's existing policy mode remains the ceiling. `off` disables the
channel. `advise` and `enforce` both deliver `warn` as advice.

## Refresh and verification

Run `yupana refresh-projection` after changing graph policy. The JSON result
includes a `trajectory` count and the durable cache includes the complete rule.
Hooks read this local cache without network requests. A missing channel in an
older cache is **unknown**, distinct from a refreshed empty catalogue; absent,
expired or invalid cache data reports `NOT EVALUATED` rather than silently
claiming no rule applies. Removing a policy and refreshing retires its advice.
A changed trigger cannot inherit evidence recorded under its old definition.

Verify with a fresh isolated session:

1. Post-edit before any selected command: silent control.
2. Pre-bash with a selected invocation: observe the attempt.
3. Post-edit: the graph's explanation appears.
4. Post-edit again: silent when `oncePer` is `session`.

Also test a custom program/verb pair using only changed graph data, and confirm
that a `block` policy refuses projection. The `trajectory_advised` metrics record
names the rule, tier, frequency and attempted-command evidence; it never records
an inferred successful work-item creation.

## Read replay evidence

`just session-guard session.jsonl` runs a separate, offline Claude transcript
replay. It reports identical successful text returned for the same requested
region, only when the earlier result preceded the later request. Failed reads,
missing results, images, concurrent requests, changed output, recorded edits,
compaction, and known background-task output polling do not produce candidates.
Overlapping but different ranges remain silent because they may contain new lines.

The output is a candidate for review, not proof of wasted work or that the
harness still retains the content: eviction without a transcript marker remains
unobservable. This replay is not automatically installed as a hook. It does not
supply a context-depth measurement or handoff trigger.

`just session-guard --selftest` exercises the discriminators and exits nonzero on
failure. Both `just test` and the pre-commit/CI quality gate run this suite.
