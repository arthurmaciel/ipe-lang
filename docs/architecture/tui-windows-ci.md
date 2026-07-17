# Sky.Tui on the Windows example sweep — RUN-vs-SKIP design

Status: design-only. No code, no build. Cross-reference:
`docs/architecture/ci-and-hosting.md` and the forthcoming
`docs/architecture/windows-ci-support.md` (this document is the
Sky.Tui-specific section of the Windows CI story; keep the two in
sync when either changes).

Guardian synthesis of a 3-reasoner design swarm + 3 cross-critique
rounds. The RUN-vs-SKIP verdict is grounded in a source trace of the
render path, independently reproduced three times, and re-verified
against HEAD.

---

## 0. Verdict (one line)

**Windows `windows-latest` Sky.Tui = `tui-build` RUN + `tui-render`
RUN (headless `element_to_cells`) + `tui-loop` SKIP (loud, ConPTY
unwired).** This strictly **exceeds** `../sky`, which SKIPs *all*
tui on Windows. The render RUN is warranted by a proven-pure render
path, not by optimism, and is held pure over time by a mechanical
fence.

Priority frame (security > correctness > soundness > efficiency >
completeness > readability): the RUN is a completeness win over
`../sky`; the fence + separate-check wiring exist so that win can
never become a **false-green** (a soundness/correctness failure of
the reporting layer), which is the single outcome this design makes
unreachable.

---

## 1. Why our surface differs from `../sky`

`../sky` skips Windows tui for a mechanical reason, not a semantic
one. Its tui check is **pty-driven**: `exercise_tui`
(`../sky/runtime-rust/scripts/lib/checks.sh:311`) spawns the built
example binary under a real terminal (`script` on Unix, `winpty` on
Windows) and drives it. On `windows-latest` `winpty` needs an
interactive console that a headless runner lacks, and the real
headless path (ConPTY / node-pty) is unwired — so
`examples-sweep.yml:24-25` + `checks.sh:324-332` return
`EXERCISE_SKIP_RC=125` and record a green-neutral SKIP for the whole
tui shape.

`../sky` had **one** tui test mechanism (pty), so when the pty was
unavailable the whole capability skipped. Our Sky.Tui surface has
**two tiers**, and only one of them needs a terminal:

| Tier | Entry | Needs TTY / pty? |
|---|---|---|
| (a) Interactive loop | `tui::app::{tui_app, tui_app_ui}` → `TuiGuard::enter` (`enable_raw_mode`, alternate screen, `terminal::size`) | **Yes** — a real console. Impossible headless on *any* OS. |
| (b) Headless render | `tui::layout::element_to_cells(view, cols, rows) -> String` | **No** — pure layout math. |

Because tier (b) exists as a directly-callable pure function with a
standing headless test, we can RUN the render half on Windows where
`../sky` could only SKIP. `../sky` never had a pty-free render entry
point to fall back to; we do.

---

## 2. The load-bearing claim — `element_to_cells` purity (traced)

`element_to_cells` (`runtime/src/sky_runtime/tui/layout.rs:2426`)
has signature `element_to_cells<M: Clone>(view: &Element<M>, cols:
usize, rows: usize) -> String`. Cols/rows are **caller-supplied
arguments**, never auto-detected. The transitive render cone and its
OS surface:

| File | Imports (render-relevant) | Console / TTY / IO touch |
|---|---|---|
| `layout.rs` (`element_to_cells`, `render_with_focus`) | `html`, `ui`, `cell::sanitize_rune`, `focus::{Focusable,InputRegistry}`, `unicode_width` | **None.** `rg -c crossterm layout.rs` = 0 (re-verified HEAD). |
| `cell.rs` (Cell/Grid/`sanitize_rune`) | `unicode_width` only | **None.** Module doc: "Pure (no terminal I/O)". |
| `focus.rs` (`Focusable`/`InputRegistry`) | `html::{Attribute,Event}`, `std::collections::HashMap` | **None.** Pure state; no key-read, no terminal poll. |

