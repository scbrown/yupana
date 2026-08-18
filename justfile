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
    # The python-side tests. Small and few, but a converter that silently
    # dropped records would misreport the false-positive rate that gates
    # advise -> enforce promotion, so it does not get to sit outside the gate.
    python3 tests/spool_to_dogwood.py

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

# === End-to-end (Quipu integration) ===

# The grounding-integration eval and the shared-yupana concurrency bench,
# against a LIVE quipu-server seeded with the camayoc policy pack (siblings
# ../quipu and ../camayoc must be checked out). See docs "E2E Grounding Eval".
#   just e2e run                     # the hallucination-prevention + isolation eval
#   just e2e bench "--levels 1,2,4,8,16,32 --edits 10"
#   just e2e f1                      # briefing retrieval F1 + ablation study
e2e cmd="run" *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release --features quipu,mcp --quiet
    (cd ../quipu && cargo build --release --features shacl,onnx,server \
        --bin quipu --bin quipu-server --quiet)
    case "{{cmd}}" in
        run)   python3 scripts/e2e/harness.py {{args}} ;;
        bench) python3 scripts/e2e/bench.py {{args}} ;;
        f1)
            # Best-effort semantic-arm provisioning: the all-MiniLM-L6-v2 ONNX
            # bundle from qdrant's mirror (HuggingFace's LFS CDN is
            # proxy-blocked in some sandboxes) and the ONNX Runtime dylib from
            # the onnxruntime PyPI wheel (quipu's ort is load-dynamic). The
            # eval runs lexical-only without them, and says so.
            if [ ! -f target/models/fast-all-MiniLM-L6-v2/model.onnx ]; then
                mkdir -p target/models
                curl -sSL --fail --max-time 300 \
                    https://storage.googleapis.com/qdrant-fastembed/sentence-transformers-all-MiniLM-L6-v2.tar.gz \
                    | tar xz -C target/models 2>/dev/null || true
            fi
            if [ ! -f target/models/libonnxruntime.so ]; then
                pip download onnxruntime --no-deps -q -d target/models/.ortwheel 2>/dev/null || true
                python3 scripts/e2e/extract_ort.py || true
            fi
            python3 scripts/e2e/eval_f1.py {{args}} ;;
        *)     echo "Unknown: {{cmd}}. Try: run bench f1"; exit 1 ;;
    esac

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
