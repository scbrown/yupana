# CLI Commands

```text
USAGE:
    yupana <COMMAND>

COMMANDS:
    serve       Run the MCP server (stdio; --http for streamable-HTTP)
    daemon      Run the resident-graph daemon (FR-31; see Resident Daemon)
    analyze     Build the base graph for a path and print a summary
    refs        Find the definition sites of a symbol by name
    callers     Direct callers and callees of a symbol
    communities Densely-connected symbol clusters (deterministic Louvain, FR-9)
    impact      Blast radius; --cochange reconciles against history (FR-11)
    dataflow    Intra-procedural data dependence within a function
    export      Emit the referential structure as Turtle (bobbin: ontology)
    hook        Harness hook adapter (post-edit advisory / pre-edit guard)
    verify      Verdict on a proposed edit buffer (FR-23/FR-24)
    promote     Promote a commit's structural facts into Quipu    [Phase 4]
    verifier    Show the verdict-signing public key to register  [quipu feature]
    verdicts    Drain the local signed-verdict spool into quipu  [quipu feature]
    status      Show base commit, tiers, and configuration
    completions Generate shell completions
    help        Print help

GLOBAL FLAGS:
    --json      Machine-readable output
    --quiet     Suppress non-essential output
    --verbose   Raise the default log level to debug (RUST_LOG still wins)
    --tenant    Tenant/session id (default: single-tenant)
    --config    Path to config file (replaces discovery; must exist)
```

`--config <path>` **replaces** config discovery rather than adding to it: it
loads exactly that file over the compiled defaults, and the ambient
`.bobbin/config.toml` is not consulted (FR-29 ranks a flag above project and
user config). A `--config` path that does not exist is a loud error, never a
silent fall-back to discovery — so pointing the pre-edit guard at a scope file
with a mistyped path fails visibly instead of quietly enforcing the wrong scope.

`--verbose` raises the default tracing level from `info` to `debug`. `RUST_LOG`,
when set, still wins and can target individual modules, so the precedence is
`RUST_LOG` > `--verbose` > the `info` default.

## Examples

```bash
yupana analyze src
yupana analyze src --at main     # structure of the tree at a baseline commit (FR-13)
yupana refs authenticate src --json
yupana status                    # resolves base_ref to a commit SHA (in a git repo)
yupana impact authenticate src --hops 5   # lookup is by bare symbol name, not file::symbol
yupana communities src --json         # symbol clusters, largest-first
yupana verify --file src/auth.rs --buffer /tmp/edited.rs
yupana promote --dry-run                  # would this projection conform? writes nothing
yupana promote --to $QUIPU --commit HEAD  # the write; --to is what authorizes it
```

## `yupana promote` — what authorizes the write

**`--to` is the only thing that authorizes a promotion.** A configured
`[yupana.quipu] endpoint` is deliberately *not* enough, and a bare `yupana promote`
refuses even where one is set — naming the endpoint it found and both remedies:

```console
$ yupana promote
Error: refusing to promote into a DISCOVERED endpoint: http://quipu.example
  `[yupana.quipu] endpoint` is configured so the pre-edit guard can READ the rule
  catalogue. It does not authorize a write, and a promotion is a live graph write
  with no undo.
  To write there, say so:        yupana promote --to http://quipu.example
  To check it without writing:   yupana promote --dry-run
```

That asymmetry is the point. The endpoint key is set once, deployment-wide, so
the *guard* can read the governed rule catalogue on every edit — a READ. Letting
it double as a write target meant a bare `promote` from any checkout in scope of
that config posted tens of thousands of triples into the live graph, and there is
no undo (`/episode/retract` is episode-scoped and does not unwind a promotion).
It was found by an operator who ran it expecting a dry run (aegis-o2h97). One
config supplying both a read capability and a write capability is the defect;
requiring the write to be spelled out on the command line is the fix.

The MCP `yupana_promote` tool keeps its own fallback to the configured endpoint:
there the target comes from a server an operator deliberately started and pointed
somewhere, not from whatever config happens to be ambient in an agent's shell.

## `yupana promote --dry-run` — validate without writing

`--dry-run` extracts the projection and runs the **same** SHACL gate a real
promotion runs, then stops. It needs no target — validation is in-process — and
reports the graph a real run *would* have written to:

```console
$ yupana promote --dry-run
  DRY RUN — conforms. WROTE NOTHING.
    would post: 4041881 bytes of Turtle in 4 chunk(s)
    would target: http://quipu.example/knot
```

A non-conforming projection produces the identical refusal (and identical
retained payload) that `promote` would produce, so a dry run cannot green-light
something the write path would reject.

## `yupana promote` — diagnosing a refusal

A promotion is all-or-nothing: one SHACL violation refuses the whole commit and
writes nothing. So the refusal has to say enough to act on, because the
projection it refused is generated on the fly and would otherwise be gone.

```console
$ yupana promote --to $QUIPU
  REFUSED — promotion did not pass SHACL, wrote nothing:
    - MaxCount(1) not satisfied — on …/code/yupana/src%2Fmcp%2Fstate_tools.rs::StateIngestRequest (path …/symbolKind)
    payload retained at: /tmp/yupana-promote-yupana-promote-yupana-45c7b660….ttl
```