`crossterm` appears in `tui/` in exactly two places, **both off the
render cone**: `app.rs` (`enable_raw_mode` / `disable_raw_mode` /
`terminal::size` — the interactive loop, app.rs:60/80/147/194) and a
`//!` comment in `diff.rs`. `layout.rs` does **not** import `diff`,
so the render path never reaches the diff-flush's crossterm writes.

The frame's ANSI (SGR sequences like `\x1b[7`, `38;2;255;0;0`) is
**hand-built string formatting inside `layout.rs`**, not crossterm
output. Confirmed by the standing test `tui_headless_render_contains_count`
(`runtime/src/lib.rs:214-242`, gated `#[cfg(all(test, feature =
"tui"))]`), which calls `element_to_cells(&elem, 80, 24)` with
literal dims and asserts `frame.contains('0')` — no TTY, no pty, no
console.

**Conclusion:** `element_to_cells` is referentially transparent:
`fn(&Element, cols, rows) -> String`, allocation-only, deterministic.
Same tree + same `(cols, rows)` → same bytes on Linux, macOS, and
Windows. It RUNs on `windows-latest` exactly as on Linux.

No silent-degradation trap: there is **no** `is_terminal` / `isatty`
/ `TERM` guard anywhere in the render cone (those live only in
`app.rs`). So the render cannot return a degraded off-TTY frame — it
always produces the full frame, and a broken render fails the
assertion loudly rather than passing by emptiness.

---

## 3. crossterm on Windows MSVC — builds, and the render never calls it

`tui = ["unicode-width", "crossterm", "tokio"]`
(`runtime/Cargo.toml:93`). crossterm is a hard link dependency of
the feature; it is **not** target-gated to unix (contrast the
`[target.'cfg(unix)']` block elsewhere in the manifest).

- **Build:** crossterm 0.28 is first-class on Windows (its Console
  API backend via `windows-sys` FFI — no C toolchain, no vcpkg).
  `cargo build --features tui` and `cargo test --features tui` link
  cleanly on `windows-latest` MSVC. This is a **real** Windows-only
  gate that `../sky` never exercised for tui.
- **Render:** `element_to_cells` calls **zero** crossterm symbols
  (§2). crossterm is *linked into* the test binary because the
  feature is monolithic (`unicode-width` is gated behind `tui`
  alongside `crossterm`), but **linked ≠ invoked**.
- **The stronger, required premise (not merely "linked ≠ invoked"):**
  crossterm 0.28 performs **no console I/O at link/load time** —
  there is no static initializer / constructor that probes a
  console when the DLL loads. The same premise applies to `tokio`,
  the feature's other link-time dep (`Cargo.toml:93`) linked into
  the render test binary: it does nothing at load without a
  constructed runtime, and `element_to_cells` builds none — so it is
  fine, but it shares the load-time-inertness premise. Every
  Console-API call is an explicit function call confined to
  `app.rs`. Therefore a headless Windows test binary that links
  crossterm + tokio but only calls `element_to_cells` performs no
  console I/O and cannot hang or panic on a missing console. This is
  the premise that makes the RUN sound; it must be restated (not
  weakened to "linked ≠ invoked") whenever the crossterm **or
  tokio** version is bumped, because a future static ctor would be a
  distinct, real risk.

Bright line for reviewers: **crossterm is a link-time dep of the
`tui` feature but a runtime dep only of `app.rs`'s interactive
loop.** The render test needs the crate present; it never needs a
live console.

---

## 4. The full `Tui.app` loop — honest scope

The loop enters via `TuiGuard::enter{,_mouse}` → `enable_raw_mode`
+ alternate screen + `std::io::stdin` (app.rs:80). That mandates a
**real TTY**, which a headless CI runner lacks on **every** OS:

