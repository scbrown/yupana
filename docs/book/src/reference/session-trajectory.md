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
unobservable. This replay is not automatically installed as a hook.

`just session-guard --selftest` exercises the discriminators and exits nonzero on
failure. Both `just test` and the pre-commit/CI quality gate run this suite.

## Live Read advisory

The existing `yupana hook post-edit` command also evaluates Claude `Read`
completions. Its `PostToolUse` matcher must include `Read` (an all-tool matcher
already does). It needs `session_id`, `tool_use_id`, `transcript_path`, the exact
`tool_input`, and a successful structured text `tool_response`. No additional
hook or background process is needed.

Only the current completed read can produce advice. The earlier structured
result must precede its request on the current conversation's parent-UUID chain;
abandoned branches cannot supply context. Identical returned text, actual line
span and requested arguments are required. Recorded compaction, edits, malformed
or missing lineage, failed/image results and task-output polling invalidate or
exclude evidence. The live adapter conservatively invalidates after **every
shell command**, since a script can write a file without naming it in the command.
This is narrower than the offline replay's filename heuristic.

The advisory asks the agent to reuse earlier text **if it remains in context**.
It does not assert that a reread was wasteful, and cannot observe eviction that
the harness did not record. The existing per-session advice suppression limits
repeated notices. This is a Claude structured-Read adapter; it does not claim
coverage of shell reads or Codex tool output.

Transcript reads use the most recent 16 MiB, discarding a partial initial record.
Large sessions can still establish a candidate from a connected pair of reads
inside that window; they do not need the session root. Missing ancestry before
the window cannot manufacture a clean pass when no pair is found. Missing,
unreadable or unrecognized evidence produces `unknown`. Each Read hook
evaluation emits `reread_evaluated` metrics with `candidate`, `no_match`, or
`unknown` and a hash of the session/request identity. Metrics contain no paths
or returned text. A candidate metric records evaluation, not delivery after
suppression, and zero candidates cannot establish a residual false-positive rate.

## Handoff advice from measured depth

The same command exposes a read-only depth evaluator:

```sh
just session-guard depth session.jsonl --harness claude \
  --session-id current-session-id --config context-policy.json
```

Use `--harness codex` for a Codex rollout. Supply the current session identity
explicitly; the evaluator does not search for a likely transcript. Both guards
share transcript parsing and recorded compaction boundaries.

Claude depth is the latest assistant request's `input_tokens` plus
`cache_read_input_tokens` and `cache_creation_input_tokens`. Codex depth is the
latest `token_count.info.last_token_usage.input_tokens`; cached input is already
included, and cumulative session usage is never substituted. These are measured
request depths, not a promise about growth after the measurement. A missing,
invalid or unreadable latest measurement invalidates the earlier sample. A
recorded compaction requires a new measurement.

The JSON config selects policy per harness under `harnesses.claude` or
`harnesses.codex`. Each policy requires:

| Field | Contract |
| --- | --- |
| `handoff_tokens` | Positive integer threshold, chosen from observed depth/compaction evidence |
| `threshold_evidence` | Nonempty reference to the observations supporting that threshold |
| `max_age_seconds` | Positive freshness window for accepting the measurement |

There is **no built-in threshold or freshness default**. An observed distribution
that supports a default has not yet been established; the empty config
`{"harnesses": {}}` deliberately leaves advice unevaluated. A reference string
records operator provenance; it does not validate the quality of those observations.
Do not substitute transcript bytes, elapsed time, or lifetime token totals.

The evaluator always prints a JSON verdict:

| Status | Action | Exit |
| --- | --- | --- |
| `UNKNOWN` | `DO_NOT_ACT` | 2 |
| `BELOW_THRESHOLD` | `DO_NOT_ACT` | 0 |
| `HANDOFF_ADVISED` | `CHECKPOINT_AND_HANDOFF` | 0 |

Missing config, missing depth, wrong session identity, stale/future timestamps,
and malformed evidence all return `UNKNOWN`. This is an explicit signal-loss
verdict, never an instruction to cycle and never a claim that context is healthy.
At or above a configured threshold, checkpoint and use your orchestrator's safe
handoff procedure. The evaluator itself never clears context, stops a session,
or installs a hook. A deployment must deliberately supply its signal and config;
no production handoff is activated by installing this repository.
