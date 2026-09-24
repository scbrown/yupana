# Map of all docs

Every document in this repository, grouped by what it is for, with its status.
If you are new, read [Installation](getting-started/installation.md) and
[Quick Start](getting-started/quick-start.md) first; everything else here is for
when you need it.

Documents outside the book live under `docs/` in the repository and open on
GitHub.

## Start here

| document | what it gives you |
|---|---|
| [README](https://github.com/scbrown/yupana/blob/main/README.md) | install, a first success, and wiring into an agent |
| [Installation](getting-started/installation.md) · [Quick Start](getting-started/quick-start.md) | the same path, in more depth |
| [Configuration](getting-started/configuration.md) · [Harness Integration](getting-started/harness-integration.md) | making it part of your workflow |
| [Why Yupana](concepts/why-yupana.md) · [The Stack](concepts/the-stack.md) | what it is for, and how it fits with Quipu and Bobbin |

## Reference

Everything under **Reference** in the book's sidebar: the
[CLI](reference/cli.md), [configuration](reference/config.md),
[MCP tools](reference/mcp-tools.md), the [resident daemon](reference/daemon.md),
the [pre-edit policy guard](reference/policy-guard.md),
[golden-path conformance](reference/golden-path.md),
the [enforcement trace](reference/enforcement-trace.md), and
[share bundles](reference/share-bundles.md).

| outside the book | status |
|---|---|
| [Agent action certification](https://github.com/scbrown/yupana/blob/main/docs/action-certification.md): the `yupana certify` record format | current |

## Specification and vision

| document | status |
|---|---|
| [Specification](https://github.com/scbrown/yupana/blob/main/docs/yupana-spec.md): the full build spec (requirements, architecture, phasing) | living; the reference for FR numbers |
| [Vision](https://github.com/scbrown/yupana/blob/main/docs/vision.md): Bobbin × Yupana × Quipu | north star |

## Design

The book's **Design** section summarises the addenda below and links to them.

| document | status |
|---|---|
| [Golden-path conformance guard](https://github.com/scbrown/yupana/blob/main/docs/golden-path-guard.md) (FR-40..42) | built, first cut |
| [Game-state and policy harness](https://github.com/scbrown/yupana/blob/main/docs/neuralamplifier-harness.md) (FR-35..39) | built |
| [Work-scoped agent governance](https://github.com/scbrown/yupana/blob/main/docs/work-scoped-governance.md) | partially implemented; the doc says which parts |
| [Governed landing policy](https://github.com/scbrown/yupana/blob/main/docs/design/landing-policy.md) | design, advise tier first |
| [Paper plan](https://github.com/scbrown/yupana/blob/main/docs/design/paper.md): evidence-local constraint enforcement | planned |

## Research and evaluation

| document | status |
|---|---|
| [Briefing retrieval eval](https://github.com/scbrown/yupana/blob/main/docs/briefing-retrieval-eval.md) | measured 2026-08-15; reproducible with `just e2e f1` |
| [E2E grounding eval](reference/e2e-grounding-eval.md) | reference |

## Working on Yupana

| document | what it covers |
|---|---|
| [Contributing](reference/contributing.md) | build, test and the pre-push gate |
| [Releasing](https://github.com/scbrown/yupana/blob/main/docs/RELEASING.md) | how release-plz cuts a release, and the crates.io lane |

## Historical

Kept for the record. They describe decisions already made, not how things work
today.

| document | what it records |
|---|---|
| [Rename: hank → yupana](https://github.com/scbrown/yupana/blob/main/docs/rename-from-hank.md) | the 2026-08-09 rename; `hank` survives only as a compatibility name |
