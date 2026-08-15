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
unknown scope draws a once-per-session advisory instead of silence. See the
Configuration Reference and `docs/work-scoped-governance.md`. `deny_paths`
beats `allow_paths`.
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