- Linux: pty fabricated via `script` → loop RUN (reference host).
- macOS: pty via BSD `script` → loop RUN.
- Windows: no working headless pty (`winpty` needs an interactive
  console; ConPTY / node-pty unwired) → loop **SKIP**.

When forced headless, the loop degrades to a clean `Task Error`
(`enable_raw_mode()` is called at app.rs:80 and its `Err` is mapped
via `.map_err(...)`; the separate `TERM=dumb` refusal lives at
app.rs:266-276) — fail-loud, no panic.

**Honesty statement (bake into the sweep legend and the doc):** the
Windows tui RUN proof covers the **render half only**
(`element_to_cells`), never the interactive update/key/focus loop.
The loop half is `build_only` on Windows — identical to its ceiling
on *every* headless host. Do not let a green Windows render row be
read as "the Tui loop is verified on Windows." No CI verifies the
interactive loop without a pty; the Windows SKIP of the loop is
**honest parity with Linux's own inability**, not a capability gap.

---

## 5. Harness wiring

### 5.1 Structural rule — split the checks, never overload `exercise_tui`

`exercise_tui` (`checks.sh:311`) is the pty loop-driver and returns
`EXERCISE_SKIP_RC=125` (green-neutral) on Windows. The render RUN
**must not** be routed through it: folding render into a path that
SKIPs=green on Windows would report a *broken render as green
forever* — the primary false-green this design forbids. Keep three
distinct checks:

```
windows-latest, shell: bash (Git Bash), IPE_HOST_OS=windows:

  tui-build   RUN  — skyc emit (assert exit 0, no panic / no CompilerBug)
                     + `cargo build --features tui` (MSVC; crossterm + tokio link).
                     Failure = RED. Never downgraded to SKIP, even on a toolchain /
                     image build break (that is a real Windows regression with a loud
                     correct signal). tui has NO stub (contrast webview) — build_only
                     compiles the real render+loop code; if a stub is ever added,
                     build_only must additionally assert the non-stub module compiled.

  tui-render  RUN  — `cargo test --features tui tui_headless` (runtime crate;
                     `element_to_cells` / `render_with_focus`, pty + console-free).
                     Returns 0 (pass) / 1 (fail) ONLY. 125 is FORBIDDEN for this check.
                     Failure = RED. Runs under Git Bash as a plain `cargo test`
                     invocation — OS-independent exit-code propagation, no shell
                     plumbing. THIS is what exceeds ../sky.

  tui-loop    SKIP — interactive Tui.app pty smoke via exercise_tui (UNCHANGED;
                     EXERCISE_SKIP_RC=125, loud message: "windows: headless pty needs
                     ConPTY/node-pty — not yet wired"). Same physical limit as every
                     headless host.

  invariant   — render-cone purity fence (§5.3). Violation = RED.
```

`IPE_HOST_OS=windows` already resolves via `$OSTYPE`
(`msys*|cygwin*|win*`) in `checks.sh`; no extra detection needed.

### 5.2 RUN scope — per-host capability, not per-example (be precise)

The sweep exercises the **emitted example binary**, whose only entry
is the `app.rs` loop (render via `render_with_focus` *inside* the
`TuiGuard`-gated loop). The emitted binary has **no headless entry
into the render path**. Therefore `element_to_cells` is reachable
today only from the **runtime crate's own test harness**.

Consequence — the honest scope of `tui-render` RUN:

> The `tui_headless` `cargo test` proves the **runtime render
> engine** executes headless on Windows MSVC and asserts frame
> content — a **per-host capability proof**. It does **not** diff
> each emitted example's initial frame.

This scope line MUST appear in the sweep legend so the RUN row does
not over-claim per-example coverage (a reporting-layer false-green).

### 5.3 The purity fence — parse, don't validate

The RUN verdict is sound only while the render cone stays
console-free. It must be held mechanically, not by reviewer
vigilance.

