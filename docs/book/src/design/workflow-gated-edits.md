# Workflow-Gated Edits — process around a change

> Some edits need a *process* around them — "changing this file requires a demo
> of its effect on the UI," "this module needs a security review." That process
> need not happen at edit time, or by the agent that made the edit. This design
> is how a governed edit assigns a workflow **asynchronously**, across the
> Yupana × Quipu × Shantytown stack, without any tool growing a job that isn't its.

## The shared currency

A governed `aegis:Workflow` (Steps + guarded Transitions, defined in Quipu's
`shapes/governance.ttl`) is the unit passed between tools. Each tool touches it
at exactly one point:

| Tool | One job |
|---|---|
| **Yupana** | Record structure — promote the entity change to Quipu (FR-19). Structure-only: it does **not** evaluate policy or orchestrate workflows. It informs the *acting agent* in-loop via the `post-edit` hook (configurable). |
| **Quipu** | Own the rule and the record — the `Policy` (`targets` + `assignsWorkflow`), the `Workflow` / `Step` / `Transition`, the resulting `Decision` / `Verdict` — and act as the **event source** (bitemporal transaction log). |
| **Shantytown** | Subscribe and route — watch Quipu's entity events, evaluate the policy at the subscription boundary, deliver the required workflow to the administrator, who *acts*. |

## The loop (async-first)

```text
agent (via Yupana) edits src/ui/widget.rs
 → Yupana promotes the entity change to Quipu               (async; a new transaction)
 → Shantytown subscribes to Quipu entity events
      sees the change → asks Quipu /policy/check "is a workflow required?"
 → yes: aegis:Workflow "UI-demo-required" → route to the ADMINISTRATOR
 → admin ACTS: creates a bead "demo UI effects of the widget change"
      (assignable to anyone, anytime — not necessarily now, not necessarily that agent)
 → Yupana's post-edit hook informs the ORIGINAL agent in-loop (configurable, FR-30)
```

The requirement surfaces as a **Quipu entity event**, not a synchronous block.
That matches the domain: "requires a UI demo" is inherently decoupled — it does
not belong to the edit or the editor.

## Why async is the right default

- **It fits the work.** The process can be done later, by someone else. A
  synchronous guard would have to fake that.
- **It stays inside "not a mayor."** The administrator reacts to an event by
  creating *work* — a bead — assignable to anyone, anytime. No agent is
  auto-assigned; no hierarchy reorganizes. Event-driven is not autonomous: the
  system surfaces, a human acts.
- **Workflows are just one event kind.** The same subscription carries verdicts,
  policy effects, and doc-staleness. Gating an edit is one filter over it.

## The secondary path: the synchronous guard

For hard trust-boundary cases (capability-scoped `polecats`), the opt-in
`pre-edit` guard (FR-30) still evaluates a `boundary=action` Policy and may
`deny` before the edit lands. On `require-approval`, its verdict *names* the
workflow rather than blocking:

```json
{
  "verdict": "require-approval",
  "effect": "require-approval",
  "required_workflow": "aegis:workflow/ui-demo-required",
  "reason": "edit touches a UI surface governed by ui-demo policy",
  "tier": "committed"
}
```

Even here Yupana only **references** the workflow — it names the requirement; it
does not run it. Blocking guards stay opt-in, because a wrong hard-deny is worse
than none (FR-30).

## Boundary discipline

- **Yupana records structure and (guard only) references a workflow.** It never
  evaluates policy on the hot path or orchestrates. Policy evaluation and routing
  live in Shantytown; the record lives in Quipu.
- **Quipu owns the rule.** "This entity type requires this workflow" is a governed
  `aegis:Policy` with `targets` + `assignsWorkflow` — SHACL-validated at write, so
  a malformed rule is rejected at definition time.
- **Shantytown owns delivery.** It is already the tool that routes work to agents
  and injects prompts into terminals; the assigned workflow is one more thing it
  routes — to the administrator, for a human decision.

## The pieces this needs

- **Quipu — `aegis:assignsWorkflow`** (Policy → Workflow) in `governance.ttl`, so
  a `require-approval` / `escalate` effect can name the workflow to run.
- **Quipu — a transactions cursor** (`GET /transactions?since=<tx>&limit=<n>`) so
  Shantytown's subscription poll is O(new events), not O(whole log).
- **Shantytown — a `QuipuEvents` subscriber** over the transaction log, filtered
  to the kinds it cares about, with the four-state liveness discipline (a stalled
  watermark on a busy graph is surfaced, never silently green).

## Worked example: "changing this file requires a UI demo"

1. A `ui-demo` `aegis:Policy` in Quipu `targets` the `src/ui/**` module type and
   `assignsWorkflow` the `ui-demo-required` `aegis:Workflow`.
2. An agent edits `src/ui/widget.rs`. Yupana promotes the entity change; the agent
   gets the in-loop `post-edit` advisory.
3. Shantytown's subscription sees the change, asks `/policy/check`, learns a
   workflow is required, and routes it to the administrator.
4. The administrator creates a bead — *demo the UI effects of the widget change* —
   assignable to any agent, at any time. The edit is not blocked; the process is
   captured and owned.

## Status

- **Exists today:** `yupana hook post-edit` (the in-loop agent advisory); Quipu's
  governance plane (`Workflow` / `Step` / `Policy` / `Verdict`) and
  `tool_policy_check`.
- **This design adds:** `aegis:assignsWorkflow` and the transactions cursor in
  Quipu; the `QuipuEvents` subscriber and admin routing in Shantytown; the
  optional `required_workflow` field on the `pre-edit` verdict in Yupana.
- **See also:** [Governed Relations](governed-relations.md) — the Yupana ⋈ Quipu
  join this builds on.
