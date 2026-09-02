# The Pre-Edit Policy Guard — Integration Contract

`yupana hook pre-edit` is a Claude Code **`PreToolUse`** hook that can **deny** an
edit whose blast radius or target path exceeds the calling agent's capability
scope (spec §5.8, FR-25/FR-30).

This page is the **normative contract** for harness integrators (Gas Town
shantytown emits this hook per role/card). It is versioned with the binary: if
any clause below changes, the change lands here in the same commit.

Blocking is **opt-in and off by default**. A wrong hard-deny is worse than no
guard at all.

## (a) Input — the payload on stdin

The hook reads one JSON object on stdin, the standard Claude Code `PreToolUse`
payload:

```json
{
  "session_id": "abc123",
  "transcript_path": "/home/agent/.claude/projects/.../session.jsonl",
  "cwd": "/home/agent/work/repo",
  "permission_mode": "default",
  "hook_event_name": "PreToolUse",
  "tool_name": "Edit",
  "tool_input": {
    "file_path": "/home/agent/work/repo/src/graph/blast.rs",
    "old_string": "...",
    "new_string": "..."
  }
}
```

Yupana reads `cwd`, `tool_name`, and `tool_input`; everything else is ignored but
tolerated. `tool_input` shape by tool:

| Tool | Fields Yupana uses |
|---|---|
| `Edit` | `file_path`, `old_string`, `new_string` |
| `Write` | `file_path`, `content` |
| `MultiEdit` | `file_path`, `edits[].old_string`, `edits[].new_string` |

Every field is optional as far as the parser is concerned. **A payload Yupana
cannot parse, or one naming a file in no known language, is an ALLOW** — the
guard only ever speaks up about edits it genuinely understands.

## (b) Output — allow and deny

**ALLOW: exit `0` with empty stdout.** This is the overwhelmingly common path.

The guard **never emits `permissionDecision: "allow"`.** That value suppresses
the user's *own* permission prompt, and a structural guard has no business
granting permission it was not asked about. The guard may only ever *subtract*
permission, never add it. Staying silent leaves Claude Code's normal permission
flow exactly as it found it.

