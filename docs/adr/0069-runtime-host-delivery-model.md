Status: Accepted
Date: 2026-09-04

# 0069. Runtime × host delivery model — five shapes, two web runtimes, orthogonal axes

Supersedes: 0052 (terminal-shape-consolidation)

## Context

ADR 0052 consolidated the terminal shapes and fixed the entry-point surface at
four shapes. As the delivery model matured it became clear that two orthogonal
questions — what does `view` render to, and where do effects execute — were not
yet given a single clean account. The `WebView` shape, for instance, was a
shape where it was really a host (a delivery vehicle for the Web shape). And the
terminal shapes needed a cleaner split along their own axis. Together, these
meant the shape surface and the runtime/host vocabulary were entangled in ways
that made the `ipe release` grammar ambiguous and the library-admissibility rules
hard to state precisely.

## Decision

**Two orthogonal axes govern every program:**

| axis | question | outcome |
|---|---|---|
| **rendering class** | what does `view` produce? | DOM / cells / lines / http / none → **shape** |
| **effect locality** | does the loop run at native effects? | co-located vs sandboxed → **runtime** |

The shape is fixed by the head of `main` — the single source of truth. It is
never declared in `package.ipe`. The runtime (for the Web shape only) is chosen
at delivery time and constrained by the program's effect surface.

**Five shapes, each with one entry:**

| shape | entry | renders | runtime(s) |
|---|---|---|---|
| `web` | `Web.app` | DOM | `live` (default) or `spa` |
| `tui` | `Tui.app` | terminal cells | terminal |
| `cli` | `Cli.app` | terminal lines | terminal |
| `server` | `Server.listen` | HTTP | server |
| `script` | `main : Task Error ()` | nothing | native binary |

`Tui.app` is the renamed `Terminal.appScreen`; `Cli.app` is the renamed
`Terminal.appLines`. The `WebView` shape is retired — desktop-webview delivery
is the `web desktop` host under the `live` runtime (same TEA loop, same
diff/patch pipeline, over a local IPC bridge instead of SSE).

**Two web runtimes:**

- **`live`** — co-located server loop; the unnamed default. `web` alone means
  served live. `web desktop` means live over a local IPC bridge (webview-native).
  Direct native effects (`Ipe.Db`, `Ipe.File`) are available.
- **`spa`** — sandboxed client loop (wasm). Effects only via Web-API
  capabilities (`Ipe.Browser.*`) and HTTP to a backend. Hosts: browser
  (default), `ios`, `android`, `desktop` (webview-wasm).

**CLI grammar (illustrative):**

```
ipe (build | release | watch) [shape] [runtime] [host] [target] [--static]
```

- No args: shape from `main`, default runtime and host applied.
- `live` is never written — it is the default; typing it is a pedagogical error.
- `spa` is the only explicit runtime word.
- `web desktop` = live webview-native; `web spa desktop` = wasm in a wry shell.

**Library admissibility is the SSOT gate.** A single
`allowed_in(module, shape, runtime)` table in the compiler drives resolve, the
LSP, and diagnostics. A disallowed import is a compile error before any runtime
boundary can be violated.

Rejected alternatives:

- **Keep `WebView` as a shape.** Rejected: webview is a host (a delivery
  mechanism for the Web shape's output), not a rendering class. Treating it as a
  shape forced a fourth entry point that shared all of `Web`'s TEA machinery and
  diverged only in the packaging step — exactly what the host axis captures.
- **Keep `Terminal.appScreen` / `Terminal.appLines` under one `Terminal` module.**
  Rejected: the two entries have distinct rendering classes (cells vs lines),
  distinct stdlib admissibility (e.g., `Tui`-only `Ui.cells`), and distinct
  shapes in the `ipe release` grammar. Separate modules (`Tui`/`Cli`) make the
  distinction statically visible at import time and align with the shape names.
- **Express `live` as an explicit runtime word.** Rejected: naming the default
  creates two ways to say the same thing (`web` and `web live`); the unnamed
  default makes the common case shorter and self-evident.

## Consequences

- The `ipe release` grammar is unambiguous: shape is a validated positional,
  `spa` is the only runtime word, host tokens are shape-scoped.
- `WebView.app` is removed from the entry surface; desktop-webview programs
  use `Web.app` with `ipe release web desktop`.
- `Terminal.appScreen` and `Terminal.appLines` are removed; callsites use
  `Tui.app` and `Cli.app` respectively.
- The library-SSOT gate can now state admissibility rules in terms of the two
  axes, without per-shape special-casing for the host dimension.
- Invariant to preserve: shape is always inferred from `main`, never from
  config or a CLI flag used as a second source of truth. Any future
  medium-specific capability is added as a node gated by shape admissibility,
  not as a new entry point.
