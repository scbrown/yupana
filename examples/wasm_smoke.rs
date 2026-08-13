//! Linked wasm smoke artifact — proves the browser build end to end.
//!
//! `cargo build --lib --target wasm32-unknown-unknown` only type-checks an
//! rlib; the claims that matter to an embedder — the C runtime links, and the
//! final module has **zero imports** — are only testable on a fully linked
//! cdylib. This example is that cdylib. `scripts/check-wasm-smoke.mjs`
//! instantiates it with an empty import object and drives a real tree-sitter
//! extraction through `wasm_smoke()`; `just wasm check` (and the CI `wasm`
//! job) run the pair.
//!
//! The exports are a raw C ABI on purpose: yupana itself carries no
//! wasm-bindgen dependency. Ergonomic JS bindings belong to the embedder
//! (creel's `wasm/yupana-provider`), which wraps the same library surface.
//!
//! `unsafe` at the FFI boundary is unavoidable; the deny is relaxed here as
//! in `src/wasm_shim.rs`.
#![allow(unsafe_code)]

/// Extract symbols from a fixed Rust source and return the symbol count.
///
/// Exercises the full pipeline an embedder relies on: grammar load, a C-side
/// parse (malloc/isw* from wasi-libc), query-driven extraction, and Rust-side
/// fact building. The Node checker asserts the exact expected count so a
/// silently-degraded parse (e.g. a broken scanner) cannot pass as green.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_smoke() -> u32 {
    let source = r#"
fn main() { helper(); }
fn helper() {}
struct Widget { size: u32 }
impl Widget {
    fn grow(&mut self) { self.size += 1; }
}
"#;
    let symbols = yupana::extract::extract_symbols(source, "rust").expect("extract");
    u32::try_from(symbols.len()).expect("symbol count fits u32")
}

/// `ts_node_string` reaches libc's real `snprintf`; return the s-expression
/// length so the checker proves the printf family linked correctly (a stubbed
/// snprintf would return 0 or a garbage length). Yupana's own surface never
/// formats s-expressions, but embedders holding raw tree-sitter nodes do.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_smoke_sexp_len() -> u32 {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("grammar");
    let tree = parser.parse("fn main() {}", None).expect("parse");
    u32::try_from(tree.root_node().to_sexp().len()).expect("sexp length fits u32")
}