**DENY: exit `0` and print this object on stdout:**

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "deny",
    "permissionDecisionReason": "yupana: edit to src/graph/blast.rs exceeds the blast-radius ceiling for tenant `polecat-3` (impacts 47 symbols across 12 files; ceiling is 25/10). Narrow the change, or request a wider capability scope."
  }
}
```

`permissionDecisionReason` is fed back to the model, so it is written for a
model to act on: what was exceeded, by how much, and what to do instead.

**Yupana never exits `2`.** Exit `2` is Claude Code's fail-*closed* channel
(block, stderr to the model). Reserving it means no Yupana crash path can ever
hard-block an agent — a panic exits `101`, which Claude Code treats as a
non-blocking error and the tool call proceeds.

That guarantee covers Yupana's own code, and argument parsing happens before any
of it runs. A Yupana that predates a hook subcommand answers it with the argument
parser's "invalid value" error and exit `2` — so **staleness fails closed even
though absence fails open.** Since this version, an unparseable `yupana hook …`
invocation degrades to a silent allow instead (exit `0`, empty stdout, a loud
stderr line). Binaries older than that fix cannot be repaired retroactively,
which is why the invocation in (d) is written to be skew-proof on its own.

## (c) Latency — the sub-100ms budget

The hook is synchronous in the agent's loop (FR-31). Yupana enforces its **own**
wall-clock deadline, `[yupana.policy] deadline_ms` (default **100**). When the
deadline expires, the in-flight analysis is abandoned and the edit is
**allowed**.

Do not rely on the harness `timeout` field for this: it is expressed in whole
seconds and defaults to ten minutes — three orders of magnitude past the budget.
Set it anyway as a backstop (`"timeout": 5`), but the real deadline is Yupana's.

Until the Phase-3 resident daemon lands (FR-31), the guard builds the call graph
transiently and will exceed 100ms on large trees — which, by the rule above,
means it fails open. That is the intended degradation, not a bug: the guard gets
teeth on big repos when the daemon does.

## (d) Fail open — non-negotiable

**Every failure mode allows the edit.** The harness launches every crew agent
through this hook; a guard that fails closed bricks the fleet the moment Yupana is
unavailable.

| Failure | Result |
|---|---|
| `yupana` not on `PATH` | exit `127` → non-blocking error → edit proceeds |
| `yupana` too old to know the subcommand | exit `2` → **would block**; see below |
| Yupana panics | exit `101` → non-blocking error → edit proceeds |
| Deadline exceeded | exit `0`, silent → edit proceeds |
| Daemon unreachable, unreadable config, unparseable payload | exit `0` + loud line → edit proceeds |
| quipu unprojectable, cache servable | exit `0` → **rules still evaluated**, verdict marked STALE |
| quipu unprojectable, no servable cache | exit `0` + loud line → edit proceeds |
| Policy says deny | exit `0` + deny JSON → **edit blocked** |

### Failing open is a LAST resort, not the first response to a slow quipu

Fail-open is non-negotiable, but it is not free, and for one failure it was
being reached far too eagerly. The governed plane projects its rule catalogue
from quipu over HTTP, and the hook is a short-lived process **per edit** — so
before aegis-0upyu every edit by every agent issued a live `/query`, and any
one that could not complete in the 2s budget dropped that edit straight to
"allow".

Measured 2026-08-04 on this fleet: **5.2% of all pre-edit invocations, and 19%
of one day's**, failed open on projection timeouts alone. The failure
self-interferes — quipu serves `/query` effectively one at a time, so heavy
graph work is exactly what starves the guard that reads the graph, and the
guard was least available precisely when it mattered most.

The guard now keeps a **durable projection cache** (`projection.json`, beside
`metrics.jsonl` under `$XDG_STATE_HOME/yupana`, overridable with
`$YUPANA_PROJECTION_CACHE_PATH`). Every successful projection writes it; a failed
one serves it. The contract around it is what keeps a cache from becoming its
own silent failure:

- a cache-served verdict is **STALE, never fresh**, and states the cache's AGE
  in seconds — "stale" alone cannot distinguish a slow quipu from a week-old
  catalogue, and those warrant opposite reactions;
- past `[yupana.quipu] projection_cache_ttl_secs` (default 3600, `0` disables
  serving) the cache is **refused** and the guard fails open loudly. A retired
  rule that keeps firing from disk is worse than no rule, because it is
  unfalsifiable from the outside;
- a cache written against a **different endpoint** is refused outright, rather
  than enforcing another deployment's policy while claiming to enforce this
  one's;
- **`served_from_cache` and `fail_open` are different record kinds** and must
  stay that way. One is the guard enforcing last-known policy; the other is the
  guard not running. Collapsing them is what made a soak count unguarded edits
  as clean ones.

### Invoke it so version skew cannot block the fleet

Every row above fails open except one, and that one is not exotic: it is what
you get by rolling the hook out ahead of the binary, which is the normal
ordering of a deploy. Invoke the guard through this wrapper rather than bare:

```sh
out=$(yupana hook pre-edit 2>/dev/null) || exit 0
printf '%s' "$out"
```

`|| exit 0` converts *every* non-zero exit — `127` absent, `2` stale, `101`
panic — into an allow. Capturing first and printing only on success also means
a Yupana that dies mid-write contributes **nothing** to stdout, so a truncated
run can never be parsed as a permission decision. Emitting the command bare is
safe only once every host is known to be past the skew fix; the wrapper is safe
now, and stays correct afterwards.

### "Loud" means `systemMessage`, not stderr

A hook's stderr is shown **only when it exits `2`**; on exit `0` it goes to the
debug log, where nobody looks. So a fail-open that only wrote to stderr would be
silent in practice — exactly the failure this clause exists to prevent.

Yupana therefore writes the stderr line *and* emits a user-visible
`systemMessage`:

```json
{ "systemMessage": "yupana: policy guard failed open (daemon unreachable) — edits are UNGUARDED this session." }
```

No `hookSpecificOutput` accompanies it, so the edit is untouched. The message is
emitted **once per `session_id`** (tracked by a marker file under the system temp
directory), because a per-edit warning on a down daemon trains agents and humans
alike to ignore it.

## Registration

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [
          { "type": "command", "command": "yupana hook pre-edit", "timeout": 5 }
        ]
      }
    ]
  }
}
```

