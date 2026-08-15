# Configuration Reference

Yupana reads the `[yupana]` table of `.bobbin/config.toml`. All keys are optional;
unspecified keys fall back to the defaults shown below.

```toml
[yupana]
# Baseline the shared read-only graph is built at.
base_ref = "main"
# (Phase 2/3 — not yet read) Run the LSP tier for precise facts where a build resolves.
enable_lsp = true
# (Phase 2 — not yet read) Run the CPG/dataflow tier.
enable_cpg = false
# Languages to extract. RESTRICTS `yupana analyze`: a file whose language is not
# listed is not counted.
languages = ["rust", "typescript", "python", "go", "java", "cpp"]

[yupana.freshness]
# Debounce for keystroke-driven tree-sitter updates (ms).
debounce_ms = 300
# (LSP tier — not yet read) When to compute LSP facts: "save" | "on_demand".
lsp_on = "save"

[yupana.tenancy]
# Maximum concurrent per-tenant overlays over one base. A new overlay past the
# cap evicts one per `overlay_eviction` (logged, never silent).
max_overlays = 32
# Symbols whose direct fan-in exceeds this get a bounded frontier cascade
# (clipped to one hop) so a widely-referenced signature edit cannot blow the
# recompute budget (§14.2). The bounding is logged.
high_fanin_threshold = 200
# Overlay eviction at the cap: "lru" (evict least-recently-used) or
# "on_session_close" (evict on close; oldest-created is the cap backstop).
overlay_eviction = "on_session_close"

[yupana.serve]
bind_address = "127.0.0.1"
# Distinct from Bobbin's server and Quipu's 3030.
mcp_http_port = 3040
# Write guard: when true, yupana REFUSES mutating operations (promotion) with a
# distinguishable error. The served MCP/HTTP surface is read-only regardless
# today; this guards the write path and any future served write.
read_only = false
# When true, the hook and MCP graph tools consult the resident daemon at
# bind_address:mcp_http_port (see Resident Daemon) and fall back to the
# transient build when it is unusable. The guard's fallback is LOUD.
use_daemon = false

# (Phase 4) Quipu promotion. `promote_on` and `shapes_path` are not yet read.
[yupana.quipu]
enabled = false
# "commit" | "merge" | "manual".
promote_on = "merge"
# "named_graph" (preferred, needs Quipu quads) | "qualifier" (fallback).
branch_model = "named_graph"
shapes_path = "shapes/"

[yupana.policy]
# "off" (inert) | "advise" (report only) | "enforce" (deny).
mode = "off"
# Wall-clock budget for the whole pre-edit guard (ms). Expiry => allow.
deadline_ms = 100
# Warn the user, once per session, when the guard fails open.
notify_on_fail_open = true
# How far to follow the call graph when sizing an edit.
max_hops = 5
# Run the FR-23 buffer verifier as an arm of the guard: reject edits that
# introduce references resolving to nothing (hallucinated identifiers, wrong
# arity, unresolved `mod` imports). Opt-in; rides inside deadline_ms.
verify = false

# Per-tenant capability scopes, keyed by tenant/role id. A tenant with no entry
# is unconstrained. See "Pre-Edit Policy Guard" for the full contract.
[yupana.policy.scopes.polecat-3]
allow_paths = ["src/**", "tests/**"]   # empty = any path
deny_paths = ["src/config.rs"]         # beats allow_paths
max_impacted_symbols = 25
max_impacted_files = 10

# Structural (tree-sitter-tier) rules — checks a linter finds hard or slow.
# Unlike scopes, rules are NOT per-tenant: they govern the code an edit
# introduces, for everyone. Each rule pairs a Selector (a tree-sitter .scm
# capture query) with a Predicate (a regex + a match_type). Evaluated against
# the text the edit ADDS, Mode-staged, fail-open. Use TOML literal (single-
# quote) strings so regex backslashes are not doubled.
[[yupana.policy.rules]]
name = "no-ticket-in-comment"
language = "rust"                      # the grammar the query targets
query = '(line_comment) @c'            # Selector: which nodes
match_type = "must-not-match"          # must-match | must-not-match | must-exist
pattern = '\b[A-Z]+-[0-9]+\b'          # Predicate: the regex
# gate = '\bTODO\b'                    # optional: only test captures matching this
# applies_to = ["src/**"]              # optional path globs; empty = any path
# message = "keep ticket refs in commits, not comments"  # optional override

# What the usage spool records about each guard decision.
[yupana.metrics]
# "off" (default) | "relative" (repo-relative) | "absolute".
record_paths = "off"
```

