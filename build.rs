//! Build script — only does work for `wasm32-unknown-unknown`.
//!
//! Tree-sitter's C runtime (and every grammar's `parser.c`/`scanner.c`) needs
//! a libc to compile and link against, and `wasm32-unknown-unknown` ships
//! none. The recipe, proven against tree-sitter 0.25:
//!
//! 1. **Headers** — the C sources are compiled against wasi-libc's headers
//!    with `__wasi__` defined (tree-sitter's own upstream wasm guard: it
//!    drops the `dup()`-based dot-graph path under that define). This half is
//!    the builder's job because `cc` env vars cannot be set from here:
//!    `CFLAGS_wasm32_unknown_unknown="-isystem <wasi-include> -D__wasi__"`.
//!    Missing flags fail the build loudly at compile time, so the recipe
//!    cannot be half-applied silently.
//! 2. **Definitions** — the pure in-memory libc functions tree-sitter calls
//!    (`malloc`/`free`/`calloc`/`realloc`, `snprintf`/`vsnprintf` — load-
//!    bearing for `ts_node_string` — and the `isw*`/`tow*` classifiers) are
//!    linked from wasi-libc's `libc.a`, emitted below.
//! 3. **Syscalls** — the WASI syscall imports that `libc.a` would add are
//!    pinned to Rust stubs in `src/wasm_shim.rs`, so the final module is
//!    self-contained (zero imports).
//!
//! `YUPANA_WASI_LIBC_DIR` overrides the `libc.a` directory; the default is
//! where Debian/Ubuntu's `wasi-libc` package puts it.

fn main() {
    println!("cargo:rerun-if-env-changed=YUPANA_WASI_LIBC_DIR");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        let libdir = std::env::var("YUPANA_WASI_LIBC_DIR")
            .unwrap_or_else(|_| "/usr/lib/wasm32-wasi".to_string());
        println!("cargo:rustc-link-search=native={libdir}");
        println!("cargo:rustc-link-lib=static=c");
    }
}