The tenant whose scope applies is resolved from `--tenant`, falling back to the
`BOBBIN_ROLE` environment variable. Shantytown sets `BOBBIN_ROLE` per agent, so
one hook registration serves every role.

## Policy configuration

Policy lives in the shared `.bobbin/config.toml` under `[yupana.policy]`, with the
usual resolution order (project > user > defaults).

```toml
[yupana.policy]
# off     — the guard is inert (default)
# advise  — compute and report violations, but never deny
# enforce — deny violations
mode = "off"
deadline_ms = 100
notify_on_fail_open = true

# Per-tenant capability scopes, keyed by tenant/role id.
[yupana.policy.scopes.polecat-3]
allow_paths = ["src/**", "tests/**"]
deny_paths = ["src/config.rs"]
max_impacted_symbols = 25
max_impacted_files = 10
```

A tenant with no scope entry is unconstrained — unless the work-item scope
rung is armed (`[yupana.policy] work_item_scope = "advise" | "enforce"`), in
which case the guard falls back to the OBSERVED scope of the agent's tracked
work item (the paths prior work on it touched, projected from quipu), and an
unknown scope draws a once-per-session advisory instead of silence. An item
with no observed ground of its own is scoped to its parent's — the **derived**
rung, which never hard-denies at any setting (an inference must not block) and
which answers `Allow` for an in-ground edit rather than falling through to
"unguarded". See the Configuration Reference and
`docs/work-scoped-governance.md`. `deny_paths` beats `allow_paths`.
Path patterns are globs matched against the repo-relative path.

With `mode = "advise"` the guard reports what it *would* have denied via
`systemMessage` and never blocks — run a new scope in `advise` for a while
before promoting it to `enforce`.

**An advise run is visible to the operator, not to the agent.** `systemMessage`
surfaces in the user's pane; it does not enter the model's context, and the tool
result of an advised edit is indistinguishable from an unguarded one. Confirmed
by running the same violating edit in both modes on a live pane: `advise`
returned an ordinary success to the model, `enforce` returned the reason.

Two consequences when staging a scope. Agents will not self-correct during an
advise run, so the violation counts you collect are the *uncorrected* rate —
which is what you want for sizing a ceiling. And "agents behaved no differently
under advise" is not evidence that they saw the advisory; they did not. Only
`enforce` puts the reason in front of the model.

The **work-item scope** advise rung is the exception: its out-of-ground notice
is delivered where the agent *can* read it — as `PostToolUse`
`additionalContext` on the post-edit hook, the channel that reaches the model
without blocking. The edit has landed, nothing is prevented, and the agent is
told before its next action that it stepped outside its item's ground — along
with what the graph knows about where it went (which work items' commits
touched that path before, and how each turned out). Silent whenever it cannot
speak honestly: no plate, no scope for the item, an unreachable store, or an
edit inside the ground. An unknown scope is UNKNOWN, not a deviation.

## Tripwires — boundaries with declared effects

A **tripwire** (`[[yupana.policy.tripwires]]`) is the local-config slice of the
governance plane's *Binding / Gate* primitive: it attaches an effect to a
boundary, and the edit itself is the crossing — nothing has to remember to
check the wire. Where a rule says what text may look like everywhere it
applies, a tripwire says what happens *at this boundary*.

```toml
# Touching the boundary at all trips the wire.
[[yupana.policy.tripwires]]
name = "auth-boundary"
paths = ["src/auth/**"]
effect = "deny"                  # warn | deny | throttle
message = "auth changes need the security workflow"   # optional

# Rule-conditioned: trips only when the named rule fires INSIDE the boundary.
# The same rule can advise repo-wide and deny here.
[[yupana.policy.tripwires]]
name = "no-tickets-in-auth"
paths = ["src/auth/**"]
rule = "no-ticket-in-comment"    # a [[yupana.policy.rules]] name
effect = "deny"

# Crossing records an expiring backoff subsequent edits surface. Never blocks.
[[yupana.policy.tripwires]]
name = "hot-file"
paths = ["src/core.rs"]
effect = "throttle"
backoff_secs = 300
```

Semantics, with the ambient `mode` a ceiling throughout:

