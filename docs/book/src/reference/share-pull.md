# Pulling a share: `yupana share`

[Code Slices as Share Bundles](share-bundles.md) is the PRODUCER half — how a Yupana code slice
becomes a Quipu share bundle somebody else can hold. This page is the CONSUMER half: how a bundle
somebody hands you gets into the graph Yupana reads, and what it can and cannot do once it is there.

```bash
yupana share pull    <bundle-dir|url> --to <quipu-url>   # stage it, and say where you stand
yupana share policy  <staging-graph>  [--to <quipu-url>] # what policy would this contribute?
yupana share promote <share-id>       --to <quipu-url>   # admit it (the governance decision)
```

## The one property to understand first

**A pulled share is inert until you promote it.**

Quipu stages an import into a named graph — `urn:quipu:import:staging:<hash>`, or
`…:quarantine:<hash>` when the receiving store does not govern the vocabulary. Yupana's policy
projection — the queries the pre-edit guard sends to build its rule catalogue — carry **no `GRAPH`
clause**, so they cannot see a named graph at all.

That is what makes `pull` safe to run on a bundle from someone you have not audited. It is also
exactly the kind of claim that deserves a measurement rather than a reassurance, so here is the
measurement, taken against a real Quipu 0.3.36 with the guard's own query constant (not a
paraphrase of it), before and after promoting the same share:

| the guard's own unscoped text-rule query | rows |
|---|---|
| after `share pull` — staged, not promoted | **0** |
| the identical query scoped `GRAPH <staging>` | **1** |
| after `share promote` | **1** |

The middle row is the control. Two zeroes would prove nothing on their own — an endpoint that
answered nothing at all would look the same — so the scoped arm establishes that the triples are
present and reachable, and only the `GRAPH` clause separates "in the store" from "enforced".

## What each verb does

### `share pull` — always stages, never promotes

```console
$ yupana share pull ./bundle --to http://quipu.example
quarantined: 15 triples from ./bundle
  share:   sha256:a350c7a7…
  staged:  urn:quipu:import:quarantine:a350c7a7…
  blocked: off_vocabulary
  next:    /usr/local/bin/yupana share policy urn:quipu:import:quarantine:a350c7a7… --to http://quipu.example
```

**A quarantine is a success, and `pull` exits 0 for it.** Pulling from a publisher whose vocabulary
you do not govern is the DEFAULT case, not the exceptional one. If staging exited nonzero, every
script wrapping this verb would treat the correct outcome as a failure, and the eventual "fix" for
that is auto-promotion — which is the silent vocabulary widening the quarantine exists to prevent.

Nonzero is reserved for two things: a bundle that fails verification, and an endpoint that would not
serve the request.

Before anything is sent, Yupana checks the manifest's SHA-256 over `export.nt` and `shapes.ttl`
against the bytes it holds. Quipu's server checks them again — this is deliberate duplication,
because a local refusal is a *different finding*: it says the bundle you were handed is not the
bundle its manifest describes, which is a story about your download and not about the graph.

```console
$ yupana share pull ./tampered --to http://quipu.example
Error: share error: share graph hash MISMATCH for export.nt: manifest declares sha256:edd17844…,
the bytes here hash to sha256:1f181001…. Nothing was sent to quipu. …
$ echo $?
1
```

**Pulling the same bundle twice is a no-op.** Quipu keys the staged graph on the share's content
hash, so a byte-identical re-pull reports `unchanged` and writes nothing — measured, with the graph
count unmoved:

| | outcome | graphs |
|---|---|---|
| first pull | `quarantined`, 4 triples | 1 |
| identical re-pull | **`unchanged`**, 4 triples | **1** |

That matters because the honest response to a lost or ambiguous reply is to re-run the pull, and it
must not be possible for that to duplicate anything.

### `share policy` — what would this share contribute?

The share is staged and invisible to the guard. Before deciding whether to admit it, ask what
admitting it would mean:

