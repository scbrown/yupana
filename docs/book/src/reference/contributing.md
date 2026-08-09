# Contributing

See [`CONTRIBUTING.md`](https://github.com/scbrown/yupana/blob/main/CONTRIBUTING.md)
and [`AGENTS.md`](https://github.com/scbrown/yupana/blob/main/AGENTS.md) at the
repository root.

The essentials:

- **Use `just`, never raw `cargo`.**
- Run `just check` and `just test` before every push — both must pass.
- Clippy is `-D warnings`; the lint policy mirrors Quipu's.
- Keep source files small (warn at 400 lines, fail at 500).
- Tag every served fact with a `tier` and `freshness`.
- No Cargo feature ships dark — add it to the CI matrix in the same change.
- Work is not complete until it is pushed and CI is green.
