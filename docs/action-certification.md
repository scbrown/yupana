# Agent action certification

`yupana certify` is the signing boundary between a host action guard and the
ActionRecord corpus. The guard observes facts; Yupana compares each ordered
`expected`/`observed` check, derives `certified` or `uncertified`, signs the
decision, and appends it to JSONL. Promotion stays off the push path.

The record is `aegis-action-certification/v1`, `kind: "action"`, and is a strict
superset of the replay record: `record_id`, `correlation_id`, `session`, `ts`,
`agent`, optional `item`, `verb`, `target`, `target_class`, `tenant`, and
`result`. Push/land evidence adds `repo`, full `sha`, `ref`, `remote_authority`,
and `scope_provenance` containing a graph query/transaction identity and
`as_of`. `target` remains the canonical repo entity; `repo` does not replace it.

Checks are ordered `{id, expected, observed, evidence_ref}` entries. Yupana
adds `outcome`; every mismatch yields a stable `<check_id>_mismatch` reason.
Missing bead references are represented by omitting `item` and including an
unsatisfied `bead-present` check. Negative attempts are signed and retained,
not dropped. One ActionRecord query selects both outcomes and keys on
`certification_status`; reason and check details are optional projections.

The signature covers all input fields, computed checks, status, reason codes,
scope provenance, verifier/key identity, and timestamp. The envelope records
Ed25519, `signed_payload_hash`, canonicalization version, `verdict_id`, and a
`key_id` derived from the public key. The existing `yupana verifier` command
creates the host key and prints the public key. Its Quipu
VerifierRegistration must bind verifier, key id/public key, and validity
interval; rotations create a new key id rather than rewriting old records.

Initial action vocabulary is `push` and `land`. Required checks are
`bead-present`, `item-match`, `in-progress-at-check`, `repo-owner-match` or
`sanctioned-handoff`, and `sha-repo-binding`. The later hook seam supplies
these observations from `aegis-tnpf6h`; certification itself does not enforce
the push. Replay and policy consumers must count all attempted records and may
filter only after ingestion.

Example:

```sh
yupana certify --key-path yupana-signing.pk8 \
  --spool action-certifications.jsonl < action.json
```
