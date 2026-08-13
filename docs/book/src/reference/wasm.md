# Browser / WASM Builds

Yupana's analysis core — the tree-sitter extractors, the in-memory graph,
the typed fact model, and the structural rules — compiles for
`wasm32-unknown-unknown`, so an embedder can run real extraction inside a
browser with no server behind it. The first consumer is
[creel](https://github.com/scbrown/creel)'s in-page provider, which gives
its browser-tab agents structural facts about the code they are writing
(the same shape of signal the native engine serves over MCP), next to the
quipu knowledge graph that already runs in-page there.

## What the wasm build carries — and what it honestly cannot

The boundary is the **target**, not a Cargo feature. A feature can be
toggled into a lie (the removed `lsp`/`cpg` flags proved that); a target
`cfg` cannot — code that needs an OS simply does not exist in the wasm
artifact.

Present on wasm32 (the library surface, called directly):

- `extract` — symbols, call sites, import refs, structural queries, for
  every grammar the build includes (`langs-extra` works on wasm too)
- `graph` — the petgraph base graph, tenant overlays, reachability
- `types`, `errors`, `config`, `rules`, `textrules` — the fact model and
  both rule planes

Absent on wasm32, by `#[cfg(not(target_arch = "wasm32"))]`:

- `cli` (and the `serve` command's tokio runtime) — a browser has no argv
- `watch` — `notify` is an inotify/FSEvents adapter; in a browser, edits
  arrive as explicit overlay touches, not filesystem events
- `render` — terminal output, gated with its only consumer

`tokio` and `notify` are declared under
`[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`, so they are
not merely unused on wasm — they are not in the build graph at all.

Filesystem-shaped APIs that *compile* on wasm (e.g.
`CodeGraph::build(root)`, which walks a directory) trap at runtime if
called there; browser embedders feed sources through the in-memory
entrypoints (`extract_structure(source, language)` and tenant overlay
touches) instead.

## The C runtime recipe

Tree-sitter's runtime and every grammar's `parser.c`/`scanner.c` are C,
and `wasm32-unknown-unknown` ships no libc. The recipe (proven against
tree-sitter 0.25, all six grammars):

1. **Headers.** The C sources compile against wasi-libc's headers with
   `__wasi__` defined — tree-sitter's own upstream wasm guard, which also
   drops its `dup()`-based dot-graph path:

   ```bash
   export CFLAGS_wasm32_unknown_unknown="-isystem /usr/include/wasm32-wasi -D__wasi__"
   ```

2. **Definitions.** The pure in-memory libc functions tree-sitter calls
   (`malloc`/`free`/`calloc`/`realloc`, `snprintf`/`vsnprintf` — load-
   bearing for `ts_node_string` — and the `isw*`/`tow*` classifiers) are
   linked from wasi-libc's `libc.a`. `build.rs` emits the link flags;
   `YUPANA_WASI_LIBC_DIR` overrides the default library dir
   (`/usr/lib/wasm32-wasi`, where Debian/Ubuntu's `wasi-libc` package
   puts it).

3. **Syscalls.** The WASI syscalls `libc.a` would import are pinned to
   Rust stubs in `src/wasm_shim.rs`: `abort`/`__assert_fail` become
   panics (wasm traps), `clock_gettime` reports a frozen clock (read only
   for parse timeouts, which yupana does not set), and the six `fd_*`
   calls reachable from the dot-graph debug printer answer `EBADF`.

The result is a **self-contained module**: zero imports, instantiable
with an empty import object — no libc, no WASI shim, no JS glue
obligations for the embedder.

## Building and verifying

```bash
rustup target add wasm32-unknown-unknown
sudo apt-get install wasi-libc        # headers + libc.a

just wasm build                # lib + linked smoke cdylib, release
just wasm check                # build, then the Node verifier
just wasm check langs-extra    # same, with all six grammars
```

A bare `cargo build --lib` only type-checks an rlib, so the properties an
embedder depends on are asserted on a fully **linked** cdylib
(`examples/wasm_smoke.rs`): `scripts/check-wasm-smoke.mjs` instantiates
it with `{}` — which fails loudly if any import exists — then drives a
real extraction and an `to_sexp` call through it and checks exact
results against native behavior. CI runs `just wasm check` for both
grammar sets on every push (the `wasm` job), so the target cannot ship
dark.

## Embedding

Yupana deliberately carries no wasm-bindgen dependency; the smoke example
exports a raw C ABI only. Ergonomic JS bindings belong to the embedder —
see creel's `wasm/yupana-provider`, which wraps the in-memory extraction
surface with wasm-bindgen the same way its `wasm/quipu-provider` wraps
quipu's store.
