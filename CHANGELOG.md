# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Migration note

- **Project renamed: hank → yupana.** Crate, binary, env-var prefixes,
  docs, and assets; full history preserved. Migration map for sibling
  repos: `docs/rename-from-hank.md`. Changelog entries below predate
  the rename and keep their original wording.

## [0.6.0] - 2026-08-07

### Added

- *(state)* The game-state harness — FR-35..FR-39 behind `game-state`([a19759e](https://github.com/scbrown/yupana/commit/a19759e581e2ea20397cba19ef88367dc736f0e6))
- *(hook)* Pre-bash records that it RAN, not only that it emitted([6b49298](https://github.com/scbrown/yupana/commit/6b49298cde0c928ad424f5309acb52a7928e0713))
- *(verdict)* Emit built blockers for failed checks([4e23ddd](https://github.com/scbrown/yupana/commit/4e23ddd429d336a4b2e919e8366e909053a3fe5f))

### Documentation

- *(spec)* Note the bounded-path gap under 9.3's archaeology claim([45c7b66](https://github.com/scbrown/yupana/commit/45c7b660f37a21550b19076f28e3bc3857142c23))
- *(promote)* State the dump-retention bound precisely([ed95a34](https://github.com/scbrown/yupana/commit/ed95a34c1e4b96d64e8014bcbb953c88ffc59e2a))
- *(status)* Explain defensive rule state fallback([ee5e8e6](https://github.com/scbrown/yupana/commit/ee5e8e6c93dad19729df11d09d734da8c949ba45))

### Fixed

- *(release)* The binary job could not fire for a release-plz tag([393447b](https://github.com/scbrown/yupana/commit/393447b181c16b71543b8cab6e22f82baf517a90))
- *(export)* Percent-encode [ and ] in symbol IRIs — a raw bracket makes the whole promotion unparseable([8898cb8](https://github.com/scbrown/yupana/commit/8898cb82ee2726b354c55a8724fb8ceb572a85c8))
- *(promote)* A refused promotion must name the node and keep the payload([30be33c](https://github.com/scbrown/yupana/commit/30be33cc2835e632a45a9dfb00aa53ced2fc06ce))
- *(export)* Collapse symbols sharing an IRI, and SAY which([bcc0a2b](https://github.com/scbrown/yupana/commit/bcc0a2b27033dde8c146ded5b87a14650b46f9d2))
- *(scrub)* Remove 38 internal identifiers and add the self-scan that was missing([8538d5d](https://github.com/scbrown/yupana/commit/8538d5dc62f5e352950632578771828f5e799b10))
- *(scrub)* Stop the ratchet naming real operators, and generalise it([57ef0fe](https://github.com/scbrown/yupana/commit/57ef0fe3ad60f53fa6cac1c4a0ecb9db71b9b75a))
- *(export)* Every emitted entity carries rdfs:label([18529d8](https://github.com/scbrown/yupana/commit/18529d846cff21d7dc65208b5651258a0a0a646f))
- *(promote)* A configured endpoint must not authorize a write([cc2c213](https://github.com/scbrown/yupana/commit/cc2c213d3b6ec328ed19fa6326c1f42600e4305a))
- *(guard)* The projection cache did not persist, so 1 in 5 edits went unguarded([7d55aff](https://github.com/scbrown/yupana/commit/7d55aff3f00016d3c0287a52ecc53d81345e3a70))
- *(status)* Report the languages the build can parse([37a7fb2](https://github.com/scbrown/yupana/commit/37a7fb2946f58a4da7b7a03b6b6c05c7ec6a688c))
- *(mcp)* Restore the mcp+quipu build — Conforms arm was never added([e15bba7](https://github.com/scbrown/yupana/commit/e15bba7fdcae450e8ba45c6047dcd7d440b6bf77))
- *(extract)* Never ingest vendored or minified bundles([da50919](https://github.com/scbrown/yupana/commit/da50919a9dca11107112f02cd5cdbe418f19d862))
- *(ci)* Ratchet the file-size limit so the check stops being always-red([f6b7414](https://github.com/scbrown/yupana/commit/f6b7414c7f94e8bb929e53b15ec8dee51c81219a))
- *(promote)* Carry each chunk's type declarations with it([0fe9b28](https://github.com/scbrown/yupana/commit/0fe9b2813b7503d79f5307ae9d6afe4468677f20))
- *(policy)* Expose workspace mode lowering([ab49fc5](https://github.com/scbrown/yupana/commit/ab49fc56f3f94df31b5cb2d7eb4bf35d78704c2c))

### Testing

- *(pre-edit)* Fixtures must not inherit the host's live quipu endpoint([f1ca99a](https://github.com/scbrown/yupana/commit/f1ca99a7291dd356c561fc90fa1afca51d666dee))
- Cargo-invoked processes must not write yupana's REAL state([b95525c](https://github.com/scbrown/yupana/commit/b95525c34b35fd014e13e0a731d5b52f9a2a6dd5))
- Seal the last three fixtures that reached the live graph([1fd9d20](https://github.com/scbrown/yupana/commit/1fd9d204095b5a95d1cda3b075497079c1d4ebb8))
- Pin the graph plane off in the tier-advertisement test([2c162a2](https://github.com/scbrown/yupana/commit/2c162a223e2e8466f2693f00ad3b44ef66867d79))
- Session ids must not collide, or tests suppress each other's fail-open notice([09d9717](https://github.com/scbrown/yupana/commit/09d971719572d840620c0427aca87049bbbff575))

## [0.5.0] - 2026-08-03

### Added

- *(guard)* Advisory for two agents mutating ONE working tree([544795b](https://github.com/scbrown/yupana/commit/544795b280f46a4cc195676d3b0cc758101083b6))
- *(guard)* Guard A — redundant re-read detection, with the discrimination that makes it usable([6d8689f](https://github.com/scbrown/yupana/commit/6d8689f636ccf9636a39d9435cb1257305583177))
- *(guard)* Delegate boundary — write-shaped work in territory you do not own([7bfca63](https://github.com/scbrown/yupana/commit/7bfca6396196cad99022d9a3b56e9ef164bc6d45))
- *(policy)* Project SARC constraint class and placement from quipu([3fcdf7f](https://github.com/scbrown/yupana/commit/3fcdf7fa623ecc00ef8759114d154ee7dd9dd483))
- *(trace)* Σ-derived constraint record, replacing the joined rule string([d24192c](https://github.com/scbrown/yupana/commit/d24192cf08b76e58da8d5089d7fbfc169b62408d))
- *(verdict)* Sign and spool verdicts at the moment a constraint fires([e448616](https://github.com/scbrown/yupana/commit/e448616ea4ab9e65055a6d4aa45e50cb342620de))
- *(paa)* A real Post-Action Auditor, with throttle([03e9d07](https://github.com/scbrown/yupana/commit/03e9d070a0ac9f6aa8ed203d99571aff33139e82))
- *(trace)* The attribution tuple, now that authority exists([eae8763](https://github.com/scbrown/yupana/commit/eae8763faae1809a5a6d09f436397df75fbaae47))
- *(hosting)* Check the claimed hosting layer against reality (H-SARC-I6)([fba8091](https://github.com/scbrown/yupana/commit/fba8091643dacdca5ee8b5f896822044b5a4a0c3))

### CI/CD

- *(crates)* Give yupana a publish path that is not a human with a token([489de89](https://github.com/scbrown/yupana/commit/489de894fcb39f802a2d689b3008f065766ef242))

### Documentation

- *(design)* SARC conformance — the gaps between yupana and quipu([5576c64](https://github.com/scbrown/yupana/commit/5576c64b8f7f13599e978042715ecf15b3d639be))
- *(design)* Cite the sources the SARC analysis was drawn from([4c59a7b](https://github.com/scbrown/yupana/commit/4c59a7bb0bc884ae951a110a8755146a0e713e37))
- *(design)* Correct the unrepresentable claim, and spec the two gaps Phase 1 exposed([5edaf0a](https://github.com/scbrown/yupana/commit/5edaf0a659dae372f48dd4ab3eb076cfda2d40d8))
- *(design)* Reconcile the gap list with what the MVP actually built([f3fdef9](https://github.com/scbrown/yupana/commit/f3fdef931e870717935dc0596a47bea5e66ff662))
- *(design)* Phase 5 and Q-SARC-VOCAB as built([28d6483](https://github.com/scbrown/yupana/commit/28d6483d08422e1e07b40641cc8ac6a6917fe067))
- *(design)* Phase 6 as built, and the four things still open([ba8fb78](https://github.com/scbrown/yupana/commit/ba8fb78c991ecd693ef5f21d7891646a480a23c3))
- The enforcement trace as a reference page, and README catch-up([992ba35](https://github.com/scbrown/yupana/commit/992ba35a8412395c33f55b50e34d2d870d43b1d5))

### Fixed

- *(changelog)* Blank lines around version headings — repair CI, second attempt([d6a3875](https://github.com/scbrown/yupana/commit/d6a3875ff0fb39c44a8b881c47b5f0d4cb30239e))
- *(ci)* Close two gates that could pass without checking anything (#84)([971448e](https://github.com/scbrown/yupana/commit/971448e51bfcd957447b6d9603dba3dc40b96242))
- *(changelog)* Regenerate the 0.5.0 section — release-plz dropped 13 commits([2f8e6fd](https://github.com/scbrown/yupana/commit/2f8e6fd4ad3005bb471354c30a2f6f10112e2523))
- *(clippy)* Take a slice in the test envelope helper, for CI's --all-targets([78cc805](https://github.com/scbrown/yupana/commit/78cc80507b5b741fcba876c070971aa0ab9bf2b2))

### Miscellaneous

- *(release)* Sync Cargo.lock to 0.4.0([b19ca9b](https://github.com/scbrown/yupana/commit/b19ca9b939a510b3a7fbf0967847b1867bbe44d3))
- Release v0.5.0([0f6ecad](https://github.com/scbrown/yupana/commit/0f6ecad623abb4239be3f42c8e70c49e28c6f5a9))

### Style

- Cargo fmt — repair CI, which I broke and did not check([7b20f40](https://github.com/scbrown/yupana/commit/7b20f40e9a39d8da5a1c0e54d1ba734ad0bd64dd))

## [0.4.0] - 2026-08-02

### Added

- *(promote)* Chunk oversized promotions so real-world repos fit through /knot (#59)([83f414b](https://github.com/scbrown/yupana/commit/83f414be6979122db0e8eab125e291b907db8d6b))
- *(extract)* Scope-qualified symbol IRIs — same-named symbols in one file stop merging (#64)([5bd0d39](https://github.com/scbrown/yupana/commit/5bd0d394d4e3fc72262f0a76de4ea0ccc83553fb))
- *(promote)* Optional bearer auth on the Quipu write path (QUIPU_AUTH_TOKEN)([b9806fa](https://github.com/scbrown/yupana/commit/b9806fa8dcfa5cd8613ce8b1ddff2a112c8115b6))
- *(promote)* Token-file fallback for the Quipu bearer — reaches pre-flip processes([a2d3cd9](https://github.com/scbrown/yupana/commit/a2d3cd9a79e1a5ca6f9ddc10c342e4490285a80f))
- *(daemon)* Complete the FR-27 query surface — /references, /symbols, /dataflow (yupana #1, stage 4) (#65)([a58467b](https://github.com/scbrown/yupana/commit/a58467b871beaeac7ed1c0dcd9b73bf83168f234))
- *(daemon)* Post-edit thin client + graceful shutdown + wire-level SLO test — closes yupana #1 (stage 5) (#66)([15756f1](https://github.com/scbrown/yupana/commit/15756f15b886d935162930cedb797ec9a0b2f912))
- *(graph)* Shared read-only base + CoW per-tenant overlays (yupana #2, slices 1+2+4) (#67)([67dcbba](https://github.com/scbrown/yupana/commit/67dcbba33c9b0e124f43b3f179032243ae7bb29c))
- *(daemon)* Wire the tenant layer live — /edit feed, tenant-scoped queries, status overlays (yupana #2 close) (#68)([fcd6958](https://github.com/scbrown/yupana/commit/fcd6958121f056f0fe1fae5014a13f0f408001ef))
- *(graph)* Frontier-bounded overlay update + overlay-new-name resolution (yupana #3, FR-16) (#69)([a298521](https://github.com/scbrown/yupana/commit/a2985215ca7035c8f7e5ff0b17b82dfb5280098c))
- *(graph)* Content-hash structural sharing — base-hit no-op + sharing stats (yupana #4, FR-15) (#70)([2a66389](https://github.com/scbrown/yupana/commit/2a66389931255b5132547c25e85b0a65b99c12a9))
- *(watch)* File-watch drives per-tenant overlays via the frontier recompute (yupana #5, FR-17) (#71)([00898b7](https://github.com/scbrown/yupana/commit/00898b7e84d2e74c755a65cd445e8092511ea930))
- *(graph)* Overlay lifecycle + eviction + high-fan-in guard — completes Phase 3 (yupana #6, FR-18) (#72)([8cb823f](https://github.com/scbrown/yupana/commit/8cb823fbc349d93074e3b2ef91d8595dba62d905))
- *(promote)* Read the committed tree, not the working tree — FR-22 + arbitrary --commit (yupana #15 slice) (#73)([12a1529](https://github.com/scbrown/yupana/commit/12a1529536671d4813cd2cfe26b4a58ca62e3943))
- *(refs,audit)* Position-based symbol resolution, graph-backed definitions, audit subjects (#85)([1627c06](https://github.com/scbrown/yupana/commit/1627c06e2933832ad448ccd76812fe2bc34546fb))
- *(status)* The rule plane is a FAILURE SURFACE, not a line of prose([e49b9cb](https://github.com/scbrown/yupana/commit/e49b9cb950ee575df90f5df0cc48beaa074068f2))
- *(metrics)* Make the advise-mode soak ADJUDICABLE — carry repo and exposure([c325e5f](https://github.com/scbrown/yupana/commit/c325e5f426af508f901b63a44b0b2d986b0acb54))
- *(action)* Resolve a command line to (verb, target, class) — or ABSTAIN([4fe96e2](https://github.com/scbrown/yupana/commit/4fe96e29eead84a57adf964bcbf1893610caba80))
- *(project)* Target the aegis:TextRule SUPERTYPE, not one concrete class([8228082](https://github.com/scbrown/yupana/commit/82280820dd1910a4b496d414e07e5d4e24c6e20c))
- *(metrics)* Record the WORK ITEM an action belongs to, read from the tracker's plate([610400e](https://github.com/scbrown/yupana/commit/610400e9965de4a2588e718034b1cfa94d62d226))
- *(hook)* Give the action resolver the input path it never had (hook pre-bash)([562609e](https://github.com/scbrown/yupana/commit/562609e0062480d21462a7014a7f14700005a33d))

### Changed

- *(project)* Split the SPARQL out of project.rs — it was over the size limit([d6d1dae](https://github.com/scbrown/yupana/commit/d6d1daee31c61371b7c549f68f765bfcf9d71e9f))

### Documentation

- *(spec)* Label the Phase-4 graph-export sections in Appendix D([a45aa7c](https://github.com/scbrown/yupana/commit/a45aa7c9f6e07874e36e86bce6427b57a2b5ebd0))
- Scope game-state & policy harness for NeuralAmplifier (FR-35..FR-39)([9f2f9d1](https://github.com/scbrown/yupana/commit/9f2f9d1391c489d62ac2ec228883913ae6b8b1ef))
- *(design)* Land the governance-plane design doc (#86)([baae03e](https://github.com/scbrown/yupana/commit/baae03e369b1853c0c7dede5b25b7dfe547b45de))
- Design for work-scoped agent governance — one scope, three consumers([5f5378d](https://github.com/scbrown/yupana/commit/5f5378dea64ce3ee58b930118cc30d4b0006f404))

### Fixed

- *(hook)* Scope the post-edit advisory to the EDITED symbol, not the whole file (#75)([c1818d3](https://github.com/scbrown/yupana/commit/c1818d38517a3b047dbea24b149320026bd4a61f))
- *(status,project)* Report the rule set that is LOADED, and stop projecting duplicates([74cfd75](https://github.com/scbrown/yupana/commit/74cfd75ffd156433c745eba0a36ad4cef9290aed))
- *(hook)* Grade the EDIT, not the SESSION — exposure follows the file's repo([01960b1](https://github.com/scbrown/yupana/commit/01960b10d2155ea7f152cc1a6dcb4fd33d7087b6))
- *(release)* Release-plz hunted for a tag template this repo has never used([85a24d2](https://github.com/scbrown/yupana/commit/85a24d23f25cbe112fc57ef3385e3829aee084a9))
- *(test)* Gate the rule-plane tests on `quipu` — they were red in 3 of 5 CI legs([9937bf1](https://github.com/scbrown/yupana/commit/9937bf116e8e87d2fc6e3aa5dd689e763dbf0c5e))
- *(ci)* Green up main — fmt, MD040 and a non_snake_case hard error([50e061d](https://github.com/scbrown/yupana/commit/50e061de12ccf806403903d4a607ded9b1eacfcf))

### Testing

- *(promote)* Round-trip-validate real export output against the shipped shapes — closes #13, #14 (#74)([f147ead](https://github.com/scbrown/yupana/commit/f147ead93a293c465886bb7e9572d5923fc1dc22))

### Style

- *(plate)* Fix doc list indentation flagged by clippy([b375280](https://github.com/scbrown/yupana/commit/b37528006a2f4aff0121fbfb2e0e322a4319d491))
- *(pre_bash)* Backtick the module name in the doc header([253131b](https://github.com/scbrown/yupana/commit/253131ba72248008704b68276a4e312bcf76d14b))

## [0.3.1] - 2026-07-23

### Added

- *(promote)* Write provenance + the rudof<->quipu verdict-agreement test([3dda463](https://github.com/scbrown/yupana/commit/3dda4638357bc096a1618993db73174fe1551c61))
- *(census)* Yupana census — count same-file symbol-name collisions at the only layer that can see them([900062a](https://github.com/scbrown/yupana/commit/900062a2043e969c47a8a477e5eb5a848c1758f4))

### Release

- V0.3.1 — quipu feature in the shipped binary + stub promote exits non-zero([f7949fd](https://github.com/scbrown/yupana/commit/f7949fd00713a3e2a560d3a40ad0a3d29d4855c2))

### Style

- Rustfmt the provenance + shape-agreement commit — third unformatted direct-to-main push today([b44bf3c](https://github.com/scbrown/yupana/commit/b44bf3c7a113b09fd718e4e0b1dd056aa9cf49f7))

## [0.3.0] - 2026-07-23

### Added

- *(change)* Answer what a CHANGE does, and name what it could not read (#39)([9d3fd2d](https://github.com/scbrown/yupana/commit/9d3fd2d7d74a7d848bc8e4bb2c55e0c2d4ba30e9))
- *(shapes)* Code-edge SHACL shapes, proven able to accept AND refuse (#13)([e7e358f](https://github.com/scbrown/yupana/commit/e7e358f601d5a5b7fcd740ef04e686e583b10dfb))
- *(status)* Make the policy layer observable in `yupana status` (#45)([6cc696f](https://github.com/scbrown/yupana/commit/6cc696fab6ba7491c1cb0e417f58803d6092b739))
- *(promote)* Wire the Quipu promotion write path — validate in-process, then write (#15/#14)([fb67ada](https://github.com/scbrown/yupana/commit/fb67ada84284d43bc01ca3b6aa15aa3a6b43a263))
- *(mcp)* Yupana_promote tool — the MCP surface of the promotion path (#15)([2d258cd](https://github.com/scbrown/yupana/commit/2d258cd50510b0a4ecd8c6cd9f2a1705566a3801))
- *(cli)* Export --to quipu promotes — the second §15 spelling, one path (#15)([36c00e0](https://github.com/scbrown/yupana/commit/36c00e0eef2f118c37105fd55969bb3aea1c2d19))
- *(daemon)* Resident graph process + liveness surface + loud-absence seam (yupana #1, stage 1) (#53)([0fc5843](https://github.com/scbrown/yupana/commit/0fc5843c09011451748bd75244ced04b47511283))
- *(daemon)* Graph-backed query endpoints over the resident graph (yupana #1, stage 2) (#55)([e4c0cb3](https://github.com/scbrown/yupana/commit/e4c0cb380c68e13b129b7386c0915b34c58e1fd6))
- *(daemon)* Resident-graph edit measurement + /measure endpoint (yupana #1, stage 3a) (#56)([a3c2c30](https://github.com/scbrown/yupana/commit/a3c2c30a94a45e61b9b05e71fedd2046693f982c))
- *(hook)* Pre-edit guard is a thin client of the resident daemon (yupana #1, stage 3b) (#57)([14313f6](https://github.com/scbrown/yupana/commit/14313f630a981267e434aa7048d228020c8a69aa))
- Tree-sitter structural edit rules (Selector + Predicate)([f927762](https://github.com/scbrown/yupana/commit/f927762632e20603f25004f83b058c35bba448d2))
- Declare verdict freshness on structural rule verdicts (FR-3 slice)([3286eaa](https://github.com/scbrown/yupana/commit/3286eaaf885fab081c0af1ef1d3376894b6dd685))
- Project quipu structural policies into the pre-edit guard (Phase 4)([8a72d7a](https://github.com/scbrown/yupana/commit/8a72d7a51abb104bedeb3484f0092acf6ed8efcf))
- Ed25519 verdict signing + promotion (H-PROMOTE-VERDICT)([bfe144e](https://github.com/scbrown/yupana/commit/bfe144e3f3684bc2968996bd5dea73441ca0eb75))
- *(mcp)* MCP graph tools are thin clients of the resident daemon (yupana #1, stage 3c) (#58)([2d2e0cc](https://github.com/scbrown/yupana/commit/2d2e0cc2e62467cf84fb70559e07e72c717168fa))
- *(policy)* Governed TEXT-rule plane — the quipu rule catalogue reaches the pre-edit guard (#60)([e84f151](https://github.com/scbrown/yupana/commit/e84f151ba7560ccfc8478ec6a8de41bcc9a63f09))
- *(metrics)* The usage spool — guard decisions, governed rules, and deliberate use, one JSONL line each (#61)([14733cb](https://github.com/scbrown/yupana/commit/14733cb1a5b3b2acd0b2d234e26bdd6b3ee9145d))
- *(metrics)* The guard and governed spool lines carry the MODE — soak hygiene for the enforce gate (#62)([52f3539](https://github.com/scbrown/yupana/commit/52f3539fd0c519e488563e9eb321d0ac3839884e))

### CI/CD

- Test the mcp+quipu combo — the shipping config for yupana_promote (#15)([db8d810](https://github.com/scbrown/yupana/commit/db8d81005a73823ac625be5002061c9833a31c5d))

### Documentation

- *(policy-guard)* Advise mode is visible to the operator, not the agent (#40)([70edae3](https://github.com/scbrown/yupana/commit/70edae31aceb83104e84e16615253408f87e7ca0))
- *(fr27)* Mark the parallel HTTP API phased instead of pretending it exists (#49)([11dd6e4](https://github.com/scbrown/yupana/commit/11dd6e453dc0c06aa39cbd5fc65fd262a166dcd8))
- Reconcile four doc-vs-code drifts and pin the tool count with a test (#50)([e93fa2a](https://github.com/scbrown/yupana/commit/e93fa2a4a716b3bc445ad903e4b7159e279c350c))
- *(promotion)* Promote is live + how to query dependencies back (#52)([fa99fac](https://github.com/scbrown/yupana/commit/fa99fac10ddfec932397786cc63bbf7fe2b3314e))
- *(design)* Add the yupana side of the policy edit-hooks path([bf9c0ff](https://github.com/scbrown/yupana/commit/bf9c0ffd90442fdc61992c0e1fd87c548c2230f3))
- Add governed-relations & workflow-gated-edits design docs([e562027](https://github.com/scbrown/yupana/commit/e56202787c738e2fbc75a2f8dd9b2df4a01133a5))
- Structural rules + projection (config + design)([5e02c14](https://github.com/scbrown/yupana/commit/5e02c142dcb7870bc6e2896e49662cdbebbc4767))

### Fixed

- *(guard)* Measure every compiled language, and REPORT what it cannot measure (#38)([3aab970](https://github.com/scbrown/yupana/commit/3aab970c3e6eefd8fa6dd1e7f12ac90bd5df9e75))
- *(baseline)* A ref that does not resolve builds NO baseline, and says so (#42)([01931e2](https://github.com/scbrown/yupana/commit/01931e219c8be68380fe75d65fcab9c2d2784fa9))
- *(cli)* Honour --config and --verbose instead of silently ignoring them (#43)([2fee30b](https://github.com/scbrown/yupana/commit/2fee30bfcbe5937c7ae20ac9806008d6bbde7317))
- *(mcp,cli)* Every served fact carries its tier; stop asserting freshness is served when it is not (#46)([45969c1](https://github.com/scbrown/yupana/commit/45969c1d0f00282b82f27858f314133b41749fce))
- *(status)* Advertise only tiers with an implementation; drop the empty lsp/cpg features (#47)([caa2d16](https://github.com/scbrown/yupana/commit/caa2d1649737f88085b0512d0b91c99bbb067a8f))
- *(config)* Wire the two live-security keys, mark the rest phased, guard against drift (#48)([3d8240e](https://github.com/scbrown/yupana/commit/3d8240e9d4da571381ee18dc6fde070cc35da271))
- *(policy-guard)* Key the fail-open notice on the KIND of gap, not just the session (#51)([d4ac723](https://github.com/scbrown/yupana/commit/d4ac7236476873cbbb590d715b73489e2faac2d4))
- *(promote)* Repo identity from --repo/origin remote, never the directory name (#15)([43af566](https://github.com/scbrown/yupana/commit/43af56679348d0f5391bf01db3c740f7eb3a30ed))
- *(promote)* Surface quipu's server-side SHACL refusal as a refusal; isolate the endpoint test from the operator's user config([9e26609](https://github.com/scbrown/yupana/commit/9e26609ab0427b28fbc19efebaa3854357f3ccd7))
- *(shapes)* Sync node shapes from quipu's registry — refuse symbol-IRI collisions before the network([6d399c5](https://github.com/scbrown/yupana/commit/6d399c5ef5c8bfa530239a5a6014105cfd868087))
- *(ci)* Main is red — stale conforming fixture, unformatted pushes, three clippy lints([d922ff9](https://github.com/scbrown/yupana/commit/d922ff9990cb16908d6d6892ee733784383a7808))
- *(export)* Every language this build parses exports — a Python repo promotes its real structure (the 81t2 class, found in export) (#63)([32e8fb8](https://github.com/scbrown/yupana/commit/32e8fb8c28a3137fe6202ca00264eb2721c23e01))

### Miscellaneous

- *(fmt)* Reformat to satisfy stable rustfmt — CI Format has been red (#54)([f3f23a1](https://github.com/scbrown/yupana/commit/f3f23a183f5da201f47a42c21242c582ff6e75b5))
- *(release)* V0.3.0 — 43 unreleased commits, plus the two gaps that let them pile up([b529984](https://github.com/scbrown/yupana/commit/b52998423709d3c26a7f701c06e1c3358595bc44))

### Testing

- *(cli)* Make refs_finds_definition actually able to fail (#44)([aa64ba9](https://github.com/scbrown/yupana/commit/aa64ba9126a2b937bef1d60d7131b61f893c853d))
- Guard-level integration tests for structural rules([18049fc](https://github.com/scbrown/yupana/commit/18049fca6228a31c75f16d0979a2ee279c2e2992))

### Design

- *(logo)* Give the feedback-loop lobes goggle eyes (#26)([b1e17dd](https://github.com/scbrown/yupana/commit/b1e17dd28688490610c619af2d9ddd321e8320db))

## [0.2.0] - 2026-07-20

### Added

- *(policy)* Pre-edit blocking guard + per-tenant capability scoping (#20, #21) (#32)([53a2c41](https://github.com/scbrown/yupana/commit/53a2c4131a848a133405d83093d0411306fc7b6f))
- *(verify)* Yupana_verify monitor-guided edit verification (#19) (#33)([8a5d5f5](https://github.com/scbrown/yupana/commit/8a5d5f544e13dbd02090fe47aa4e44d3c05afbfb))

### CI/CD

- *(release)* Outlast a GitHub API blip instead of stranding the tag (#34)([5b6e690](https://github.com/scbrown/yupana/commit/5b6e690fd9dd7a314165c3fa6669d1984282041d))

### Fixed

- *(policy)* A stale yupana must not block every edit in the fleet (#35)([e38b5b8](https://github.com/scbrown/yupana/commit/e38b5b8288f3fb32da7429a22c47350a6ebbc3f3))
- *(config)* A workspace config must not silently disarm the guard (#36)([831f6fc](https://github.com/scbrown/yupana/commit/831f6fc08afef1cd9c3e4a731099f937698b195d))

### Miscellaneous

- *(release)* V0.2.0 (#37)([3fc8e76](https://github.com/scbrown/yupana/commit/3fc8e765ace94e797f25970931ae449faf1fccad))

## [0.1.0] - 2026-07-20

### Added

- Scaffold Yupana — Phase 1 CLI, tooling, docs([ea0f78b](https://github.com/scbrown/yupana/commit/ea0f78bd3ae44d5a2a0396dcc77d163438eb83e0))
- Phase-1 MCP server over rmcp (stdio + streamable-HTTP)([90a0e73](https://github.com/scbrown/yupana/commit/90a0e73e55ff45e728e1257fdf4a8ec3df8a7d7d))
- Phase-2 call graph and blast radius([9951f47](https://github.com/scbrown/yupana/commit/9951f479bf8cc16aac9ecb7f9453926cfdd8161e))
- Phase-2 intra-procedural dataflow (Rust-native)([69bd5f9](https://github.com/scbrown/yupana/commit/69bd5f958671a7ca05094ed48dc1b8c3664463e9))
- Phase-2 exit — co-change reconciliation (FR-11)([90df6b5](https://github.com/scbrown/yupana/commit/90df6b54377ffe5ddee2a5e29c42ce7759049aaa))
- Edit-reactive harness hook + interface-model spec (FR-30/31)([6bd66a2](https://github.com/scbrown/yupana/commit/6bd66a253d9893959a2295a1c5cc2df744942bb3))
- Referential-structure export + code/docs synergy (§5.10, FR-33/34)([d5668ec](https://github.com/scbrown/yupana/commit/d5668eccced7d151d077384f98a98cda382a4189))
- Extract module import edges (bobbin:imports) in export (#23)([f0de144](https://github.com/scbrown/yupana/commit/f0de144c60968f4114756176f7778b0e37606558))
- Git baseline — resolve base_ref to a commit + commit diff (OQ2) (#24)([9be9c49](https://github.com/scbrown/yupana/commit/9be9c49241abfb0f13c6d7dad46692031a0de7bc))
- Live Louvain community detection over the in-memory graph (FR-9) (#27)([fec15ca](https://github.com/scbrown/yupana/commit/fec15cabacc1320e4b140f9ad2665fa15e97eae0))
- Build the base graph from git-tree content at a ref (#12 slice 2, FR-13) (#28)([153b02c](https://github.com/scbrown/yupana/commit/153b02ce19be56ab5346666e22011712d923dd90))
- Yupana #5 (file-watch) + #16 (doc→code refs) + #9 grammars (langs-extra) (#29)([5fc6749](https://github.com/scbrown/yupana/commit/5fc6749ba0f59da7c57aac3024b216bc07087030))

### CI/CD

- Self-enable GitHub Pages in docs workflow([64d6601](https://github.com/scbrown/yupana/commit/64d66014aeb2a3e672f319417f76660b1db6177b))
- Publish docs via gh-pages branch (contents:write)([d9a2eb3](https://github.com/scbrown/yupana/commit/d9a2eb3d6e20c1e81a41a0c7a8f248b55511f359))
- Publish a release binary on version tags (#31)([a8da3e0](https://github.com/scbrown/yupana/commit/a8da3e021ee62e9fb1d75a0f978a492f222b24c9))
- *(release)* Retry the publish step and assert the asset actually landed([6dddb07](https://github.com/scbrown/yupana/commit/6dddb0795bfb6bd4856c36792cd165df45273e9b))

### Changed

- *(graph)* Extract the FR-12 BFS behind an Adjacency trait (Slice 0) (#30)([f336d06](https://github.com/scbrown/yupana/commit/f336d06be36071ec5774e72024c3d34282b5d03f))

### Documentation

- Add Yupana vision and build specification([41519e9](https://github.com/scbrown/yupana/commit/41519e9ef71e0f0db0d98671dab7e7090374b510))
- Record FR-11 invariant — Yupana borrows co-change, never derives it([3a47108](https://github.com/scbrown/yupana/commit/3a47108e8e33c169a35190558e6c7627fde68fe0))
- Note promotion feeds Quipu work-item co-occurrence (quipu#37)([0bb76a2](https://github.com/scbrown/yupana/commit/0bb76a29151d5447df21e789927c804957e6327d))
- Consolidate design + add handoff appendices to the spec([a08ef52](https://github.com/scbrown/yupana/commit/a08ef526ba2385bc36ec48bcefa7179552e478bf))
- *(readme)* Named-competitor comparison + selling points + yupana×quipu use-cases; reference the yupana CLI (not cargo run)([18e813d](https://github.com/scbrown/yupana/commit/18e813df8e091e4a9bb5917b28aed13efb97335a))

### Build

- Add `just install` — put the `yupana` binary on PATH (#25)([0abefac](https://github.com/scbrown/yupana/commit/0abefac41f485bb269a2ba814967d774877f072e))

### Design

- *(logo)* Reshape the yupana into an infinity/feedback-loop([47099e6](https://github.com/scbrown/yupana/commit/47099e6801c3894ab99ee46f16d5b90c983ef596))
