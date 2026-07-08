# ipe-index — Rust port plan (reuse skydex; do NOT rewrite)

skydex (`../sky/tools/skydex/`, ~1600 LOC Rust) already does what we need:
walk (git-aware) + store (sqlite) + model + parity (cross-lang) + query
(locate/rdeps/deps/covers) + `update` (incremental from git diff). It handles
`.rs .hs .go .sky`. Port = ADAPT it for our two-repo layout, not start over.

## Steps (fresh-token session)
1. `cp -r ../sky/tools/skydex/{src,Cargo.toml,tests} tools/ipe-index/`; rename
   package `skydex`→`ipe-index` in Cargo.toml + bin name.
2. `src/walk.rs`: roots are SKY-specific (`runtime-rust/`, `src/Sky/`,
   `runtime-go/`, `sky-stdlib/`). Make roots a config of TWO repos:
   - Ipê (this repo): `crates/` `runtime/` `tools/` (Rust) + `crates/skyc/stdlib/**.sky`.
   - Sky ref (`../sky`): `src/Sky/**.hs` (Haskell) + `runtime-go/**.go` (Go) +
     `sky-stdlib/**.sky` (Sky) + `runtime-go/**` FFI.
3. Add extractors for JS (Live artifacts: `runtime-*/**.js`, emitted) + Bash
   (`scripts/**.sh`, `*.sh`) — def regexes like the python v0
   (`scripts/ipe-index`), which is the reference for the Rust def patterns.
4. `src/parity.rs`: cross-lang parity already maps Sky-kernel → Haskell/Go/Rust;
   extend the Rust side to point at OUR `crates/` impls (Ipê) alongside the
   reference's `runtime-rust/` so `parity` shows Ipê-vs-reference gaps.
5. `cargo build --release`; swap: point the autopilot SHIMDIR `ipe-index`
   symlink (scripts/progressive-development/autopilot.sh) at the Rust bin;
   retire `scripts/ipe-index` (python v0).
6. Hooks: this repo's `.git/hooks/post-commit` already runs `ipe-index build`
   (swap to `ipe-index update`). Add the SAME hook to `../sky/.git/hooks/` so a
   Sky-upstream sync refreshes the reference side.

## Phase gate (per sky-rust-is-ipe-ancestor plan)
Until Ipê goes green + FFI complete: run BOTH skydex (reference) and ipe-index.
After: drop skydex, ipe-index only.
