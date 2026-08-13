//! C-runtime shims for `wasm32-unknown-unknown`.
//!
//! Tree-sitter's C runtime and the grammar scanners are compiled against
//! wasi-libc headers (see `build.rs`), and the pure in-memory pieces of libc —
//! `malloc`, `snprintf`, the `isw*` family — are linked from wasi-libc's
//! `libc.a`. What libc cannot honestly provide in a browser is the syscall
//! layer, so this module pins those symbols to Rust definitions at link time:
//!
//! - `abort` / `__assert_fail` become Rust panics (surfaced to the embedder as
//!   a wasm trap with a message, instead of a WASI `proc_exit` import);
//! - `clock_gettime` reports a frozen clock (tree-sitter reads it only for
//!   parse timeouts, which yupana does not set);
//! - the six `fd_*` WASI syscalls reachable from tree-sitter's dot-graph debug
//!   printer answer `EBADF`, making that path inert rather than importing
//!   `wasi_snapshot_preview1`.
//!
//! The net effect is a self-contained module: `WebAssembly.Module.imports()`
//! on a build of this crate lists no libc or WASI imports, so any embedder
//! (creel's in-page provider, a Node test, wasm-bindgen glue) can instantiate
//! it without supplying a C environment.
//!
//! `unsafe` is unavoidable here (raw C pointers at the FFI boundary), so the
//! crate-level `unsafe_code = "deny"` is relaxed for this module alone.
#![allow(unsafe_code)]

/// C `abort(3)`: trap with a message instead of importing `proc_exit`.
#[unsafe(no_mangle)]
pub extern "C" fn abort() -> ! {
    panic!("C abort() called (tree-sitter runtime)")
}

/// C `__assert_fail`, the assert(3) failure hook: trap instead of writing to
/// stderr (which a browser module does not have) and aborting.
#[unsafe(no_mangle)]
pub extern "C" fn __assert_fail(
    _assertion: *const u8,
    _file: *const u8,
    _line: u32,
    _function: *const u8,
) -> ! {
    panic!("C assertion failure (tree-sitter runtime)")
}

/// C `clock_gettime(3)`: a frozen clock. Tree-sitter consults it only to
/// enforce parse timeouts; yupana sets none, so time never needs to advance.
/// Writes `{0, 0}` as the `timespec` (two 64-bit words on wasm32 wasi-libc).
#[unsafe(no_mangle)]
pub extern "C" fn clock_gettime(_clock_id: i32, timespec: *mut u64) -> i32 {
    if !timespec.is_null() {
        unsafe {
            *timespec = 0;
            *timespec.add(1) = 0;
        }
    }
    0
}

/// WASI errno `EBADF` — the honest answer from a runtime with no file
/// descriptors.
const EBADF: i32 = 8;

/// `fd_close` stub (reachable via tree-sitter's dot-graph `fclose`).
#[unsafe(no_mangle)]
pub extern "C" fn __imported_wasi_snapshot_preview1_fd_close(_fd: i32) -> i32 {
    EBADF
}

/// `fd_fdstat_get` stub (reachable via `fdopen`).
#[unsafe(no_mangle)]
pub extern "C" fn __imported_wasi_snapshot_preview1_fd_fdstat_get(_fd: i32, _stat: i32) -> i32 {
    EBADF
}

/// `fd_fdstat_set_flags` stub (reachable via `fdopen`).
#[unsafe(no_mangle)]
pub extern "C" fn __imported_wasi_snapshot_preview1_fd_fdstat_set_flags(
    _fd: i32,
    _flags: i32,
) -> i32 {
    EBADF
}

/// `fd_read` stub (reachable via stdio buffering).
#[unsafe(no_mangle)]
pub extern "C" fn __imported_wasi_snapshot_preview1_fd_read(
    _fd: i32,
    _iovs: i32,
    _iovs_len: i32,
    _nread: i32,
) -> i32 {
    EBADF
}

/// `fd_seek` stub (reachable via stdio buffering).
#[unsafe(no_mangle)]
pub extern "C" fn __imported_wasi_snapshot_preview1_fd_seek(
    _fd: i32,
    _offset: i64,
    _whence: i32,
    _newoffset: i32,
) -> i32 {
    EBADF
}

/// `fd_write` stub (reachable via tree-sitter's dot-graph `fprintf`).
#[unsafe(no_mangle)]
pub extern "C" fn __imported_wasi_snapshot_preview1_fd_write(
    _fd: i32,
    _iovs: i32,
    _iovs_len: i32,
    _nwritten: i32,
) -> i32 {
    EBADF
}
