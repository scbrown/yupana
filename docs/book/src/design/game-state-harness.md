# Game-State & Policy Harness (addendum)

The FR-35..FR-39 addendum — the net-new capabilities the
[NeuralAmplifier](https://github.com/scbrown/NeuralAmplifier) project needs
from Yupana — lives at the repository root:

- **[`docs/neuralamplifier-harness.md`](https://github.com/scbrown/yupana/blob/main/docs/neuralamplifier-harness.md)**

**Status: BUILT, behind the `game-state` Cargo feature.** The code is
`src/state/`; what shipped, as opposed to what was designed, is described in
[The Game-State Harness](../concepts/game-state.md).

Yupana ingests facts an engine *adapter* stated — a game board, not source —
and serves them at the `engine-state` tier, a peer of the code tiers rather
than a rung above or below them. Nothing there is span-anchored, so a consumer
that read an `engine-state` fact as if it pointed at a `file:line` would be
wrong in exactly the way FR-3 exists to prevent.

One spelling trap the addendum fixes and this page repeats because it is a
wire value: **`game-state` is the Cargo feature; `engine-state` is the tier.**
Two names for two things, and a consumer discriminating on the wrong string
matches nothing, silently.