Two things make that actionable, and both are load-bearing:

- **The violation names its focus node and property path.** A bare
  `MaxCount(1) not satisfied` is true of every `maxCount` shape in the file and
  identifies nothing.
- **The refused projection is written to disk**, so you can read the document
  that failed. Parse errors are positional (`line 8656`) and the line MOVES
  between runs because the content is regenerated — without the payload, the one
  fact the error gives you is unusable.

The dump also lands on an invalid-Turtle failure and on a partial chunked write.
It goes to `$YUPANA_PROMOTE_DUMP_DIR`, else the system temp dir.

The name carries the repo and the resolved commit, so a scheduled promotion
refusing the same commit hour after hour overwrites **one** file. A *different*
failing commit gets its own dump — the SHA is what tells you which projection
you are holding, and reusing one name would overwrite the payload you were still
reading. So the bound is distinct failing commits, not runs.

Retention is best-effort: a promotion that is already failing never fails
*differently* because a dump could not be written.

## `yupana status`

Reports the resolved baseline, the tiers this binary serves, and — the part a
script gates on — **the state of the policy rule plane**.

```console
$ yupana status
  policy      : mode=advise  scope=none for this tenant
  mode source : ~/.config/bobbin/config.toml
  tamper state: TAMPER-EVIDENT, NOT TAMPER-PROOF — a local agent can alter policy; a clean report is no evidence that tampering was prevented
  rule set    : 7 projected from quipu (0 structural, 7 text) + 0 local
  rule digest : sha256:3fb9f179b9229755 (unsigned)
```

`rule digest` answers *which* rule set is live, not just how many rules there
are: two hosts showing different digests are enforcing different policy, which
the counts alone cannot reveal. It is computed over the fields that decide
enforcement (rule identity, pattern, tier, exemptions), so a rationale typo does
not churn it. It is **not** a version — there is no signed rule set yet, and
`verification` reports `unsigned` to say so rather than implying provenance
yupana does not have.

`mode source` identifies the layer that set the effective mode. If a
workspace-writable `.bobbin/config.toml` lowers a user policy (for example,
`enforce` to `off`), status calls it out as **LOWERED** and exits non-zero. This
does not claim that a local agent was prevented from changing a local file: the
surface is deliberately **tamper-evident, not tamper-proof**. Read a clean
report as “no evidence of tampering”, never as proof it was prevented.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Rule plane `loaded`, `empty`, or `off` |
| `3` | Rule plane **`degraded`** — the rules could not be projected |
| `4` | Workspace config lowered the user's policy mode |

Exit `3` is the one that matters. In that state the guard **fails open** on
every edit: nothing is enforced, and before this had an exit code the condition
printed in red and exited `0`, so nothing could gate on it and a human had to
happen to look. `empty` and `off` deliberately exit `0` — a graph with no rules
is a true, quiet answer, and failing on it would make the code useless in every
tree that has no governed policy yet.

`status` retries the projection three times before declaring `degraded`
(`attempts` is reported, so a flap stays visible). The pre-edit hook does **not**
retry: it runs on every edit across the fleet and its latency ceiling is the
reason it exists. A status surface that went red once in ten runs from a
transient blip would be routed around, and then the real red would be invisible
too.

## `yupana refs`

Definition sites of a symbol, by name, from the same graph `callers`, `impact`
and `communities` read — so every language this build has a grammar for is
searched, not just Rust. (Grammars beyond Rust need the `langs-extra` feature;
`yupana status` lists what a given binary can parse.)

```console
$ yupana refs derive_agents
quipu.py:1 derive_agents (function) [TreeSitter]
```

A zero result says **which** kind of nothing it is, because the two are not the
same fact:

```console
$ yupana refs no_such_symbol           # a populated graph, name genuinely absent
no definition found for no_such_symbol (searched 412 symbol(s))

$ yupana refs anything ./docs          # nothing here was parseable at all
no definition found for anything (nothing parseable under ./docs — the graph is
empty, so this is not evidence the symbol is absent)
```

Under `--json` the same distinction is `count` against `searched_symbols`, and
the answer carries its `tier` whether or not it found anything (FR-3):

```json
{
  "symbol": "derive_agents",
  "count": 1,
  "definitions": [
    {
      "file": "quipu.py",
      "name": "derive_agents",
      "kind": "function",
      "start_line": 1,
      "end_line": 2,
      "tier": "treesitter"
    }
  ],
  "searched_symbols": 412,
  "tier": "treesitter"
}
```

### Resolving by position

Name lookup over-connects on common names — `build`, `new`, `write`. When you
are reading code you know *where* you are, not which of the twelve it is, so
point at it instead (FR-4):

```console
$ yupana refs build                 # by name: ambiguous
a.rs:3 build (function) [TreeSitter]
a.rs:7 build (function) [TreeSitter]

$ yupana refs --at a.rs:7           # by position: the one you pointed at
a.rs:7 build (function) [TreeSitter]
```

