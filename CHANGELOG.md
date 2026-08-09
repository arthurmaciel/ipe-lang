# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/).

Entries below the header are maintained by
[release-please](https://github.com/googleapis/release-please): each release
section is generated from Conventional Commit messages and prepended when the
standing release pull request is merged.

## [0.1.42](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.41...ipe-v0.1.42) (2026-08-09)


### Features

* **lower:** FCF Approach A slice 3 — collection-element Arc carrier (frontier total, fail-closed) ([#776](https://github.com/arthurmaciel/ipe-lang/issues/776)) ([83a2ab6](https://github.com/arthurmaciel/ipe-lang/commit/83a2ab6c84f304ec75273417117f15b3222866cc))


### Bug Fixes

* **backend:** normalize fn record/enum-literal fields onto the SharedFun Arc carrier ([#789](https://github.com/arthurmaciel/ipe-lang/issues/789)) ([#792](https://github.com/arthurmaciel/ipe-lang/issues/792)) ([1697e36](https://github.com/arthurmaciel/ipe-lang/commit/1697e362cc784afd97bbc050ca08ac63e5f3cda4))
* **canon:** home Ipe.Ui's re-exported Attribute to the Ui carrier so stdui animation/grid/transition seals build ([#777](https://github.com/arthurmaciel/ipe-lang/issues/777)) ([#784](https://github.com/arthurmaciel/ipe-lang/issues/784)) ([9ba6609](https://github.com/arthurmaciel/ipe-lang/commit/9ba6609b113faaf62aecb4840c3e6f4d45bec131))
* **canon:** honor explicit exposing(Type(..)) for qualified-home union constructors ([#653](https://github.com/arthurmaciel/ipe-lang/issues/653) follow-up) ([#779](https://github.com/arthurmaciel/ipe-lang/issues/779)) ([43be51f](https://github.com/arthurmaciel/ipe-lang/commit/43be51f9e7a6c582435a1484a1eb00f2f7081cd5))
* **cli:** bare-word mode selectors (ipe doc list / ipe diff check) with deprecation shims ([#699](https://github.com/arthurmaciel/ipe-lang/issues/699)) ([#787](https://github.com/arthurmaciel/ipe-lang/issues/787)) ([b42b880](https://github.com/arthurmaciel/ipe-lang/commit/b42b88092a99f09452d5d1dfdf8b71a15503faa6))
* **ffi:** pin externally-referenced crates in emitted FFI Cargo.toml ([#777](https://github.com/arthurmaciel/ipe-lang/issues/777)) ([#785](https://github.com/arthurmaciel/ipe-lang/issues/785)) ([1ccad89](https://github.com/arthurmaciel/ipe-lang/commit/1ccad896bf878eacc160d67c3e64eb77612978df))
* **json-dec:** migrate pipeline fixtures so valid nested-decoder pipelines compile ([#777](https://github.com/arthurmaciel/ipe-lang/issues/777)) ([#783](https://github.com/arthurmaciel/ipe-lang/issues/783)) ([2810468](https://github.com/arthurmaciel/ipe-lang/commit/2810468ea11ece6ce1f4dac7161471262369de0d))
* **lower:** narrow the RetryPolicy fn-carrier exemption to the closed 5-field shape ([#665](https://github.com/arthurmaciel/ipe-lang/issues/665)) ([#790](https://github.com/arthurmaciel/ipe-lang/issues/790)) ([303ddfe](https://github.com/arthurmaciel/ipe-lang/commit/303ddfeaf03b29376970089cd7b9ac49d9f54e7b))
* **lower:** select the decimal feature on a Money/Decimal type-mention ([#777](https://github.com/arthurmaciel/ipe-lang/issues/777)) ([#781](https://github.com/arthurmaciel/ipe-lang/issues/781)) ([a755d44](https://github.com/arthurmaciel/ipe-lang/commit/a755d446826963522c2311edcf9b78bf85b47e30))
* **random:** resolve Ipe.Random shuffle/weighted/seed/seeded* members ([#672](https://github.com/arthurmaciel/ipe-lang/issues/672)) ([#791](https://github.com/arthurmaciel/ipe-lang/issues/791)) ([37e0b7f](https://github.com/arthurmaciel/ipe-lang/commit/37e0b7f992798b055d69043eeca3bde942cc58b5))
* **wasm:** emit named MainHydrationState so hydrate glue is generated for wasm ([#224](https://github.com/arthurmaciel/ipe-lang/issues/224)) ([#786](https://github.com/arthurmaciel/ipe-lang/issues/786)) ([8e0fa84](https://github.com/arthurmaciel/ipe-lang/commit/8e0fa84b03e88c8bba377a84610b381fb5a006c6))

## [0.1.41](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.40...ipe-v0.1.41) (2026-08-09)


### Features

* **lower:** FCF Approach A slices 1+2 — Arc-carrier promotion for forwarded functions ([#767](https://github.com/arthurmaciel/ipe-lang/issues/767)) ([f0bf65c](https://github.com/arthurmaciel/ipe-lang/commit/f0bf65c074d70202f9a981adcdb75560ecde550f))


### Bug Fixes

* **cli:** single-source subcommand registry so dispatch and help cannot drift ([#701](https://github.com/arthurmaciel/ipe-lang/issues/701)) ([#770](https://github.com/arthurmaciel/ipe-lang/issues/770)) ([e1e1f9f](https://github.com/arthurmaciel/ipe-lang/commit/e1e1f9fae4e2f954342a9ad137a2e47491dfe243))
* **cli:** stream emitted cargo build progress in ipe build/run ([#757](https://github.com/arthurmaciel/ipe-lang/issues/757)) ([#765](https://github.com/arthurmaciel/ipe-lang/issues/765)) ([5ee6986](https://github.com/arthurmaciel/ipe-lang/commit/5ee698649029e984252c1e715059a8c77de46bf1))
* **codegen:** clone reused non-Copy cache handle across Task steps ([#676](https://github.com/arthurmaciel/ipe-lang/issues/676)) ([#768](https://github.com/arthurmaciel/ipe-lang/issues/768)) ([eb5226f](https://github.com/arthurmaciel/ipe-lang/commit/eb5226ff8dbaf772220354f825e7ca0c68accb07))
* **doc:** single-source the documented-module registry so --list and query agree ([#698](https://github.com/arthurmaciel/ipe-lang/issues/698)) ([#771](https://github.com/arthurmaciel/ipe-lang/issues/771)) ([ad95865](https://github.com/arthurmaciel/ipe-lang/commit/ad958653dc4356fc4d1cb921d48af50e08af2cd7))

## [0.1.40](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.39...ipe-v0.1.40) (2026-08-05)


### Features

* **capability:** add the `unsafe` capability + `Ipe.<M>.Unsafe` disclosure plumbing ([#679](https://github.com/arthurmaciel/ipe-lang/issues/679) slice 0) ([#729](https://github.com/arthurmaciel/ipe-lang/issues/729)) ([a159246](https://github.com/arthurmaciel/ipe-lang/commit/a1592464232ed9dbc20a1572afc82e3b0685b354))
* **stdlib:** add Ipe.Codec JSON-direction compiled-source surface (codec slice 1) ([#740](https://github.com/arthurmaciel/ipe-lang/issues/740)) ([d96c0d5](https://github.com/arthurmaciel/ipe-lang/commit/d96c0d5369c1c4ad53d2f12fea1cc711c1459b12))
* **stdlib:** add scoped Secret.use + relocate Secret.reveal -&gt; Ipe.Secret.Unsafe.unsafeReveal (unsafe-axis slice E) ([#679](https://github.com/arthurmaciel/ipe-lang/issues/679)) ([#736](https://github.com/arthurmaciel/ipe-lang/issues/736)) ([cdde69b](https://github.com/arthurmaciel/ipe-lang/commit/cdde69b9f3a600e9e304d72a062083103e8af183))
* **stdlib:** convert Ipe.Ui layout builders to compiled-source ([#726](https://github.com/arthurmaciel/ipe-lang/issues/726)) ([0a7f808](https://github.com/arthurmaciel/ipe-lang/commit/0a7f8086fc3ee7b58e54c41645eda9e3f559c726))
* **stdlib:** relocate Html.unsafeRaw -&gt; Ipe.Html.Unsafe.unsafeRaw (unsafe-axis slice A) ([#679](https://github.com/arthurmaciel/ipe-lang/issues/679)) ([#730](https://github.com/arthurmaciel/ipe-lang/issues/730)) ([0ab85b0](https://github.com/arthurmaciel/ipe-lang/commit/0ab85b01d1b07356c82ae59796e2dc47ce8f363a))
* **stdlib:** relocate raw-SQL / untyped-read Db hatches to Ipe.Db.Unsafe + add unsafeFragment (unsafe-axis slice C) ([#679](https://github.com/arthurmaciel/ipe-lang/issues/679)) ([#733](https://github.com/arthurmaciel/ipe-lang/issues/733)) ([903cba4](https://github.com/arthurmaciel/ipe-lang/commit/903cba44734b7fc59643cc0cf9076fb5f80a3e7d))
* **stdlib:** relocate Web.Head.unsafeJsonLd to Ipe.Web.Head.Unsafe (unsafe-axis slice D) ([#679](https://github.com/arthurmaciel/ipe-lang/issues/679)) ([#735](https://github.com/arthurmaciel/ipe-lang/issues/735)) ([6a2a529](https://github.com/arthurmaciel/ipe-lang/commit/6a2a5298829f6ad60629c5a252656fe3df685ccf))


### Bug Fixes

* **lower:** close http-stream ChunkEvent/StreamId module-set SEAL breach ([#724](https://github.com/arthurmaciel/ipe-lang/issues/724)) ([d7b2c52](https://github.com/arthurmaciel/ipe-lang/commit/d7b2c52b0930958b5ab88e1ce17b9866b21589b3))

## [0.1.39](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.38...ipe-v0.1.39) (2026-08-05)


### Features

* **kernels,types:** polymorphic TyShape (type vars) + migrate the List family, on the D.2 base (Kernel Row stage D.3) ([#696](https://github.com/arthurmaciel/ipe-lang/issues/696)) ([fc17bec](https://github.com/arthurmaciel/ipe-lang/commit/fc17becf6d27d498091928f671ad8e31364da2b6))
* **kernels,types:** TyShape scheme ADT + interpreter, migrate the Bitwise family (Kernel Row stage D template slice) ([#694](https://github.com/arthurmaciel/ipe-lang/issues/694)) ([14603fe](https://github.com/arthurmaciel/ipe-lang/commit/14603fe7eb2fb8b05e54453f990d26e79d55bd5d))
* **kernels:** KernelDef descriptor projecting the existing kernel row + emit-symbol-defined invariant test (Kernel Row stage A) ([#685](https://github.com/arthurmaciel/ipe-lang/issues/685)) ([4e5c7df](https://github.com/arthurmaciel/ipe-lang/commit/4e5c7df592e4188f634fb5de55a30e7c3f447bef))
* **sandbox:** aarch64 Linux Tier-2 certifying seccomp arm ([#620](https://github.com/arthurmaciel/ipe-lang/issues/620)) ([#670](https://github.com/arthurmaciel/ipe-lang/issues/670)) ([040ba10](https://github.com/arthurmaciel/ipe-lang/commit/040ba1012651f01c20277aa612c53b3c5009c40e))
* **types:** resolve KernelDef scheme by key + arity-vs-scheme coherence test (Kernel Row stage C) ([#692](https://github.com/arthurmaciel/ipe-lang/issues/692)) ([0ae0298](https://github.com/arthurmaciel/ipe-lang/commit/0ae029879b835cf5c0b31a33a3b812e1ea27ec26))


### Bug Fixes

* **backend,lower:** emit the Ipe.Cache runtime module + close the bare-handle SEAL hole ([#661](https://github.com/arthurmaciel/ipe-lang/issues/661)) ([#684](https://github.com/arthurmaciel/ipe-lang/issues/684)) ([35c33d4](https://github.com/arthurmaciel/ipe-lang/commit/35c33d4a9959daf05bc597e1a07c9a53d18420f3))
* **canon:** resolve Random.range as an int-kernel alias ([#667](https://github.com/arthurmaciel/ipe-lang/issues/667)) ([#673](https://github.com/arthurmaciel/ipe-lang/issues/673)) ([b5eb86e](https://github.com/arthurmaciel/ipe-lang/commit/b5eb86e09bc5f868fe216a9e563c8675dfb16fa8))
* **examples:** green 02/03/18/26/32 mirror examples (Task-boundary, Element view, Http/Regex APIs) ([#580](https://github.com/arthurmaciel/ipe-lang/issues/580)) ([#669](https://github.com/arthurmaciel/ipe-lang/issues/669)) ([88b0cc4](https://github.com/arthurmaciel/ipe-lang/commit/88b0cc4451345ee333566ef5fb7722b1264cf99e))
* **examples:** remap N0036 Task.run examples 07/14/35 onto TEA auto-run entry ([#580](https://github.com/arthurmaciel/ipe-lang/issues/580)) ([#660](https://github.com/arthurmaciel/ipe-lang/issues/660)) ([74ad83d](https://github.com/arthurmaciel/ipe-lang/commit/74ad83d90f90c852968cd77c4cf11e7e4b5e5e29))
* **sandbox:** deny pidfd_getfd/bpf/userfaultfd/keyctl/kexec in the seccomp baseline (both ABIs) ([#671](https://github.com/arthurmaciel/ipe-lang/issues/671)) ([#682](https://github.com/arthurmaciel/ipe-lang/issues/682)) ([126bbdb](https://github.com/arthurmaciel/ipe-lang/commit/126bbdbf872c348a4e5868f0438cc4c7df049031))
* **sandbox:** root FreeBSD /proc-mask source outside writable scratch ([#658](https://github.com/arthurmaciel/ipe-lang/issues/658)) ([#675](https://github.com/arthurmaciel/ipe-lang/issues/675)) ([e8b0797](https://github.com/arthurmaciel/ipe-lang/commit/e8b0797acf08654a177dd94c09ba003adfaf012c))
* **types:** fail closed on a managed-loop view that settles to Html — IPE-T0020 ([#647](https://github.com/arthurmaciel/ipe-lang/issues/647)) ([#668](https://github.com/arthurmaciel/ipe-lang/issues/668)) ([217fe00](https://github.com/arthurmaciel/ipe-lang/commit/217fe00fa5f6e56c38b5d01c30ec230c269e1b36))

## [0.1.38](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.37...ipe-v0.1.38) (2026-08-04)


### Features

* **examples:** idiomatic TEA async-DB port of 17-skymon (ipe-overrides) ([#644](https://github.com/arthurmaciel/ipe-lang/issues/644)) ([a391879](https://github.com/arthurmaciel/ipe-lang/commit/a3918795b4c2b23954236011289ac4587b772000))
* **examples:** ipe-overrides/12-skyvote — TEA async-DB port ([#638](https://github.com/arthurmaciel/ipe-lang/issues/638)) ([3ac8b65](https://github.com/arthurmaciel/ipe-lang/commit/3ac8b65a6773dd09fa14c66e29cf2556d97f4a5b))
* **examples:** ipe-overrides/16-skychess — TEA async-DB port ([#580](https://github.com/arthurmaciel/ipe-lang/issues/580)) ([#639](https://github.com/arthurmaciel/ipe-lang/issues/639)) ([e9d56b7](https://github.com/arthurmaciel/ipe-lang/commit/e9d56b70436c7cd6b5af799cfaec0390b0524835))
* **scripts:** generalize the Sky→Ipê transform to any project + Go→Rust FFI dependency map ([#652](https://github.com/arthurmaciel/ipe-lang/issues/652)) ([d9fbbc3](https://github.com/arthurmaciel/ipe-lang/commit/d9fbbc3d6938f382f1be287a9d5c65c33f3a242d))


### Bug Fixes

* **canon:** scope HttpMethod verbs to Http qualifier, unshadowing user ctors ([#653](https://github.com/arthurmaciel/ipe-lang/issues/653)) ([0a0a949](https://github.com/arthurmaciel/ipe-lang/commit/0a0a949e3033be76785a2b35a3b8ab64990f2686)), closes [#646](https://github.com/arthurmaciel/ipe-lang/issues/646)
* **diagnostics:** register IPE-N0036 + IPE-N0030 so `ipe explain` resolves them ([#629](https://github.com/arthurmaciel/ipe-lang/issues/629)) ([#640](https://github.com/arthurmaciel/ipe-lang/issues/640)) ([7c793cc](https://github.com/arthurmaciel/ipe-lang/commit/7c793cc9a5284985f6c54c7ff0fa4627c5896d31))
* **emit:** sound curry lowering for `succeed` applied to a fn value ([#634](https://github.com/arthurmaciel/ipe-lang/issues/634)) ([#642](https://github.com/arthurmaciel/ipe-lang/issues/642)) ([2d75899](https://github.com/arthurmaciel/ipe-lang/commit/2d75899fbdcc5725116aa6a50a2d38ef62679d12))
* **examples:** add missing stdlib imports to 8 Sky-mirror ports ([#580](https://github.com/arthurmaciel/ipe-lang/issues/580)) ([#655](https://github.com/arthurmaciel/ipe-lang/issues/655)) ([0f8e41d](https://github.com/arthurmaciel/ipe-lang/commit/0f8e41d2f4a9a1523d9f4c857dc9e0c690e72f9d))
* **examples:** green 19-skyforum, 28-streaming-chat, 37-composite-live-shop ([#580](https://github.com/arthurmaciel/ipe-lang/issues/580)) ([#650](https://github.com/arthurmaciel/ipe-lang/issues/650)) ([c3adc63](https://github.com/arthurmaciel/ipe-lang/commit/c3adc63d0a84bf37149d575095d9abeab832856e))
* **examples:** map removed Cli/Tui/Webview mirror shapes onto Ipe.Tea.Terminal/WebView ([#656](https://github.com/arthurmaciel/ipe-lang/issues/656)) ([cc8f538](https://github.com/arthurmaciel/ipe-lang/commit/cc8f5388a6913a7e0117dce15a71c24598d543c5)), closes [#580](https://github.com/arthurmaciel/ipe-lang/issues/580)
* **examples:** remap 24-tui-kitchen-sink + 38-composite mirror shapes onto Terminal/WebView ([#580](https://github.com/arthurmaciel/ipe-lang/issues/580)) ([#659](https://github.com/arthurmaciel/ipe-lang/issues/659)) ([798a022](https://github.com/arthurmaciel/ipe-lang/commit/798a02210a98f234adb45f957231db3837ca678a))
* **lower:** fail-closed gate for a fn value reaching a record field via a reified generic slot ([#584](https://github.com/arthurmaciel/ipe-lang/issues/584)) ([#636](https://github.com/arthurmaciel/ipe-lang/issues/636)) ([229e400](https://github.com/arthurmaciel/ipe-lang/commit/229e4008d1e37f29b687752667176e5ddf51305e))
* **sandbox:** FreeBSD build-jail mounts fresh devfs + masks /proc ([#645](https://github.com/arthurmaciel/ipe-lang/issues/645)) ([#657](https://github.com/arthurmaciel/ipe-lang/issues/657)) ([aa58734](https://github.com/arthurmaciel/ipe-lang/commit/aa58734230879978f7a8b65b23190157662206e6))
* **sandbox:** FreeBSD Tier-2 jail truly denies network + filesystem axes ([#266](https://github.com/arthurmaciel/ipe-lang/issues/266)) ([#648](https://github.com/arthurmaciel/ipe-lang/issues/648)) ([7f0f1bf](https://github.com/arthurmaciel/ipe-lang/commit/7f0f1bf52decf7f7ba3e6375cfc952bf79684508))
* **sandbox:** render macOS SBPL scratch write-allow in symlink-resolved form ([#654](https://github.com/arthurmaciel/ipe-lang/issues/654)) ([b5bddff](https://github.com/arthurmaciel/ipe-lang/commit/b5bddffbcb6ff09eda2347af65bb0a0d6ffb6df1)), closes [#266](https://github.com/arthurmaciel/ipe-lang/issues/266)
* **sandbox:** Windows CreateProcessW env block sorts in uppercase-ordinal order ([#266](https://github.com/arthurmaciel/ipe-lang/issues/266)) ([#649](https://github.com/arthurmaciel/ipe-lang/issues/649)) ([4c983b8](https://github.com/arthurmaciel/ipe-lang/commit/4c983b8fec720402a050eebaa97037163154eefe))

## [0.1.37](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.36...ipe-v0.1.37) (2026-08-04)


### Features

* **backend:** converge wasm emit onto the runtime dependency-crate model ([#514](https://github.com/arthurmaciel/ipe-lang/issues/514)) ([#602](https://github.com/arthurmaciel/ipe-lang/issues/602)) ([d056102](https://github.com/arthurmaciel/ipe-lang/commit/d056102dca94d6338d5d6ca81135ce31ac95061c))
* **canon,lower,diagnostics:** CustomElement typing acceptance + fail-closed seal gate ([#333](https://github.com/arthurmaciel/ipe-lang/issues/333) increment 2) ([#600](https://github.com/arthurmaciel/ipe-lang/issues/600)) ([67d1535](https://github.com/arthurmaciel/ipe-lang/commit/67d153546191cac50a48d49786736411abcef9e1))
* **canon:** IPE-N0040 rejects hand-nested decoder pipelines, incl. binder indirection, on type-check too ([#614](https://github.com/arthurmaciel/ipe-lang/issues/614) [#615](https://github.com/arthurmaciel/ipe-lang/issues/615) [#619](https://github.com/arthurmaciel/ipe-lang/issues/619) [#622](https://github.com/arthurmaciel/ipe-lang/issues/622)) ([#633](https://github.com/arthurmaciel/ipe-lang/issues/633)) ([bc18895](https://github.com/arthurmaciel/ipe-lang/commit/bc188954c274986be8a036f2e90e553dd81871f6))
* **cli:** ipe test command + verify calls it + standardized verify output ([#609](https://github.com/arthurmaciel/ipe-lang/issues/609) [#610](https://github.com/arthurmaciel/ipe-lang/issues/610)) ([#631](https://github.com/arthurmaciel/ipe-lang/issues/631)) ([4cf38fa](https://github.com/arthurmaciel/ipe-lang/commit/4cf38fad7129c196353b084817f3f270999d8f2d))
* **cli:** rename check→type-check, add `ipe clean`, sectioned usage, safe `ipe init` re-init ([#611](https://github.com/arthurmaciel/ipe-lang/issues/611) [#607](https://github.com/arthurmaciel/ipe-lang/issues/607) [#608](https://github.com/arthurmaciel/ipe-lang/issues/608) [#612](https://github.com/arthurmaciel/ipe-lang/issues/612)) ([#621](https://github.com/arthurmaciel/ipe-lang/issues/621)) ([2d84c15](https://github.com/arthurmaciel/ipe-lang/commit/2d84c15137ca26eba773816cbdd16430322dd496))
* **cli:** streamlined stage-progress output standard + adopt in install.sh and ipe upgrade ([#613](https://github.com/arthurmaciel/ipe-lang/issues/613)) ([#616](https://github.com/arthurmaciel/ipe-lang/issues/616)) ([ee526e6](https://github.com/arthurmaciel/ipe-lang/commit/ee526e681ba5f194b7f3541b432f7211f395c1a7))
* **examples:** ipe-overrides/27-multi-session-chat — TEA async-DB port ([#635](https://github.com/arthurmaciel/ipe-lang/issues/635)) ([285008f](https://github.com/arthurmaciel/ipe-lang/commit/285008fb35fdbe94aad759e22a3ad3ea7c4ac708))
* **lower,backend:** row-poly multi-field argument rows monomorphise per call-site shape ([#287](https://github.com/arthurmaciel/ipe-lang/issues/287)) ([#617](https://github.com/arthurmaciel/ipe-lang/issues/617)) ([da3b07b](https://github.com/arthurmaciel/ipe-lang/commit/da3b07b5ee70f5779389b2c172ebbd492d57be64))


### Bug Fixes

* **canon:** local module shadows stdlib import gate; helper submodule exempt from Program/TEA gate ([#605](https://github.com/arthurmaciel/ipe-lang/issues/605)) ([95c5486](https://github.com/arthurmaciel/ipe-lang/commit/95c54861da74a2d8aec89c16617ba4c71a94be9e))
* **cli:** ipe build compiles the emitted crate so a cargo failure exits non-zero ([#590](https://github.com/arthurmaciel/ipe-lang/issues/590)) ([#627](https://github.com/arthurmaciel/ipe-lang/issues/627)) ([e0dc830](https://github.com/arthurmaciel/ipe-lang/commit/e0dc830a65042d3cd3dbf39bf5f13796a401e221))
* **cli:** rename `ipe doctor` → `ipe health`; real free-disk check; clearer version wording ([#603](https://github.com/arthurmaciel/ipe-lang/issues/603)) ([60bd42c](https://github.com/arthurmaciel/ipe-lang/commit/60bd42cf145f0fa3a07ff20849b6afe265cb9537))
* **examples/transform:** alias-aware stdlib Db raw-surface marking ([#630](https://github.com/arthurmaciel/ipe-lang/issues/630)) ([#632](https://github.com/arthurmaciel/ipe-lang/issues/632)) ([4e67e89](https://github.com/arthurmaciel/ipe-lang/commit/4e67e89729d1e8da9c17e10b26c58e7f6f11d643))
* **examples:** map Std.Live -&gt; Ipe.Tea.Web in the Sky mirror (fixes [#588](https://github.com/arthurmaciel/ipe-lang/issues/588)) ([#618](https://github.com/arthurmaciel/ipe-lang/issues/618)) ([1282864](https://github.com/arthurmaciel/ipe-lang/commit/1282864e5e3164d47559da28f657020d09d01d98))
* **examples:** web-shape view reshape (Html→Element) + Math import — mirror green 11→15/52 ([#580](https://github.com/arthurmaciel/ipe-lang/issues/580)) ([#624](https://github.com/arthurmaciel/ipe-lang/issues/624)) ([43a5195](https://github.com/arthurmaciel/ipe-lang/commit/43a5195350c5e11d42c909c9198296e3f57015a0))
* **lower:** exhaustive, uniform ir_type_mentions feature detection ([#577](https://github.com/arthurmaciel/ipe-lang/issues/577)) ([#628](https://github.com/arthurmaciel/ipe-lang/issues/628)) ([0c9d555](https://github.com/arthurmaciel/ipe-lang/commit/0c9d5552eadee29d5b0a78ed2d8f1c60f2c0f94b))
* **lower:** fail-closed gate for point-free generic-fn-carrier instantiation ([#572](https://github.com/arthurmaciel/ipe-lang/issues/572)) ([#626](https://github.com/arthurmaciel/ipe-lang/issues/626)) ([efd70a7](https://github.com/arthurmaciel/ipe-lang/commit/efd70a75fa7902cf08c79fe88de4c7ca964481f2))

## [0.1.36](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.35...ipe-v0.1.36) (2026-08-04)


### Features

* **backend:** row-polymorphic single-field argument records — Increment 1 ([#287](https://github.com/arthurmaciel/ipe-lang/issues/287)) ([#593](https://github.com/arthurmaciel/ipe-lang/issues/593)) ([8077355](https://github.com/arthurmaciel/ipe-lang/commit/80773550fd57901e1a9357e3b84f46776997fece))
* **cli:** ipe eject — self-contained Rust project with a tree-shaken vendored runtime ([#515](https://github.com/arthurmaciel/ipe-lang/issues/515)) ([#596](https://github.com/arthurmaciel/ipe-lang/issues/596)) ([9252ef5](https://github.com/arthurmaciel/ipe-lang/commit/9252ef579ce285674492417a3658fbec46781ce0))
* **runtime,backend:** bounded recursion — depth guard converts stack-overflow DoS into a contained error ([#532](https://github.com/arthurmaciel/ipe-lang/issues/532)) ([#591](https://github.com/arthurmaciel/ipe-lang/issues/591)) ([ed9d719](https://github.com/arthurmaciel/ipe-lang/commit/ed9d719aab07ff7b2716a63e91ad90fe157821b2))


### Bug Fixes

* **canon:** local module shadows stdlib import gate; helper submodule exempt from Program/TEA gate ([#589](https://github.com/arthurmaciel/ipe-lang/issues/589)) ([6801c37](https://github.com/arthurmaciel/ipe-lang/commit/6801c377adf9027be7d0c8d23c1db2f3799f9629))
* **ci,sandbox:** install nextest in e2e + macOS/Windows/FreeBSD run-jail correctness ([#266](https://github.com/arthurmaciel/ipe-lang/issues/266)) ([#599](https://github.com/arthurmaciel/ipe-lang/issues/599)) ([4676ef2](https://github.com/arthurmaciel/ipe-lang/commit/4676ef2dcf56cb3159ca8d615fb5e0c40209f382))
* **release,runtime:** Windows binary builds again + resilient/loud publish + sanction the recursion trip for panic-scan ([#598](https://github.com/arthurmaciel/ipe-lang/issues/598)) ([e1c7a2e](https://github.com/arthurmaciel/ipe-lang/commit/e1c7a2ed535c396d8da081be48c802f2c3d829aa))
* **runtime,backend:** cmd double-render + wasm url import ([#483](https://github.com/arthurmaciel/ipe-lang/issues/483)) ([#586](https://github.com/arthurmaciel/ipe-lang/issues/586)) ([5af51e9](https://github.com/arthurmaciel/ipe-lang/commit/5af51e90c24b7f6eab1f8756848781e66e4bfef8))

## [0.1.35](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.34...ipe-v0.1.35) (2026-08-03)


### Features

* **stdlib:** Ipe.Markdown follows the surrounding theme ([#548](https://github.com/arthurmaciel/ipe-lang/issues/548)) + exposes its parser ([#549](https://github.com/arthurmaciel/ipe-lang/issues/549)) ([#585](https://github.com/arthurmaciel/ipe-lang/issues/585)) ([1baa831](https://github.com/arthurmaciel/ipe-lang/commit/1baa83156406a02fe8f782277472610cd12bdc87))


### Bug Fixes

* **backend:** key record structs by full structural shape, not field-name set ([#553](https://github.com/arthurmaciel/ipe-lang/issues/553)) ([#576](https://github.com/arthurmaciel/ipe-lang/issues/576)) ([1ddd164](https://github.com/arthurmaciel/ipe-lang/commit/1ddd164ff19a3bb8cb8f8dfbd6be3048aca48dcd))
* **backend:** Web programs emit compilable crates — app serde dep ([#566](https://github.com/arthurmaciel/ipe-lang/issues/566)) + no duplicate runtime mod ([#567](https://github.com/arthurmaciel/ipe-lang/issues/567)) ([#570](https://github.com/arthurmaciel/ipe-lang/issues/570)) ([f1a818d](https://github.com/arthurmaciel/ipe-lang/commit/f1a818d9c03629142fdddb34cf9b4482768c2165))
* **cli:** fail early on a version-mismatched runtime; drop help page on build failure ([#571](https://github.com/arthurmaciel/ipe-lang/issues/571)) ([56e3bd8](https://github.com/arthurmaciel/ipe-lang/commit/56e3bd8ad224f154aaeeb7d2a5589e39c60204e2))
* **doctor:** reformat suggested-fixes as indented bright-yellow bullets ([#562](https://github.com/arthurmaciel/ipe-lang/issues/562)) ([3e32d83](https://github.com/arthurmaciel/ipe-lang/commit/3e32d83e5c02c473d3ad10b65cbcc788b36e3647))
* **examples:** task-publish uses a typed Topic, not a bare String ([#556](https://github.com/arthurmaciel/ipe-lang/issues/556)) ([#557](https://github.com/arthurmaciel/ipe-lang/issues/557)) ([7ad335f](https://github.com/arthurmaciel/ipe-lang/commit/7ad335f50bfdd1b00dea9408d7d0a2ea520dabbd))
* **examples:** typed Topic handles for pub/sub sites (IPE-T0001) ([#581](https://github.com/arthurmaciel/ipe-lang/issues/581)) ([3e82264](https://github.com/arthurmaciel/ipe-lang/commit/3e8226495eefde84cc1cbd348ca473738772423e))
* **lower:** fail-closed gate for a function instantiating a generic slot ([#579](https://github.com/arthurmaciel/ipe-lang/issues/579)) ([1d28a9b](https://github.com/arthurmaciel/ipe-lang/commit/1d28a9b8833e2cf930485cbb366c24d85630cd24))
* **lower:** scan function bodies for feature-gated types so uses_json is a superset of emission ([#578](https://github.com/arthurmaciel/ipe-lang/issues/578)) ([efb82bb](https://github.com/arthurmaciel/ipe-lang/commit/efb82bb12d8e5799d74629befb0f0a2acc0bdb12))
* **runtime:** textarea/select pseudo-class CSS no longer leaks into value ([#545](https://github.com/arthurmaciel/ipe-lang/issues/545)); fix(fmt): keep inter-constructor comments inside a type ([#554](https://github.com/arthurmaciel/ipe-lang/issues/554)) ([#582](https://github.com/arthurmaciel/ipe-lang/issues/582)) ([1e15016](https://github.com/arthurmaciel/ipe-lang/commit/1e15016b48e1a2177f76ea7bec08e079794a39b1))
* **runtime:** Time.timeString UTC ([#529](https://github.com/arthurmaciel/ipe-lang/issues/529)) + wasm-client build resolves crate::web + weak-hash crates ([#527](https://github.com/arthurmaciel/ipe-lang/issues/527)) ([#568](https://github.com/arthurmaciel/ipe-lang/issues/568)) ([d57b403](https://github.com/arthurmaciel/ipe-lang/commit/d57b4030f74de9ace55b5cb13bc93c3098f67184))
* **ui:** fillPortion flex-basis ([#543](https://github.com/arthurmaciel/ipe-lang/issues/543)) + mediaQuery cascade/target ([#544](https://github.com/arthurmaciel/ipe-lang/issues/544)); feat(stdlib): Ipe.List combinators ([#555](https://github.com/arthurmaciel/ipe-lang/issues/555)) ([#575](https://github.com/arthurmaciel/ipe-lang/issues/575)) ([081eb8d](https://github.com/arthurmaciel/ipe-lang/commit/081eb8d9b2e206f1d306f0c263cc084689002e0a))
* **verify:** resolve project src/ modules from the test stage ([#565](https://github.com/arthurmaciel/ipe-lang/issues/565)) ([#569](https://github.com/arthurmaciel/ipe-lang/issues/569)) ([a3186d4](https://github.com/arthurmaciel/ipe-lang/commit/a3186d4b2ae68d45915797a6416595882218bf10))
* **web:** client-JS event dispatch + navigation/scroll ([#546](https://github.com/arthurmaciel/ipe-lang/issues/546) [#547](https://github.com/arthurmaciel/ipe-lang/issues/547) [#550](https://github.com/arthurmaciel/ipe-lang/issues/550) [#551](https://github.com/arthurmaciel/ipe-lang/issues/551) [#552](https://github.com/arthurmaciel/ipe-lang/issues/552)) ([#583](https://github.com/arthurmaciel/ipe-lang/issues/583)) ([69c4fc4](https://github.com/arthurmaciel/ipe-lang/commit/69c4fc419ed837b32e9d0553297e35118dac42a4))

## [0.1.34](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.33...ipe-v0.1.34) (2026-08-03)


### Features

* **feature-split:** P7b — demote json off the emitted floor ([#540](https://github.com/arthurmaciel/ipe-lang/issues/540)) ([2459993](https://github.com/arthurmaciel/ipe-lang/commit/2459993c3fd055dfc0442296af2987f655be381e))
* **runtime,backend:** gate chrono behind time-core/log so a bare Program drops it (feature-split P4) ([#531](https://github.com/arthurmaciel/ipe-lang/issues/531)) ([03e1179](https://github.com/arthurmaciel/ipe-lang/commit/03e1179cec15029c34ccd114a143a61e4718b86f))
* **runtime,backend:** gate encoding codecs behind a feature (feature-split P2) + fix static allocator splice ([#524](https://github.com/arthurmaciel/ipe-lang/issues/524)) ([e4d1b95](https://github.com/arthurmaciel/ipe-lang/commit/e4d1b95c0633b9329aeeb49b4a7426fb937b3e4a))
* **runtime,backend:** gate regex/uuid/random behind features (feature-split P3) + fix http-only encoding under-inclusion ([#526](https://github.com/arthurmaciel/ipe-lang/issues/526)) ([f934987](https://github.com/arthurmaciel/ipe-lang/commit/f934987f989a0bd8dc62bc84e3ac2dcf2ac7ee5a))
* **runtime,backend:** gate rust_decimal + unicode-general-category so a bare Program drops them (feature-split P5) ([#536](https://github.com/arthurmaciel/ipe-lang/issues/536)) ([a35b0e6](https://github.com/arthurmaciel/ipe-lang/commit/a35b0e66d2cd72cfeff073120ff4a3d9fee19e9a))
* **runtime:** gate crypto_core behind crypto-core feature, secret behind secret (phase 6) ([#538](https://github.com/arthurmaciel/ipe-lang/issues/538)) ([59468bd](https://github.com/arthurmaciel/ipe-lang/commit/59468bd84fd094d717a4b14d1620b21410f4280f))
* **runtime:** String.toInt trims surrounding Unicode whitespace ([#530](https://github.com/arthurmaciel/ipe-lang/issues/530)) ([9ba9cdb](https://github.com/arthurmaciel/ipe-lang/commit/9ba9cdb5d20e009444d259b73efd9fe8e9706c1c))


### Bug Fixes

* **backend:** reorder Db.Decode.andThen args to the runtime's decoder-first shape ([#535](https://github.com/arthurmaciel/ipe-lang/issues/535)) ([432243a](https://github.com/arthurmaciel/ipe-lang/commit/432243adf26b7cd02162bb1d07666df31c7bf174))
* **parse:** distinct sub-spans per access-chain node to stop type-region collision ([#537](https://github.com/arthurmaciel/ipe-lang/issues/537)) ([f0392f7](https://github.com/arthurmaciel/ipe-lang/commit/f0392f7e7c0d5612d61c209b28c3f0c44e6e80d7))

## [0.1.33](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.32...ipe-v0.1.33) (2026-08-03)


### Features

* **backend:** runtime feature-map SSOT + feature-set closure SEAL (S3 P2) ([#508](https://github.com/arthurmaciel/ipe-lang/issues/508)) ([8634a60](https://github.com/arthurmaciel/ipe-lang/commit/8634a60d09d41288b35af675e95b50e601943314))
* **cli:** ipe doctor — environment diagnostics + consent-gated setup ([#512](https://github.com/arthurmaciel/ipe-lang/issues/512)) ([#519](https://github.com/arthurmaciel/ipe-lang/issues/519)) ([f73ee80](https://github.com/arthurmaciel/ipe-lang/commit/f73ee80a818c495f4e3c3b0c0ecab47d7d12f91c))
* **cli:** S3 P4+P5 — dep-model default emit, embed+materialize runtime, walk-up IPE_RUNTIME_DIR ([#517](https://github.com/arthurmaciel/ipe-lang/issues/517)) ([684d602](https://github.com/arthurmaciel/ipe-lang/commit/684d60274122b0d62c11337821c867c593e02611))
* **emit,runtime:** dependency-model native emit behind IPE_RUNTIME_DEP (S3 P3) ([#511](https://github.com/arthurmaciel/ipe-lang/issues/511)) ([9ed0c39](https://github.com/arthurmaciel/ipe-lang/commit/9ed0c395cfd66d7443b79cc6d92c7eb618f56f8a))
* goldens are byte-identical. ([4d3dc8b](https://github.com/arthurmaciel/ipe-lang/commit/4d3dc8b98f1716e22813496356d20524aff08b1f))
* **lower:** function-level dependency emission via IR reachability ([#509](https://github.com/arthurmaciel/ipe-lang/issues/509)) ([#520](https://github.com/arthurmaciel/ipe-lang/issues/520)) ([9c79f0e](https://github.com/arthurmaciel/ipe-lang/commit/9c79f0e527d446f37705c8216560e93e11e04986))


### Bug Fixes

* **runtime:** gate log's wasm browser-console path on the wasm-client feature ([#518](https://github.com/arthurmaciel/ipe-lang/issues/518)) ([1341f58](https://github.com/arthurmaciel/ipe-lang/commit/1341f58bed263d2d94207238a7c6249c061c8e39))

## [0.1.32](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.31...ipe-v0.1.32) (2026-08-02)


### Features

* **backend,runtime:** gate chrono-tz on uses_time (drop IANA zone DB from non-Time programs) ([#502](https://github.com/arthurmaciel/ipe-lang/issues/502)) ([58b2621](https://github.com/arthurmaciel/ipe-lang/commit/58b2621946427a4cf42ddc3de2c783e8ec6d1b7d))
* **backend,runtime:** synchronous fn main for pure programs — hello-world 53 crates ([#498](https://github.com/arthurmaciel/ipe-lang/issues/498)) ([9f8d4e6](https://github.com/arthurmaciel/ipe-lang/commit/9f8d4e6ada4e9781630bc8d783b8b22c7cfa596b))
* **backend:** gate the rsa crate off the always-on crypto_core floor ([#497](https://github.com/arthurmaciel/ipe-lang/issues/497)) ([b2fa817](https://github.com/arthurmaciel/ipe-lang/commit/b2fa81748fe79892c03184498f44cebe33b8bf79))
* **backend:** gate the url crate on uses_url so pure programs shed its idna/ICU4X subtree ([#495](https://github.com/arthurmaciel/ipe-lang/issues/495)) ([4d81da3](https://github.com/arthurmaciel/ipe-lang/commit/4d81da34b240cbe207430e5a78e885ab3331cb41))
* **playground:** add sandboxed jail-runner and wire /run to it ([#490](https://github.com/arthurmaciel/ipe-lang/issues/490)) ([ba9ebdc](https://github.com/arthurmaciel/ipe-lang/commit/ba9ebdcf4999d64878b5b005403d9835e2ab7ee1))
* **runtime:** crate feature-parity with emitted trimming (S3 precondition) ([#504](https://github.com/arthurmaciel/ipe-lang/issues/504)) ([1a0ceab](https://github.com/arthurmaciel/ipe-lang/commit/1a0ceabc37e6a97edd997587d20696f4d0165477))


### Bug Fixes

* **lower:** give a generic enum-payload type argument the Arc fn carrier ([#484](https://github.com/arthurmaciel/ipe-lang/issues/484)) ([#506](https://github.com/arthurmaciel/ipe-lang/issues/506)) ([59c6147](https://github.com/arthurmaciel/ipe-lang/commit/59c61473affaa63a02f691f4e0dc848d7612eca2))

## [0.1.31](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.30...ipe-v0.1.31) (2026-08-01)


### Features

* **backend:** gate csv crate behind Ipe.Csv usage ([#481](https://github.com/arthurmaciel/ipe-lang/issues/481)) ([9c6816f](https://github.com/arthurmaciel/ipe-lang/commit/9c6816f9b60213176dfca4e510d5d36f6cc6eab2))
* **backend:** gate flate2 + zstd behind Ipe.Compression usage ([#480](https://github.com/arthurmaciel/ipe-lang/issues/480)) ([89d8d59](https://github.com/arthurmaciel/ipe-lang/commit/89d8d593918e8ffb63e442f8a1e0f50aaa5e0e87))
* **backend:** gate heavy crypto on uses_crypto + jwt on uses_jwt||uses_auth ([#475](https://github.com/arthurmaciel/ipe-lang/issues/475) D-E) ([#489](https://github.com/arthurmaciel/ipe-lang/issues/489)) ([9377cbf](https://github.com/arthurmaciel/ipe-lang/commit/9377cbf3aa441dff66fc88aa73bc30789e67f969))
* **backend:** gate toml + serde_yaml behind Ipe.Config TOML/YAML decoder usage ([#478](https://github.com/arthurmaciel/ipe-lang/issues/478)) ([325f452](https://github.com/arthurmaciel/ipe-lang/commit/325f452f4c4b4070f159514b536052cdfd56c4d1))


### Bug Fixes

* **goldens:** align two stale run-oracles with sanctioned surface/semantics ([#487](https://github.com/arthurmaciel/ipe-lang/issues/487)) ([a83e3af](https://github.com/arthurmaciel/ipe-lang/commit/a83e3af7b543931440653d297861c38732aafc0b))
* **lower:** Arc-carrier a non-literal fn value into a user-enum payload ctor ([#486](https://github.com/arthurmaciel/ipe-lang/issues/486)) ([5f0c617](https://github.com/arthurmaciel/ipe-lang/commit/5f0c6177fc9de00b055e078d78f49be2ad97e919))

## [0.1.30](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.29...ipe-v0.1.30) (2026-07-31)


### Features

* **backend:** gate reqwest + http_client behind actual HTTP-client usage ([#466](https://github.com/arthurmaciel/ipe-lang/issues/466)) ([#474](https://github.com/arthurmaciel/ipe-lang/issues/474)) ([544566c](https://github.com/arthurmaciel/ipe-lang/commit/544566c30ad1bbded0dec71977025daef882fe4a))
* **cli:** human-friendly error when the Rust toolchain is missing ([#467](https://github.com/arthurmaciel/ipe-lang/issues/467)) ([#469](https://github.com/arthurmaciel/ipe-lang/issues/469)) ([c99aa0a](https://github.com/arthurmaciel/ipe-lang/commit/c99aa0a365bc2881f7e1ddf247dcb7e1d4f64ffe))
* **lower:** per-module fresh-name allocation seeding (byte-identical) ([#279](https://github.com/arthurmaciel/ipe-lang/issues/279)) ([#468](https://github.com/arthurmaciel/ipe-lang/issues/468)) ([a973a94](https://github.com/arthurmaciel/ipe-lang/commit/a973a940657bebce2cbb825086855282d60caed8))
* **playground:** replace build.sh with an Ipê build script and add an Ipê static server ([#477](https://github.com/arthurmaciel/ipe-lang/issues/477)) ([6639749](https://github.com/arthurmaciel/ipe-lang/commit/663974912b60d26e64f2dacf1b70ad5c97bb5a63))
* **playground:** sandboxed server build+run, relocated into examples/ ([#317](https://github.com/arthurmaciel/ipe-lang/issues/317), closes [#465](https://github.com/arthurmaciel/ipe-lang/issues/465)) ([#472](https://github.com/arthurmaciel/ipe-lang/issues/472)) ([a6a8a7c](https://github.com/arthurmaciel/ipe-lang/commit/a6a8a7cdbdbaa3e3e6dc0365655c582b74c990ed))
* **static:** aarch64 triple-aware C-compiler preflight + C-free CProfile axis ([#270](https://github.com/arthurmaciel/ipe-lang/issues/270)) ([#463](https://github.com/arthurmaciel/ipe-lang/issues/463)) ([8b4f81f](https://github.com/arthurmaciel/ipe-lang/commit/8b4f81f4a7c202dd2a94aada2d3a7a13bec8d841))


### Bug Fixes

* **sandbox:** gate the FreeBSD shell-quote helper off Windows so the Tier-2 build-jail crate compiles there ([#292](https://github.com/arthurmaciel/ipe-lang/issues/292)) ([#460](https://github.com/arthurmaciel/ipe-lang/issues/460)) ([ae52c01](https://github.com/arthurmaciel/ipe-lang/commit/ae52c0169f27a09f83a81b41b600e5046dd77558))
* serve .wasm browser-noise files as application/wasm ([#476](https://github.com/arthurmaciel/ipe-lang/issues/476)) ([5fde088](https://github.com/arthurmaciel/ipe-lang/commit/5fde088e213331eed8e2ca177208142ae859a786))

## [0.1.29](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.28...ipe-v0.1.29) (2026-07-31)


### Features

* **backend:** monomorphize direct-position Fn params to impl Fn ([#431](https://github.com/arthurmaciel/ipe-lang/issues/431)) ([#455](https://github.com/arthurmaciel/ipe-lang/issues/455)) ([7ec1040](https://github.com/arthurmaciel/ipe-lang/commit/7ec1040d598d841ef540dbc1c72f00de56f69289))
* **canon,diagnostics:** CustomElement JS-boundary reserved-type seal ([#333](https://github.com/arthurmaciel/ipe-lang/issues/333)) ([#443](https://github.com/arthurmaciel/ipe-lang/issues/443)) ([0007618](https://github.com/arthurmaciel/ipe-lang/commit/00076184ab3798c528b31d4e299c37e167ad8438))
* **ffi:** Rust.Ffi.call asserted-call — exact-carrier shims, ffi-raw capability, panic boundary ([#396](https://github.com/arthurmaciel/ipe-lang/issues/396)) ([#448](https://github.com/arthurmaciel/ipe-lang/issues/448)) ([745fbec](https://github.com/arthurmaciel/ipe-lang/commit/745fbec20051219c91c5fa87be44a4b4a36c898f))
* **http:** typed Url request target + fail-closed API-layer scheme narrowing ([#399](https://github.com/arthurmaciel/ipe-lang/issues/399)) ([#441](https://github.com/arthurmaciel/ipe-lang/issues/441)) ([83e21d5](https://github.com/arthurmaciel/ipe-lang/commit/83e21d5eac1e6bda821beb55de55f800a9aa6ac2))
* **index:** curated-index-repository side — schema, validator, admission CI ([#291](https://github.com/arthurmaciel/ipe-lang/issues/291)) ([#440](https://github.com/arthurmaciel/ipe-lang/issues/440)) ([7cfc21f](https://github.com/arthurmaciel/ipe-lang/commit/7cfc21f7bd76c24b8a41b8f2823f839b6e0dd77a))
* **io,runtime:** echo-suppressed password line read (Io.readSecret) ([#402](https://github.com/arthurmaciel/ipe-lang/issues/402)) ([#436](https://github.com/arthurmaciel/ipe-lang/issues/436)) ([b289601](https://github.com/arthurmaciel/ipe-lang/commit/b2896013e4cd51fc844714feb65937da434ef5db))
* **lower,backend,types:** first-class functions in enum variant payloads — Phase 2 carrier normalization ([#445](https://github.com/arthurmaciel/ipe-lang/issues/445)) ([103ece2](https://github.com/arthurmaciel/ipe-lang/commit/103ece2cdd70bc5081c0986479988c63a76a0d44))
* **lower:** first-class functions in record fields — Phase 1 carrier normalization ([#438](https://github.com/arthurmaciel/ipe-lang/issues/438)) ([ae1904d](https://github.com/arthurmaciel/ipe-lang/commit/ae1904d98bddd10844a0e9dec65861f41bfdd649))
* **runtime:** async FFI join-error funnel — no silently dropped panic payloads ([#396](https://github.com/arthurmaciel/ipe-lang/issues/396) async-breadth) ([#437](https://github.com/arthurmaciel/ipe-lang/issues/437)) ([01e23ab](https://github.com/arthurmaciel/ipe-lang/commit/01e23ab346965a5d807c74cd3f94bfc2d665d147))


### Bug Fixes

* **backend:** dedup libc dependency in emitted manifest for live/webview + readSecret shapes ([#446](https://github.com/arthurmaciel/ipe-lang/issues/446)) ([#449](https://github.com/arthurmaciel/ipe-lang/issues/449)) ([e5b93ac](https://github.com/arthurmaciel/ipe-lang/commit/e5b93ac519f325070e844e79a7591e22a646f962))
* **http:** resolve HttpMethod ADT surface as values, patterns, and methodToString ([#432](https://github.com/arthurmaciel/ipe-lang/issues/432)) ([#447](https://github.com/arthurmaciel/ipe-lang/issues/447)) ([d1f4549](https://github.com/arthurmaciel/ipe-lang/commit/d1f454980b6f2d5ff89b156c3cdb42712dec4cbb))
* **lower,backend:** erase Ipe.PubSub.Topic phantom uniformly across decl and CAF emit ([#457](https://github.com/arthurmaciel/ipe-lang/issues/457)) ([#458](https://github.com/arthurmaciel/ipe-lang/issues/458)) ([f480b42](https://github.com/arthurmaciel/ipe-lang/commit/f480b426aea0b41bb25ba3237f6097a924834074))
* **runtime:** disambiguate url crate from local Url newtype in ws_client ([#433](https://github.com/arthurmaciel/ipe-lang/issues/433)) ([#444](https://github.com/arthurmaciel/ipe-lang/issues/444)) ([d800c56](https://github.com/arthurmaciel/ipe-lang/commit/d800c5628eb8a0138a6ba9862698a53361347e00))
* **test:** add missing Ipe.Ui import to five onsubmit live_e2e fixtures ([#456](https://github.com/arthurmaciel/ipe-lang/issues/456)) ([f840ee7](https://github.com/arthurmaciel/ipe-lang/commit/f840ee7a2b2707ce42bea357aa0434d2595b2328))
* **test:** isolate g_http_live cargo-build tests from concurrent emit-dir wipes ([#454](https://github.com/arthurmaciel/ipe-lang/issues/454)) ([15fc3dc](https://github.com/arthurmaciel/ipe-lang/commit/15fc3dc733c92eb0caa54d4166ca27e005ce2faf))
* **test:** web routed-view golden returns Element per framework contract, not Html via Ui.layout ([#450](https://github.com/arthurmaciel/ipe-lang/issues/450)) ([#451](https://github.com/arthurmaciel/ipe-lang/issues/451)) ([d9308fa](https://github.com/arthurmaciel/ipe-lang/commit/d9308fa26879f2f8d084b3d90d9dc6ff16a0d274))

## [0.1.28](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.27...ipe-v0.1.28) (2026-07-31)


### Features

* **ffi:** define-transparency unification — all-identity-carrier define types surface as records/unions through the transparent-import glue ([#427](https://github.com/arthurmaciel/ipe-lang/issues/427)) ([55933a6](https://github.com/arthurmaciel/ipe-lang/commit/55933a69d38d68bfd08fb1334aef5300e9b3d2a3))
* **stdlib:** Ipe.Url.Parser routing patterns over the typed Url ([#399](https://github.com/arthurmaciel/ipe-lang/issues/399)) ([#425](https://github.com/arthurmaciel/ipe-lang/issues/425)) ([bcb132c](https://github.com/arthurmaciel/ipe-lang/commit/bcb132c558586ddcf1bd96110011cf1170382142))


### Bug Fixes

* **runtime,ui:** flow paragraph el children inline on the web backend ([#434](https://github.com/arthurmaciel/ipe-lang/issues/434)) ([3e724d3](https://github.com/arthurmaciel/ipe-lang/commit/3e724d3f7092bd297431d61c9bab8698690fbc6d))
* **stdlib:** drop the Ipe.Pure band-aid; arity-0 effect kernels take () directly ([#429](https://github.com/arthurmaciel/ipe-lang/issues/429)) ([54fbc88](https://github.com/arthurmaciel/ipe-lang/commit/54fbc88297c44932787e59a3babf24b3dd4f7cfa))

## [0.1.27](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.26...ipe-v0.1.27) (2026-07-31)


### Features

* **doc:** local-first module grouping + searchable soft-dark HTML site ([#418](https://github.com/arthurmaciel/ipe-lang/issues/418)) ([b7f2953](https://github.com/arthurmaciel/ipe-lang/commit/b7f2953cef4885ed4c730c74ec89e6ec5f6ddc56))
* **emit:** native formatter replaces the rustfmt subprocess — full byte-parity incl. or-patterns ([#278](https://github.com/arthurmaciel/ipe-lang/issues/278)) ([#415](https://github.com/arthurmaciel/ipe-lang/issues/415)) ([9de5efd](https://github.com/arthurmaciel/ipe-lang/commit/9de5efd2a742d90309cca691954eac6c76fd71fd))


### Bug Fixes

* **runtime/db:** Db.Decode int rejects out-of-range instead of saturating ([#420](https://github.com/arthurmaciel/ipe-lang/issues/420)) ([2f025d9](https://github.com/arthurmaciel/ipe-lang/commit/2f025d9786352f7314b376f2310588ff1604a1a4))
* **stdlib:** tail-recursive Result.combine / Maybe.combine ([#419](https://github.com/arthurmaciel/ipe-lang/issues/419)) ([0eafc48](https://github.com/arthurmaciel/ipe-lang/commit/0eafc48759f6608bc13c37c6957189b04aa077b7))

## [0.1.26](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.25...ipe-v0.1.26) (2026-07-31)


### Features

* **canon:** move renderStatic to shape-neutral Ipe.Html ([#323](https://github.com/arthurmaciel/ipe-lang/issues/323)) ([#404](https://github.com/arthurmaciel/ipe-lang/issues/404)) ([6e9fbca](https://github.com/arthurmaciel/ipe-lang/commit/6e9fbca1fab61531acd189ba4fcfecccc54bdd6e))
* **db:** typed SQL part 2 — mark the stringly row-read surface unsafe* ([#376](https://github.com/arthurmaciel/ipe-lang/issues/376)) ([#405](https://github.com/arthurmaciel/ipe-lang/issues/405)) ([9e8ef8f](https://github.com/arthurmaciel/ipe-lang/commit/9e8ef8fbdf329561a2ee6f9329f939c18bb44e78))
* **ffi:** panic-boundary + fail-closed getter classification ([#396](https://github.com/arthurmaciel/ipe-lang/issues/396) pkg 1) ([#407](https://github.com/arthurmaciel/ipe-lang/issues/407)) ([7cb01fc](https://github.com/arthurmaciel/ipe-lang/commit/7cb01fcc53e88ff3e26a189c4b41ba5b723b16b6))
* **ffi:** transparent-import decode side — inspector schema + classification + .ipei vocab ([#396](https://github.com/arthurmaciel/ipe-lang/issues/396)) ([#408](https://github.com/arthurmaciel/ipe-lang/issues/408)) ([d9d2bb3](https://github.com/arthurmaciel/ipe-lang/commit/d9d2bb37f8a042f4254aba8359ff372e3c36cb44))
* **ffi:** transparent-import write-side cutover — record/union surface + conversion glue ([#396](https://github.com/arthurmaciel/ipe-lang/issues/396)) ([#414](https://github.com/arthurmaciel/ipe-lang/issues/414)) ([2c11ec5](https://github.com/arthurmaciel/ipe-lang/commit/2c11ec5710adcedbec1c6fea0d74d79d51c4fdc3))
* **stdlib:** additive Elm coverage — Bitwise, Tuple, Random Generator ([#274](https://github.com/arthurmaciel/ipe-lang/issues/274)) ([#409](https://github.com/arthurmaciel/ipe-lang/issues/409)) ([795ee6f](https://github.com/arthurmaciel/ipe-lang/commit/795ee6fa942dd0406e2284feddb2559786b77d69))
* **stdlib:** route hand-written String/Basics through their kernels ([#271](https://github.com/arthurmaciel/ipe-lang/issues/271)) ([#401](https://github.com/arthurmaciel/ipe-lang/issues/401)) ([437df51](https://github.com/arthurmaciel/ipe-lang/commit/437df5166e5f89dbed68fe0db6cdf371f899bc96))
* **web:** onNavigate cfg field — URL navigation flows through update ([#393](https://github.com/arthurmaciel/ipe-lang/issues/393)) ([cefea5f](https://github.com/arthurmaciel/ipe-lang/commit/cefea5f8f65a9001cbebefe0d6cffdccab4af0b5))


### Bug Fixes

* **cli:** resolve project-root entry for capabilities and --emit-ir; friendlier check/explain defaults ([#411](https://github.com/arthurmaciel/ipe-lang/issues/411)) ([df699b1](https://github.com/arthurmaciel/ipe-lang/commit/df699b1bf48bc3b63dc33c47816157d16a6f24eb))
* **ssrf:** correct stale module doc — guard is production-gated, not opt-in ([#403](https://github.com/arthurmaciel/ipe-lang/issues/403)) ([65e2258](https://github.com/arthurmaciel/ipe-lang/commit/65e22588058e0ed8b182c36a484b4e44ef0f872a))

## [0.1.25](https://github.com/arthurmaciel/ipe-lang/compare/ipe-v0.1.24...ipe-v0.1.25) (2026-07-30)


### Features

* **audit:** Windows Tier-2 native .ps1 probe wrapper — promote windows-x64 to a certifying platform ([#260](https://github.com/arthurmaciel/ipe-lang/issues/260)) ([#386](https://github.com/arthurmaciel/ipe-lang/issues/386)) ([dd94474](https://github.com/arthurmaciel/ipe-lang/commit/dd9447483dbecfb836ffb1236f9604265196799b))
* **canon,lsp:** shape-scoped Cmd/Sub + IPE-N0035 cross-shape gate + Web PubSub doc ([#302](https://github.com/arthurmaciel/ipe-lang/issues/302), [#303](https://github.com/arthurmaciel/ipe-lang/issues/303)) ([#331](https://github.com/arthurmaciel/ipe-lang/issues/331)) ([1646bb8](https://github.com/arthurmaciel/ipe-lang/commit/1646bb831df37dca448341103b1f0b4728d8e186))
* **cli:** infer --target wasm from [wasm].mode in ipe.toml ([#320](https://github.com/arthurmaciel/ipe-lang/issues/320)) ([#366](https://github.com/arthurmaciel/ipe-lang/issues/366)) ([b2bbfc9](https://github.com/arthurmaciel/ipe-lang/commit/b2bbfc9f02042ac8f4baf54e03cf066e59777fe7))
* **cli:** ipe verify — one-command project gate (fmt + type-check + build) ([#301](https://github.com/arthurmaciel/ipe-lang/issues/301)) ([#361](https://github.com/arthurmaciel/ipe-lang/issues/361)) ([9ce9b2a](https://github.com/arthurmaciel/ipe-lang/commit/9ce9b2aa70194b510c70ef104f75ddd1468eed83))
* **db:** mark the raw-SQL escape hatch — Db.execRaw → Db.unsafeExecRaw ([#339](https://github.com/arthurmaciel/ipe-lang/issues/339)) ([#377](https://github.com/arthurmaciel/ipe-lang/issues/377)) ([b4e9f8f](https://github.com/arthurmaciel/ipe-lang/commit/b4e9f8f62fe3fd79a71b110c62b1dfd783bfefb4))
* **doc:** always include stdlib; add --list and &lt;module&gt; query ([#325](https://github.com/arthurmaciel/ipe-lang/issues/325)) ([#370](https://github.com/arthurmaciel/ipe-lang/issues/370)) ([82e5a76](https://github.com/arthurmaciel/ipe-lang/commit/82e5a76eb94a1efff7b7a985ad665d1d38208d83))
* **http:** HttpMethod ADT replaces stringly Http.method ([#343](https://github.com/arthurmaciel/ipe-lang/issues/343)) ([#364](https://github.com/arthurmaciel/ipe-lang/issues/364)) ([6b5874b](https://github.com/arthurmaciel/ipe-lang/commit/6b5874b06aaf728cdb2973b787e800615a30cccc))
* **lexer,canon:** path "…" literal sugar for typed Path ([#358](https://github.com/arthurmaciel/ipe-lang/issues/358)) ([#373](https://github.com/arthurmaciel/ipe-lang/issues/373)) ([0b8d23b](https://github.com/arthurmaciel/ipe-lang/commit/0b8d23b98528862ffd7ad435e33f31a112791028))
* **runtime:** Windows-aware Path.clean; drop cfg(windows) compile_error ([#359](https://github.com/arthurmaciel/ipe-lang/issues/359)) ([#368](https://github.com/arthurmaciel/ipe-lang/issues/368)) ([2609a98](https://github.com/arthurmaciel/ipe-lang/commit/2609a988b73dec8bb7fbf28aae45b7158676f22e))
* **stdlib,types:** typed pub/sub Topic a payload contract ([#340](https://github.com/arthurmaciel/ipe-lang/issues/340)) [salvaged] ([#372](https://github.com/arthurmaciel/ipe-lang/issues/372)) ([e6ade1e](https://github.com/arthurmaciel/ipe-lang/commit/e6ade1ebde6d15f9c2e28d13f179746a1b0fa625))
* **stdlib:** compiled Regex type + Regex.compile — invalid patterns are typed Err ([#341](https://github.com/arthurmaciel/ipe-lang/issues/341)) ([#360](https://github.com/arthurmaciel/ipe-lang/issues/360)) ([c684294](https://github.com/arthurmaciel/ipe-lang/commit/c6842945a02562779967e29ecf2459e34a0a281f))
* **stdlib:** Ipe.Markdown — markdown → Ui.Element renderer ([#321](https://github.com/arthurmaciel/ipe-lang/issues/321)) ([#380](https://github.com/arthurmaciel/ipe-lang/issues/380)) ([d500806](https://github.com/arthurmaciel/ipe-lang/commit/d500806541ac0b2919e5205883a449db302fb450))
* **stdlib:** locale-correct case mapping — Locale + String.toUpperIn/toLowerIn (ICU4X) ([#277](https://github.com/arthurmaciel/ipe-lang/issues/277)) ([#388](https://github.com/arthurmaciel/ipe-lang/issues/388)) ([99872dc](https://github.com/arthurmaciel/ipe-lang/commit/99872dce561bea7ffe09992fec0d965f9247b908))
* **stdlib:** typed Ipe.Url (parse-don't-validate) + injection-safe query builder ([#347](https://github.com/arthurmaciel/ipe-lang/issues/347)) ([#383](https://github.com/arthurmaciel/ipe-lang/issues/383)) ([d455011](https://github.com/arthurmaciel/ipe-lang/commit/d455011a131a1689ba5e29ded6921d0e435437bb))
* **stdlib:** typed Path (parse-don't-validate) + Ipe.File migration ([#334](https://github.com/arthurmaciel/ipe-lang/issues/334)) ([#357](https://github.com/arthurmaciel/ipe-lang/issues/357)) ([438f95c](https://github.com/arthurmaciel/ipe-lang/commit/438f95cdf959b53f81d326136d71f1a8d118f093))
* **stdlib:** typed security newtypes — Crypto Key/Mac, Email EmailAddress ([#344](https://github.com/arthurmaciel/ipe-lang/issues/344)) ([#367](https://github.com/arthurmaciel/ipe-lang/issues/367)) ([a772025](https://github.com/arthurmaciel/ipe-lang/commit/a7720256edf62bdd35f81657b2f1469f78cb22c3))
* **surface:** drop Task.run + Task.perform from the Ipê surface ([#282](https://github.com/arthurmaciel/ipe-lang/issues/282)) ([#389](https://github.com/arthurmaciel/ipe-lang/issues/389)) ([e727ad7](https://github.com/arthurmaciel/ipe-lang/commit/e727ad75cac70dd2c64a02af5d24398a4f757b24))
* **types:** closed-union case refuses catch-all arms — IPE-T0018 fail-closed ([#276](https://github.com/arthurmaciel/ipe-lang/issues/276)) ([#392](https://github.com/arthurmaciel/ipe-lang/issues/392)) ([480433e](https://github.com/arthurmaciel/ipe-lang/commit/480433e72aa80bf8c2b4a4763ee2327987a70689))
* **types:** exhaustiveness-aware wildcard warning IPE-T0018 ([#272](https://github.com/arthurmaciel/ipe-lang/issues/272)) ([#379](https://github.com/arthurmaciel/ipe-lang/issues/379)) ([b9f8d55](https://github.com/arthurmaciel/ipe-lang/commit/b9f8d5527b89b94836c34745eee8f330fb8b1f72))
* **verify:** wire the test stage (Ipe.Test runner) ([#390](https://github.com/arthurmaciel/ipe-lang/issues/390)) ([11201b2](https://github.com/arthurmaciel/ipe-lang/commit/11201b2ca98c3dbaa3eba985cfc15f609e107d25))
* **wasm:** client-side router for the WasmClient shape ([#268](https://github.com/arthurmaciel/ipe-lang/issues/268)) ([#391](https://github.com/arthurmaciel/ipe-lang/issues/391)) ([fb5d165](https://github.com/arthurmaciel/ipe-lang/commit/fb5d165ca2294f1e4983ba710305d4a1de9fa8ba))


### Bug Fixes

* **bytes:** migrate Email/attachment byte pipeline to the typed Bytes carrier ([#275](https://github.com/arthurmaciel/ipe-lang/issues/275)) ([#387](https://github.com/arthurmaciel/ipe-lang/issues/387)) ([4312307](https://github.com/arthurmaciel/ipe-lang/commit/43123076a2a7fb75c4d5add66ec24f1046ca5d5e))
* **cli:** box the CliError::Pipeline diagnostic to shrink the driver error ([#332](https://github.com/arthurmaciel/ipe-lang/issues/332)) ([#350](https://github.com/arthurmaciel/ipe-lang/issues/350)) ([cf8add1](https://github.com/arthurmaciel/ipe-lang/commit/cf8add1fba019ef2d770ded12d6e9645167de254))
* **cli:** ipe upgrade no-prebuilt-binary is a typed error, never shows help ([#351](https://github.com/arthurmaciel/ipe-lang/issues/351)) ([#365](https://github.com/arthurmaciel/ipe-lang/issues/365)) ([6d11ea5](https://github.com/arthurmaciel/ipe-lang/commit/6d11ea503fe40884bf086a55ac00589b991f660a))
* **cli:** route all human-facing prose through style::gutter — closes [#354](https://github.com/arthurmaciel/ipe-lang/issues/354) ([#374](https://github.com/arthurmaciel/ipe-lang/issues/374)) ([2314b5f](https://github.com/arthurmaciel/ipe-lang/commit/2314b5f827d2a4094f636b50f78efdd6ee5a3810))
* **diagnostics:** ipe check caret parity with build + capped/collapsed 'did you mean' ([#355](https://github.com/arthurmaciel/ipe-lang/issues/355)) ([#356](https://github.com/arthurmaciel/ipe-lang/issues/356)) ([333dac8](https://github.com/arthurmaciel/ipe-lang/commit/333dac8de51da4de06a9ccbaab287e0bcff0cf50))
* **html:** close the raw-String HTML/script injection hole — Html.raw→unsafeRaw, Head.jsonLd→unsafeJsonLd ([#338](https://github.com/arthurmaciel/ipe-lang/issues/338)) ([#378](https://github.com/arthurmaciel/ipe-lang/issues/378)) ([c5a0181](https://github.com/arthurmaciel/ipe-lang/commit/c5a01812b8c82f3fac1dd398c04e4350cf292181))
* **install:** success + report-bugs lines at the 2-space banner/GUTTER indent ([#353](https://github.com/arthurmaciel/ipe-lang/issues/353)) ([6d74342](https://github.com/arthurmaciel/ipe-lang/commit/6d743420f72e95f0af4b10d6b589d65e1ffffb38))
* **path:** harden escapes_root — reject any leading all-dots (&gt;=2) element ([#384](https://github.com/arthurmaciel/ipe-lang/issues/384)) ([cc8ed1d](https://github.com/arthurmaciel/ipe-lang/commit/cc8ed1dac710cbde3b1013c403368d57fe2b67e3))
* **stdlib:** Money.parseCurrency returns Maybe Currency (kill silent CurrencyRaw default) ([#363](https://github.com/arthurmaciel/ipe-lang/issues/363)) ([fd77fd5](https://github.com/arthurmaciel/ipe-lang/commit/fd77fd5424c4ed06cf1c8327b87b3cacad2c3875))
* **test:** make doc-serve test robust to the framed announce line ([#375](https://github.com/arthurmaciel/ipe-lang/issues/375)) ([2412bed](https://github.com/arthurmaciel/ipe-lang/commit/2412bed2177358f6d3000ae835ff712896c02b67))
* **web:** SSE reconnect reconciles page with connection URL ([#385](https://github.com/arthurmaciel/ipe-lang/issues/385)) ([0b45851](https://github.com/arthurmaciel/ipe-lang/commit/0b45851882b49cccaedcf52b7b1a623d621568d6))

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
