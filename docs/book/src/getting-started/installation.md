# Installation

Yupana is a Rust project built with [`just`](https://github.com/casey/just).

## Prerequisites

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
just install
```

The install recipe builds the complete feature set into `~/.local/bin/yupana`
and installs `~/.local/bin/hank` as a relative symlink to that same executable.
Both names therefore report the same version and expose `exemplar`, `verifier`,
and `verdicts`; they can never silently select different feature builds. Set
`YUPANA_INSTALL_ROOT` to use another prefix.

## Install the git hooks

```bash
just setup            # installs pre-commit hooks
```