- **Preferred (structural / compile-enforced):** split the feature
  so the render path cannot *name* crossterm. Introduce `tui-render
  = ["unicode-width"]` and redefine `tui = ["tui-render",
  "crossterm", "tokio"]`; build the pure render modules
  (`layout.rs` / `cell.rs` / `focus.rs`) so they compile under
  `tui-render` **without** pulling crossterm / tokio. A crossterm
  call sneaking into the render path then becomes a **compile
  error**, and the render-proof's trust surface no longer links
  crossterm at all. This is the parse-don't-validate form: purity
  is a build fact, not a text search.
- **Interim bridge (weaker, allowed until the split lands):** a CI
  assertion that the render cone references zero `crossterm` /
  `std::io::std{in,out}` tokens. The fenced set MUST be the **full**
  render cone, not just its top three files: `layout.rs:19-20`
  imports `super::super::html::Html` and `super::super::ui::{...}`,
  so the cone's leaves include `html.rs` + `ui.rs` (both currently
  crossterm / `std::io`-clean, 0 hits). A grep scoped to only
  `{layout.rs, cell.rs, focus.rs}` would miss a crossterm /
  `std::io` token added to `html.rs` or `ui.rs` and is therefore
  **knowingly cone-incomplete**. So either (a) extend the interim
  fenced set to `{layout.rs, cell.rs, focus.rs, html.rs, ui.rs}` —
  the full cone, still excluding `key.rs` (input decoding off the
  `element_to_cells` graph) — or (b) if the set is left at the three
  files, label it explicitly as cone-incomplete pending the
  structural split. Prefer a build-time assertion compiled into the
  runtime test suite over a sweep-level grep, so a CI-config change
  cannot silently drop the guard. Either way, label it explicitly as
  a weaker validation scheduled for replacement by the feature
  split — which closes the gap **permanently**, because under
  `tui-render` the render modules (including `html.rs` / `ui.rs`)
  must compile crossterm-free or the build fails.

---

## 6. Three failure modes, and how each is closed

1. **False-green (render folded into the pty path).** A broken
   render reported as SKIP=green because it rode `exercise_tui`.
   *Closed:* `tui-render` is a separate check returning 0/1 only;
   125 forbidden. Highest-priority structural constraint.
2. **False-green (reporting over-claim).** A bare `tui RUN` row read
   as interactive-loop or per-example coverage. *Closed:* row reads
   `tui-render: RUN (render engine, per-host)` + `tui-loop: SKIP`
   as **two distinct sub-rows** (§7); build success alone never
   yields RUN — the render assertion must fire.
3. **Under-report (blanket SKIP mirroring `../sky`).** Copying
   `../sky`'s tui-SKIP discards a proven pty-free capability and
   leaves the one OS-independent-correctness half of tui with zero
   Windows coverage. *Closed:* RUN the render; the §2 trace
   disproves the need to skip.
4. **Hidden-console-dep regression.** A future edit moving
   `terminal::size()` (or any console call) into `layout.rs` to
   auto-detect width would silently give the render a TTY
   dependency and hang/false-fail on headless Windows. *Closed:*
   the §5.3 fence (structural preferred) makes it a compile error /
   loud RED.
5. **Build break masquerading as SKIP.** A future `windows-latest`
   image where crossterm/tokio fail to compile under `--features
   tui`. *Closed:* that is `tui-build` RED, never a downgrade to
   SKIP.

---

## 7. Reporting granularity

Emit **two distinct tui sub-rows** for Windows, not one averaged
`tui` cell:

```
tui-render : RUN   (headless element_to_cells; per-host capability; RED on fail)
tui-loop   : SKIP  (interactive pty smoke; ConPTY unwired; loud, green-neutral)
```

so the capability that exceeds `../sky` is **visible** and a render
regression is **loud**, not hidden inside a single ambiguous status.

Legend line:

> *Windows tui: build + headless render-engine RUN (pure
> `element_to_cells` / `render_with_focus`, no pty — per-host
> capability proof, per-example frame not diffed); interactive loop
> SKIP (ConPTY unwired). Render RUN is not interactive-loop
> verification.*

