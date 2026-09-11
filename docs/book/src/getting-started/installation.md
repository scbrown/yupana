# Installation

Yupana is a Rust project built with [`just`](https://github.com/casey/just).

## Source-build prerequisites

- Rust (stable) — the project targets edition 2021.
- A C compiler (`cc`/`gcc`) — tree-sitter grammars compile a small C parser.
- `just`, and for docs `mdbook`, `npx` (markdownlint/prettier), and `vale`.

## Build from source

```bash
git clone https://github.com/scbrown/yupana
cd yupana
just build            # or: cargo build --release
```

The binary is produced at `target/debug/yupana` (or `target/release/yupana`).

## Install locally

```bash
just install-release 0.8.0

# Developer build from this checkout (includes uncommitted source):
just install
```

`install-release` downloads the published Linux x86_64 archive and verifies its
`.sha256`, version, and `exemplar`, `verifier`, and `verdicts` capabilities. It
installs the exact archive binary without compiling the checkout. This path
requires curl, Python 3, `flock`, and Linux file utilities, but not Rust.

`install` builds the current checkout, including uncommitted changes. Its banner
identifies the source commit and dirty state. Dirty shared checkouts with linked
worktrees are refused; use your own worktree for source development.

Both paths publish atomically under an install lock to `~/.local/bin/yupana`
and install `~/.local/bin/hank` as a relative symlink to that executable. Set
`YUPANA_INSTALL_ROOT` to use another prefix. To roll back a release, run
`just install-release` with the previous version.

## Install the git hooks

```bash
just setup            # installs pre-commit hooks
```
