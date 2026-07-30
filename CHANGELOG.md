# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/).

Entries below the header are maintained by
[release-please](https://github.com/googleapis/release-please): each release
section is generated from Conventional Commit messages and prepended when the
standing release pull request is merged.

## [0.1.24](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.23...ipe-v0.1.24) (2026-07-30)


### Features

* **cli:** add `ipe check` — type-check a project without build or run ([#346](https://github.com/arthurmaciel/ipe-lang/issues/346)) ([2a1010e](https://github.com/arthurmaciel/ipe-lang/commit/2a1010e2e100eb39712d3564c883a6a76ab29d3d))
* **lexer,canon:** strip source indentation margin from triple-quoted strings via anchor column ([#324](https://github.com/arthurmaciel/ipe-lang/issues/324)) ([70346bb](https://github.com/arthurmaciel/ipe-lang/commit/70346bb45ecab1004253655f37bf8f2c4b03affe))
* **stdlib:** Ipe.Process.run — no-shell subprocess execution, WasmClient-denied ([#316](https://github.com/arthurmaciel/ipe-lang/issues/316)) ([#336](https://github.com/arthurmaciel/ipe-lang/issues/336)) ([89198e5](https://github.com/arthurmaciel/ipe-lang/commit/89198e50548f410a71e67cf2c25b3f59e3c27287))

## [0.1.23](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.22...ipe-v0.1.23) (2026-07-30)


### Features

* **audit:** promote Tier-2 native certification to FreeBSD; keep Windows deferred ([#149](https://github.com/arthurmaciel/ipe-lang/issues/149)) ([#261](https://github.com/arthurmaciel/ipe-lang/issues/261)) ([89fd217](https://github.com/arthurmaciel/ipe-lang/commit/89fd2176c2eb0b40513af59e18015afe25a217a5))
* **error:** Ipe.Error inspector kernels + Ipe.Test.expectErr/kindName ([#288](https://github.com/arthurmaciel/ipe-lang/issues/288)) ([#309](https://github.com/arthurmaciel/ipe-lang/issues/309)) ([58fe06e](https://github.com/arthurmaciel/ipe-lang/commit/58fe06e1416cac40a70aeb97ad687e159f693131))
* **shapes:** consolidate Tui+Console into Terminal + Ui.cells escape node ([#296](https://github.com/arthurmaciel/ipe-lang/issues/296)) ([d6eb635](https://github.com/arthurmaciel/ipe-lang/commit/d6eb635dc215222ecfc951b5e18f7d93ca58a2f9))


### Bug Fixes

* **backend:** emit top-level nullary bindings as evaluate-once shared values ([#315](https://github.com/arthurmaciel/ipe-lang/issues/315)) ([139922b](https://github.com/arthurmaciel/ipe-lang/commit/139922b351370a80329f744de4c67b91a95a1337))
* **canon:** make reachable stdlib member imply backing kernel by construction ([#286](https://github.com/arthurmaciel/ipe-lang/issues/286)) ([#306](https://github.com/arthurmaciel/ipe-lang/issues/306)) ([5c3e961](https://github.com/arthurmaciel/ipe-lang/commit/5c3e96108ea7f5b18c4ead783728b9a230a50e1e))
* **ci:** green main — compare builtin, sky-transform round-trip, panic-scan gate ([#304](https://github.com/arthurmaciel/ipe-lang/issues/304)) ([0bcc874](https://github.com/arthurmaciel/ipe-lang/commit/0bcc874db6405d084872641e1e1806bfe30b8c17))
* **ci:** migrate Tier-C-broken examples + remove sky-parity job ([#262](https://github.com/arthurmaciel/ipe-lang/issues/262)) ([922ace3](https://github.com/arthurmaciel/ipe-lang/commit/922ace347fcd8d96809bf16895e35a65e91ed5f8))
* **ci:** migrate Tier-C-broken examples + remove sky-parity job ([#264](https://github.com/arthurmaciel/ipe-lang/issues/264)) ([6230e9f](https://github.com/arthurmaciel/ipe-lang/commit/6230e9f66905b5ad2456a75d5fb09a67b33db6e6))
* **cli:** route ipe analysis surfaces through the injection-aware source graph ([#310](https://github.com/arthurmaciel/ipe-lang/issues/310)) ([#313](https://github.com/arthurmaciel/ipe-lang/issues/313)) ([463baa9](https://github.com/arthurmaciel/ipe-lang/commit/463baa917139d91d5cca57da6b124693d069de27))
* **json:** strict integer decoder + Elm behaviour verdict ledger ([#293](https://github.com/arthurmaciel/ipe-lang/issues/293)) ([#308](https://github.com/arthurmaciel/ipe-lang/issues/308)) ([f279bbe](https://github.com/arthurmaciel/ipe-lang/commit/f279bbe7d6d3d7a231623cd89615c8eb59d397a5))

## [0.1.22](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.21...ipe-v0.1.22) (2026-07-29)


### Features

* **canon:** Prelude→Basics + three-tier auto-import ([#231](https://github.com/arthurmaciel/ipe-lang/issues/231)) ([#244](https://github.com/arthurmaciel/ipe-lang/issues/244)) ([cdd2414](https://github.com/arthurmaciel/ipe-lang/commit/cdd241481dc89e1a8c54a3a07fde7cce34b9a8a1))
* **lsp:** add-import quick-fix for the IPE-N0034 must-import diagnostic ([#242](https://github.com/arthurmaciel/ipe-lang/issues/242)) ([#258](https://github.com/arthurmaciel/ipe-lang/issues/258)) ([9853a7c](https://github.com/arthurmaciel/ipe-lang/commit/9853a7cb82b200e40167824785c75f2696da6702))
* **pubsub:** activate top-level Ipe.PubSub Task surface + relocate TEA-side under Ipe.Tea.Web.PubSub ([#235](https://github.com/arthurmaciel/ipe-lang/issues/235) Stage 3) ([#252](https://github.com/arthurmaciel/ipe-lang/issues/252)) ([4515bc0](https://github.com/arthurmaciel/ipe-lang/commit/4515bc06e46e58498f7ab05973c9b8aa278cd15e))
* **resolve:** enforce Tier-C explicit-import per ADR 0047 ([#243](https://github.com/arthurmaciel/ipe-lang/issues/243)) ([#256](https://github.com/arthurmaciel/ipe-lang/issues/256)) ([e5e7760](https://github.com/arthurmaciel/ipe-lang/commit/e5e77607a0d335fdf665ce45bb9ad99f2735bc07))
* **sandbox:** Windows + FreeBSD returning build-jail arms ([#228](https://github.com/arthurmaciel/ipe-lang/issues/228), impl of ADR 0051) ([#253](https://github.com/arthurmaciel/ipe-lang/issues/253)) ([971906c](https://github.com/arthurmaciel/ipe-lang/commit/971906ca0cb6bac94e25ac910cdb14bc3a2b8a18))
* **shapes:** relocate TEA shapes under Ipe.Tea.&lt;Shape&gt; + Program gate + scaffold/guard ([#235](https://github.com/arthurmaciel/ipe-lang/issues/235) Stage 1, closes [#238](https://github.com/arthurmaciel/ipe-lang/issues/238)) ([#248](https://github.com/arthurmaciel/ipe-lang/issues/248)) ([a7b36fd](https://github.com/arthurmaciel/ipe-lang/commit/a7b36fd134949b5f0905444d8a1ec7763717b2e3))
* **shapes:** unify Web/WebView view on Element + Web.appHtml raw-Html escape ([#235](https://github.com/arthurmaciel/ipe-lang/issues/235) Stage 2) ([#250](https://github.com/arthurmaciel/ipe-lang/issues/250)) ([588ad72](https://github.com/arthurmaciel/ipe-lang/commit/588ad728cbb0481c1e36c97ae1a6dadf7e55871b))


### Bug Fixes

* **sandbox:** losslessly lower FreeBSD jail command= + correct shell-free claim ([#254](https://github.com/arthurmaciel/ipe-lang/issues/254)) ([#259](https://github.com/arthurmaciel/ipe-lang/issues/259)) ([c3ab280](https://github.com/arthurmaciel/ipe-lang/commit/c3ab28078b3448a394bd5e8acefe282735ca8763))
* **wasm:** hydrate glue references the real emitted record-alias type name ([#224](https://github.com/arthurmaciel/ipe-lang/issues/224)) ([#234](https://github.com/arthurmaciel/ipe-lang/issues/234)) ([4c256de](https://github.com/arthurmaciel/ipe-lang/commit/4c256de119d4eb47b89f6eaa1caaf53d460efed0))

## [0.1.21](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.20...ipe-v0.1.21) (2026-07-28)


### Features

* **audit:** macOS Tier-2 native enforcement ([#149](https://github.com/arthurmaciel/ipe-lang/issues/149) sub-PR 4) ([#196](https://github.com/arthurmaciel/ipe-lang/issues/196)) ([a97d8ee](https://github.com/arthurmaciel/ipe-lang/commit/a97d8ee6cd9eb179a32eeaacfe3915f407d6f7d1))
* **audit:** Tier-2 exercise harness — Tier-2 now certifies native packages on Linux ([#149](https://github.com/arthurmaciel/ipe-lang/issues/149) sub-PR 3) ([#193](https://github.com/arthurmaciel/ipe-lang/issues/193)) ([ebd579e](https://github.com/arthurmaciel/ipe-lang/commit/ebd579e54c5ddb3a9c690853db39df8c20f4f41e))
* **audit:** Tier-2 native differential-confinement reconciler + fail-closed gate ([#149](https://github.com/arthurmaciel/ipe-lang/issues/149) sub-PR 2) ([#192](https://github.com/arthurmaciel/ipe-lang/issues/192)) ([5bcac8c](https://github.com/arthurmaciel/ipe-lang/commit/5bcac8c8a24f797a5ae6739e6681bd3dbf85cd6b))
* **ci:** harden clippy gate to pedantic/correctness/style/complexity ([#167](https://github.com/arthurmaciel/ipe-lang/issues/167)) ([206b5d0](https://github.com/arthurmaciel/ipe-lang/commit/206b5d0665ac128dbdf0f33bf1001383d781fc24))
* **cli:** headless PR-open for ipe package publish ([#171](https://github.com/arthurmaciel/ipe-lang/issues/171)) ([#175](https://github.com/arthurmaciel/ipe-lang/issues/175)) ([4eca539](https://github.com/arthurmaciel/ipe-lang/commit/4eca539d40ed3582235531fbca3de969441ee77a))
* **cli:** ipe doc — API documentation generation ([#141](https://github.com/arthurmaciel/ipe-lang/issues/141)) ([#223](https://github.com/arthurmaciel/ipe-lang/issues/223)) ([ac5c1e4](https://github.com/arthurmaciel/ipe-lang/commit/ac5c1e4386c22b84a9d87b444a96fb92912a5658))
* **cli:** ipe doc — HTML site, cross-reference linking, serve ([#222](https://github.com/arthurmaciel/ipe-lang/issues/222)) ([#225](https://github.com/arthurmaciel/ipe-lang/issues/225)) ([dc312c9](https://github.com/arthurmaciel/ipe-lang/commit/dc312c9da2775ba35b2a8e843edfa3ef635f8c0a))
* **cli:** ipe login — GitHub device-code OAuth for a publish token ([#138](https://github.com/arthurmaciel/ipe-lang/issues/138)) ([#170](https://github.com/arthurmaciel/ipe-lang/issues/170)) ([0672c1d](https://github.com/arthurmaciel/ipe-lang/commit/0672c1d15625a31eae277c9b4b5f42d5bb2608a8))
* **examples:** network-only Sky mirror with committed trees + anchored edits ([#181](https://github.com/arthurmaciel/ipe-lang/issues/181)) ([6831ad6](https://github.com/arthurmaciel/ipe-lang/commit/6831ad6059b67209986b57a1eca98cd7a6ac86bd))
* **lower,backend:** function-value reuse for contained record-of-functions ([#178](https://github.com/arthurmaciel/ipe-lang/issues/178)) ([#185](https://github.com/arthurmaciel/ipe-lang/issues/185)) ([fe1d6f0](https://github.com/arthurmaciel/ipe-lang/commit/fe1d6f0cbb01616fc38d21f52c5c93074b29cd2c))
* **parse:** `do` and `parallelDo` notation ([#199](https://github.com/arthurmaciel/ipe-lang/issues/199)) ([659e4ed](https://github.com/arthurmaciel/ipe-lang/commit/659e4ed8b4b7962304b6f1ad7d4f745eee2e0fcc))
* **parse:** the `>>` / `<<` function-composition operators ([#177](https://github.com/arthurmaciel/ipe-lang/issues/177)) ([#183](https://github.com/arthurmaciel/ipe-lang/issues/183)) ([0b15f88](https://github.com/arthurmaciel/ipe-lang/commit/0b15f88f0678f6498e85d50aeed50103f733061b))
* **patterns:** or-patterns (| alternatives) in case…of ([#214](https://github.com/arthurmaciel/ipe-lang/issues/214)) ([#233](https://github.com/arthurmaciel/ipe-lang/issues/233)) ([3011fd2](https://github.com/arthurmaciel/ipe-lang/commit/3011fd261ed7aa3ebd365090704d5a557589063b))
* **sandbox:** macOS run-jail SBPL arm → JailForTarget::Holds on macOS ([#198](https://github.com/arthurmaciel/ipe-lang/issues/198)) ([#212](https://github.com/arthurmaciel/ipe-lang/issues/212)) ([6349f77](https://github.com/arthurmaciel/ipe-lang/commit/6349f77c6bd71440532b7d55d9338b60368fb203))
* **sandbox:** Tier-2 audit — build-jail outcome primitive + design ([#149](https://github.com/arthurmaciel/ipe-lang/issues/149) sub-PR 1) ([#191](https://github.com/arthurmaciel/ipe-lang/issues/191)) ([045b43b](https://github.com/arthurmaciel/ipe-lang/commit/045b43bba6c5b1762a9fef448950e7cb9097d60e))
* **sandbox:** Windows runtime run-jail arm (partial per-axis confinement) ([#215](https://github.com/arthurmaciel/ipe-lang/issues/215)) ([#220](https://github.com/arthurmaciel/ipe-lang/issues/220)) ([1a1094c](https://github.com/arthurmaciel/ipe-lang/commit/1a1094ce18e4be2817c8baecea4bbca3ecaa7d3c))
* **stdlib:** Io.println/eprintln kernels + dev-only Debug.log, remove Log.println ([#207](https://github.com/arthurmaciel/ipe-lang/issues/207)) ([957da73](https://github.com/arthurmaciel/ipe-lang/commit/957da73a67fab3f583143636862903a34f5b77fd))
* **tooling:** regen-goldens tool + decouple emit template from golden fixture ([#206](https://github.com/arthurmaciel/ipe-lang/issues/206)) ([7a50d87](https://github.com/arthurmaciel/ipe-lang/commit/7a50d87e19751d56062294d4cb13072c054f6ec1))


### Bug Fixes

* **audit:** honest surface — Tier-2 certifies linux-x64 AND macos-arm64 ([#149](https://github.com/arthurmaciel/ipe-lang/issues/149)) ([#229](https://github.com/arthurmaciel/ipe-lang/issues/229)) ([064add5](https://github.com/arthurmaciel/ipe-lang/commit/064add5e27cb065a2c0095557e8ff85f5b72d0db))
* **ci:** ASCII-only PowerShell in the Windows admission-sandbox skip step ([#189](https://github.com/arthurmaciel/ipe-lang/issues/189)) ([b7dadc7](https://github.com/arthurmaciel/ipe-lang/commit/b7dadc7558fcaf7634404a989dfed9f50f013630))
* **ci:** repoint static.yml + fuzz to relocated example homes (post-[#188](https://github.com/arthurmaciel/ipe-lang/issues/188)) ([#194](https://github.com/arthurmaciel/ipe-lang/issues/194)) ([768f1c4](https://github.com/arthurmaciel/ipe-lang/commit/768f1c409842a38930ca8800364636ef69c6cb64))
* **cli:** inject the compiled-source stdlib closure in capability inference ([#169](https://github.com/arthurmaciel/ipe-lang/issues/169)) ([#176](https://github.com/arthurmaciel/ipe-lang/issues/176)) ([7cb173b](https://github.com/arthurmaciel/ipe-lang/commit/7cb173b5ed5a8f7dd68446feef0cc5d971893c70))
* **examples:** wasm-* build — Ipe.Live→Ipe.Web + wasm-safe async, add to sweep ([#209](https://github.com/arthurmaciel/ipe-lang/issues/209)) ([#227](https://github.com/arthurmaciel/ipe-lang/issues/227)) ([e840919](https://github.com/arthurmaciel/ipe-lang/commit/e8409198ded0b673e63425d3f2acf9497875e3b1))
* **resolve:** exclude hidden dirs from the package content hash ([#201](https://github.com/arthurmaciel/ipe-lang/issues/201)) ([25734c1](https://github.com/arthurmaciel/ipe-lang/commit/25734c147a7dab22ec39be9c22d58e32f53c902a))
* **sweep:** honest cli exit-code gate + self-explanatory HS256 error ([#182](https://github.com/arthurmaciel/ipe-lang/issues/182)) ([b49098a](https://github.com/arthurmaciel/ipe-lang/commit/b49098a387db3b46b79774ee6b55f589f6fe010d))

## [0.1.20](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.19...ipe-v0.1.20) (2026-07-26)


### Features

* **cli:** wire ipe package publish live submit (fork push + prefilled PR) ([#165](https://github.com/arthurmaciel/ipe-lang/issues/165)) ([329bf27](https://github.com/arthurmaciel/ipe-lang/commit/329bf270a9f6fb0c021c762b665d1c901eae8a56)), closes [#137](https://github.com/arthurmaciel/ipe-lang/issues/137) [#152](https://github.com/arthurmaciel/ipe-lang/issues/152)


### Bug Fixes

* **cli:** surface the real diagnostic when package capability inference finds nothing lowerable ([#168](https://github.com/arthurmaciel/ipe-lang/issues/168)) ([0bf550c](https://github.com/arthurmaciel/ipe-lang/commit/0bf550c2cc627804b3fc90cd7d1f0c91e248930a)), closes [#159](https://github.com/arthurmaciel/ipe-lang/issues/159)

## [0.1.19](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.18...ipe-v0.1.19) (2026-07-26)


### Features

* **cli:** apply 2-space gutter + frame to all human-facing output ([#164](https://github.com/arthurmaciel/ipe-lang/issues/164)) ([5956c10](https://github.com/arthurmaciel/ipe-lang/commit/5956c107e4bcea26f07be381092ae70264ca8cc3))
* **cli:** distribute AGENTS.md — ipe init writes it + ipe upgrade-agents refreshes it ([#146](https://github.com/arthurmaciel/ipe-lang/issues/146)) ([#158](https://github.com/arthurmaciel/ipe-lang/issues/158)) ([15d2096](https://github.com/arthurmaciel/ipe-lang/commit/15d20968fffc4863b2dda2cee0919711049a2126))
* **cli:** frame + gutter human output uniformly (part of [#148](https://github.com/arthurmaciel/ipe-lang/issues/148)) ([#153](https://github.com/arthurmaciel/ipe-lang/issues/153)) ([474cd59](https://github.com/arthurmaciel/ipe-lang/commit/474cd5985589f8c748eb95f802bf6559e973be2e))
* **cli:** ipe package publish — compute the index entry and open the index PR ([#151](https://github.com/arthurmaciel/ipe-lang/issues/151)) ([373455e](https://github.com/arthurmaciel/ipe-lang/commit/373455e8770dddedd621506319d18add610e42da))
* **cli:** ipe upgrade — self-update via the release installer ([#145](https://github.com/arthurmaciel/ipe-lang/issues/145)) ([#161](https://github.com/arthurmaciel/ipe-lang/issues/161)) ([cba4090](https://github.com/arthurmaciel/ipe-lang/commit/cba4090ac18ce955539e29ec639465431703cea5))
* **cli:** show human-friendly build progress ([#143](https://github.com/arthurmaciel/ipe-lang/issues/143)) ([#160](https://github.com/arthurmaciel/ipe-lang/issues/160)) ([d06705f](https://github.com/arthurmaciel/ipe-lang/commit/d06705ff01f518379d332fd0e5c5d0c8f912470c))
* **cli:** suggest the nearest command when one is mistyped ([#147](https://github.com/arthurmaciel/ipe-lang/issues/147)) ([#162](https://github.com/arthurmaciel/ipe-lang/issues/162)) ([3c1326c](https://github.com/arthurmaciel/ipe-lang/commit/3c1326cc0e8a711f95fd70005b950c6fb560250a))


### Bug Fixes

* **backend:** skip the rustfmt normalization pass when rustfmt is absent ([#156](https://github.com/arthurmaciel/ipe-lang/issues/156)) ([ab384ca](https://github.com/arthurmaciel/ipe-lang/commit/ab384cad3f105caa660991d9cca20da3a0e03d5d))

## [0.1.18](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.17...ipe-v0.1.18) (2026-07-26)


### Features

* **install:** add binary availability check and cargo detection ([7c9147f](https://github.com/arthurmaciel/ipe-lang/commit/7c9147f137830a7916263f2ffbd72fd91509c6f2))
* polish install.sh — +2sp indent, ~/.cargo/env detection, reword bugs line ([#136](https://github.com/arthurmaciel/ipe-lang/issues/136)) ([6fb3e7a](https://github.com/arthurmaciel/ipe-lang/commit/6fb3e7a1d308615dd7b365e6894524264247b553))
* **sandbox:** scope the runtime jail to native-bearing programs (ADR 0040) ([#101](https://github.com/arthurmaciel/ipe-lang/issues/101)) ([9a12f23](https://github.com/arthurmaciel/ipe-lang/commit/9a12f23a1790f08c8df03819a3cb5bd7463622e7))


### Bug Fixes

* **installer:** mirror the style footer phrase (fixes install_style_drift) ([#154](https://github.com/arthurmaciel/ipe-lang/issues/154)) ([46d24bb](https://github.com/arthurmaciel/ipe-lang/commit/46d24bba1f631828ae9f344952594382d9817180))

## [0.1.17](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.16...ipe-v0.1.17) (2026-07-25)


### Bug Fixes

* **case:** ipê is not valid -&gt; Ipê ([dc7689f](https://github.com/arthurmaciel/ipe-lang/commit/dc7689f4c7a090d95c89a05f19d3e3fb842b1d02))
* **ipe-cli:** add --stdin flag to ipe fmt for editor integration ([34b6c40](https://github.com/arthurmaciel/ipe-lang/commit/34b6c40ea4afd42cf4b47b10c0ea8c58806db921))

## [0.1.16](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.15...ipe-v0.1.16) (2026-07-24)


### Bug Fixes

* **runtime:** gate cli_run_cmd on the tui feature (its sole caller) ([#93](https://github.com/arthurmaciel/ipe-lang/issues/93)) ([cb1f625](https://github.com/arthurmaciel/ipe-lang/commit/cb1f6251217d1f56300927dbf64a5c89f4f4f3fa))

## [0.1.15](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.14...ipe-v0.1.15) (2026-07-22)


### Features

* [#337](https://github.com/arthurmaciel/ipe-lang/issues/337) row polymorphism + first-class accessors ([#10](https://github.com/arthurmaciel/ipe-lang/issues/10)) ([90af7a5](https://github.com/arthurmaciel/ipe-lang/commit/90af7a5f16bf10028dd4220aa18a1f212504b38d))
* [#337](https://github.com/arthurmaciel/ipe-lang/issues/337) row-polymorphic record annotations { r | f : T } ([#14](https://github.com/arthurmaciel/ipe-lang/issues/14)) ([d0c2514](https://github.com/arthurmaciel/ipe-lang/commit/d0c2514f1aa80ce22161a7fbcdabe3c8ca77eb78))
* **#210:** seal Ipe.Email — Email.send + EmailMessage/EmailProvider fold ([e1486e4](https://github.com/arthurmaciel/ipe-lang/commit/e1486e4768ad2737b30f53d9d953dcb15d9763d0))
* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) call-arg combining render primitive + fn_call_width ([#12](https://github.com/arthurmaciel/ipe-lang/issues/12)) ([becfd89](https://github.com/arthurmaciel/ipe-lang/commit/becfd891a6626476e5d4fc2a531da7a3e9636383))
* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) leaf-arm + statement emitters toward cutover ([#9](https://github.com/arthurmaciel/ipe-lang/issues/9)) ([a5bb75e](https://github.com/arthurmaciel/ipe-lang/commit/a5bb75ee22d7ddf5b697b8574a6773f324c295e5))
* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) native emitter sweep to 0 divergences (cutover gated by non-body content) ([#32](https://github.com/arthurmaciel/ipe-lang/issues/32)) ([5581976](https://github.com/arthurmaciel/ipe-lang/commit/558197635e215d68beff405b67aacc7cd3e31760))
* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) port IpeStringify format! emitters to native Doc rendering ([#35](https://github.com/arthurmaciel/ipe-lang/issues/35)) ([ce4a766](https://github.com/arthurmaciel/ipe-lang/commit/ce4a7660796ca3e6a48257e5116f17ab2f57f7ff))
* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) recursive-Shape combine + chain glue — sweep 9→5 ([#16](https://github.com/arthurmaciel/ipe-lang/issues/16)) ([29ff276](https://github.com/arthurmaciel/ipe-lang/commit/29ff2761462b48145a9794d875287bff9d7c894a))
* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) wire native Doc emitter into production emit_func ([#40](https://github.com/arthurmaciel/ipe-lang/issues/40)) ([f20e430](https://github.com/arthurmaciel/ipe-lang/commit/f20e430fec9a0fb20a3c40ca6d807bc27c01de23))
* **backend:** assignment-RHS-break Doc token for the let-value layout axis ([3d07e66](https://github.com/arthurmaciel/ipe-lang/commit/3d07e66caafd881205f0a486f5017f856db94b69))
* **backend:** native Doc emitter for Expr::Match via MatchArmTail ([0e4b182](https://github.com/arthurmaciel/ipe-lang/commit/0e4b182317cf742dfb1e520d15c16e029f863de5))
* **backend:** native Doc emitters for Lambda/SharedLambda + immediately-applied Apply ([740ba27](https://github.com/arthurmaciel/ipe-lang/commit/740ba2772ef25e38c34b52fcb27f11dd552f2682))
* **backend:** native Rust formatter Doc IR + renderer (P0) ([a675f3f](https://github.com/arthurmaciel/ipe-lang/commit/a675f3f484ab70a6c106dd5a527b7c5eb50aa037))
* **backend:** P1 Doc-building emit path — binop-chain builder + SEAL property test ([f7dbaeb](https://github.com/arthurmaciel/ipe-lang/commit/f7dbaeba919bd41b3155aa8a78047470a049107c))
* **backend:** real flat-vs-break Group in native renderer + structured if builder ([ff902e7](https://github.com/arthurmaciel/ipe-lang/commit/ff902e753ce2dc4d20b2f7d86a0c229c5943be2d))
* **backend:** SEAL-visible BraceBody Doc token for rustfmt brace add/strip ([e562ac9](https://github.com/arthurmaciel/ipe-lang/commit/e562ac9912db8a429bb9d86921b595bae24a9fa7))
* **backend:** structured Ctor Doc builder (payload + runtime-enum) ([aadb94c](https://github.com/arthurmaciel/ipe-lang/commit/aadb94c32ff32ead5790384ec0c5c31786c1f2b4))
* **backend:** structured delimited-list Doc builders + break-conditional trailing comma ([bcdc2c7](https://github.com/arthurmaciel/ipe-lang/commit/bcdc2c7f141b693c0c549f7ecfcde245c49dbb1a))
* **backend:** structured Destructure-block Doc builder ([c8c0e46](https://github.com/arthurmaciel/ipe-lang/commit/c8c0e46ff948a63bc0458b1e37a1ed095b2ca85b))
* **backend:** structured general-apply Doc builder ([059e4e5](https://github.com/arthurmaciel/ipe-lang/commit/059e4e52c16bd93aa5b83429c249c82b24013837))
* **backend:** structured generic call-tail Doc builder ([0fc9554](https://github.com/arthurmaciel/ipe-lang/commit/0fc9554c2a1a91f490e058f534eda6b1bb282d82))
* **backend:** structured let-block Doc builder ([7e7c274](https://github.com/arthurmaciel/ipe-lang/commit/7e7c274ee1ad8966bb7b729f9257b7f5eb8343b0))
* **backend:** structured record-literal Doc builder ([7cd66ca](https://github.com/arthurmaciel/ipe-lang/commit/7cd66cadfd5083030ab13dbfcf0c32ac6d65292f))
* **backend:** structured record-update Doc builder ([fb00c59](https://github.com/arthurmaciel/ipe-lang/commit/fb00c59105e47d565b5be4c2e14a7c026ee342db))
* **backend:** structured sync task-seq Doc builder ([87caa50](https://github.com/arthurmaciel/ipe-lang/commit/87caa50abafbd37590fc3cfafbe99588ac245c32))
* **ci:** examples-sweep bot-commits the refreshed upstream mirror ([5b71393](https://github.com/arthurmaciel/ipe-lang/commit/5b7139328c222d078ac0b0d23f13e4690e59bfd2))
* **ci:** live upstream-Sky parity comparison (retires the cached oracle) ([2404d2e](https://github.com/arthurmaciel/ipe-lang/commit/2404d2ec2de99ff9b985ef6b9353d203196e70cf))
* **cli:** add `ipe version` (also --version / -V) ([f504895](https://github.com/arthurmaciel/ipe-lang/commit/f5048959458057b210c9372c67a341b47464c064))
* **cli:** aligned --help column + consent-based installer PATH setup ([#50](https://github.com/arthurmaciel/ipe-lang/issues/50)) ([fb98598](https://github.com/arthurmaciel/ipe-lang/commit/fb98598988dc6fd877ea5fc9e329c22458c460af))
* **cli:** capabilities acceptance over examples + README ([333a3b4](https://github.com/arthurmaciel/ipe-lang/commit/333a3b42b98f27c66dbf07e2d5cec65e884983e7))
* **cli:** CLI-UI single-source-of-truth (style module) + installer polish + SSOT principle ([#75](https://github.com/arthurmaciel/ipe-lang/issues/75)) ([720e2c6](https://github.com/arthurmaciel/ipe-lang/commit/720e2c6b1304d10084c7baa3eb690e70845e5b16))
* **cli:** declutter ipe help (soft yellow, no optional-arg overview, bug-report footer) ([#15](https://github.com/arthurmaciel/ipe-lang/issues/15)) ([2f1b6dd](https://github.com/arthurmaciel/ipe-lang/commit/2f1b6dd85434decc6b20518cad82f4b7673c619b))
* **cli:** default entry for build/run/watch in project directories ([ebe88c7](https://github.com/arthurmaciel/ipe-lang/commit/ebe88c765479c391a4a467f4d76b12d1e3cc5735))
* **cli:** human-first output model — --plain/--json, gutter, error-shows-help, Package authoring section ([#78](https://github.com/arthurmaciel/ipe-lang/issues/78)) ([ea01f7f](https://github.com/arthurmaciel/ipe-lang/commit/ea01f7f3530646557c3916149d47c720828ec2e3))
* **cli:** ipe capabilities report + declared-set verify ([7eb4877](https://github.com/arthurmaciel/ipe-lang/commit/7eb48774be0c008278510ebeb96aa80b2e1ce81c))
* **cli:** ipe fmt — elm-format-compatible source formatter ([8e3cbef](https://github.com/arthurmaciel/ipe-lang/commit/8e3cbefe6969d990dea0dcfa9a3c07d5547986fe))
* **cli:** ipe init scaffolds an Ipe.Live counter project ([60277f6](https://github.com/arthurmaciel/ipe-lang/commit/60277f64e91a8dade4ca8c898740a47e9b2c8ff0))
* **cli:** ipe run --static — shared static-flag parser + plan resolver across build/run; binary located via cargo metadata target_directory (honours CARGO_TARGET_DIR / user target-dir pins) ([5e25b11](https://github.com/arthurmaciel/ipe-lang/commit/5e25b11ddd4b452eed3a97446477b62912915448))
* **cli:** sectioned, coloured top-level help and per-command --help ([976dc92](https://github.com/arthurmaciel/ipe-lang/commit/976dc92030706e38005e358f6083fb5a2177ba41))
* **cli:** SP2 — ipe rust group + ipe.toml schema ([#4](https://github.com/arthurmaciel/ipe-lang/issues/4)) ([364b213](https://github.com/arthurmaciel/ipe-lang/commit/364b213fc5dd69b9a95d17e5db8147ddf0397d69))
* **cli:** SP3 — index resolver + lockfile + ipe add ([#8](https://github.com/arthurmaciel/ipe-lang/issues/8)) ([c358c2a](https://github.com/arthurmaciel/ipe-lang/commit/c358c2af27271ed2b766c0f888eb537fa8251bad))
* **cli:** SP5 — ipe diff + enforced semver ([#11](https://github.com/arthurmaciel/ipe-lang/issues/11)) ([038ad53](https://github.com/arthurmaciel/ipe-lang/commit/038ad534d0b65ab74f57ed80e1c0b030547e7b44))
* **cli:** typed arg parsing — invalid optional-flag combinations unrepresentable + exhaustive tests ([#34](https://github.com/arthurmaciel/ipe-lang/issues/34)) ([783e922](https://github.com/arthurmaciel/ipe-lang/commit/783e92212020b6d6c2bde9dd1c11564be9790af5))
* **emit:** post-emit rustfmt pass (fail-closed) so emitted Rust is rustfmt-clean; regenerate 73 goldens to canonical form ([3f624bc](https://github.com/arthurmaciel/ipe-lang/commit/3f624bc8a246e2eaf7a2ec541cb4cd050d8aaa7f))
* **examples:** 13-skyshop transpose in progress — Db+Auth de-shimmed onto real SDKs, 8-crate cache checked in ([41aeca9](https://github.com/arthurmaciel/ipe-lang/commit/41aeca91c1920133fda3b629e32eea29526c09e3))
* **examples:** add examples/sky/manifest.toml — Sky→Ipe patch registry ([#299](https://github.com/arthurmaciel/ipe-lang/issues/299)) ([60764be](https://github.com/arthurmaciel/ipe-lang/commit/60764bebbc9d183f054e492f54f46e089dfe1cf0))
* **examples:** bring composite examples 36-38 into sweep scope ([#377](https://github.com/arthurmaciel/ipe-lang/issues/377)) ([#83](https://github.com/arthurmaciel/ipe-lang/issues/83)) ([6f6193a](https://github.com/arthurmaciel/ipe-lang/commit/6f6193a9af83587b69b41fba2d874d286bb25e40))
* **examples:** port go-ffi examples to Ipê + Rust crates (7 examples) ([#80](https://github.com/arthurmaciel/ipe-lang/issues/80)) ([c1f3bf7](https://github.com/arthurmaciel/ipe-lang/commit/c1f3bf782b2f78274d6f5db3297cb5086d196d15))
* **examples:** track the upstream Sky example mirror (42 examples, source-only) ([ee24e09](https://github.com/arthurmaciel/ipe-lang/commit/ee24e0987162f83520eb1e7f823abda672d546f4))
* **ffi-inspector:** param-shape admission — conversion-bound nominal targets (identity + From&lt;String&gt; preference), enum-level non_exhaustive ctor restoration, Clone-enum field accessors ([0c7e9d2](https://github.com/arthurmaciel/ipe-lang/commit/0c7e9d28594b19b3ac95a9ed72eceded2b9d5dbd))
* **ffi-inspector:** resumable manifest inspection — stable probe root + cross-crate proof-map checkpoint ([989ee3b](https://github.com/arthurmaciel/ipe-lang/commit/989ee3bd35091840062b910f9929e0a3973b9a26))
* **ffi-inspector:** stripe-send — doc-hidden surfacing + cross-crate Send proof (3 of 4 walls) ([8a2f590](https://github.com/arthurmaciel/ipe-lang/commit/8a2f5902ffec29c2b8487934bd9ce842650083c2))
* **ffi-inspector:** stripe-send F2 — cross-crate proven-public Output paths (GLOBAL_XC_PUBLIC_PATHS) ([3146360](https://github.com/arthurmaciel/ipe-lang/commit/314636064522c0e887470a520963df966a1569d2))
* **ffi-inspector:** stripe-send F2 (cont.) — resolve cross-crate send Output in type_to_typeref ([a29b0d3](https://github.com/arthurmaciel/ipe-lang/commit/a29b0d30903717cd3b06f41962ba8a49d4a73c70))
* **ffi-inspector:** stripe-send W4 — return-nameability by defining-type identity (4th wall) ([1fefa83](https://github.com/arthurmaciel/ipe-lang/commit/1fefa83f99f6c129552704695019674120bb0982))
* **ffi:** [#317](https://github.com/arthurmaciel/ipe-lang/issues/317)+[#326](https://github.com/arthurmaciel/ipe-lang/issues/326) auto-binding coverage — bundle-generics, dyn-Fn systems, multi-result tuples ([#72](https://github.com/arthurmaciel/ipe-lang/issues/72)) ([572ddbd](https://github.com/arthurmaciel/ipe-lang/commit/572ddbdd1ada659dd04e4920b619786b8dbf781a))
* **ffi:** [#347](https://github.com/arthurmaciel/ipe-lang/issues/347) sync closure adapter ([rust.provide.closure]) ([#36](https://github.com/arthurmaciel/ipe-lang/issues/36)) ([72cfd04](https://github.com/arthurmaciel/ipe-lang/commit/72cfd041b1c87a29e3415f4590fdf638b507e605))
* **ffi:** [#350](https://github.com/arthurmaciel/ipe-lang/issues/350) closure-manifest glue + [#348](https://github.com/arthurmaciel/ipe-lang/issues/348) struct-with-trait-impl ([#38](https://github.com/arthurmaciel/ipe-lang/issues/38)) ([ade2673](https://github.com/arthurmaciel/ipe-lang/commit/ade2673adffb0c3d118384f57e06b5b5bc9b06a1))
* **ffi:** [#352](https://github.com/arthurmaciel/ipe-lang/issues/352) provide.* Ipê-side forwarder plumbing ([#46](https://github.com/arthurmaciel/ipe-lang/issues/46)) ([cc9718b](https://github.com/arthurmaciel/ipe-lang/commit/cc9718b34d7e26a562797466d0b9fc2bcd07653b))
* **ffi:** [#353](https://github.com/arthurmaciel/ipe-lang/issues/353) provide.closure opaque returns ([#47](https://github.com/arthurmaciel/ipe-lang/issues/47)) ([bc86028](https://github.com/arthurmaciel/ipe-lang/commit/bc86028122e36aee235f36877a1f46191c07d397))
* **ffi:** [#354](https://github.com/arthurmaciel/ipe-lang/issues/354) opaque struct fields / enum payloads ([#57](https://github.com/arthurmaciel/ipe-lang/issues/57)) ([41af8c7](https://github.com/arthurmaciel/ipe-lang/commit/41af8c77b55e55422ab08c76a8db91ae84595c0a))
* **ffi:** [#364](https://github.com/arthurmaciel/ipe-lang/issues/364) Tier 2 phases 1-3 — bind author-supplied Rust wrapper crates ([#62](https://github.com/arthurmaciel/ipe-lang/issues/62)) ([28618f7](https://github.com/arthurmaciel/ipe-lang/commit/28618f7a7536b4c293ae6ddc65e02e2092e1114c))
* **ffi:** [#365](https://github.com/arthurmaciel/ipe-lang/issues/365) Tier 2 capability inference + fail-closed enforcement ([#71](https://github.com/arthurmaciel/ipe-lang/issues/71)) ([965aa15](https://github.com/arthurmaciel/ipe-lang/commit/965aa158a2616ad47c7e3441ad3ee9f83b989267))
* **ffi:** [#366](https://github.com/arthurmaciel/ipe-lang/issues/366) Tier 2 #[ipe::provide] trait-impl escape hatch ([#69](https://github.com/arthurmaciel/ipe-lang/issues/69)) ([2cc5114](https://github.com/arthurmaciel/ipe-lang/commit/2cc5114470177a84a6c7b67d4ffa96b90c5222db))
* **ffi:** [#369](https://github.com/arthurmaciel/ipe-lang/issues/369) closure-&gt;run handoff — drive foreign loops with Ipê closures ([#70](https://github.com/arthurmaciel/ipe-lang/issues/70)) ([8c4b662](https://github.com/arthurmaciel/ipe-lang/commit/8c4b6627baa46a7aeb1df4ebb462fd76599b612e))
* **ffi:** async wrappers arm AbortOnDrop + route JoinError through ipe_error_from_foreign (Δ1/Δ2) ([cde8092](https://github.com/arthurmaciel/ipe-lang/commit/cde809246b7feeef8acfb5d3d53f28340bfda23f))
* **ffi:** async-returning provide.closure ([#55](https://github.com/arthurmaciel/ipe-lang/issues/55)) ([8bce55b](https://github.com/arthurmaciel/ipe-lang/commit/8bce55badf5c678df9e4066fa0a78e0c75dc28b2))
* **ffi:** async-SDK consumer path — closed-instance synthesis, alias fold, used-set forwarder DCE; firestore 0.49 bound shim-free E2E ([d5327f2](https://github.com/arthurmaciel/ipe-lang/commit/d5327f276d727792c2ecec4fc2f56e294bcfb4e5))
* **ffi:** borrow-thread &self/&mut self FFI readers through the result ([9b1f4ce](https://github.com/arthurmaciel/ipe-lang/commit/9b1f4ce6bd936b6fb4bb83b2998bcaf38d0f63c2))
* **ffi:** checked fallible setters for narrowing integer fields — try_from + typed Err replaces the setter drop (no silent truncation; f32/containers stay dropped) ([af035d3](https://github.com/arthurmaciel/ipe-lang/commit/af035d3ded91ab211fd177fc287189960363a4ef))
* **ffi:** firebase-bind shim-free — rs-firebase-admin-sdk 4.3 SEAL green (verify chain live) ([06f334b](https://github.com/arthurmaciel/ipe-lang/commit/06f334bd68899f582987da9501ccc458ee406756))
* **ffi:** foreign-type-one-home — defid-keyed nominal unification across the installed-crate catalog ([857eb62](https://github.com/arthurmaciel/ipe-lang/commit/857eb62acf482f1b3c1c38b8a0c8317f41d8fc4c))
* **ffi:** one-shot manifest install + submodule Ipe-head path map + prerelease pin pass-through ([46f4e23](https://github.com/arthurmaciel/ipe-lang/commit/46f4e23af417ed9c06d0382a97859f078311e2bb))
* **ffi:** pkg.json is the sole catalog source — load re-derives the full consumer view ([9154383](https://github.com/arthurmaciel/ipe-lang/commit/915438347af734a8bb9b278b6b9cb76eac08f6bb))
* **ffi:** provide.enum (P4) + Debug derive — Iced binding spike ([#42](https://github.com/arthurmaciel/ipe-lang/issues/42)) ([eef982a](https://github.com/arthurmaciel/ipe-lang/commit/eef982a8e8d6a499c4d9ce38f19200b25d2c43b4))
* **ffi:** stripe-send W4 verified end-to-end + multi-crate dep-line unification ([1a20c74](https://github.com/arthurmaciel/ipe-lang/commit/1a20c746b27966b6b453157d715c51864744791d))
* **ffi:** version-pinned crate specs + feature/pin propagation through ipe add/install ([0d9c433](https://github.com/arthurmaciel/ipe-lang/commit/0d9c4336626d1855fce90b513d10d2430f34df32))
* **install:** fix curl (23), add spinner/percent/ETA + friendly branded messages ([#28](https://github.com/arthurmaciel/ipe-lang/issues/28)) ([b27654d](https://github.com/arthurmaciel/ipe-lang/commit/b27654d0ed7f3bdb72bec2772c23db14245f13ab))
* **kernels:** add Database capability; reclassify Db-family kernels ([5ec4ccd](https://github.com/arthurmaciel/ipe-lang/commit/5ec4ccde5c3c897cf785b84411901b4885146b64))
* **kernels:** per-kernel capability tag + Capability vocabulary ([2fd7401](https://github.com/arthurmaciel/ipe-lang/commit/2fd74017f2b80c97af362cdbd31a6e118d05b20e))
* **lower:** whole-program capability inference ([5a48b73](https://github.com/arthurmaciel/ipe-lang/commit/5a48b734ffcc31015c2caffaf267252db9b6e6d7))
* **lsp,db:** per-module typecheck_module query SEAM + migrate home-keyed handlers ([570760e](https://github.com/arthurmaciel/ipe-lang/commit/570760e20c2568b3acecf783f5eb1723a40175ae))
* **lsp:** [#295](https://github.com/arthurmaciel/ipe-lang/issues/295) document formatting + rangeFormatting ([71f81b6](https://github.com/arthurmaciel/ipe-lang/commit/71f81b6b429e649f5f655d42b56e756b5f571a10))
* **lsp:** [#296](https://github.com/arthurmaciel/ipe-lang/issues/296) code actions — diagnostic-driven quick-fixes ([3177844](https://github.com/arthurmaciel/ipe-lang/commit/3177844f6a33861cd31a0a46ebbc1bd7dedc3a4f))
* **lsp:** [#297](https://github.com/arthurmaciel/ipe-lang/issues/297) semantic tokens full — 10-type legend over the parse AST ([7ef7aa2](https://github.com/arthurmaciel/ipe-lang/commit/7ef7aa24619d6599d558d0a51908044f0e3e0d07))
* **lsp:** [#298](https://github.com/arthurmaciel/ipe-lang/issues/298) signature help + inlay hints ([8e5b3dc](https://github.com/arthurmaciel/ipe-lang/commit/8e5b3dc68619b2696ef9c2175fdb794778b06566))
* **lsp:** completion, go-to-definition, find-references, rename ([926bb34](https://github.com/arthurmaciel/ipe-lang/commit/926bb34f3008e8f2a755243f3688f027432dba4b))
* **lsp:** document links + folding ranges ([62e9141](https://github.com/arthurmaciel/ipe-lang/commit/62e9141e07b49e496153bdec2addf3880098a7ea))
* **lsp:** ipe lsp server — live diagnostics, hover, document symbols over the salsa graph ([64b684e](https://github.com/arthurmaciel/ipe-lang/commit/64b684e7a4a8aa014a7023fe4786626ba494f0a6))
* **lsp:** type-directed completion via additive ExpectedTypes solver sidecar ([d1b475e](https://github.com/arthurmaciel/ipe-lang/commit/d1b475e27066286828d09a79501ef1588065a03a))
* **lsp:** wire [#295](https://github.com/arthurmaciel/ipe-lang/issues/295)-298 into server — capabilities + request handlers ([f0b5046](https://github.com/arthurmaciel/ipe-lang/commit/f0b5046600cbacbf97c65c8462be38fef74b8825))
* **pkg:** [#368](https://github.com/arthurmaciel/ipe-lang/issues/368) SP4 Tier-1 package gate + ipe package audit ([#66](https://github.com/arthurmaciel/ipe-lang/issues/66)) ([5838ff5](https://github.com/arthurmaciel/ipe-lang/commit/5838ff5de03157dd07bad5a3bc3f846e0fb8e1b9))
* **runtime:** process-global tokio runtime for block_on + AbortOnDrop cancel guard (async-FFI bridge H1/Δ1 primitives) ([e901c2b](https://github.com/arthurmaciel/ipe-lang/commit/e901c2bf2a586ac6e3861231c2ad5e0d6d8cde57))
* **security:** [#359](https://github.com/arthurmaciel/ipe-lang/issues/359) drive the abrupt-failure ledger toward zero ([#58](https://github.com/arthurmaciel/ipe-lang/issues/58)) ([ba4c309](https://github.com/arthurmaciel/ipe-lang/commit/ba4c3091ec33785b975d610d00a7d29cff7b5442))
* **security:** [#371](https://github.com/arthurmaciel/ipe-lang/issues/371) runtime capability sandbox + admit-and-isolate Tier 2 wrappers ([#82](https://github.com/arthurmaciel/ipe-lang/issues/82)) ([04214e5](https://github.com/arthurmaciel/ipe-lang/commit/04214e5ab0868dace57586c4c254fc7f8f746ee7))
* **security:** token-scanner gate + clippy hardening for authored abrupt-failure ([#54](https://github.com/arthurmaciel/ipe-lang/issues/54)) ([843a17b](https://github.com/arthurmaciel/ipe-lang/commit/843a17baedb626ed4ec90bec2ad02e14fb61a67b))
* **static:** [#244](https://github.com/arthurmaciel/ipe-lang/issues/244) add aarch64-unknown-linux-musl static target (config + CI) ([297927a](https://github.com/arthurmaciel/ipe-lang/commit/297927aef8385db05b66492b15a4c489f90a681b))
* **static:** pin rust-lld self-contained for aarch64-musl — portable static cross-build (no musl-cross-gcc) ([2935170](https://github.com/arthurmaciel/ipe-lang/commit/2935170bb92e1fca5f1917daa5b4a15c44031d38))
* **stdlib:** [#339](https://github.com/arthurmaciel/ipe-lang/issues/339) pure elm/core fills (List/Dict/Set/Result/Char/String) ([#43](https://github.com/arthurmaciel/ipe-lang/issues/43)) ([37da719](https://github.com/arthurmaciel/ipe-lang/commit/37da7192da96bf9ca651bc594ded7a4482402145))
* **stdlib:** [#342](https://github.com/arthurmaciel/ipe-lang/issues/342) Task + decoder combinators ([#49](https://github.com/arthurmaciel/ipe-lang/issues/49)) ([1fc9149](https://github.com/arthurmaciel/ipe-lang/commit/1fc91496758dfa4ac63dd02f2018243b111264b3))
* **stdlib:** Cmd.map / Sub.map ([#44](https://github.com/arthurmaciel/ipe-lang/issues/44)) ([eca501b](https://github.com/arthurmaciel/ipe-lang/commit/eca501b9265720c1970b290ee67df220e935fbdf))
* **sweep:** fail loud on unpatched new upstream examples + self-regression docs ([#300](https://github.com/arthurmaciel/ipe-lang/issues/300)) ([f60bdb5](https://github.com/arthurmaciel/ipe-lang/commit/f60bdb50cc045b75664ba10d0b4ab520fc696ebe))
* **sweep:** IPE_SWEEP_STATIC=1 — per-example --static musl build (CWD = emitted crate dir), ldd-asserted static-ness, static-binary RUN, webview typed-refusal assertion ([d177666](https://github.com/arthurmaciel/ipe-lang/commit/d177666721ad4a7c6db89810986498cd5a7b0869))
* **sweep:** Ipê-only upstream-mirror sweep, retire the Go-oracle equivalence infra ([7773a7f](https://github.com/arthurmaciel/ipe-lang/commit/7773a7f9c12a6eed634dfe00251aebcb32933c95))
* **types,db:** per-module scoped typecheck behind typed interfaces ([6991b4c](https://github.com/arthurmaciel/ipe-lang/commit/6991b4c9f79e3b3987e641613020c2005db5f9c8))
* **wasm:** browser TEA sink + target-neutral dom re-home ([4d0a74c](https://github.com/arthurmaciel/ipe-lang/commit/4d0a74c8f28211e8f2f5bb7ba37662bb8fbbc3de))
* **wasm:** compile the Ipê frontend to WASM + browser-native playground ([40587ca](https://github.com/arthurmaciel/ipe-lang/commit/40587ca95b78ab4c6073950e0c092249fc931d3f))
* **wasm:** Ipe.Env.public kernel + build-time publicEnv embedding ([#287](https://github.com/arthurmaciel/ipe-lang/issues/287)) ([6729c68](https://github.com/arthurmaciel/ipe-lang/commit/6729c68081dc9fe9df8ec7231ad1d84e40d4c95a))
* **wasm:** M0 pure-kernel wasm floor — runtime builds to wasm32-unknown-unknown (default + json) as an enforced CI gate ([9a66a2d](https://github.com/arthurmaciel/ipe-lang/commit/9a66a2d2272e1afbca5654729832593c45c6eee6))
* **wasm:** M1 target-keyed kernel gate + M2 emission branch + M3 browser slice — ipe build --target wasm, Ipe.Ui proven in Chromium ([9523e03](https://github.com/arthurmaciel/ipe-lang/commit/9523e03d01b3f724cd56b07153626913ba23c75c))
* **wasm:** M4 Cmd/Sub browser-effects bridge — Log/Random/Http/WebSocket/Task substitutes, timers, in-tab pub/sub ([e95af5a](https://github.com/arthurmaciel/ipe-lang/commit/e95af5ad106fce8ae05eccf3f13e8e3e94429ca0))
* **wasm:** M5 Layer-2 module classification + reachability closure + [wasm] ipe.toml config (IPE-N0030) ([d668b89](https://github.com/arthurmaciel/ipe-lang/commit/d668b897d32cf9e576fedbbca3a56499e9d634b2))
* **wasm:** M6 Target A MVP — pure-client SPA end-to-end ([#240](https://github.com/arthurmaciel/ipe-lang/issues/240)) ([30533d1](https://github.com/arthurmaciel/ipe-lang/commit/30533d1de968f44ed406e121ddc4fea52a4a9d44))
* **wasm:** M7 SSR hydration — island serialiser, adopt path, hydrate export, field-type gate ([0b7f543](https://github.com/arthurmaciel/ipe-lang/commit/0b7f543108a5ffc7e8ef67b126555ef4c01f6012))
* **wasm:** M8 playground B1 — server-compile-then-ship-WASM backend ([0171f27](https://github.com/arthurmaciel/ipe-lang/commit/0171f27dfd023bab77dd56ff616d7e9ef0d1813a))


### Bug Fixes

* **#210:** register Ipe.Config family — Decoder carrier + 16 kernels SEAL ([627afe0](https://github.com/arthurmaciel/ipe-lang/commit/627afe0563141f68355390b4cc5b6ed4eb902cec))
* **backend,lower:** SEAL 13-skyshop — cfg-record arg-order hoist, sync-capture param promotion trigger, single-boundary Arc callback ([4c2ac25](https://github.com/arthurmaciel/ipe-lang/commit/4c2ac252c345920f0bcd1ad8682be597926c7860))
* **backend/tests:** pass wasm_hydrate_mode to EmitCtx::build test call sites (WASM M7 arity debt) ([711c725](https://github.com/arthurmaciel/ipe-lang/commit/711c725f151f520a24fdb518908e9234a5e59e5a))
* **backend:** close the module-set SEAL breach class (tea/live/http_stream drift) ([d3d0bd8](https://github.com/arthurmaciel/ipe-lang/commit/d3d0bd806ad128175f728600f522affd312daffb))
* **backend:** emitter emits at most one consecutive blank line ([7e69cb8](https://github.com/arthurmaciel/ipe-lang/commit/7e69cb8dd81798a81601d82de6abcb1923efd803))
* **backend:** FFI shake keep-decision accumulates instead of overwriting ([#283](https://github.com/arthurmaciel/ipe-lang/issues/283)) ([1a08994](https://github.com/arthurmaciel/ipe-lang/commit/1a08994c12097be16306a81e9bb4067e2ce41432))
* **backend:** two emitter fallbacks fail closed instead of emitting invalid Rust ([#281](https://github.com/arthurmaciel/ipe-lang/issues/281)) ([13ae62d](https://github.com/arthurmaciel/ipe-lang/commit/13ae62df0ae42ed3c1003e3c1494046df882df71))
* **canon:** bound type-alias expansion with depth + node-count limits (IPE-N0032) ([2d973e6](https://github.com/arthurmaciel/ipe-lang/commit/2d973e6f6dae10ee6b6322e9736d91cc8d5a0cf9))
* **canon:** canon-arity-gate — reject mis-arity built-in containers (IPE-N0031) ([1089a76](https://github.com/arthurmaciel/ipe-lang/commit/1089a76ae6096bbdc7d0a109ca725ab1e32f0962))
* **canon:** qualified cross-module alias references expand without exposing ([3fe074f](https://github.com/arthurmaciel/ipe-lang/commit/3fe074f073d3df9cdf45a0035269ab741090138d))
* **ci:** clippy duration_suboptimal_units (from_secs-&gt;from_mins, toolchain drift: CI stable was ahead of local rustup) + golden_alias_move_seal stale substring assertions (rustfmt reflow, same class as [#269](https://github.com/arthurmaciel/ipe-lang/issues/269)) + .gitignore drop ../sky ref + point oracle-version comment at its SSOT (tools/oracle/README.md) ([737eb50](https://github.com/arthurmaciel/ipe-lang/commit/737eb5030bb1baed34f24431ea77bcf26ddf4669))
* **ci:** clippy duration_suboptimal_units in lsp_stdio_e2e.rs (from_secs(60)-&gt;from_mins(1)) ([a69d923](https://github.com/arthurmaciel/ipe-lang/commit/a69d9233eb97d28f9973586ab1e461a54d32d8a4))
* **ci:** clippy map_unwrap_or in ipe_watch scope.rs (map_or(0, |d| d.as_nanos())) ([3dea51c](https://github.com/arthurmaciel/ipe-lang/commit/3dea51c8da00fe8a7626588ce09af616620bf0b3))
* **ci:** e2e shards — set IPE_ORACLE_SHARED_TARGET, closing the disk-exhaustion class ([bf61ebb](https://github.com/arthurmaciel/ipe-lang/commit/bf61ebbd555fdba38e8b06657d9ff478738479f6))
* **ci:** install jail primitives for static e2e + pin goldens to LF ([#86](https://github.com/arthurmaciel/ipe-lang/issues/86)) ([dbdd2a0](https://github.com/arthurmaciel/ipe-lang/commit/dbdd2a0bdf477b27f10e0bf6caed149a0f9ada1b))
* **ci:** install wry/tao Linux link deps in e2e job to clear SEAL breach ([#51](https://github.com/arthurmaciel/ipe-lang/issues/51)) ([4378a36](https://github.com/arthurmaciel/ipe-lang/commit/4378a36a54940fc223eb2200fa3a8854f3a071c3))
* **ci:** nextest ci profile, 6 E2E shards, sccache v0.0.9, --no-fail-fast ([d9adfbe](https://github.com/arthurmaciel/ipe-lang/commit/d9adfbe3f3c8b82d681025ab4522ea66427fb07e))
* **ci:** sky-parity picks the compiler binary, not the FFI inspector ([8239648](https://github.com/arthurmaciel/ipe-lang/commit/82396481cd0ee74fa3c3784862bee6a76266a078))
* **ci:** stale golden-test substring assertions masked by nextest fail-fast ([#191](https://github.com/arthurmaciel/ipe-lang/issues/191), [#193](https://github.com/arthurmaciel/ipe-lang/issues/193), [#195](https://github.com/arthurmaciel/ipe-lang/issues/195), [#190](https://github.com/arthurmaciel/ipe-lang/issues/190), ws-onerror, Ipe.Ui.Animation/Transition) ([facd9a7](https://github.com/arthurmaciel/ipe-lang/commit/facd9a76e7d6920dc0e867588df61742e75ae3b2))
* **ci:** three more E2E failures the disk-exhaustion fix stopped masking (server-clone reflow, webview Linux link gap, watch cold-build headroom) ([193760c](https://github.com/arthurmaciel/ipe-lang/commit/193760ce78a2c4dab6f21912d57e3f855cf64fb5))
* **cli:** clear nightly-clippy debt in wasm bundle step and manifest parsing ([47d89c2](https://github.com/arthurmaciel/ipe-lang/commit/47d89c2f3682ef58f15f165c320c46bf96b4bc7c))
* **clippy+fmt:** ffi.rs map_or + doc-paragraph split; rustfmt resolve.rs/types-lib drift ([89280bf](https://github.com/arthurmaciel/ipe-lang/commit/89280bf30629a99b948792820cfc1d49abbd20d7))
* **clippy:** clear pre-existing --all-targets lint debt in LSP, backend, canon, types, playground ([78d1d6e](https://github.com/arthurmaciel/ipe-lang/commit/78d1d6e07cd8ad3fe66e4c87363f8c5f7c7248f4))
* **cli:** rewrite two test match blocks as let-else (clippy pedantic on --all-targets) ([#3](https://github.com/arthurmaciel/ipe-lang/issues/3)) ([d5a7b37](https://github.com/arthurmaciel/ipe-lang/commit/d5a7b370760e39a4ca3f98eefed386afcea46d99))
* **cli:** Usage error strings match the redesigned help ([#23](https://github.com/arthurmaciel/ipe-lang/issues/23)) ([c0872ee](https://github.com/arthurmaciel/ipe-lang/commit/c0872ee57b08eae365f4ae14be2fc5a96d7718db))
* **deps:** regenerate package-lock.json — was stale, missing pixelmatch/pngjs entirely (only had playwright) ([d5d6dcf](https://github.com/arthurmaciel/ipe-lang/commit/d5d6dcf0961c6e3f3afe792dca24e22139e74a55))
* **diagnostics:** drop duplicate unreachable BuiltinTypeArity render arm (merge artifact) ([803d751](https://github.com/arthurmaciel/ipe-lang/commit/803d7515ba588b5fdb73207e0fb912e5940ef56a))
* **docs:** correct stale warm-db-reuse doc — the parity gate already exists ([#277](https://github.com/arthurmaciel/ipe-lang/issues/277)) ([96ece49](https://github.com/arthurmaciel/ipe-lang/commit/96ece49a8517fbaae2131e785e13b7b76d9ffa00))
* **emit:** opaque-type special-cases keyed on old Std home -&gt; Ipe (Cache/Config/Email) ([4a3cb4c](https://github.com/arthurmaciel/ipe-lang/commit/4a3cb4cd2be14f960ea7f37b0e002e8db4dfb8dd))
* **example:** 41-money-allocate-regression — correct main sig (drop Never) + fromMajor takes Int not Decimal; T4's example never compiled as shipped ([375e339](https://github.com/arthurmaciel/ipe-lang/commit/375e339b1ff5b81748415cda0579ccc10f3b905f))
* examples-sweep 26/29/31 build+run green ([#33](https://github.com/arthurmaciel/ipe-lang/issues/33)) ([26c0c11](https://github.com/arthurmaciel/ipe-lang/commit/26c0c1167d66e5a266307e106783bc45d6ff5fc8))
* **examples:** patch 00-standard-libs Money tests for Ipê's Result API ([bdfbca2](https://github.com/arthurmaciel/ipe-lang/commit/bdfbca2ea2a65b11664c51a2ce1c798d9239fc04))
* **examples:** restore native 01-hello-world; untrack sky-out build output ([bab7bc6](https://github.com/arthurmaciel/ipe-lang/commit/bab7bc6d65a94891237defdb9415dbc07ff4f1f4))
* **explain:** github issue links + trailing newline ([#18](https://github.com/arthurmaciel/ipe-lang/issues/18)) ([f8fbc6f](https://github.com/arthurmaciel/ipe-lang/commit/f8fbc6fa78359c0461a9739d0a5b41243a739a02))
* **ffi-inspector:** private-path-admission — drop external trait UFCS qualifiers threading a private module ([b1de3de](https://github.com/arthurmaciel/ipe-lang/commit/b1de3de4569d02b46587afdbfd605dd95138743c))
* **ffi-inspector:** serde-trait identity by raw defining path — restores the firestore serde document surface ([b8afea3](https://github.com/arthurmaciel/ipe-lang/commit/b8afea35b35d6932e9f77d0ce2367e5418f58915))
* **ffi-sandbox:** T1 F2-F6 — two-phase no-egress jail, narrowed ~/.cargo binds, mandatory caps + concurrent drain, bwrap-or-refuse, bounded owned cache root, scratch under ~/.cache/ipe + install prompt ([adf54c3](https://github.com/arthurmaciel/ipe-lang/commit/adf54c328ff444ebe68f74cd5bcefca33c6da5a6))
* **ffi:** [#326](https://github.com/arthurmaciel/ipe-lang/issues/326) admit coercible multi-result tuples for non-borrow-reader methods ([#21](https://github.com/arthurmaciel/ipe-lang/issues/21)) ([f37fe03](https://github.com/arthurmaciel/ipe-lang/commit/f37fe03f8e775b68b5ee5ddfb8dac3c3e167f52c))
* **ffi:** [#363](https://github.com/arthurmaciel/ipe-lang/issues/363) refuse recursive provide types at decode (SEAL) ([#61](https://github.com/arthurmaciel/ipe-lang/issues/61)) ([51bc01d](https://github.com/arthurmaciel/ipe-lang/commit/51bc01d0e6895a375f4b638e00beee2ba8438710))
* **ffi:** compositional OK-lift for generic wrappers + owned pass for bare-str substitutes ([5a04388](https://github.com/arthurmaciel/ipe-lang/commit/5a043886381beabe8edbddbddfdf728f59e207ff))
* **ffi:** fail-closed on reuse of a non-Clone FFI opaque handle (SEAL) ([3b0c86e](https://github.com/arthurmaciel/ipe-lang/commit/3b0c86ef51fd4286ad763cd80c5b6f089e8922df))
* **ffi:** fallible setter surface carries the Result layer the wrapper renders ([92c4905](https://github.com/arthurmaciel/ipe-lang/commit/92c490579d13dbd4418213a843d8b526ab3eba9b))
* **ffi:** gate Cargo feature names at the manifest boundary ([c159a9a](https://github.com/arthurmaciel/ipe-lang/commit/c159a9a914c203db0842042b58c26a69a6a5da1b))
* **ffi:** gate dep features + transitive name at the pkg.json decode boundary ([50f7934](https://github.com/arthurmaciel/ipe-lang/commit/50f793451e6668f9a31e536429cdcd73ab7e7e5b))
* **ffi:** maybe-coercion — IpeMaybe&lt;-&gt;Option at synthesised-instance boundaries ([dfa851c](https://github.com/arthurmaciel/ipe-lang/commit/dfa851ce1db08ad88ea78f83aea4784dd797d25b))
* **ffi:** one-home unification verified E2E — stripe 6-crate SEAL green ([36a4413](https://github.com/arthurmaciel/ipe-lang/commit/36a441383235c92ac7c0e584da566bb4447da748))
* **ffi:** seal the RCE sandbox for SDK-scale installs — chunk per crate + calibrate caps ([#309](https://github.com/arthurmaciel/ipe-lang/issues/309)) ([d2bec5d](https://github.com/arthurmaciel/ipe-lang/commit/d2bec5d87ef571b4894e405e568a84b48e8480f2))
* **ffi:** T1 clippy clean-up + warm-load byte-identity test ([80a5171](https://github.com/arthurmaciel/ipe-lang/commit/80a51714323e126636a5045fefa5b105ab936e38))
* **ffi:** T1 F1 — validated type/path/selector newtypes at the FFI decode boundary + re-derive load_catalog from validated inspection doc ([f1082f5](https://github.com/arthurmaciel/ipe-lang/commit/f1082f516ca619ad9077b3496ef8cab93190981c))
* **ffi:** T1 F1e — graceful legacy fallback for caches without pkg.json ([1a71754](https://github.com/arthurmaciel/ipe-lang/commit/1a717549b6370fae9cefb757033b51ac009207cb))
* **ffi:** validate crate version at decode boundary (CrateVersion newtype) ([6becd88](https://github.com/arthurmaciel/ipe-lang/commit/6becd887652bac03e9fe25ab5cf87c668cc7d327))
* **ffi:** validate pkg_path at the pkginfo decode boundary ([fe0338e](https://github.com/arthurmaciel/ipe-lang/commit/fe0338e3e9088cf8432b00817e3a2674e61cb40b))
* **fmt:** [#338](https://github.com/arthurmaciel/ipe-lang/issues/338) parenthesise negative literals in atom position ([#24](https://github.com/arthurmaciel/ipe-lang/issues/24)) ([3156db5](https://github.com/arthurmaciel/ipe-lang/commit/3156db56ce741dec9c95ec749bc4fdbe101a61df))
* **fmt:** a simple-reference first call arg always hugs the broken head line ([6d095cf](https://github.com/arthurmaciel/ipe-lang/commit/6d095cfc6b308e5dd16a16f54a843ba1ac7c8dbc))
* **fmt:** backward pipe `<|` breaks at the end of the left operand's line ([01a039a](https://github.com/arthurmaciel/ipe-lang/commit/01a039ac8096c6a6e92954ad2e74093a5e872b13))
* **fmt:** elm-format parity — modal layout, paren-safety, let/lambda/signature bugs ([cfa4d0b](https://github.com/arthurmaciel/ipe-lang/commit/cfa4d0bcc80666af9e95213a864ace98fee3570f))
* **fmt:** emit a trailing chain lambda bare, without wrapping parens ([eecf197](https://github.com/arthurmaciel/ipe-lang/commit/eecf197c4047448ffac350ab51a919d4bea642b4))
* **fmt:** FAJoinFirst hugs first call arg only when a block arg is present ([ae118a3](https://github.com/arthurmaciel/ipe-lang/commit/ae118a3dd8b74d7710c7886d5d67271b67aef00e))
* **fmt:** indent record-update fields one level past the brace ([ebed6f8](https://github.com/arthurmaciel/ipe-lang/commit/ebed6f8e3681f1e803d90080506927197e617a6c))
* **fmt:** keep multiline-string call args inline; break modal lambda bodies ([d1830dc](https://github.com/arthurmaciel/ipe-lang/commit/d1830dc91a047e8963ae9aae231f06ac31e26469))
* **fmt:** reflow shake_ffi_by_fn_ident signature — clears pre-existing rustfmt drift (workspace now fmt-clean) ([0b92cb0](https://github.com/arthurmaciel/ipe-lang/commit/0b92cb0df89f2fd9e8567ac04479dcfff0dc7145))
* **fmt:** two blank lines between a pre-header comment block and the module header ([1d31f00](https://github.com/arthurmaciel/ipe-lang/commit/1d31f004b4f6e3a913450c2667bf66f05465490e))
* **gate:** 13 rename-stale test + 2 real bugs surfaced by full-workspace run ([01e6fed](https://github.com/arthurmaciel/ipe-lang/commit/01e6feda1f67432c3e77acbd0bd47318994ff7b1))
* **gate:** clippy --all-targets clean (pedantic + nursery, clippy 1.92) ([53e3740](https://github.com/arthurmaciel/ipe-lang/commit/53e3740787bef25cce042e8b5d3f768bbd61cc21))
* **gate:** recover corrupted stdlib .ipe + reserved-namespace + compiled-source fixes ([6343643](https://github.com/arthurmaciel/ipe-lang/commit/634364358cd59bf905e3cbb73d39e468fa6cd656))
* **gate:** rename stragglers surfaced by full-workspace compile ([e11ced3](https://github.com/arthurmaciel/ipe-lang/commit/e11ced3c1e52d87f0a7f9587025b3f5d6447d54b))
* **gate:** resolve 6 workspace test failures — 4 stale rustfmt snapshots, 1 feature-gated dispatch test, 1 Db.Decode registry drift ([c8041ea](https://github.com/arthurmaciel/ipe-lang/commit/c8041eab1751550dac61274fbb3c412be0267d0a))
* **install:** allow backslash in INSTALL_DIR so Windows paths (D:\...) install ([#81](https://github.com/arthurmaciel/ipe-lang/issues/81)) ([1e74d8b](https://github.com/arthurmaciel/ipe-lang/commit/1e74d8bf90db22de9b04acafcd09468050a0ddbd))
* **ipe_backend:** [#233](https://github.com/arthurmaciel/ipe-lang/issues/233) Stream.stream re-wrap moves captured non-Copy strings (2x E0507) ([bcdfb03](https://github.com/arthurmaciel/ipe-lang/commit/bcdfb03ec53b2c7e4b9174978e8dc80e1a17ffc1))
* **ipe_canon:** register Sub.subscribeWebSocket in QUALIFIERS (anti-drift gap from [#210](https://github.com/arthurmaciel/ipe-lang/issues/210) WebSocket) ([6aaf010](https://github.com/arthurmaciel/ipe-lang/commit/6aaf010833ff96e13f69c0bc6d31573567853b5c))
* **ipe_lower,backend:** [#228](https://github.com/arthurmaciel/ipe-lang/issues/228) type-directed onSubmit handler classification ([3d5c1b9](https://github.com/arthurmaciel/ipe-lang/commit/3d5c1b9c9fb38430cf591a7a78fa4b00e2660fcf))
* **ipe_lower:** fold Ipe.Csv `{header,rows}` record to nominal CsvDoc ([#232](https://github.com/arthurmaciel/ipe-lang/issues/232)) ([e320dd9](https://github.com/arthurmaciel/ipe-lang/commit/e320dd939a69e16dcc95e744978c95a42ae98e38))
* **ipe:** thread on_form on two Expr::Call sites in cache.rs test IR ([889ef36](https://github.com/arthurmaciel/ipe-lang/commit/889ef36a53be23f29d814006b3c458c73d505f29))
* **ir:** bound the IR pretty-printer's recursion depth ([#282](https://github.com/arthurmaciel/ipe-lang/issues/282)) ([bad60bf](https://github.com/arthurmaciel/ipe-lang/commit/bad60bff3a08f2d77e95591efdb11b24a171dae7))
* **jwt:** seal the JWT Algorithm descriptor in Ipe.Secret ([#276](https://github.com/arthurmaciel/ipe-lang/issues/276)) ([5a609ae](https://github.com/arthurmaciel/ipe-lang/commit/5a609ae87afa70b8d752548ae70c28516a17ec22))
* **kernels:** complete required_runtime_module SSOT for PubSub kernels ([1164814](https://github.com/arthurmaciel/ipe-lang/commit/1164814474a122bbc8ae68baba5907558fa94499))
* **lsp:** don't drop the prior project layout on a transient load failure ([#278](https://github.com/arthurmaciel/ipe-lang/issues/278)) ([d7bed12](https://github.com/arthurmaciel/ipe-lang/commit/d7bed12b7ffacfcd14bb00c0a5434aab70e68d95))
* **mirror-parity:** D1 bare Css keyword constants + D2 record-alias-ctor coexistence; advance D3-18 row-poly, file rest ([a7df836](https://github.com/arthurmaciel/ipe-lang/commit/a7df8362dd7272005ce9b96fda5f7eabb707dbb7))
* **money:** kernel-wire Ipe.Money — route currency table / format / FX / allocate through guarded Money_* kernels ([8d45b03](https://github.com/arthurmaciel/ipe-lang/commit/8d45b03cb6208bb97c038ca929aa46e6bbd94c32))
* **parse:** reject space-before-dot instead of misparsing as field access ([9eec146](https://github.com/arthurmaciel/ipe-lang/commit/9eec1467c210e0c7b43471842c78a519ba327a0a))
* **playground:** correct IPE_RUNTIME_DIR path in README + resolver error to src/runtime/rust/src ([5bc57de](https://github.com/arthurmaciel/ipe-lang/commit/5bc57de8e70f58554711bd743a44b4bb22d6a3df))
* **project:** module discovery filtered .sky not .ipe (post-rename regression) ([5678e22](https://github.com/arthurmaciel/ipe-lang/commit/5678e2215522b1d06219719f25070a6a40315ef3))
* **rename:** normalize skyshop config to ipe.toml + fix stray ipe.toml/out in README usage ([#212](https://github.com/arthurmaciel/ipe-lang/issues/212)) ([5488229](https://github.com/arthurmaciel/ipe-lang/commit/54882299554c9ffc7d7f72a26db5787d60a70115))
* **rename:** update base64 expected constant for renamed 'Hello, Ipe!' plaintext ([#212](https://github.com/arthurmaciel/ipe-lang/issues/212)) ([2ffb474](https://github.com/arthurmaciel/ipe-lang/commit/2ffb474eba1736add14bde2abf56598693fa7fb9))
* **rename:** update string_reverse expected constant for renamed 'ipewasm' ([#212](https://github.com/arthurmaciel/ipe-lang/issues/212)) ([aa3c89c](https://github.com/arthurmaciel/ipe-lang/commit/aa3c89c922593d3001bbe506868cf53e7b89dba7))
* **runtime:** deliver outstanding init Cmd.perform effects before EOF terminates cli_program ([#379](https://github.com/arthurmaciel/ipe-lang/issues/379)) ([#85](https://github.com/arthurmaciel/ipe-lang/issues/85)) ([b07b42e](https://github.com/arthurmaciel/ipe-lang/commit/b07b42e6315a745285c33d87654889859af04ffa))
* **runtime:** enforce WS per-message size cap at the framing layer ([#274](https://github.com/arthurmaciel/ipe-lang/issues/274)) ([7a19539](https://github.com/arthurmaciel/ipe-lang/commit/7a1953965172ead483c737562d9c52f5c7e9817d))
* **runtime:** reap abandoned Server.Stream.stream handlers on a TTL ([#273](https://github.com/arthurmaciel/ipe-lang/issues/273)) ([4e730e9](https://github.com/arthurmaciel/ipe-lang/commit/4e730e99d54532d6b01b29f2888a8a1d7978d288))
* **runtime:** refuse to push the ingest token over cleartext HTTP ([#275](https://github.com/arthurmaciel/ipe-lang/issues/275)) ([e3338ec](https://github.com/arthurmaciel/ipe-lang/commit/e3338ec3b7aaced6c72aa0d26d577469096e1972))
* **runtime:** ssrf sibling refs crate::ssrf -&gt; super::ssrf (SEAL: emitted build) ([abb4135](https://github.com/arthurmaciel/ipe-lang/commit/abb41357e5cde0d6e7ff5516ebd6b7e889755c82))
* **runtime:** stop byte-slicing caller-derived JWT descriptor in error messages ([4a98578](https://github.com/arthurmaciel/ipe-lang/commit/4a98578e046d632760581c3bf7a64d53ac63fdb2))
* **sandbox:** skip run-jail e2e when the environment cannot establish a jail, not only when bwrap is absent ([#380](https://github.com/arthurmaciel/ipe-lang/issues/380)) ([#88](https://github.com/arthurmaciel/ipe-lang/issues/88)) ([43b2f7f](https://github.com/arthurmaciel/ipe-lang/commit/43b2f7fe2bf58c3f7af2f7dcb09736bf7cfc94ec))
* **seal-006:** route Basics.toString stringify family through IpeStringify ([9d569cf](https://github.com/arthurmaciel/ipe-lang/commit/9d569cfdacc369ab740edb7f798b9b35eab04ae4))
* **stdlib:** [#261](https://github.com/arthurmaciel/ipe-lang/issues/261) Money.add/sub/sumOf → Result Error Money (currency-mismatch now typed Err) ([969fb19](https://github.com/arthurmaciel/ipe-lang/commit/969fb19d53baabec3cc0183f2856ef415fb7af2e))
* **stdlib:** [#324](https://github.com/arthurmaciel/ipe-lang/issues/324) identify + fix 4 00-standard-libs run-time failures ([#22](https://github.com/arthurmaciel/ipe-lang/issues/22)) ([7dcbf21](https://github.com/arthurmaciel/ipe-lang/commit/7dcbf2160bde4fcdf491012a426dbe3c06d63e05))
* **sweep:** _shape_match strips {- -} block comments, not just -- lines ([0693b9f](https://github.com/arthurmaciel/ipe-lang/commit/0693b9fe52a66b6927d5e177229a7cc6be10ae6c))
* **sweep,ci:** mirror fetches upstream FIRST (local only as offline fallback); ci golden E2E compares against latest installed Sky, retire the cached expected_go oracle ([680edd1](https://github.com/arthurmaciel/ipe-lang/commit/680edd11bad095d109748fdfa67e51530da98c44))
* **sweep:** example_shape classifier -&gt; Ipe.* namespace (Live/Tui/Webview/Http) ([6099d34](https://github.com/arthurmaciel/ipe-lang/commit/6099d347a1b85b0c287aafe19d179234c09019e6))
* **sweep:** FFI-install examples SKIP, not false-RED (13-skyshop) ([ab71e37](https://github.com/arthurmaciel/ipe-lang/commit/ab71e37015f9c9b85a7d7d92e3ccf1201aca1f1c))
* **sweep:** mirror renames sky.toml -&gt; ipe.toml (Ipê's canonical manifest) ([fad2316](https://github.com/arthurmaciel/ipe-lang/commit/fad23166a9dcfc24366abd76f7057419819ca2da))
* **T2:** close SEAL-breach class — exhaustiveness over Prelude builtin ADTs, crate::-qualified top-level calls, live mod-ident gate ([aabbe0d](https://github.com/arthurmaciel/ipe-lang/commit/aabbe0d68974318ba4edbfd0a10c468ac911090e))
* **t3:** bound untrusted recursion/allocation — closes CO-FRONT-001, RT-UI-001, RT-TUI-001, RT-TUI-002 ([17151fe](https://github.com/arthurmaciel/ipe-lang/commit/17151feffa1e9c09e65560a6955df32a9a7c4d51))
* **t4:** JWT-exp NumericDate + Money allocate correctness (CO-INCR-001/002/003, RT-AUTH-001/002/003) ([4d8fc7c](https://github.com/arthurmaciel/ipe-lang/commit/4d8fc7c1f4484733d1c30a95be3aee3823be325f))
* **T5:** data/decode completeness + incremental wiring + SEAL (6 findings) ([8a4ef82](https://github.com/arthurmaciel/ipe-lang/commit/8a4ef82bb2f48ab111223bd815bb129745822465))
* **tests:** repair pre-existing base failures — env_public Module field + kernel-resolution allowlist ([bf3ca58](https://github.com/arthurmaciel/ipe-lang/commit/bf3ca58722998b99b96a5be3b74d710a042161f4))
* **wasm:** M1 gate WebSocket Sub-tier substitute — onOpen/onMessage/onClose/onError live in a browser ([#286](https://github.com/arthurmaciel/ipe-lang/issues/286)) ([bc57e10](https://github.com/arthurmaciel/ipe-lang/commit/bc57e10930cd2f892fbdd8b3bda31d23d685b8e5))
* **watch:** retry the rebuild cycle after a transient resolve failure ([#279](https://github.com/arthurmaciel/ipe-lang/issues/279)) ([3dc1000](https://github.com/arthurmaciel/ipe-lang/commit/3dc100051796b0c3a3a6824ac4fbfc7f05f49b2b))
* **watch:** scope the tests/ watch rule to the root-level directory only ([#280](https://github.com/arthurmaciel/ipe-lang/issues/280)) ([554c90d](https://github.com/arthurmaciel/ipe-lang/commit/554c90d7ca090d26dd243f03cec315c7f373f9d1))

## [0.1.14](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.13...ipe-v0.1.14) (2026-07-22)


### Bug Fixes

* **ci:** install jail primitives for static e2e + pin goldens to LF ([#86](https://github.com/arthurmaciel/ipe-lang/issues/86)) ([dbdd2a0](https://github.com/arthurmaciel/ipe-lang/commit/dbdd2a0bdf477b27f10e0bf6caed149a0f9ada1b))
* **sandbox:** skip run-jail e2e when the environment cannot establish a jail, not only when bwrap is absent ([#380](https://github.com/arthurmaciel/ipe-lang/issues/380)) ([#88](https://github.com/arthurmaciel/ipe-lang/issues/88)) ([43b2f7f](https://github.com/arthurmaciel/ipe-lang/commit/43b2f7fe2bf58c3f7af2f7dcb09736bf7cfc94ec))

## [0.1.13](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.12...ipe-v0.1.13) (2026-07-22)


### Features

* **examples:** bring composite examples 36-38 into sweep scope ([#377](https://github.com/arthurmaciel/ipe-lang/issues/377)) ([#83](https://github.com/arthurmaciel/ipe-lang/issues/83)) ([6f6193a](https://github.com/arthurmaciel/ipe-lang/commit/6f6193a9af83587b69b41fba2d874d286bb25e40))
* **security:** [#371](https://github.com/arthurmaciel/ipe-lang/issues/371) runtime capability sandbox + admit-and-isolate Tier 2 wrappers ([#82](https://github.com/arthurmaciel/ipe-lang/issues/82)) ([04214e5](https://github.com/arthurmaciel/ipe-lang/commit/04214e5ab0868dace57586c4c254fc7f8f746ee7))


### Bug Fixes

* **runtime:** deliver outstanding init Cmd.perform effects before EOF terminates cli_program ([#379](https://github.com/arthurmaciel/ipe-lang/issues/379)) ([#85](https://github.com/arthurmaciel/ipe-lang/issues/85)) ([b07b42e](https://github.com/arthurmaciel/ipe-lang/commit/b07b42e6315a745285c33d87654889859af04ffa))

## [0.1.12](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.11...ipe-v0.1.12) (2026-07-22)


### Features

* **cli:** CLI-UI single-source-of-truth (style module) + installer polish + SSOT principle ([#75](https://github.com/arthurmaciel/ipe-lang/issues/75)) ([720e2c6](https://github.com/arthurmaciel/ipe-lang/commit/720e2c6b1304d10084c7baa3eb690e70845e5b16))
* **cli:** human-first output model — --plain/--json, gutter, error-shows-help, Package authoring section ([#78](https://github.com/arthurmaciel/ipe-lang/issues/78)) ([ea01f7f](https://github.com/arthurmaciel/ipe-lang/commit/ea01f7f3530646557c3916149d47c720828ec2e3))
* **examples:** port go-ffi examples to Ipê + Rust crates (7 examples) ([#80](https://github.com/arthurmaciel/ipe-lang/issues/80)) ([c1f3bf7](https://github.com/arthurmaciel/ipe-lang/commit/c1f3bf782b2f78274d6f5db3297cb5086d196d15))


### Bug Fixes

* **install:** allow backslash in INSTALL_DIR so Windows paths (D:\...) install ([#81](https://github.com/arthurmaciel/ipe-lang/issues/81)) ([1e74d8b](https://github.com/arthurmaciel/ipe-lang/commit/1e74d8bf90db22de9b04acafcd09468050a0ddbd))

## [0.1.11](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.10...ipe-v0.1.11) (2026-07-21)


### Features

* **ffi:** [#317](https://github.com/arthurmaciel/ipe-lang/issues/317)+[#326](https://github.com/arthurmaciel/ipe-lang/issues/326) auto-binding coverage — bundle-generics, dyn-Fn systems, multi-result tuples ([#72](https://github.com/arthurmaciel/ipe-lang/issues/72)) ([572ddbd](https://github.com/arthurmaciel/ipe-lang/commit/572ddbdd1ada659dd04e4920b619786b8dbf781a))
* **ffi:** [#365](https://github.com/arthurmaciel/ipe-lang/issues/365) Tier 2 capability inference + fail-closed enforcement ([#71](https://github.com/arthurmaciel/ipe-lang/issues/71)) ([965aa15](https://github.com/arthurmaciel/ipe-lang/commit/965aa158a2616ad47c7e3441ad3ee9f83b989267))
* **ffi:** [#366](https://github.com/arthurmaciel/ipe-lang/issues/366) Tier 2 #[ipe::provide] trait-impl escape hatch ([#69](https://github.com/arthurmaciel/ipe-lang/issues/69)) ([2cc5114](https://github.com/arthurmaciel/ipe-lang/commit/2cc5114470177a84a6c7b67d4ffa96b90c5222db))
* **ffi:** [#369](https://github.com/arthurmaciel/ipe-lang/issues/369) closure-&gt;run handoff — drive foreign loops with Ipê closures ([#70](https://github.com/arthurmaciel/ipe-lang/issues/70)) ([8c4b662](https://github.com/arthurmaciel/ipe-lang/commit/8c4b6627baa46a7aeb1df4ebb462fd76599b612e))
* **pkg:** [#368](https://github.com/arthurmaciel/ipe-lang/issues/368) SP4 Tier-1 package gate + ipe package audit ([#66](https://github.com/arthurmaciel/ipe-lang/issues/66)) ([5838ff5](https://github.com/arthurmaciel/ipe-lang/commit/5838ff5de03157dd07bad5a3bc3f846e0fb8e1b9))

## [0.1.10](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.9...ipe-v0.1.10) (2026-07-21)


### Features

* **ffi:** [#364](https://github.com/arthurmaciel/ipe-lang/issues/364) Tier 2 phases 1-3 — bind author-supplied Rust wrapper crates ([#62](https://github.com/arthurmaciel/ipe-lang/issues/62)) ([28618f7](https://github.com/arthurmaciel/ipe-lang/commit/28618f7a7536b4c293ae6ddc65e02e2092e1114c))

## [0.1.9](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.8...ipe-v0.1.9) (2026-07-21)


### Features

* **security:** [#359](https://github.com/arthurmaciel/ipe-lang/issues/359) drive the abrupt-failure ledger toward zero ([#58](https://github.com/arthurmaciel/ipe-lang/issues/58)) ([ba4c309](https://github.com/arthurmaciel/ipe-lang/commit/ba4c3091ec33785b975d610d00a7d29cff7b5442))


### Bug Fixes

* **ffi:** [#363](https://github.com/arthurmaciel/ipe-lang/issues/363) refuse recursive provide types at decode (SEAL) ([#61](https://github.com/arthurmaciel/ipe-lang/issues/61)) ([51bc01d](https://github.com/arthurmaciel/ipe-lang/commit/51bc01d0e6895a375f4b638e00beee2ba8438710))

## [0.1.8](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.7...ipe-v0.1.8) (2026-07-21)


### Features

* **ffi:** [#354](https://github.com/arthurmaciel/ipe-lang/issues/354) opaque struct fields / enum payloads ([#57](https://github.com/arthurmaciel/ipe-lang/issues/57)) ([41af8c7](https://github.com/arthurmaciel/ipe-lang/commit/41af8c77b55e55422ab08c76a8db91ae84595c0a))
* **ffi:** async-returning provide.closure ([#55](https://github.com/arthurmaciel/ipe-lang/issues/55)) ([8bce55b](https://github.com/arthurmaciel/ipe-lang/commit/8bce55badf5c678df9e4066fa0a78e0c75dc28b2))
* **security:** token-scanner gate + clippy hardening for authored abrupt-failure ([#54](https://github.com/arthurmaciel/ipe-lang/issues/54)) ([843a17b](https://github.com/arthurmaciel/ipe-lang/commit/843a17baedb626ed4ec90bec2ad02e14fb61a67b))

## [0.1.7](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.6...ipe-v0.1.7) (2026-07-21)


### Features

* **cli:** aligned --help column + consent-based installer PATH setup ([#50](https://github.com/arthurmaciel/ipe-lang/issues/50)) ([fb98598](https://github.com/arthurmaciel/ipe-lang/commit/fb98598988dc6fd877ea5fc9e329c22458c460af))


### Bug Fixes

* **ci:** install wry/tao Linux link deps in e2e job to clear SEAL breach ([#51](https://github.com/arthurmaciel/ipe-lang/issues/51)) ([4378a36](https://github.com/arthurmaciel/ipe-lang/commit/4378a36a54940fc223eb2200fa3a8854f3a071c3))

## [0.1.6](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.5...ipe-v0.1.6) (2026-07-21)


### Features

* **ffi:** [#353](https://github.com/arthurmaciel/ipe-lang/issues/353) provide.closure opaque returns ([#47](https://github.com/arthurmaciel/ipe-lang/issues/47)) ([bc86028](https://github.com/arthurmaciel/ipe-lang/commit/bc86028122e36aee235f36877a1f46191c07d397))
* **stdlib:** [#342](https://github.com/arthurmaciel/ipe-lang/issues/342) Task + decoder combinators ([#49](https://github.com/arthurmaciel/ipe-lang/issues/49)) ([1fc9149](https://github.com/arthurmaciel/ipe-lang/commit/1fc91496758dfa4ac63dd02f2018243b111264b3))

## [0.1.5](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.4...ipe-v0.1.5) (2026-07-21)


### Features

* **ffi:** [#352](https://github.com/arthurmaciel/ipe-lang/issues/352) provide.* Ipê-side forwarder plumbing ([#46](https://github.com/arthurmaciel/ipe-lang/issues/46)) ([cc9718b](https://github.com/arthurmaciel/ipe-lang/commit/cc9718b34d7e26a562797466d0b9fc2bcd07653b))
* **stdlib:** Cmd.map / Sub.map ([#44](https://github.com/arthurmaciel/ipe-lang/issues/44)) ([eca501b](https://github.com/arthurmaciel/ipe-lang/commit/eca501b9265720c1970b290ee67df220e935fbdf))

## [0.1.4](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.3...ipe-v0.1.4) (2026-07-21)


### Features

* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) port IpeStringify format! emitters to native Doc rendering ([#35](https://github.com/arthurmaciel/ipe-lang/issues/35)) ([ce4a766](https://github.com/arthurmaciel/ipe-lang/commit/ce4a7660796ca3e6a48257e5116f17ab2f57f7ff))
* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) wire native Doc emitter into production emit_func ([#40](https://github.com/arthurmaciel/ipe-lang/issues/40)) ([f20e430](https://github.com/arthurmaciel/ipe-lang/commit/f20e430fec9a0fb20a3c40ca6d807bc27c01de23))
* **ffi:** [#347](https://github.com/arthurmaciel/ipe-lang/issues/347) sync closure adapter ([rust.provide.closure]) ([#36](https://github.com/arthurmaciel/ipe-lang/issues/36)) ([72cfd04](https://github.com/arthurmaciel/ipe-lang/commit/72cfd041b1c87a29e3415f4590fdf638b507e605))
* **ffi:** [#350](https://github.com/arthurmaciel/ipe-lang/issues/350) closure-manifest glue + [#348](https://github.com/arthurmaciel/ipe-lang/issues/348) struct-with-trait-impl ([#38](https://github.com/arthurmaciel/ipe-lang/issues/38)) ([ade2673](https://github.com/arthurmaciel/ipe-lang/commit/ade2673adffb0c3d118384f57e06b5b5bc9b06a1))
* **ffi:** provide.enum (P4) + Debug derive — Iced binding spike ([#42](https://github.com/arthurmaciel/ipe-lang/issues/42)) ([eef982a](https://github.com/arthurmaciel/ipe-lang/commit/eef982a8e8d6a499c4d9ce38f19200b25d2c43b4))
* **stdlib:** [#339](https://github.com/arthurmaciel/ipe-lang/issues/339) pure elm/core fills (List/Dict/Set/Result/Char/String) ([#43](https://github.com/arthurmaciel/ipe-lang/issues/43)) ([37da719](https://github.com/arthurmaciel/ipe-lang/commit/37da7192da96bf9ca651bc594ded7a4482402145))

## [0.1.3](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.2...ipe-v0.1.3) (2026-07-21)


### Features

* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) native emitter sweep to 0 divergences (cutover gated by non-body content) ([#32](https://github.com/arthurmaciel/ipe-lang/issues/32)) ([5581976](https://github.com/arthurmaciel/ipe-lang/commit/558197635e215d68beff405b67aacc7cd3e31760))
* **cli:** declutter ipe help (soft yellow, no optional-arg overview, bug-report footer) ([#15](https://github.com/arthurmaciel/ipe-lang/issues/15)) ([2f1b6dd](https://github.com/arthurmaciel/ipe-lang/commit/2f1b6dd85434decc6b20518cad82f4b7673c619b))
* **cli:** typed arg parsing — invalid optional-flag combinations unrepresentable + exhaustive tests ([#34](https://github.com/arthurmaciel/ipe-lang/issues/34)) ([783e922](https://github.com/arthurmaciel/ipe-lang/commit/783e92212020b6d6c2bde9dd1c11564be9790af5))
* **install:** fix curl (23), add spinner/percent/ETA + friendly branded messages ([#28](https://github.com/arthurmaciel/ipe-lang/issues/28)) ([b27654d](https://github.com/arthurmaciel/ipe-lang/commit/b27654d0ed7f3bdb72bec2772c23db14245f13ab))


### Bug Fixes

* **cli:** Usage error strings match the redesigned help ([#23](https://github.com/arthurmaciel/ipe-lang/issues/23)) ([c0872ee](https://github.com/arthurmaciel/ipe-lang/commit/c0872ee57b08eae365f4ae14be2fc5a96d7718db))
* examples-sweep 26/29/31 build+run green ([#33](https://github.com/arthurmaciel/ipe-lang/issues/33)) ([26c0c11](https://github.com/arthurmaciel/ipe-lang/commit/26c0c1167d66e5a266307e106783bc45d6ff5fc8))
* **ffi:** [#326](https://github.com/arthurmaciel/ipe-lang/issues/326) admit coercible multi-result tuples for non-borrow-reader methods ([#21](https://github.com/arthurmaciel/ipe-lang/issues/21)) ([f37fe03](https://github.com/arthurmaciel/ipe-lang/commit/f37fe03f8e775b68b5ee5ddfb8dac3c3e167f52c))
* **fmt:** [#338](https://github.com/arthurmaciel/ipe-lang/issues/338) parenthesise negative literals in atom position ([#24](https://github.com/arthurmaciel/ipe-lang/issues/24)) ([3156db5](https://github.com/arthurmaciel/ipe-lang/commit/3156db56ce741dec9c95ec749bc4fdbe101a61df))
* **stdlib:** [#324](https://github.com/arthurmaciel/ipe-lang/issues/324) identify + fix 4 00-standard-libs run-time failures ([#22](https://github.com/arthurmaciel/ipe-lang/issues/22)) ([7dcbf21](https://github.com/arthurmaciel/ipe-lang/commit/7dcbf2160bde4fcdf491012a426dbe3c06d63e05))

## [0.1.2](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.1...ipe-v0.1.2) (2026-07-21)


### Features

* [#337](https://github.com/arthurmaciel/ipe-lang/issues/337) row-polymorphic record annotations { r | f : T } ([#14](https://github.com/arthurmaciel/ipe-lang/issues/14)) ([d0c2514](https://github.com/arthurmaciel/ipe-lang/commit/d0c2514f1aa80ce22161a7fbcdabe3c8ca77eb78))
* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) recursive-Shape combine + chain glue — sweep 9→5 ([#16](https://github.com/arthurmaciel/ipe-lang/issues/16)) ([29ff276](https://github.com/arthurmaciel/ipe-lang/commit/29ff2761462b48145a9794d875287bff9d7c894a))


### Bug Fixes

* **explain:** github issue links + trailing newline ([#18](https://github.com/arthurmaciel/ipe-lang/issues/18)) ([f8fbc6f](https://github.com/arthurmaciel/ipe-lang/commit/f8fbc6fa78359c0461a9739d0a5b41243a739a02))

## [0.1.1](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.0...ipe-v0.1.1) (2026-07-21)


### Features

* [#337](https://github.com/arthurmaciel/ipe-lang/issues/337) row polymorphism + first-class accessors ([#10](https://github.com/arthurmaciel/ipe-lang/issues/10)) ([90af7a5](https://github.com/arthurmaciel/ipe-lang/commit/90af7a5f16bf10028dd4220aa18a1f212504b38d))
* **#210:** seal Ipe.Email — Email.send + EmailMessage/EmailProvider fold ([e1486e4](https://github.com/arthurmaciel/ipe-lang/commit/e1486e4768ad2737b30f53d9d953dcb15d9763d0))
* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) call-arg combining render primitive + fn_call_width ([#12](https://github.com/arthurmaciel/ipe-lang/issues/12)) ([becfd89](https://github.com/arthurmaciel/ipe-lang/commit/becfd891a6626476e5d4fc2a531da7a3e9636383))
* **backend:** [#315](https://github.com/arthurmaciel/ipe-lang/issues/315) leaf-arm + statement emitters toward cutover ([#9](https://github.com/arthurmaciel/ipe-lang/issues/9)) ([a5bb75e](https://github.com/arthurmaciel/ipe-lang/commit/a5bb75ee22d7ddf5b697b8574a6773f324c295e5))
* **backend:** assignment-RHS-break Doc token for the let-value layout axis ([3d07e66](https://github.com/arthurmaciel/ipe-lang/commit/3d07e66caafd881205f0a486f5017f856db94b69))
* **backend:** native Doc emitter for Expr::Match via MatchArmTail ([0e4b182](https://github.com/arthurmaciel/ipe-lang/commit/0e4b182317cf742dfb1e520d15c16e029f863de5))
* **backend:** native Doc emitters for Lambda/SharedLambda + immediately-applied Apply ([740ba27](https://github.com/arthurmaciel/ipe-lang/commit/740ba2772ef25e38c34b52fcb27f11dd552f2682))
* **backend:** native Rust formatter Doc IR + renderer (P0) ([a675f3f](https://github.com/arthurmaciel/ipe-lang/commit/a675f3f484ab70a6c106dd5a527b7c5eb50aa037))
* **backend:** P1 Doc-building emit path — binop-chain builder + SEAL property test ([f7dbaeb](https://github.com/arthurmaciel/ipe-lang/commit/f7dbaeba919bd41b3155aa8a78047470a049107c))
* **backend:** real flat-vs-break Group in native renderer + structured if builder ([ff902e7](https://github.com/arthurmaciel/ipe-lang/commit/ff902e753ce2dc4d20b2f7d86a0c229c5943be2d))
* **backend:** SEAL-visible BraceBody Doc token for rustfmt brace add/strip ([e562ac9](https://github.com/arthurmaciel/ipe-lang/commit/e562ac9912db8a429bb9d86921b595bae24a9fa7))
* **backend:** structured Ctor Doc builder (payload + runtime-enum) ([aadb94c](https://github.com/arthurmaciel/ipe-lang/commit/aadb94c32ff32ead5790384ec0c5c31786c1f2b4))
* **backend:** structured delimited-list Doc builders + break-conditional trailing comma ([bcdc2c7](https://github.com/arthurmaciel/ipe-lang/commit/bcdc2c7f141b693c0c549f7ecfcde245c49dbb1a))
* **backend:** structured Destructure-block Doc builder ([c8c0e46](https://github.com/arthurmaciel/ipe-lang/commit/c8c0e46ff948a63bc0458b1e37a1ed095b2ca85b))
* **backend:** structured general-apply Doc builder ([059e4e5](https://github.com/arthurmaciel/ipe-lang/commit/059e4e52c16bd93aa5b83429c249c82b24013837))
* **backend:** structured generic call-tail Doc builder ([0fc9554](https://github.com/arthurmaciel/ipe-lang/commit/0fc9554c2a1a91f490e058f534eda6b1bb282d82))
* **backend:** structured let-block Doc builder ([7e7c274](https://github.com/arthurmaciel/ipe-lang/commit/7e7c274ee1ad8966bb7b729f9257b7f5eb8343b0))
* **backend:** structured record-literal Doc builder ([7cd66ca](https://github.com/arthurmaciel/ipe-lang/commit/7cd66cadfd5083030ab13dbfcf0c32ac6d65292f))
* **backend:** structured record-update Doc builder ([fb00c59](https://github.com/arthurmaciel/ipe-lang/commit/fb00c59105e47d565b5be4c2e14a7c026ee342db))
* **backend:** structured sync task-seq Doc builder ([87caa50](https://github.com/arthurmaciel/ipe-lang/commit/87caa50abafbd37590fc3cfafbe99588ac245c32))
* **ci:** examples-sweep bot-commits the refreshed upstream mirror ([5b71393](https://github.com/arthurmaciel/ipe-lang/commit/5b7139328c222d078ac0b0d23f13e4690e59bfd2))
* **ci:** live upstream-Sky parity comparison (retires the cached oracle) ([2404d2e](https://github.com/arthurmaciel/ipe-lang/commit/2404d2ec2de99ff9b985ef6b9353d203196e70cf))
* **cli:** add `ipe version` (also --version / -V) ([f504895](https://github.com/arthurmaciel/ipe-lang/commit/f5048959458057b210c9372c67a341b47464c064))
* **cli:** capabilities acceptance over examples + README ([333a3b4](https://github.com/arthurmaciel/ipe-lang/commit/333a3b42b98f27c66dbf07e2d5cec65e884983e7))
* **cli:** default entry for build/run/watch in project directories ([ebe88c7](https://github.com/arthurmaciel/ipe-lang/commit/ebe88c765479c391a4a467f4d76b12d1e3cc5735))
* **cli:** ipe capabilities report + declared-set verify ([7eb4877](https://github.com/arthurmaciel/ipe-lang/commit/7eb48774be0c008278510ebeb96aa80b2e1ce81c))
* **cli:** ipe fmt — elm-format-compatible source formatter ([8e3cbef](https://github.com/arthurmaciel/ipe-lang/commit/8e3cbefe6969d990dea0dcfa9a3c07d5547986fe))
* **cli:** ipe init scaffolds an Ipe.Live counter project ([60277f6](https://github.com/arthurmaciel/ipe-lang/commit/60277f64e91a8dade4ca8c898740a47e9b2c8ff0))
* **cli:** ipe run --static — shared static-flag parser + plan resolver across build/run; binary located via cargo metadata target_directory (honours CARGO_TARGET_DIR / user target-dir pins) ([5e25b11](https://github.com/arthurmaciel/ipe-lang/commit/5e25b11ddd4b452eed3a97446477b62912915448))
* **cli:** sectioned, coloured top-level help and per-command --help ([976dc92](https://github.com/arthurmaciel/ipe-lang/commit/976dc92030706e38005e358f6083fb5a2177ba41))
* **cli:** SP2 — ipe rust group + ipe.toml schema ([#4](https://github.com/arthurmaciel/ipe-lang/issues/4)) ([364b213](https://github.com/arthurmaciel/ipe-lang/commit/364b213fc5dd69b9a95d17e5db8147ddf0397d69))
* **cli:** SP3 — index resolver + lockfile + ipe add ([#8](https://github.com/arthurmaciel/ipe-lang/issues/8)) ([c358c2a](https://github.com/arthurmaciel/ipe-lang/commit/c358c2af27271ed2b766c0f888eb537fa8251bad))
* **cli:** SP5 — ipe diff + enforced semver ([#11](https://github.com/arthurmaciel/ipe-lang/issues/11)) ([038ad53](https://github.com/arthurmaciel/ipe-lang/commit/038ad534d0b65ab74f57ed80e1c0b030547e7b44))
* **emit:** post-emit rustfmt pass (fail-closed) so emitted Rust is rustfmt-clean; regenerate 73 goldens to canonical form ([3f624bc](https://github.com/arthurmaciel/ipe-lang/commit/3f624bc8a246e2eaf7a2ec541cb4cd050d8aaa7f))
* **examples:** 13-skyshop transpose in progress — Db+Auth de-shimmed onto real SDKs, 8-crate cache checked in ([41aeca9](https://github.com/arthurmaciel/ipe-lang/commit/41aeca91c1920133fda3b629e32eea29526c09e3))
* **examples:** add examples/sky/manifest.toml — Sky→Ipe patch registry ([#299](https://github.com/arthurmaciel/ipe-lang/issues/299)) ([60764be](https://github.com/arthurmaciel/ipe-lang/commit/60764bebbc9d183f054e492f54f46e089dfe1cf0))
* **examples:** track the upstream Sky example mirror (42 examples, source-only) ([ee24e09](https://github.com/arthurmaciel/ipe-lang/commit/ee24e0987162f83520eb1e7f823abda672d546f4))
* **ffi-inspector:** param-shape admission — conversion-bound nominal targets (identity + From&lt;String&gt; preference), enum-level non_exhaustive ctor restoration, Clone-enum field accessors ([0c7e9d2](https://github.com/arthurmaciel/ipe-lang/commit/0c7e9d28594b19b3ac95a9ed72eceded2b9d5dbd))
* **ffi-inspector:** resumable manifest inspection — stable probe root + cross-crate proof-map checkpoint ([989ee3b](https://github.com/arthurmaciel/ipe-lang/commit/989ee3bd35091840062b910f9929e0a3973b9a26))
* **ffi-inspector:** stripe-send — doc-hidden surfacing + cross-crate Send proof (3 of 4 walls) ([8a2f590](https://github.com/arthurmaciel/ipe-lang/commit/8a2f5902ffec29c2b8487934bd9ce842650083c2))
* **ffi-inspector:** stripe-send F2 — cross-crate proven-public Output paths (GLOBAL_XC_PUBLIC_PATHS) ([3146360](https://github.com/arthurmaciel/ipe-lang/commit/314636064522c0e887470a520963df966a1569d2))
* **ffi-inspector:** stripe-send F2 (cont.) — resolve cross-crate send Output in type_to_typeref ([a29b0d3](https://github.com/arthurmaciel/ipe-lang/commit/a29b0d30903717cd3b06f41962ba8a49d4a73c70))
* **ffi-inspector:** stripe-send W4 — return-nameability by defining-type identity (4th wall) ([1fefa83](https://github.com/arthurmaciel/ipe-lang/commit/1fefa83f99f6c129552704695019674120bb0982))
* **ffi:** async wrappers arm AbortOnDrop + route JoinError through ipe_error_from_foreign (Δ1/Δ2) ([cde8092](https://github.com/arthurmaciel/ipe-lang/commit/cde809246b7feeef8acfb5d3d53f28340bfda23f))
* **ffi:** async-SDK consumer path — closed-instance synthesis, alias fold, used-set forwarder DCE; firestore 0.49 bound shim-free E2E ([d5327f2](https://github.com/arthurmaciel/ipe-lang/commit/d5327f276d727792c2ecec4fc2f56e294bcfb4e5))
* **ffi:** borrow-thread &self/&mut self FFI readers through the result ([9b1f4ce](https://github.com/arthurmaciel/ipe-lang/commit/9b1f4ce6bd936b6fb4bb83b2998bcaf38d0f63c2))
* **ffi:** checked fallible setters for narrowing integer fields — try_from + typed Err replaces the setter drop (no silent truncation; f32/containers stay dropped) ([af035d3](https://github.com/arthurmaciel/ipe-lang/commit/af035d3ded91ab211fd177fc287189960363a4ef))
* **ffi:** firebase-bind shim-free — rs-firebase-admin-sdk 4.3 SEAL green (verify chain live) ([06f334b](https://github.com/arthurmaciel/ipe-lang/commit/06f334bd68899f582987da9501ccc458ee406756))
* **ffi:** foreign-type-one-home — defid-keyed nominal unification across the installed-crate catalog ([857eb62](https://github.com/arthurmaciel/ipe-lang/commit/857eb62acf482f1b3c1c38b8a0c8317f41d8fc4c))
* **ffi:** one-shot manifest install + submodule Ipe-head path map + prerelease pin pass-through ([46f4e23](https://github.com/arthurmaciel/ipe-lang/commit/46f4e23af417ed9c06d0382a97859f078311e2bb))
* **ffi:** pkg.json is the sole catalog source — load re-derives the full consumer view ([9154383](https://github.com/arthurmaciel/ipe-lang/commit/915438347af734a8bb9b278b6b9cb76eac08f6bb))
* **ffi:** stripe-send W4 verified end-to-end + multi-crate dep-line unification ([1a20c74](https://github.com/arthurmaciel/ipe-lang/commit/1a20c746b27966b6b453157d715c51864744791d))
* **ffi:** version-pinned crate specs + feature/pin propagation through ipe add/install ([0d9c433](https://github.com/arthurmaciel/ipe-lang/commit/0d9c4336626d1855fce90b513d10d2430f34df32))
* **ipe watch:** SIGTERM-to-shutdown forwarder, run()-only, + 3 proof tests ([a996933](https://github.com/arthurmaciel/ipe-lang/commit/a996933cd8d385589da9dcb20f1578aaf83094e9))
* **ipe_backend_rust:** emit the Model schema tag into Live entry calls (1B.5) ([8331b01](https://github.com/arthurmaciel/ipe-lang/commit/8331b011f545a8428ae048b0a89e88ae6f490a70))
* **ipe_backend_rust:** Model schema structural hash — records (Stage A, 1A.1-1A.2) ([ff7f4c3](https://github.com/arthurmaciel/ipe-lang/commit/ff7f4c3411fd1658672ce42637fb1f8809ce5030))
* **ipe_backend_rust:** schema hash — fuel bound + exhaustiveness (1A.4, Stage A complete) ([ecfca2c](https://github.com/arthurmaciel/ipe-lang/commit/ecfca2c71fba683580258f0a2ac94041e34adb92))
* **ipe_backend_rust:** schema hash enum arm — nominal identity + variant names at position (1A.3) ([a264826](https://github.com/arthurmaciel/ipe-lang/commit/a264826deed73402f9d15b4f3c8bbaf58a815e62))
* **ipe_ir:** carrier_is_clone — single carrier-Clone authority ([3649167](https://github.com/arthurmaciel/ipe-lang/commit/364916715a7d5bdbe11536bc8cc7884e13038371))
* **ipe_lower:** [#221](https://github.com/arthurmaciel/ipe-lang/issues/221) fn-value Arc-carrier promotion on the lowered IR (position-typed, replaces the canon pre-pass) ([cda33ca](https://github.com/arthurmaciel/ipe-lang/commit/cda33ca7954dfb7cc105289cd50150ff394a25e5))
* **ipe_lower:** per-let Arc-promotion look-ahead for depth-1 fn captures ([e113899](https://github.com/arthurmaciel/ipe-lang/commit/e1138991c970821d9ec7e51a4f0774256682609c))
* **ipe_watch:** safe SIGTERM listener module (signal.rs) ([9dbbc9a](https://github.com/arthurmaciel/ipe-lang/commit/9dbbc9a97020a5311637225687d59989f6762b55))
* **kernels:** add Database capability; reclassify Db-family kernels ([5ec4ccd](https://github.com/arthurmaciel/ipe-lang/commit/5ec4ccde5c3c897cf785b84411901b4885146b64))
* **kernels:** per-kernel capability tag + Capability vocabulary ([2fd7401](https://github.com/arthurmaciel/ipe-lang/commit/2fd74017f2b80c97af362cdbd31a6e118d05b20e))
* **lower:** whole-program capability inference ([5a48b73](https://github.com/arthurmaciel/ipe-lang/commit/5a48b734ffcc31015c2caffaf267252db9b6e6d7))
* **lsp,db:** per-module typecheck_module query SEAM + migrate home-keyed handlers ([570760e](https://github.com/arthurmaciel/ipe-lang/commit/570760e20c2568b3acecf783f5eb1723a40175ae))
* **lsp:** [#295](https://github.com/arthurmaciel/ipe-lang/issues/295) document formatting + rangeFormatting ([71f81b6](https://github.com/arthurmaciel/ipe-lang/commit/71f81b6b429e649f5f655d42b56e756b5f571a10))
* **lsp:** [#296](https://github.com/arthurmaciel/ipe-lang/issues/296) code actions — diagnostic-driven quick-fixes ([3177844](https://github.com/arthurmaciel/ipe-lang/commit/3177844f6a33861cd31a0a46ebbc1bd7dedc3a4f))
* **lsp:** [#297](https://github.com/arthurmaciel/ipe-lang/issues/297) semantic tokens full — 10-type legend over the parse AST ([7ef7aa2](https://github.com/arthurmaciel/ipe-lang/commit/7ef7aa24619d6599d558d0a51908044f0e3e0d07))
* **lsp:** [#298](https://github.com/arthurmaciel/ipe-lang/issues/298) signature help + inlay hints ([8e5b3dc](https://github.com/arthurmaciel/ipe-lang/commit/8e5b3dc68619b2696ef9c2175fdb794778b06566))
* **lsp:** completion, go-to-definition, find-references, rename ([926bb34](https://github.com/arthurmaciel/ipe-lang/commit/926bb34f3008e8f2a755243f3688f027432dba4b))
* **lsp:** document links + folding ranges ([62e9141](https://github.com/arthurmaciel/ipe-lang/commit/62e9141e07b49e496153bdec2addf3880098a7ea))
* **lsp:** ipe lsp server — live diagnostics, hover, document symbols over the salsa graph ([64b684e](https://github.com/arthurmaciel/ipe-lang/commit/64b684e7a4a8aa014a7023fe4786626ba494f0a6))
* **lsp:** type-directed completion via additive ExpectedTypes solver sidecar ([d1b475e](https://github.com/arthurmaciel/ipe-lang/commit/d1b475e27066286828d09a79501ef1588065a03a))
* **lsp:** wire [#295](https://github.com/arthurmaciel/ipe-lang/issues/295)-298 into server — capabilities + request handlers ([f0b5046](https://github.com/arthurmaciel/ipe-lang/commit/f0b5046600cbacbf97c65c8462be38fef74b8825))
* **runtime live:** checkpoint wire format -&gt; base64(tag ++ bincode) (Stage C 1C.2-1C.4) ([100994a](https://github.com/arthurmaciel/ipe-lang/commit/100994ad4a66ba77eaed9c237ab73d9af61b7c37))
* **runtime live:** Model schema-tag column gates session-checkpoint reuse (H24, Stage B 1B.1-1B.4) ([e19a723](https://github.com/arthurmaciel/ipe-lang/commit/e19a72321e2afb394ed014c92cc7d0b601bfbdce))
* **runtime live:** proactive event: reload SSE frame on dev shutdown (Problem 2) ([1f77e2c](https://github.com/arthurmaciel/ipe-lang/commit/1f77e2cf785614b567982141fdc993ed8b5ba58e))
* **runtime:** process-global tokio runtime for block_on + AbortOnDrop cancel guard (async-FFI bridge H1/Δ1 primitives) ([e901c2b](https://github.com/arthurmaciel/ipe-lang/commit/e901c2bf2a586ac6e3861231c2ad5e0d6d8cde57))
* **static:** [#244](https://github.com/arthurmaciel/ipe-lang/issues/244) add aarch64-unknown-linux-musl static target (config + CI) ([297927a](https://github.com/arthurmaciel/ipe-lang/commit/297927aef8385db05b66492b15a4c489f90a681b))
* **static:** pin rust-lld self-contained for aarch64-musl — portable static cross-build (no musl-cross-gcc) ([2935170](https://github.com/arthurmaciel/ipe-lang/commit/2935170bb92e1fca5f1917daa5b4a15c44031d38))
* **sweep:** fail loud on unpatched new upstream examples + self-regression docs ([#300](https://github.com/arthurmaciel/ipe-lang/issues/300)) ([f60bdb5](https://github.com/arthurmaciel/ipe-lang/commit/f60bdb50cc045b75664ba10d0b4ab520fc696ebe))
* **sweep:** IPE_SWEEP_STATIC=1 — per-example --static musl build (CWD = emitted crate dir), ldd-asserted static-ness, static-binary RUN, webview typed-refusal assertion ([d177666](https://github.com/arthurmaciel/ipe-lang/commit/d177666721ad4a7c6db89810986498cd5a7b0869))
* **sweep:** Ipê-only upstream-mirror sweep, retire the Go-oracle equivalence infra ([7773a7f](https://github.com/arthurmaciel/ipe-lang/commit/7773a7f9c12a6eed634dfe00251aebcb32933c95))
* **types,db:** per-module scoped typecheck behind typed interfaces ([6991b4c](https://github.com/arthurmaciel/ipe-lang/commit/6991b4c9f79e3b3987e641613020c2005db5f9c8))
* **wasm:** browser TEA sink + target-neutral dom re-home ([4d0a74c](https://github.com/arthurmaciel/ipe-lang/commit/4d0a74c8f28211e8f2f5bb7ba37662bb8fbbc3de))
* **wasm:** compile the Ipê frontend to WASM + browser-native playground ([40587ca](https://github.com/arthurmaciel/ipe-lang/commit/40587ca95b78ab4c6073950e0c092249fc931d3f))
* **wasm:** Ipe.Env.public kernel + build-time publicEnv embedding ([#287](https://github.com/arthurmaciel/ipe-lang/issues/287)) ([6729c68](https://github.com/arthurmaciel/ipe-lang/commit/6729c68081dc9fe9df8ec7231ad1d84e40d4c95a))
* **wasm:** M0 pure-kernel wasm floor — runtime builds to wasm32-unknown-unknown (default + json) as an enforced CI gate ([9a66a2d](https://github.com/arthurmaciel/ipe-lang/commit/9a66a2d2272e1afbca5654729832593c45c6eee6))
* **wasm:** M1 target-keyed kernel gate + M2 emission branch + M3 browser slice — ipe build --target wasm, Ipe.Ui proven in Chromium ([9523e03](https://github.com/arthurmaciel/ipe-lang/commit/9523e03d01b3f724cd56b07153626913ba23c75c))
* **wasm:** M4 Cmd/Sub browser-effects bridge — Log/Random/Http/WebSocket/Task substitutes, timers, in-tab pub/sub ([e95af5a](https://github.com/arthurmaciel/ipe-lang/commit/e95af5ad106fce8ae05eccf3f13e8e3e94429ca0))
* **wasm:** M5 Layer-2 module classification + reachability closure + [wasm] ipe.toml config (IPE-N0030) ([d668b89](https://github.com/arthurmaciel/ipe-lang/commit/d668b897d32cf9e576fedbbca3a56499e9d634b2))
* **wasm:** M6 Target A MVP — pure-client SPA end-to-end ([#240](https://github.com/arthurmaciel/ipe-lang/issues/240)) ([30533d1](https://github.com/arthurmaciel/ipe-lang/commit/30533d1de968f44ed406e121ddc4fea52a4a9d44))
* **wasm:** M7 SSR hydration — island serialiser, adopt path, hydrate export, field-type gate ([0b7f543](https://github.com/arthurmaciel/ipe-lang/commit/0b7f543108a5ffc7e8ef67b126555ef4c01f6012))
* **wasm:** M8 playground B1 — server-compile-then-ship-WASM backend ([0171f27](https://github.com/arthurmaciel/ipe-lang/commit/0171f27dfd023bab77dd56ff616d7e9ef0d1813a))


### Bug Fixes

* **#210:** register Ipe.Config family — Decoder carrier + 16 kernels SEAL ([627afe0](https://github.com/arthurmaciel/ipe-lang/commit/627afe0563141f68355390b4cc5b6ed4eb902cec))
* **#221 defect B:** home-attribute lowering + emit diagnostics to owning module ([1149870](https://github.com/arthurmaciel/ipe-lang/commit/1149870fdc0687ed5e79cf899f09a4ef8ad95327))
* **backend,lower:** SEAL 13-skyshop — cfg-record arg-order hoist, sync-capture param promotion trigger, single-boundary Arc callback ([4c2ac25](https://github.com/arthurmaciel/ipe-lang/commit/4c2ac252c345920f0bcd1ad8682be597926c7860))
* **backend/tests:** pass wasm_hydrate_mode to EmitCtx::build test call sites (WASM M7 arity debt) ([711c725](https://github.com/arthurmaciel/ipe-lang/commit/711c725f151f520a24fdb518908e9234a5e59e5a))
* **backend:** close the module-set SEAL breach class (tea/live/http_stream drift) ([d3d0bd8](https://github.com/arthurmaciel/ipe-lang/commit/d3d0bd806ad128175f728600f522affd312daffb))
* **backend:** emitter emits at most one consecutive blank line ([7e69cb8](https://github.com/arthurmaciel/ipe-lang/commit/7e69cb8dd81798a81601d82de6abcb1923efd803))
* **backend:** FFI shake keep-decision accumulates instead of overwriting ([#283](https://github.com/arthurmaciel/ipe-lang/issues/283)) ([1a08994](https://github.com/arthurmaciel/ipe-lang/commit/1a08994c12097be16306a81e9bb4067e2ce41432))
* **backend:** two emitter fallbacks fail closed instead of emitting invalid Rust ([#281](https://github.com/arthurmaciel/ipe-lang/issues/281)) ([13ae62d](https://github.com/arthurmaciel/ipe-lang/commit/13ae62df0ae42ed3c1003e3c1494046df882df71))
* **canon:** bound type-alias expansion with depth + node-count limits (IPE-N0032) ([2d973e6](https://github.com/arthurmaciel/ipe-lang/commit/2d973e6f6dae10ee6b6322e9736d91cc8d5a0cf9))
* **canon:** canon-arity-gate — reject mis-arity built-in containers (IPE-N0031) ([1089a76](https://github.com/arthurmaciel/ipe-lang/commit/1089a76ae6096bbdc7d0a109ca725ab1e32f0962))
* **canon:** qualified cross-module alias references expand without exposing ([3fe074f](https://github.com/arthurmaciel/ipe-lang/commit/3fe074f073d3df9cdf45a0035269ab741090138d))
* **ci:** clippy duration_suboptimal_units (from_secs-&gt;from_mins, toolchain drift: CI stable was ahead of local rustup) + golden_alias_move_seal stale substring assertions (rustfmt reflow, same class as [#269](https://github.com/arthurmaciel/ipe-lang/issues/269)) + .gitignore drop ../sky ref + point oracle-version comment at its SSOT (tools/oracle/README.md) ([737eb50](https://github.com/arthurmaciel/ipe-lang/commit/737eb5030bb1baed34f24431ea77bcf26ddf4669))
* **ci:** clippy duration_suboptimal_units in lsp_stdio_e2e.rs (from_secs(60)-&gt;from_mins(1)) ([a69d923](https://github.com/arthurmaciel/ipe-lang/commit/a69d9233eb97d28f9973586ab1e461a54d32d8a4))
* **ci:** clippy map_unwrap_or in ipe_watch scope.rs (map_or(0, |d| d.as_nanos())) ([3dea51c](https://github.com/arthurmaciel/ipe-lang/commit/3dea51c8da00fe8a7626588ce09af616620bf0b3))
* **ci:** e2e shards — set IPE_ORACLE_SHARED_TARGET, closing the disk-exhaustion class ([bf61ebb](https://github.com/arthurmaciel/ipe-lang/commit/bf61ebbd555fdba38e8b06657d9ff478738479f6))
* **ci:** nextest ci profile, 6 E2E shards, sccache v0.0.9, --no-fail-fast ([d9adfbe](https://github.com/arthurmaciel/ipe-lang/commit/d9adfbe3f3c8b82d681025ab4522ea66427fb07e))
* **ci:** sky-parity picks the compiler binary, not the FFI inspector ([8239648](https://github.com/arthurmaciel/ipe-lang/commit/82396481cd0ee74fa3c3784862bee6a76266a078))
* **ci:** stale golden-test substring assertions masked by nextest fail-fast ([#191](https://github.com/arthurmaciel/ipe-lang/issues/191), [#193](https://github.com/arthurmaciel/ipe-lang/issues/193), [#195](https://github.com/arthurmaciel/ipe-lang/issues/195), [#190](https://github.com/arthurmaciel/ipe-lang/issues/190), ws-onerror, Ipe.Ui.Animation/Transition) ([facd9a7](https://github.com/arthurmaciel/ipe-lang/commit/facd9a76e7d6920dc0e867588df61742e75ae3b2))
* **ci:** three more E2E failures the disk-exhaustion fix stopped masking (server-clone reflow, webview Linux link gap, watch cold-build headroom) ([193760c](https://github.com/arthurmaciel/ipe-lang/commit/193760ce78a2c4dab6f21912d57e3f855cf64fb5))
* **cli:** clear nightly-clippy debt in wasm bundle step and manifest parsing ([47d89c2](https://github.com/arthurmaciel/ipe-lang/commit/47d89c2f3682ef58f15f165c320c46bf96b4bc7c))
* **clippy+fmt:** ffi.rs map_or + doc-paragraph split; rustfmt resolve.rs/types-lib drift ([89280bf](https://github.com/arthurmaciel/ipe-lang/commit/89280bf30629a99b948792820cfc1d49abbd20d7))
* **clippy:** clear pre-existing --all-targets lint debt in LSP, backend, canon, types, playground ([78d1d6e](https://github.com/arthurmaciel/ipe-lang/commit/78d1d6e07cd8ad3fe66e4c87363f8c5f7c7248f4))
* **cli:** rewrite two test match blocks as let-else (clippy pedantic on --all-targets) ([#3](https://github.com/arthurmaciel/ipe-lang/issues/3)) ([d5a7b37](https://github.com/arthurmaciel/ipe-lang/commit/d5a7b370760e39a4ca3f98eefed386afcea46d99))
* **deps:** regenerate package-lock.json — was stale, missing pixelmatch/pngjs entirely (only had playwright) ([d5d6dcf](https://github.com/arthurmaciel/ipe-lang/commit/d5d6dcf0961c6e3f3afe792dca24e22139e74a55))
* **diagnostics:** drop duplicate unreachable BuiltinTypeArity render arm (merge artifact) ([803d751](https://github.com/arthurmaciel/ipe-lang/commit/803d7515ba588b5fdb73207e0fb912e5940ef56a))
* **docs:** correct stale warm-db-reuse doc — the parity gate already exists ([#277](https://github.com/arthurmaciel/ipe-lang/issues/277)) ([96ece49](https://github.com/arthurmaciel/ipe-lang/commit/96ece49a8517fbaae2131e785e13b7b76d9ffa00))
* **emit:** opaque-type special-cases keyed on old Std home -&gt; Ipe (Cache/Config/Email) ([4a3cb4c](https://github.com/arthurmaciel/ipe-lang/commit/4a3cb4cd2be14f960ea7f37b0e002e8db4dfb8dd))
* **example:** 41-money-allocate-regression — correct main sig (drop Never) + fromMajor takes Int not Decimal; T4's example never compiled as shipped ([375e339](https://github.com/arthurmaciel/ipe-lang/commit/375e339b1ff5b81748415cda0579ccc10f3b905f))
* **examples:** patch 00-standard-libs Money tests for Ipê's Result API ([bdfbca2](https://github.com/arthurmaciel/ipe-lang/commit/bdfbca2ea2a65b11664c51a2ce1c798d9239fc04))
* **examples:** restore native 01-hello-world; untrack sky-out build output ([bab7bc6](https://github.com/arthurmaciel/ipe-lang/commit/bab7bc6d65a94891237defdb9415dbc07ff4f1f4))
* **ffi-inspector:** private-path-admission — drop external trait UFCS qualifiers threading a private module ([b1de3de](https://github.com/arthurmaciel/ipe-lang/commit/b1de3de4569d02b46587afdbfd605dd95138743c))
* **ffi-inspector:** serde-trait identity by raw defining path — restores the firestore serde document surface ([b8afea3](https://github.com/arthurmaciel/ipe-lang/commit/b8afea35b35d6932e9f77d0ce2367e5418f58915))
* **ffi-sandbox:** T1 F2-F6 — two-phase no-egress jail, narrowed ~/.cargo binds, mandatory caps + concurrent drain, bwrap-or-refuse, bounded owned cache root, scratch under ~/.cache/ipe + install prompt ([adf54c3](https://github.com/arthurmaciel/ipe-lang/commit/adf54c328ff444ebe68f74cd5bcefca33c6da5a6))
* **ffi:** compositional OK-lift for generic wrappers + owned pass for bare-str substitutes ([5a04388](https://github.com/arthurmaciel/ipe-lang/commit/5a043886381beabe8edbddbddfdf728f59e207ff))
* **ffi:** fail-closed on reuse of a non-Clone FFI opaque handle (SEAL) ([3b0c86e](https://github.com/arthurmaciel/ipe-lang/commit/3b0c86ef51fd4286ad763cd80c5b6f089e8922df))
* **ffi:** fallible setter surface carries the Result layer the wrapper renders ([92c4905](https://github.com/arthurmaciel/ipe-lang/commit/92c490579d13dbd4418213a843d8b526ab3eba9b))
* **ffi:** gate Cargo feature names at the manifest boundary ([c159a9a](https://github.com/arthurmaciel/ipe-lang/commit/c159a9a914c203db0842042b58c26a69a6a5da1b))
* **ffi:** gate dep features + transitive name at the pkg.json decode boundary ([50f7934](https://github.com/arthurmaciel/ipe-lang/commit/50f793451e6668f9a31e536429cdcd73ab7e7e5b))
* **ffi:** maybe-coercion — IpeMaybe&lt;-&gt;Option at synthesised-instance boundaries ([dfa851c](https://github.com/arthurmaciel/ipe-lang/commit/dfa851ce1db08ad88ea78f83aea4784dd797d25b))
* **ffi:** one-home unification verified E2E — stripe 6-crate SEAL green ([36a4413](https://github.com/arthurmaciel/ipe-lang/commit/36a441383235c92ac7c0e584da566bb4447da748))
* **ffi:** seal the RCE sandbox for SDK-scale installs — chunk per crate + calibrate caps ([#309](https://github.com/arthurmaciel/ipe-lang/issues/309)) ([d2bec5d](https://github.com/arthurmaciel/ipe-lang/commit/d2bec5d87ef571b4894e405e568a84b48e8480f2))
* **ffi:** T1 clippy clean-up + warm-load byte-identity test ([80a5171](https://github.com/arthurmaciel/ipe-lang/commit/80a51714323e126636a5045fefa5b105ab936e38))
* **ffi:** T1 F1 — validated type/path/selector newtypes at the FFI decode boundary + re-derive load_catalog from validated inspection doc ([f1082f5](https://github.com/arthurmaciel/ipe-lang/commit/f1082f516ca619ad9077b3496ef8cab93190981c))
* **ffi:** T1 F1e — graceful legacy fallback for caches without pkg.json ([1a71754](https://github.com/arthurmaciel/ipe-lang/commit/1a717549b6370fae9cefb757033b51ac009207cb))
* **ffi:** validate crate version at decode boundary (CrateVersion newtype) ([6becd88](https://github.com/arthurmaciel/ipe-lang/commit/6becd887652bac03e9fe25ab5cf87c668cc7d327))
* **ffi:** validate pkg_path at the pkginfo decode boundary ([fe0338e](https://github.com/arthurmaciel/ipe-lang/commit/fe0338e3e9088cf8432b00817e3a2674e61cb40b))
* **fmt:** a simple-reference first call arg always hugs the broken head line ([6d095cf](https://github.com/arthurmaciel/ipe-lang/commit/6d095cfc6b308e5dd16a16f54a843ba1ac7c8dbc))
* **fmt:** backward pipe `<|` breaks at the end of the left operand's line ([01a039a](https://github.com/arthurmaciel/ipe-lang/commit/01a039ac8096c6a6e92954ad2e74093a5e872b13))
* **fmt:** elm-format parity — modal layout, paren-safety, let/lambda/signature bugs ([cfa4d0b](https://github.com/arthurmaciel/ipe-lang/commit/cfa4d0bcc80666af9e95213a864ace98fee3570f))
* **fmt:** emit a trailing chain lambda bare, without wrapping parens ([eecf197](https://github.com/arthurmaciel/ipe-lang/commit/eecf197c4047448ffac350ab51a919d4bea642b4))
* **fmt:** FAJoinFirst hugs first call arg only when a block arg is present ([ae118a3](https://github.com/arthurmaciel/ipe-lang/commit/ae118a3dd8b74d7710c7886d5d67271b67aef00e))
* **fmt:** indent record-update fields one level past the brace ([ebed6f8](https://github.com/arthurmaciel/ipe-lang/commit/ebed6f8e3681f1e803d90080506927197e617a6c))
* **fmt:** keep multiline-string call args inline; break modal lambda bodies ([d1830dc](https://github.com/arthurmaciel/ipe-lang/commit/d1830dc91a047e8963ae9aae231f06ac31e26469))
* **fmt:** reflow shake_ffi_by_fn_ident signature — clears pre-existing rustfmt drift (workspace now fmt-clean) ([0b92cb0](https://github.com/arthurmaciel/ipe-lang/commit/0b92cb0df89f2fd9e8567ac04479dcfff0dc7145))
* **fmt:** two blank lines between a pre-header comment block and the module header ([1d31f00](https://github.com/arthurmaciel/ipe-lang/commit/1d31f004b4f6e3a913450c2667bf66f05465490e))
* **gate:** 13 rename-stale test + 2 real bugs surfaced by full-workspace run ([01e6fed](https://github.com/arthurmaciel/ipe-lang/commit/01e6feda1f67432c3e77acbd0bd47318994ff7b1))
* **gate:** clippy --all-targets clean (pedantic + nursery, clippy 1.92) ([53e3740](https://github.com/arthurmaciel/ipe-lang/commit/53e3740787bef25cce042e8b5d3f768bbd61cc21))
* **gate:** recover corrupted stdlib .ipe + reserved-namespace + compiled-source fixes ([6343643](https://github.com/arthurmaciel/ipe-lang/commit/634364358cd59bf905e3cbb73d39e468fa6cd656))
* **gate:** rename stragglers surfaced by full-workspace compile ([e11ced3](https://github.com/arthurmaciel/ipe-lang/commit/e11ced3c1e52d87f0a7f9587025b3f5d6447d54b))
* **gate:** resolve 6 workspace test failures — 4 stale rustfmt snapshots, 1 feature-gated dispatch test, 1 Db.Decode registry drift ([c8041ea](https://github.com/arthurmaciel/ipe-lang/commit/c8041eab1751550dac61274fbb3c412be0267d0a))
* **ipe test:** allow missing_const_for_fn on false_marker ([259b458](https://github.com/arthurmaciel/ipe-lang/commit/259b4589eb748db1f5152b200e3f1fa4df0ad0b4))
* **ipe_backend_rust:** user module named after a kernel namespace collides with the runtime glob ([db862f0](https://github.com/arthurmaciel/ipe-lang/commit/db862f08e0617eea96a605df091d987cb53bb620))
* **ipe_backend:** [#233](https://github.com/arthurmaciel/ipe-lang/issues/233) Stream.stream re-wrap moves captured non-Copy strings (2x E0507) ([bcdfb03](https://github.com/arthurmaciel/ipe-lang/commit/bcdfb03ec53b2c7e4b9174978e8dc80e1a17ffc1))
* **ipe_canon:** register Sub.subscribeWebSocket in QUALIFIERS (anti-drift gap from [#210](https://github.com/arthurmaciel/ipe-lang/issues/210) WebSocket) ([6aaf010](https://github.com/arthurmaciel/ipe-lang/commit/6aaf010833ff96e13f69c0bc6d31573567853b5c))
* **ipe_lower,backend:** [#228](https://github.com/arthurmaciel/ipe-lang/issues/228) type-directed onSubmit handler classification ([3d5c1b9](https://github.com/arthurmaciel/ipe-lang/commit/3d5c1b9c9fb38430cf591a7a78fa4b00e2660fcf))
* **ipe_lower:** [#218](https://github.com/arthurmaciel/ipe-lang/issues/218) clone-relay across intermediate closure boundaries (E0507 SEAL breach) ([976a075](https://github.com/arthurmaciel/ipe-lang/commit/976a075256a2ea7c159f352488881b62b437ece8))
* **ipe_lower:** fold Ipe.Csv `{header,rows}` record to nominal CsvDoc ([#232](https://github.com/arthurmaciel/ipe-lang/issues/232)) ([e320dd9](https://github.com/arthurmaciel/ipe-lang/commit/e320dd939a69e16dcc95e744978c95a42ae98e38))
* **ipe_lower:** unify move-ownership discipline at one entry point, closing the clone-relay class ([#222](https://github.com/arthurmaciel/ipe-lang/issues/222)/[#224](https://github.com/arthurmaciel/ipe-lang/issues/224)/[#225](https://github.com/arthurmaciel/ipe-lang/issues/225)) ([c9b7345](https://github.com/arthurmaciel/ipe-lang/commit/c9b7345f90c2de4e581d72b4363bdb3f651d6fcf))
* **Ipe.Test:** [#219](https://github.com/arthurmaciel/ipe-lang/issues/219) runMain prints pass/fail summary line to stdout ([30ef1b2](https://github.com/arthurmaciel/ipe-lang/commit/30ef1b2151b9aea8b9086ffe9afb14b9749a3538))
* **ipe:** thread on_form on two Expr::Call sites in cache.rs test IR ([889ef36](https://github.com/arthurmaciel/ipe-lang/commit/889ef36a53be23f29d814006b3c458c73d505f29))
* **ir:** bound the IR pretty-printer's recursion depth ([#282](https://github.com/arthurmaciel/ipe-lang/issues/282)) ([bad60bf](https://github.com/arthurmaciel/ipe-lang/commit/bad60bff3a08f2d77e95591efdb11b24a171dae7))
* **jwt:** seal the JWT Algorithm descriptor in Ipe.Secret ([#276](https://github.com/arthurmaciel/ipe-lang/issues/276)) ([5a609ae](https://github.com/arthurmaciel/ipe-lang/commit/5a609ae87afa70b8d752548ae70c28516a17ec22))
* **kernels:** complete required_runtime_module SSOT for PubSub kernels ([1164814](https://github.com/arthurmaciel/ipe-lang/commit/1164814474a122bbc8ae68baba5907558fa94499))
* **lsp:** don't drop the prior project layout on a transient load failure ([#278](https://github.com/arthurmaciel/ipe-lang/issues/278)) ([d7bed12](https://github.com/arthurmaciel/ipe-lang/commit/d7bed12b7ffacfcd14bb00c0a5434aab70e68d95))
* **mirror-parity:** D1 bare Css keyword constants + D2 record-alias-ctor coexistence; advance D3-18 row-poly, file rest ([a7df836](https://github.com/arthurmaciel/ipe-lang/commit/a7df8362dd7272005ce9b96fda5f7eabb707dbb7))
* **money:** kernel-wire Ipe.Money — route currency table / format / FX / allocate through guarded Money_* kernels ([8d45b03](https://github.com/arthurmaciel/ipe-lang/commit/8d45b03cb6208bb97c038ca929aa46e6bbd94c32))
* **parity-matrix:** skip canon-parity for compiled-source Layer-3 qualifiers ([#223](https://github.com/arthurmaciel/ipe-lang/issues/223)) ([acc04a1](https://github.com/arthurmaciel/ipe-lang/commit/acc04a1add9340939024ea92fd3cc57e3647ad2a))
* **parse:** reject space-before-dot instead of misparsing as field access ([9eec146](https://github.com/arthurmaciel/ipe-lang/commit/9eec1467c210e0c7b43471842c78a519ba327a0a))
* **playground:** correct IPE_RUNTIME_DIR path in README + resolver error to src/runtime/rust/src ([5bc57de](https://github.com/arthurmaciel/ipe-lang/commit/5bc57de8e70f58554711bd743a44b4bb22d6a3df))
* **project:** module discovery filtered .sky not .ipe (post-rename regression) ([5678e22](https://github.com/arthurmaciel/ipe-lang/commit/5678e2215522b1d06219719f25070a6a40315ef3))
* **rename:** normalize skyshop config to ipe.toml + fix stray ipe.toml/out in README usage ([#212](https://github.com/arthurmaciel/ipe-lang/issues/212)) ([5488229](https://github.com/arthurmaciel/ipe-lang/commit/54882299554c9ffc7d7f72a26db5787d60a70115))
* **rename:** update base64 expected constant for renamed 'Hello, Ipe!' plaintext ([#212](https://github.com/arthurmaciel/ipe-lang/issues/212)) ([2ffb474](https://github.com/arthurmaciel/ipe-lang/commit/2ffb474eba1736add14bde2abf56598693fa7fb9))
* **rename:** update string_reverse expected constant for renamed 'ipewasm' ([#212](https://github.com/arthurmaciel/ipe-lang/issues/212)) ([aa3c89c](https://github.com/arthurmaciel/ipe-lang/commit/aa3c89c922593d3001bbe506868cf53e7b89dba7))
* **runtime/live:** unrouted GETs no longer wipe a session's handler index ([#170](https://github.com/arthurmaciel/ipe-lang/issues/170) root cause) ([b767c4c](https://github.com/arthurmaciel/ipe-lang/commit/b767c4c53ee8dcaf481caa24fcb2bf7db6f94ddd))
* **runtime:** enforce WS per-message size cap at the framing layer ([#274](https://github.com/arthurmaciel/ipe-lang/issues/274)) ([7a19539](https://github.com/arthurmaciel/ipe-lang/commit/7a1953965172ead483c737562d9c52f5c7e9817d))
* **runtime:** inject dev-console banner into Ipe.Http.Server text/html responses ([#220](https://github.com/arthurmaciel/ipe-lang/issues/220)) ([6df0780](https://github.com/arthurmaciel/ipe-lang/commit/6df0780c6bdef35fd847eab8cb3cf15652987b9a))
* **runtime:** reap abandoned Server.Stream.stream handlers on a TTL ([#273](https://github.com/arthurmaciel/ipe-lang/issues/273)) ([4e730e9](https://github.com/arthurmaciel/ipe-lang/commit/4e730e99d54532d6b01b29f2888a8a1d7978d288))
* **runtime:** refuse to push the ingest token over cleartext HTTP ([#275](https://github.com/arthurmaciel/ipe-lang/issues/275)) ([e3338ec](https://github.com/arthurmaciel/ipe-lang/commit/e3338ec3b7aaced6c72aa0d26d577469096e1972))
* **runtime:** ssrf sibling refs crate::ssrf -&gt; super::ssrf (SEAL: emitted build) ([abb4135](https://github.com/arthurmaciel/ipe-lang/commit/abb41357e5cde0d6e7ff5516ebd6b7e889755c82))
* **runtime:** stop byte-slicing caller-derived JWT descriptor in error messages ([4a98578](https://github.com/arthurmaciel/ipe-lang/commit/4a98578e046d632760581c3bf7a64d53ac63fdb2))
* **seal-006:** route Basics.toString stringify family through IpeStringify ([9d569cf](https://github.com/arthurmaciel/ipe-lang/commit/9d569cfdacc369ab740edb7f798b9b35eab04ae4))
* **stdlib-contracts:** converge Jwt.withClaim / Response / Db.Migration to the reference ([#217](https://github.com/arthurmaciel/ipe-lang/issues/217)) ([7afe3ee](https://github.com/arthurmaciel/ipe-lang/commit/7afe3ee4d0d35f916db6d82ef4653c68fdd292a2))
* **stdlib:** [#261](https://github.com/arthurmaciel/ipe-lang/issues/261) Money.add/sub/sumOf → Result Error Money (currency-mismatch now typed Err) ([969fb19](https://github.com/arthurmaciel/ipe-lang/commit/969fb19d53baabec3cc0183f2856ef415fb7af2e))
* **sweep:** _shape_match strips {- -} block comments, not just -- lines ([0693b9f](https://github.com/arthurmaciel/ipe-lang/commit/0693b9fe52a66b6927d5e177229a7cc6be10ae6c))
* **sweep,ci:** mirror fetches upstream FIRST (local only as offline fallback); ci golden E2E compares against latest installed Sky, retire the cached expected_go oracle ([680edd1](https://github.com/arthurmaciel/ipe-lang/commit/680edd11bad095d109748fdfa67e51530da98c44))
* **sweep:** example_shape classifier -&gt; Ipe.* namespace (Live/Tui/Webview/Http) ([6099d34](https://github.com/arthurmaciel/ipe-lang/commit/6099d347a1b85b0c287aafe19d179234c09019e6))
* **sweep:** FFI-install examples SKIP, not false-RED (13-skyshop) ([ab71e37](https://github.com/arthurmaciel/ipe-lang/commit/ab71e37015f9c9b85a7d7d92e3ccf1201aca1f1c))
* **sweep:** mirror renames sky.toml -&gt; ipe.toml (Ipê's canonical manifest) ([fad2316](https://github.com/arthurmaciel/ipe-lang/commit/fad23166a9dcfc24366abd76f7057419819ca2da))
* **T2:** close SEAL-breach class — exhaustiveness over Prelude builtin ADTs, crate::-qualified top-level calls, live mod-ident gate ([aabbe0d](https://github.com/arthurmaciel/ipe-lang/commit/aabbe0d68974318ba4edbfd0a10c468ac911090e))
* **t3:** bound untrusted recursion/allocation — closes CO-FRONT-001, RT-UI-001, RT-TUI-001, RT-TUI-002 ([17151fe](https://github.com/arthurmaciel/ipe-lang/commit/17151feffa1e9c09e65560a6955df32a9a7c4d51))
* **t4:** JWT-exp NumericDate + Money allocate correctness (CO-INCR-001/002/003, RT-AUTH-001/002/003) ([4d8fc7c](https://github.com/arthurmaciel/ipe-lang/commit/4d8fc7c1f4484733d1c30a95be3aee3823be325f))
* **T5:** data/decode completeness + incremental wiring + SEAL (6 findings) ([8a4ef82](https://github.com/arthurmaciel/ipe-lang/commit/8a4ef82bb2f48ab111223bd815bb129745822465))
* **tests:** repair pre-existing base failures — env_public Module field + kernel-resolution allowlist ([bf3ca58](https://github.com/arthurmaciel/ipe-lang/commit/bf3ca58722998b99b96a5be3b74d710a042161f4))
* **wasm:** M1 gate WebSocket Sub-tier substitute — onOpen/onMessage/onClose/onError live in a browser ([#286](https://github.com/arthurmaciel/ipe-lang/issues/286)) ([bc57e10](https://github.com/arthurmaciel/ipe-lang/commit/bc57e10930cd2f892fbdd8b3bda31d23d685b8e5))
* **watch:** retry the rebuild cycle after a transient resolve failure ([#279](https://github.com/arthurmaciel/ipe-lang/issues/279)) ([3dc1000](https://github.com/arthurmaciel/ipe-lang/commit/3dc100051796b0c3a3a6824ac4fbfc7f05f49b2b))
* **watch:** scope the tests/ watch rule to the root-level directory only ([#280](https://github.com/arthurmaciel/ipe-lang/issues/280)) ([554c90d](https://github.com/arthurmaciel/ipe-lang/commit/554c90d7ca090d26dd243f03cec315c7f373f9d1))

## [0.1.0](https://github.com/arthurmaciel/ipe-lang/releases/tag/v0.1.0)

### Added

- First tagged release of the Ipê compiler, runtime, and CLI.