---

## 8. Open decisions

- **OPEN-1 (fence mechanism).** Ship the interim import-fence
  immediately, or block the Windows tui RUN on landing the
  structural `tui-render` sub-feature split first? Recommendation:
  ship RUN now behind the interim fence; file the split as the
  correct permanent guard. The RUN is sound today on the interim
  fence because §3's no-static-ctor premise holds at the pinned
  crossterm 0.28.
- **OPEN-2 (per-example render coverage — `IPE_TUI_RENDER_ONCE`).**
  Whether to add a headless one-shot to `tui_app` / `tui_app_ui`
  that early-returns before `TuiGuard::enter`, computes `init →
  render_with_focus(view(model)) → print frame → exit(0)`, giving
  **per-example** Windows render diffs. **Contested — do not adopt
  as specified.** Guardian objections that must be resolved before
  it lands:
  - It adds a control-flow divergence (a hidden `exit(0)` path) and
    an env-string branch to the **shipped desktop binary** — a
    representable invalid state ("rendered-once-and-quit") and a
    validate-don't-parse env gate deep in the production driver.
    **Requirement:** gate it `#[cfg(test)]` / harness-only so it
    **cannot exist in a release binary**; a plain env gate on the
    shipped driver is rejected.
  - Cols/rows source is a latent console leak: it MUST come from the
    Tui cfg's `canvasWidth`/`canvasHeight` (or a fixed
    `IPE_TUI_RENDER_COLS`/`ROWS`), **never**
    `crossterm::terminal::size()`.
  - The early-return MUST be provably the **first** statement of the
    driver, structurally before **any** crossterm call (including
    `terminal::size()`), and its own test must assert it yields a
    **frame, not an `Err`**, with stdin redirected from `/dev/null`.
  Until these are met, `tui-render` stays a per-host capability
  proof (§5.2) and the completeness increment is deferred — it is
  not worth a shipped control-flow divergence (completeness is the
  second-lowest principle).
- **OPEN-3 (verdict fallback).** Default is **RUN (render) +
  build_only (loop)**. The single observation that would flip
  `tui-render` to a conservative SKIP: crossterm/tokio prove
  **unbuildable or console-probing at link/load** on `windows-latest`
  MSVC. Absent that, RUN is the correct call (completeness win);
  SKIP would be the safe-but-under-reporting fallback. False-green
  (RUN asserted while the render is console-contaminated) is the one
  outcome made unreachable by §5.3 + §5.1.

---

## 9. Ground-truth references

- `runtime/src/sky_runtime/tui/layout.rs:2426` — `element_to_cells`
  signature (explicit cols/rows); render cone crossterm-free
  (`rg -c crossterm` = 0, re-verified HEAD).
- `runtime/src/sky_runtime/tui/cell.rs`, `focus.rs` — pure; no
  crossterm / `std::io`.
- `runtime/src/sky_runtime/tui/app.rs:60/80/147/194` — the only
  console surface (`TuiGuard`, `enable_raw_mode`, `terminal::size`).
- `runtime/src/sky_runtime/tui/diff.rs` — crossterm named only in a
  comment; not on the render cone; not imported by `layout.rs`.
- `runtime/src/lib.rs:214-242` — `tui_headless_render_contains_count`,
  the standing headless render proof (the `tui-render` RUN vehicle).
- `runtime/Cargo.toml:93` — `tui = ["unicode-width", "crossterm",
  "tokio"]` (monolith; crossterm NOT unix-gated; the split target
  for §5.3).
- `../sky/.github/workflows/examples-sweep.yml:24-25` and
  `../sky/runtime-rust/scripts/lib/checks.sh:71-77/295-332` —
  `../sky`'s pty-only `exercise_tui` and its `EXERCISE_SKIP_RC=125`
  Windows SKIP.
