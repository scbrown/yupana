# Contributing to Yupana

Thanks for helping build Yupana. This guide covers the essentials; the full design
lives in [`docs/yupana-spec.md`](docs/yupana-spec.md).

## Workflow

1. **Use `just`, never raw `cargo`.** The justfile keeps output quiet to save
   context (`verbose=true` to override).
2. Make your change with tests.
3. Run the gate:

   ```bash
   just check     # fmt, clippy (-D warnings), markdownlint, file-size
   just test      # all tests green
   ```

4. Commit with [Conventional Commits](https://www.conventionalcommits.org/)
   (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `ci:`).
5. Push and open a PR.

**Work is not complete until it is pushed and CI is green.**

## Code standards

- **Clippy is `-D warnings`.** The lint policy in `Cargo.toml` mirrors Quipu's;
  resolve every lint before merge.
- **Small files.** Pre-commit warns at 400 lines and fails at 500 (tests
  exempt). One responsibility per module — follow the layout in
  [`docs/yupana-spec.md` §7.2](docs/yupana-spec.md).
- **Tag every fact** with a `tier` and `freshness` (FR-3).
- **No feature ships dark.** When you wire a Cargo feature (`mcp`, `quipu`,
  `cpg`, `lsp`), add it to the CI matrix in the same PR.

## Documentation

- User-facing changes must update the mdBook (`just docs build` must be clean)
  and the README if quick-start or usage changes.
- Prose is checked with markdownlint, Prettier, and Vale (`just docs check`).

## Tests

- Unit tests live inline (`#[cfg(test)] mod tests`).
- CLI/integration tests use `assert_cmd` + `predicates` under `tests/`, and must
  skip gracefully when an optional toolchain (a language server, etc.) is
  unavailable.