`--at FILE:LINE` resolves to the **innermost symbol enclosing that line** and
answers with that symbol alone — it does not re-expand to every symbol sharing
its name, which would restore the ambiguity the position was given to remove.
`FILE` is relative to the search path; with `--at`, the positional argument is
the search path rather than a symbol name.

A **column is refused, not ignored**:

```console
$ yupana refs --at a.rs:3:9
error: --at takes FILE:LINE, not FILE:LINE:COL (got column `9`). The tree-sitter
tier resolves to the innermost symbol on a LINE; column-precise resolution needs
the LSP tier (FR-2), which is not built. Retry as `a.rs:3`.
```

Accepting the column and answering for the line would serve a line-precise
answer to a column-precise question — an approximation presented as the finer
tier, which FR-3 forbids. Two symbols on one line are not separable at the
tree-sitter tier, and you are told so rather than handed a guess.

A position that resolves to nothing explains which kind of nothing it is,
rather than borrowing the vocabulary of "no such symbol":

```console
$ yupana refs --at a.rs:3
no symbol encloses a.rs:3 — it falls between definitions (a blank line, an
import, a top-level comment). `a.rs` defines: one:1-1, two:5-5
```

`refs` answers "where is this **defined**". For "what **reaches** this", use
`yupana callers`; for the transitive form, `yupana impact`.

## `yupana verifier` and `yupana verdicts`

Both require the `quipu` feature. The guard signs a verdict the moment a
constraint fires and appends it locally; these are the registration and the
drain. Full field list and the reasoning in
[The Enforcement Trace](enforcement-trace.md).

```bash
yupana verifier --key-path yupana-signing.pk8   # public key to register in quipu
yupana verdicts --to http://localhost:7878    # drain the spool
```

`yupana verifier` is the **deliberate key-creation act** — it mints the signing key
if absent. The hook path never does: a signing identity materialising from an
agent's edit is not something that should happen quietly.

`yupana verdicts` truncates the spool only when every verdict was accepted, so a
partial drain leaves the remainder intact rather than losing it.

## `yupana verify`

Checks a *proposed* buffer against the graph Yupana already holds and returns a
boolean verdict plus violations (FR-23/FR-24). Exits **non-zero** when the buffer
has violations, so scripts and CI can gate on it.

```console
$ yupana verify --file src/a.rs --buffer /tmp/proposed.rs
violations src/a.rs [TreeSitter]
  ghost:2 `ghost` is called here but is defined nowhere in this buffer or the
          project graph, and is not brought into scope by a `use`.
  takes_two:2 `takes_two` is called with 1 argument(s) but is defined at line 1
          taking 2.
```

Only violations the edit *introduces* are reported: the file's current contents
are the baseline, so pre-existing breakage is not blamed on this edit.

**Read the `unchecked` list before trusting a clean verdict.** At the tree-sitter
tier there is no type information and no name resolution, so:

| Violation (FR-23) | At this tier |
|---|---|
| `identifier-does-not-exist` | free calls only, and only ones the edit introduces |
| `wrong-arity` | free calls resolving to exactly one known definition |
| `unresolved-import` | bodiless `mod foo;` with no sibling file |
| `type-violation` | **not checked** — needs the LSP tier |

The bias is against false positives throughout: method calls, path-qualified
calls, imports, locals, closures, and function-typed parameters are all left
alone rather than guessed at. `ok: true` means "nothing this tier can see is
wrong", not "this compiles".

`yupana status` resolves the configured `base_ref` (default `main`) to a concrete
commit via the system `git`; outside a git repository the base commit shows as
unresolved and Yupana falls back to the working tree.

`yupana analyze --at <ref>` builds the summary from the **git tree** at a baseline
commit (FR-13) rather than the working copy — the shared read-only base the
Phase-3 resident graph will hold. It reads blob content at the ref (never the
working tree), and degrades to an empty result outside a repo or for an
unresolved ref.

`yupana communities` partitions the call graph into densely-connected symbol
clusters using deterministic Louvain (FR-9) — the same partition on every run,
no RNG. Communities are ordered largest-first; members carry a `tier` tag.
Quipu runs community detection over *committed* facts; Yupana computes it live
over the hot graph.

Commands marked with a phase print a notice until their engine lands; see the
[Specification](../design/specification.md) §12.

## `yupana exemplar`

Drafts the raw material of a governed policy from an observed instance —
policy-by-example, yupana's half. Point it at the offending text (with the file
it appeared in, so the Selector can name the structural context), or at a
verdict spool to read the newest denial:

```console
$ yupana exemplar "ABC-123" --file src/a.rs
$ yupana exemplar --spool ~/.local/state/yupana/verdicts.jsonl --policy no-ticket-in-comment
```

The output is JSON for quipu's drafting scaffold: a Selector draft (the
enclosing node kind as a tree-sitter query) and predicate candidates at each
viable tier — the exact token(s) for membership (the only hard-capable tier),
a generated narrowing pattern **offered for human approval**, and the
exemplar's embedding as a similarity anchor with a suggested threshold that
quipu's backtest replaces. Nothing emitted is a policy; quipu's
definition-time placement check remains the refusal authority.
