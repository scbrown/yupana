#!/usr/bin/env node
// Verifier for the wasm32 build (pairs with examples/wasm_smoke.rs).
//
// Asserts the two properties an embedder depends on and a bare
// `cargo build --target wasm32-unknown-unknown` cannot check:
//   1. the linked module is SELF-CONTAINED — zero imports, so it
//      instantiates with an empty import object (no libc, no WASI shim);
//   2. a real tree-sitter extraction runs inside it, including the
//      libc-backed paths (malloc from wasi-libc, snprintf via to_sexp).
//
// Usage: node scripts/check-wasm-smoke.mjs [path-to-wasm]
// Default path matches `just wasm build`'s output.

import { readFileSync } from "node:fs";

const path =
  process.argv[2] ??
  "target/wasm32-unknown-unknown/release/examples/wasm_smoke.wasm";

const module_ = new WebAssembly.Module(readFileSync(path));

const imports = WebAssembly.Module.imports(module_);
if (imports.length !== 0) {
  console.error("FAIL: module is not self-contained; imports:", imports);
  process.exit(1);
}

const { exports } = new WebAssembly.Instance(module_, {});

// examples/wasm_smoke.rs source defines main, helper, Widget, grow.
const symbols = exports.wasm_smoke();
if (symbols !== 4) {
  console.error(`FAIL: expected 4 extracted symbols, got ${symbols}`);
  process.exit(1);
}

// to_sexp of `fn main() {}` is "(source_file (function_item name: (identifier)
// parameters: (parameters) body: (block)))" — 87 chars (verified native).
// A stubbed snprintf would produce 0 or a garbage length.
const sexpLen = exports.wasm_smoke_sexp_len();
if (sexpLen !== 87) {
  console.error(`FAIL: expected sexp length 87, got ${sexpLen}`);
  process.exit(1);
}

console.log(
  `wasm smoke ok: zero imports, ${symbols} symbols extracted, sexp length ${sexpLen}`,
);
