<p align="center">
  <img src="assets/logo.svg" width="200" alt="Yupana logo: an Incan counting board, five place-value rows of seeded and empty compartments"/>
</p>

<h1 align="center">yupana</h1>

<p align="center">
  <em>🧵 Know what a change will break before you make it</em>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache-2.0"/></a>
  <a href="https://github.com/scbrown/yupana/actions/workflows/ci.yml"><img src="https://github.com/scbrown/yupana/actions/workflows/ci.yml/badge.svg" alt="CI"/></a>
  <a href="https://github.com/scbrown/caboodle"><img src="https://img.shields.io/badge/stack-quipu-8B5E3C.svg" alt="Part of the quipu stack"/></a>
</p>

**Yupana reads a codebase and answers structural questions about it: what calls
this function, what would changing it break, and where does this value flow.**
It is for people and AI coding agents (Claude Code, Codex, Cursor) who are about
to edit code they did not write and want the blast radius first. It runs as a
command-line tool, as an MCP server an agent can call, and as editor hooks that
warn at the moment of an edit.

A [yupana](https://en.wikipedia.org/wiki/Yupana) is the Andean counting board
that worked alongside the quipu: the quipu recorded, the yupana computed.

## Why you would want it

- **Blast radius in one command.** `yupana impact <symbol>` lists everything a
  change reaches, hop by hop, instead of a text search that misses callers and
  finds comments.
- **Cheap for agents.** An agent asks for structure instead of reading whole
  files into its context.
- **Rules at edit time.** With [Quipu](https://github.com/scbrown/quipu), the
  same graph checks a proposed edit against your team's rules before it lands.

More on the design, and how it compares with LSP, Joern and embedding search:
[Why Yupana](https://scbrown.github.io/yupana/concepts/why-yupana.html).

## Install

**Linux x86_64 and macOS (Apple Silicon or Intel): download the release.**
No Rust toolchain needed. Mac archives are available starting with the first
release containing the native macOS build workflow.

```bash
V=$(curl -fsSL https://api.github.com/repos/scbrown/yupana/releases/latest \
  | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p')
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) PLATFORM=x86_64-linux-gnu ;;
  Darwin-arm64) PLATFORM=aarch64-apple-darwin ;;
  Darwin-x86_64) PLATFORM=x86_64-apple-darwin ;;
  *) echo 'Build from source on this platform'; exit 1 ;;
esac
ARCHIVE="yupana-v$V-$PLATFORM.tar.gz"
curl -fsSLO "https://github.com/scbrown/yupana/releases/download/v$V/$ARCHIVE"
curl -fsSLO "https://github.com/scbrown/yupana/releases/download/v$V/$ARCHIVE.sha256"
shasum -a 256 -c "$ARCHIVE.sha256"
mkdir -p ~/.local/bin && tar -xzf "$ARCHIVE" -C ~/.local/bin
yupana --version
```

The archive also contains `hank`, the tool's former name, as a symlink.

**Other platforms, or releases without your platform archive: build from source** with a
[Rust toolchain](https://rustup.rs):

```bash
cargo install --git https://github.com/scbrown/yupana --locked --features mcp,langs-extra
yupana --version
```

`mcp` adds the agent server and `langs-extra` adds every language beyond Rust.
If `yupana --version` prints an older version than you just installed, another
copy earlier on your `PATH` is winning; `which -a yupana` lists them in order.

For a developer checkout, `just install` reports
`0.12.0+local.<12-digit-commit>` (with `.dirty` for uncommitted changes).
On a shared crew host, a local install is temporary. To keep a fix, merge the
release-plz release PR so it ships in a published release. Release tracking
preserves a local build at the same version and replaces it when a newer
release publishes. Plain release builds retain the package version.

## First success in three commands

Make a tiny crate where `run` calls `load` and `load` calls `parse`:

```bash
mkdir -p demo/src && cd demo
printf 'pub fn parse(input: &str) -> usize {\n    input.trim().len()\n}\n\npub fn load(path: &str) -> usize {\n    parse(path) + 1\n}\n\npub fn run() -> usize {\n    load("config.toml")\n}\n' > src/lib.rs
```

Then build the graph, ask who calls `parse`, and ask what changing it would
break:

```bash
yupana analyze src
yupana callers parse src
yupana impact parse src
```

```text
analyzed 1 file(s), 3 symbol(s) [tree-sitter]
callers of parse:
  lib.rs:5 load
callees of parse: (none)
impact 2 symbol(s) affected by changing parse:
  lib.rs:5 load (hop 1)
  lib.rs:9 run (hop 2)
```

`run` never calls `parse` directly, yet changing `parse` can still break it.
That second hop is what a text search for `parse(` does not show you.

## On your own code

Run these from the root of any repository; each takes the path to analyze.

| question | command |
|---|---|
| what is in this tree? | `yupana analyze .` |
| where is this symbol defined? | `yupana refs <symbol> .` |
| who calls it, and what does it call? | `yupana callers <symbol> .` |
| what would changing it break? | `yupana impact <symbol> . --hops 5` |
| where does this variable flow inside a function? | `yupana dataflow <function> . --var <variable>` |

Every command has `--help`. The full list is in the
[CLI reference](https://scbrown.github.io/yupana/reference/cli.html).

## Wire it into your agent

**As an MCP server.** `yupana serve` speaks MCP over stdio and analyzes the
directory it starts in. Register it by **absolute path**: MCP clients launch
servers without your shell's `PATH`, so a bare `yupana` fails with `ENOENT`.
For Claude Code, from your repository:

```bash
claude mcp add yupana -- "$(command -v yupana)" serve
```

Any other MCP client takes the same command, with the absolute path. The agent gets fifteen `yupana_*`
tools, for symbols, references, callers, impact and more; see the
[MCP tools reference](https://scbrown.github.io/yupana/reference/mcp-tools.html).

**As an edit hook.** Add this to `.claude/settings.json` and, after each edit,
the agent is told which other files call what it just changed:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [{ "type": "command", "command": "yupana hook post-edit" }]
      }
    ]
  }
}
```

The pre-edit rule guard, the session-start briefing and the Bash action hook
are in [Harness Integration](https://scbrown.github.io/yupana/getting-started/harness-integration.html).
Rules that come from Quipu are covered in the
[pre-edit policy guard](https://scbrown.github.io/yupana/reference/policy-guard.html).

## Before you start

**Platforms.** Releases ship Linux x86_64 and macOS (Apple Silicon and Intel)
archives. Anywhere else, use the source install above; it needs a Rust
toolchain and nothing else.

**Languages.** Rust is always built in. The `langs-extra` feature, included in
the release and in the source command above, adds the rest.

| language | extensions |
|---|---|
| Rust | `.rs` |
| TypeScript / JavaScript | `.ts` `.mts` `.cts` `.js` `.mjs` `.cjs` |
| TSX / JSX | `.tsx` `.jsx` |
| Python | `.py` `.pyi` |
| Go | `.go` |
| Java | `.java` |
| C / C++ | `.c` `.h` `.cc` `.cpp` `.cxx` `.hpp` `.hh` `.hxx` |

## What's next

- [The book](https://scbrown.github.io/yupana/): installation, configuration, concepts
  and reference, in reading order.
- [Map of all docs](https://scbrown.github.io/yupana/docs-map.html): where every design note,
  spec and research document lives, and which ones are historical.
- [Why Yupana](https://scbrown.github.io/yupana/concepts/why-yupana.html) and
  [how it works with Quipu and Bobbin](https://scbrown.github.io/yupana/concepts/the-stack.html).

## 🧺 The stack

Caboodle installs these together and proves each one works; every tool also stands alone.

| tool | what it gives your agents |
|---|---|
| [caboodle](https://github.com/scbrown/caboodle) | one wizard that installs the stack and proves it works |
| [quipu](https://github.com/scbrown/quipu) | a knowledge graph that refuses facts that break its rules |
| [camayoc](https://github.com/scbrown/camayoc) | the starter vocabulary, and how new knowledge earns its way in |
| [bobbin](https://github.com/scbrown/bobbin) | search and context over your repositories, served over MCP |
| [yupana](https://github.com/scbrown/yupana) **(you are here)** | which code calls which: the blast radius before an edit |
| [desire-path](https://github.com/scbrown/desire-path) | the tool calls your agents get wrong, so you can fix them |
| [seeds](https://github.com/scbrown/seeds) | the work your agents track, as facts in the graph with full history |

## Contributing

```bash
just setup    # install the pre-commit hooks
just check    # the full pre-push gate: fmt, clippy, markdown lint, links, file size
just test
```

Use `just`, not raw `cargo`. Conventions are in [`AGENTS.md`](AGENTS.md) and
[`CONTRIBUTING.md`](CONTRIBUTING.md); releases are described in
[`docs/RELEASING.md`](docs/RELEASING.md).

## 📜 License

Licensed under the Apache License, Version 2.0 (see [LICENSE](LICENSE)). Releases before 2026-09-24 were MIT-licensed. Copyright 2026 Steve Brown.
