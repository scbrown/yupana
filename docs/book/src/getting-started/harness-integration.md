# Harness Integration (Claude Code Hooks)

Yupana can plug into an agent harness so it responds to edits **synchronously**,
the way a language server reacts to a keystroke. The agent's edit tool call *is*
the change event; a harness hook feeds it to Yupana, and Yupana replies inline — no
tool call for the agent to remember.

For Claude Code this uses [hooks](https://docs.claude.com/en/docs/claude-code/hooks).

## Post-edit advisory (available now)

`yupana hook post-edit` reads the `PostToolUse` payload on stdin and, when the
edited file has symbols called from *other* files, returns a cross-file
blast-radius advisory as injected context.

Wire it in `.claude/settings.json`:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [
          { "type": "command", "command": "yupana hook post-edit" }
        ]
      }
    ]
  }
}
```

After the agent edits `src/auth.rs`, it sees something like:

```text
Yupana (tree-sitter): your edit to src/auth.rs touches symbol(s) with callers
elsewhere — re-check these still compile.
  authenticate <- 3 caller(s)
  verify_token <- 1 caller(s)
Impacted files: src/api/login.rs, src/api/session.rs
```

The advisory is emitted only when there is cross-file impact, and the hook never
fails the harness (no output = nothing to say).

With the `quipu` feature and the work-item scope rung at `advise`, the same
hook also delivers the **out-of-ground notice** as `additionalContext`: the
edit has landed, nothing is blocked, and the agent is told — before its next
action — that it stepped outside its work item's ground, with what the graph
knows about that path (who worked it before, and how that went). This channel
exists because a pre-edit `systemMessage` reaches the operator's pane, not the
model's context; see the
[policy guard contract](../reference/policy-guard.md).

## Work-item briefing (available now, `quipu` feature)

`yupana hook session-start` (`SessionStart`) injects the graph's knowledge of
the agent's tracked work item into its context at assignment time — before the
first edit, not at the first denial: the item's ground (paths prior work
touched, with symbols and caller locality), the central entities around it
(quipu pagerank), semantically similar past items with their outcomes so
**successful work gets reused**, related in-flight items, the governed rules
in force, and the scope posture. Silent when no plate or no quipu seam.

What lands in context is the **L0** briefing: the item, its ground paths and
the scope posture, plus a **census** of every section held back — one line
each, naming how many items it holds and the real `quipu_ask` /
`quipu_context` / `quipu_project` call that expands it. The census is the
whole difference between a briefing that is *small* and one that is
*incomplete*: a short briefing with no counts is indistinguishable from a
graph that knew nothing. Sections with zero items are omitted entirely rather
than shown as empty headings.

Division of labor with Bobbin: Bobbin's own hooks (`bobbin hook install` —
`inject-context` on UserPromptSubmit, `session-context`/`prime-context` on
SessionStart) own *semantic code* context; this briefing owns the
*governance/work-item* context. Wire both side by side — the briefing
deliberately does not nest a bobbin call, so the same context is never
injected twice.

```json
{
  "env": { "SHANTY_ROOT": "/gt", "SHANTY_AGENT": "polecat-3" },
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "yupana hook session-start", "timeout": 15 }
        ]
      }
    ]
  }
}
```

## Pre-edit guard (available now)

`yupana hook pre-edit` (`PreToolUse`) checks an edit *before* it lands against the
calling tenant's capability scope, and can **deny** it with a reason the model
can act on. Blocking is **opt-in and off by default**.

```json
{
  "env": { "BOBBIN_ROLE": "polecat-3" },
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

With a scope configured for that tenant (see
[Configuration](../reference/config.md)), an over-reaching edit comes back as:

```text
yupana: editing `src/core.rs` reaches 4 symbols (ceiling 2) and 4 files
(ceiling 1) — beyond the blast radius allowed for tenant `polecat-demo`.
Split this into a narrower change that touches fewer callers, or ask for a
wider capability scope. (tree-sitter tier: the reach is an approximation.)
```

The guard **always fails open**: every error, timeout, and unparseable payload
allows the edit, because a guard that fails closed would brick every agent the
moment Yupana is unavailable. Read
[the full contract](../reference/policy-guard.md) before wiring it into a fleet
— particularly the rules that allow is *silence* and that Yupana never exits `2`.

## The action surface (`pre-bash`)

`yupana hook pre-bash` (`PreToolUse` on `Bash`) records each command's
resolved `(verb, target, class)` for the enforcement trace, and is
**record-only by default**: it prints nothing unless a deployment explicitly
sets `[yupana.policy] action_scope` (off out of the box), at which point a
declared scope's `allow_targets` / `deny_targets` can advise or refuse a
command. Commands whose target is not unambiguous from syntax are abstentions,
never violations. See
[the guard contract](../reference/policy-guard.md#the-action-surface-pre-bash).

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "yupana hook pre-bash", "timeout": 5 }
        ]
      }
    ]
  }
}
```

## Performance note

By default the hooks build the call graph transiently on each invocation. With
a running [resident daemon](../reference/daemon.md) and `[yupana.serve]
use_daemon = true`, both hooks become thin clients of the resident graph — no
per-invocation build, which is what meets the sub-100ms budget a synchronous
guard needs. The daemon being down degrades differently per hook: the pre-edit
guard fails open **loudly**, the post-edit advisory falls back silently (it is
advice, not enforcement). See
[the specification](../design/specification.md) §5.9 (FR-30/FR-31).
