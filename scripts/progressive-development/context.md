# Progressive Development — operating contract

**This file is the loop's COMPLETE instruction set.** The repo's large
`CLAUDE.md` is written for interactive general-purpose work and is NOT your
contract — this is. Where they conflict, follow this. Everything else you need
is `backlog.md` + the `../sky` reference + the code you touch.

(Distilled from `CLAUDE.md` §7 core-principles + §8 non-regression + the seal.
If the human meant a different specific subset, they will edit this file.)

## 0. Above all — never traded for speed or tokens
**Security > correctness > soundness > everything else.** If a fix would weaken
any of these, STOP and escalate. Do not land it. This outranks "make progress".

## 1. The seal (the core mandate)
**No exit-0-then-cargo-fail.** If `skyc` accepts a program (exit 0), the emitted
Rust MUST `cargo build`. Never emit codegen that type-checks in skyc but fails
`cargo`. Closing that gap is the point of most mechanical items.

## 2. The six principles (CLAUDE.md §7)
1. **If it compiles, it works** — no runtime panic from well-typed Sky. Every panic class has a regression test.
2. **Root-cause only** — never suppress a type error or warning; a defensive cover-up that hides a contract violation IS a violation.
3. **Match the reference** — Go/Haskell (`../sky`) parity is the default. Diverge ONLY where strictly better (Rust/Unicode/modern) AND recorded in `docs/divergences-from-sky.md`. A hack is never a "divergence".
4. **Production-grade + maintainable** — scales to Stripe-SDK-scale FFI; stays clean.
5. **Typed effects & secrets** — effects are `Task Error a`; no `Result String`/`Task String` in public surfaces; secrets are typed, never `fmt`-stringified.
6. **Exhaustive wiring** — new AST/IR/kernel work updates EVERY match arm the compiler enforces; no `_ ->` catch-all cover-ups. Lean on the type checker to prove you got them all.

## 3. The two rules
1. **Test-first** — every new feature/bug becomes a regression test before the fix lands; the failing test is the discovery artefact.
2. **No deferral** — "pre-existing" / "known edge case" is never a shipping excuse. Root-cause it, or escalate it; never paper over it.

## 4. Resource non-negotiables (loop preconditions)
`mem-guard.sh` MUST be running. Every build/test wrapped in `timeout`. No
background processes (`&`/`nohup`). Abort if free disk < 15 GB.

## 5. Boundary
Only the Rust-port surface: `crates/`, `runtime/`, sky-stdlib compiled-source,
`examples/` fixtures, `docs/`. `../sky` is READ-ONLY reference — never edit the
Haskell/Go backend or upstream.

## 6. The gate (the only thing that authorises a commit)
```
touch runtime/tests/*.rs crates/skyc/tests/*.rs
CARGO_TARGET_DIR="${MASTER_GATE_TARGET:-$HOME/.cache/master-gate-target}" timeout 3000 cargo test --workspace
CARGO_TARGET_DIR="${MASTER_GATE_TARGET:-$HOME/.cache/master-gate-target}" timeout 1200 cargo clippy --workspace --all-targets -- -D warnings
```
Both exit 0. For a sweep blocker, also rebuild the example with the fresh `skyc`
and confirm the original diagnostic is gone (note the new blocker). Green →
commit. Red → `git reset --hard` + log the reason. The tree only advances.

## 7. Output style — caveman-ultra (mandatory)
Your prose is watched live. Be EXTREMELY terse. Drop articles, filler, hedging,
pleasantries. Fragments fine. One line where one line does. No emojis. No
preamble ("I'll now…", "Let me…", "Sure"). State the action or the finding, not
the intent. Code, paths, identifiers, and error text stay EXACT and verbatim —
never abbreviate those. Terseness never trades away correctness, the gate, or a
required verdict line. Final line is always the verdict (LANDED / ESCALATED /
DONE / REVIEW: … / TRIAGE: … / AUDIT: …).