An unrecognized `mode` is a config **error**, not a silently inert guard.

## Auditing guard decisions (`[yupana.metrics]`)

A guard record carries `result`, `mode`, `ext`, `agent`, `tenant` and `ts`. That
is enough to count denies and not enough to review one: a record without a
subject cannot confirm a rule is scoped correctly, cannot show a false positive,
and cannot support an incident timeline. `ext` is a lossy proxy — six denies on
`.json` tells an operator nothing actionable.

`record_paths` adds the subject:

| Value | Records | Use when |
| --- | --- | --- |
| `off` (default) | no path | The deployment treats paths as sensitive. |
| `relative` | `src/auth.rs` | The useful setting for a fleet — identifies the file without disclosing where the checkout lives. |
| `absolute` | `/srv/repo/src/auth.rs` | One host, several checkouts, where a relative path is ambiguous. |

It defaults to `off` because paths are more sensitive than extensions: a
deployment opts **in**, and recording is never switched on beneath one.

The **rule id** is recorded whatever `record_paths` says. It names what actually
fired — the matching `deny_paths` glob, `allow_paths` when a path matched no
allow pattern, the exceeded ceiling, or the governed rule's name — and it is the
field that makes a false positive diagnosable, since a wrongly-scoped rule and a
correctly-scoped one are otherwise indistinguishable. A rule id is a name the
operator wrote, not user content, so it carries none of the sensitivity that
argues for gating paths.

Both fields are recorded for **allow** as well as deny, under the same knob.
Scope that can only be inferred from the absence of denies cannot be verified at
all; confirming a rule is scoped correctly needs what it let through as much as
what it stopped.

```json
{"kind":"guard","result":"deny","mode":"enforce","ext":"yaml",
 "path":".beads/config.yaml","rule":"deny_paths:.beads/**",
 "agent":"mathis","tenant":"worker","ts":1785544543}
```

Both fields are **omitted** rather than blanked when they have nothing to say
(recording off, or a clean allow with no deciding rule), so a reader never has to
tell "recorded as empty" from "not recorded".

`yupana status` reports the setting in force, so an operator can confirm from
outside the process that recording is actually on — a control believed active and
silently inert reads exactly like a quiet week.

```console
$ yupana status
  audit       : record_paths=relative
```

## Projected governed policy (Phase 4, `quipu` feature)

With the `quipu` feature and `[yupana.quipu] enabled = true` plus an `endpoint`,
the guard also fetches quipu's `boundary:"action"` structural policies and
evaluates them like any rule — a governed `deny` policy blocks under
`mode = "enforce"`. An unreachable quipu fails open loudly; the verdict declares
whether the projection was fresh. Yupana never defines a governed policy — it only
projects quipu's. See the design note "Policy edit hooks — the yupana side".

**`endpoint` is a READ capability, not a write one.** It is what the guard fetches
the rule catalogue from, and that is all it grants: `yupana promote` will not write
to a merely-configured endpoint, because setting this key deployment-wide is how
every checkout in its scope came to be one bare command away from a live,
un-undoable graph write (aegis-o2h97). A promotion must name its target with
`--to`. See "`yupana promote` — what authorizes the write" in the CLI reference.
