# yupana
# Run `just --list` to see available recipes

# Quiet by default to save context; use verbose=true for full output
verbose := "false"

# Default recipe - show available commands
default:
    @just --list

# === Setup ===

# Install pre-commit hooks
setup:
    pre-commit install
    @echo "Setup complete."

# === Quality ===

# Run all quality checks (pre-push gate)
check:
    pre-commit run --all-files

# === Rust ===

# Build the project
build:
    cargo build

# Run tests
test *args="":
    cargo test {{args}}

# Run the linter (matches CI: deny warnings, allow missing-docs)
# --all-targets so TESTS are linted too. Without it the lint gate skipped every
# test target, and the lints hiding there were real: a spawned daemon never
# reaped on the timeout path, and bool `assert_eq!`s (yupana #83).
lint:
    cargo clippy --all-targets -- -D warnings -A missing-docs

# Format code
fmt:
    cargo fmt

# Run the yupana binary (e.g. `just run status`)
run *args="":
    cargo run -- {{args}}

# Install `yupana` onto PATH; pass features e.g. `just install "mcp langs-extra"`
install features="":
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -n "{{features}}" ]; then
        cargo install --path . --locked --features "{{features}}"
    else
        cargo install --path . --locked
    fi
    echo "Installed: $(command -v yupana)"

# === Documentation ===

# Documentation management: just docs <cmd>
# Commands: build, serve, lint, fix, fmt, vale, check

docs cmd="build":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{cmd}}" in
        build)    mdbook build docs/book ;;
        serve)    mdbook serve docs/book --open ;;
        lint)     npx markdownlint-cli2 "docs/**/*.md" "README.md" "CONTRIBUTING.md" ;;
        fix)      npx markdownlint-cli2 --fix "docs/**/*.md" "README.md" "CONTRIBUTING.md" ;;
        fmt)      npx prettier --write "docs/**/*.md" --prose-wrap preserve ;;
        vale)     vale docs/book/src/ ;;
        check)    just docs lint && just docs build ;;
        *)        echo "Unknown: {{cmd}}. Try: build serve lint fix fmt vale check" ;;
    esac