```console
$ yupana share policy urn:quipu:import:staging:a350c7a7… --to http://quipu.example
policy this share would contribute to urn:quipu:import:staging:a350c7a7…:
  structural policies: 1
    - publisher rule: no TODO in comments [deny]
  text rules:          1
    - no-internal-host
```

This runs Yupana's **own** projection queries and decoders, scoped into the staging graph — not a
second query written to resemble them. The catalogue's definition of "a policy" is intricate, and a
hand-written preview would be a parallel definition that drifts silently, showing you a different
rule set from the one that will actually be enforced. Reusing the constants means the preview is
wrong only if the guard is wrong too.

One consequence worth knowing: if the publisher ships a policy whose vocabulary Yupana cannot
decode, this verb **fails loudly** rather than reporting zero policies. A share carrying rules your
tooling does not understand is a thing you want to be told about.

`share policy` is a read, so it falls back to the configured `[yupana.quipu] endpoint`. `pull` and
`promote` are writes and require `--to` — that key is set host-wide so every agent's pre-edit guard
can fetch the rule catalogue, and a read credential must not silently become a write one.

### `share promote` — the governance decision

```console
$ yupana share promote sha256:a350c7a7… --to http://quipu.example
promoted sha256:a350c7a7…: 15 triples admitted at tx 8
```

Quipu refuses to promote a share that is still blocked, and Yupana relays the refusal rather than
dressing it up:

```console
$ yupana share promote sha256:2b9610a4… --to http://quipu.example
Error: share error: … refused with 400: {"error":"no eligible staged import for sha256:2b9610a4…"}
$ echo $?
1
```

To clear an `off_vocabulary` block, the receiving store must govern the types the publisher used.
That is a deliberate act on the receiving store — adopting a publisher's vocabulary is a governance
change, and Yupana does not perform it as a side effect of fetching bytes. Load the shapes you
intend to govern (the bundle ships its own `shapes.ttl` for exactly this purpose), then pull again:
the same bundle that quarantined will come back `staged`.

## What `pull` does NOT take, and why

**A `.qpack.db`, and a bundle archive.** Both are refused by name, with the command that does work:

```console
$ yupana share pull ./graph1.qpack.db --to http://quipu.example
Error: share error: ./graph1.qpack.db is a packed graph (`.qpack.db`), and yupana cannot load it:
quipu's `unpack`/`pack` exist only on the CLI against a LOCAL store, and yupana talks to a quipu
SERVER over HTTP. Load it with the quipu CLI on the machine holding the store:
    quipu pack --verify ./graph1.qpack.db
    quipu unpack ./graph1.qpack.db --db <store.db>
```

This is an architectural fact rather than a missing feature. Quipu's `pack`/`unpack` have no REST
route — they operate on a local store file — and Yupana has no local store: every Quipu interaction
it has, including `promote` and the policy projection, is HTTP to an endpoint whose database lives
somewhere else. There is no honest way for Yupana to unpack a pack, and a verb that accepted one and
failed at the far end would be worse than one that says so up front.

The refusal names commands that run. Yupana emits **no** suggestion it has not made runnable: every
emitted command names an absolute path to the running binary and the endpoint that just worked, and
the test suite parses each one with the real CLI definition rather than asserting on its spelling.

## Where the endpoint and credentials come from

| | resolution |
|---|---|
| endpoint | `--to` (required for `pull`/`promote`); `[yupana.quipu] endpoint` for `share policy` |
| bearer | `QUIPU_AUTH_TOKEN`, else `QUIPU_AUTH_TOKEN_FILE`, else `~/.config/quipu/token` |

`/import` and `/import/promote` are write endpoints and need Quipu's bearer; reads on the same
endpoint stay open, so a working `yupana impact` proves nothing about whether a pull will be
authorized. A 401 says so explicitly rather than surfacing as a generic failure — and a 404 is
reported as "this Quipu has no `/import` route" rather than as a failed import, because those want
opposite responses from the reader.