- **`deny`** blocks under `enforce` and advises under `advise`.
- **`warn`** notifies and never blocks.
- **`throttle`** records a backoff via the same machinery the Post-Action
  Auditor uses; the next edits surface the advisory. It is recorded even when a
  sibling `deny` wire blocks the same edit — the attempt crossed the boundary,
  and the crossing is recorded either way.
- A **path-only** wire trips on any edit whose repo-relative path matches
  (a pure deletion included: the crossing is the edit's target, not its text).
  A **rule-conditioned** wire needs the introduced text and a grammar; without
  them it has no evidence and does not trip.
- One decision per edit: a deny-effect trip decides it, and every tripped wire
  rides in the same message and is recorded as its own constraint evaluation
  (point `PAG`, tree-sitter tier, `fresh` — local config is authoritative).

A misconfigured wire — a malformed glob, a `rule` no rules entry defines, a
`throttle` with no `backoff_secs`, or a wire with no paths and no rule — is a
**loud fail-open**, never a silently inert control: the wire set names each
broken entry once per session and the guard allows.

**Governed tripwires** (`quipu` feature). The same concept with quipu as the
canonical store: an `aegis:Policy` at `boundary:"action"` carrying
`aegis:appliesTo` globs and *no* Selector/Predicate (quipu
`shapes/policies/tripwire.ttl`). They ride the governed plane's one registry
refresh (and its durable cache — a projection failure serves last-known wires,
stale and saying so), and evaluate class-first like every projected policy: a
`hard` wire's `deny` blocks under `enforce`, a `soft` wire never blocks, a
`throttle` wire's declared `aegis:backoffFormula` is compiled into a recorded
backoff. A wire declaring `verificationPoint "PAA"` is skipped at the gate —
quipu's placement law puts soft throttle wires there, and the PAA-side
projection is that seam's sequencing step 2. Effects this seam cannot enforce
(`require-approval`, `record`, …) refuse the projection loudly rather than
decode into an inert wire.

## The action surface (`pre-bash`)

`yupana hook pre-bash` always records resolved actions. Its target-scope arm is
**record-only by default** and stays that way unless a deployment sets
`[yupana.policy] action_scope`, which is `off` out of the box.

```toml
[yupana.policy]
action_scope = "advise"          # off | advise | enforce; `mode` is a ceiling

[yupana.policy.scopes.polecat-3]
allow_targets = ["host:build-*", "service:metrics"]
deny_targets  = ["service:etcd"]
```

Targets are matched as `class:target` against the `(verb, target, class)` a
command resolves to — `host`, `service`, `repo`, `container`. Precedence and the
empty rule match the path halves exactly: `deny_targets` beats `allow_targets`,
and an empty `allow_targets` permits any target rather than none.

**Declared only.** There is no observed rung here: nothing in the graph records
which hosts an item's prior work touched, so there is no record to infer from —
and `declared` is the one provenance the trust ladder permits to hard-deny.

**An abstention is never a violation.** The resolver answers `unknown` for every
command whose target is not unambiguous from syntax — a pipeline, a shell
function, a script that ssh's internally, or a bare hostname with no dot. Those
reach the check as *no check performed*. A guard that refused what it could not
identify would refuse most of the shell, which is why the recognised set is
deliberately small.

**Stage `advise` first.** At `advise` the violation goes to stderr and the spool
and nothing is refused; at `enforce` the command is denied. The spool's
`action_scope` records say what would have been refused, which is what a
deployment should read before arming the boundary.

### Governed host-memory policy

The same hook projects memory-heavy command policies from Quipu. A policy owns
all of the tunable data: a live command regex in its `Selector`/`Predicate`, and
a `deterministic_threshold` `OperatingPoint` whose `threshold` is interpreted
against the declared calibration basis `MemAvailable GiB`. Yupana reads
`MemAvailable` and `MemTotal` from `/proc/meminfo`; it does not substitute
`MemFree` when the signal is absent.

A matching command below the floor reports the measured available, total, and
governed threshold values. Under `mode = "advise"` it emits a system message and
records a `memory_policy` metric without stopping the command. A hard policy can
deny only under `mode = "enforce"`; changing the floor or matcher is a graph
write and needs no Yupana rebuild. A missing projection, stale cache past its
TTL, unreadable memory signal, or threshold larger than the host's total memory
is loud fail-open.

This event is an agent-harness execution seam. Non-harness launchers must invoke
the same hook contract before execution (for example from a scheduler wrapper);
installing the binary alone does not interpose on arbitrary processes. Do not
claim cron coverage until the cron entry itself is wired through that seam.

### Quipu-backed disk-impact advisory

The pre-Bash hook also learns the disk cost of commands that commonly create
large artifacts (`cargo`, container builds, package managers, copy/archive
tools, and explicit allocation tools). It snapshots filesystem free space with
`df` before a command and closes that sample when the next matching command
arrives in the same harness session. This is intentionally a filesystem delta,
not a directory walk: hardlinks, deleted-open files, caches outside the working
tree, root, and tmpfs must remain visible as different budgets.

History is keyed by a normalized `binary:subcommand:repo-class` signature and
an opaque filesystem hash. Raw argv, paths, devices, and mount points are never
written to Quipu. The guard projects the most recent 100 samples and uses their
nearest-rank p90. If p90 exceeds 80% of current headroom, it emits a
`governed, not blocking` system message. There is no enforcement branch in this
first slice.

No history is **UNKNOWN**, never safe: the matching command is allowed with an
explicit message that no Quipu-recorded basis exists. A failed history write or
query is likewise loud fail-open. Unrelated commands do not pay for a Quipu
round trip and remain silent. The graph vocabulary is governed by
`CommandDiskImpactObservationShape`; deployments must accept that shape before
expecting samples to accrue.

## What the guard checks

1. **Path scope** (FR-25) — is the edited file inside this tenant's writable
   capability scope?
2. **Blast radius** (FR-12/FR-25) — do the symbols the edit touches transitively
   affect more symbols or files than the scope permits?
3. **Buffer verification** (FR-23, opt-in: `[yupana.policy] verify = true`) —
   does the proposed buffer introduce references that resolve to nothing?
   The same verifier `yupana verify` exposes, run at the blocking seam: a
   hallucinated identifier, a wrong-arity call, or an unresolved `mod` import
   denies under `enforce` and advises under `advise`. Rust-only today (the
   verifier is a tree-sitter-rust pass), scoped to what the edit *introduces*
   (pre-existing breakage is not this edit's business), and inside the same
   `deadline_ms` — a blown budget or unparseable buffer degrades to allow,
   never to a block.

All are computed at the **tree-sitter tier** against the *requesting tenant's*
graph, and every verdict carries that tier tag (FR-3). A tree-sitter blast radius
is an approximation; the ceilings should be set with that in mind.

Alongside the checks, the guard runs a **denied-edit recurrence advisory**
(stage 1 of the semantic-grounded ordering): the verdict spool retains denied
edits as a similarity corpus, and an edit near a prior denial surfaces that
verdict as context — before a refusal's own text, so the refusal arrives
explained, or as a plain notice on an allow. Similarity **never denies**; the
advisory seals its cosine score, threshold, embedding-model id and corpus
watermark so it can be reproduced or falsified.

## Observing the guard — `yupana status`

A guard you cannot observe is a guard you cannot trust: enforcement that has
quietly stopped looks exactly like enforcement that found nothing wrong. So the
policy layer is visible in `yupana status`, resolved for the queried `--tenant`:

```text
  policy      : mode=enforce  scope=configured (allow=1 deny=0 sym≤— files≤3)
  rule set    : none — never loaded (local config only)
```

Two states are reported **loudly** rather than left to silence:

- **`enforce` with no scope for the tenant** — armed in appearance, inert in
  effect — prints a `⚠` caveat (`enforcing_without_scope: true` in `--json`).
  This is the disarm-that-reads-as-healthy shape behind more than one past bug.
- **No signed rule set loaded.** The resident, signed rule cache does not exist
  yet; `status` reports its absence (`signed_rule_set.state: "never-loaded"`)
  rather than omitting it, so the surface is present and its absence is never
  silent. When the cache lands it populates the live rule-set version and age
  into this same field.

`--json` emits the full `policy` object (mode, `scope_configured`, ceilings) and
the `signed_rule_set` state for `st doctor` and other tooling to gate on.
