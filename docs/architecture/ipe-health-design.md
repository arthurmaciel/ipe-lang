# `ipe health` — environment diagnostics and consent-gated build-optimization setup

A diagnostic-and-guided-setup command in the tradition of `flutter doctor` /
`brew doctor`, but precise because the compiler knows exactly what its own
emit + cargo pipeline needs. It has two halves with a hard wall between them:

1. a **read-only diagnosis** — grouped checks over the toolchain, linker,
   compilation cache, shared build target, sandbox prerequisites, and disk;
2. a **consent-gated apply engine** — for each fixable finding, show exactly
   what the fix will do (a diff for a config edit, the exact command for an
   install), then apply it only on explicit consent.

Health is the opt-in, consent-based alternative to the compiler unilaterally
managing build optimizations: the ipe-managed shared `CARGO_TARGET_DIR` (the
S2 strategy of `precompiled-runtime-and-shared-target.md`) ships as a
health-offered setup, never a default-on behavior. Everything health can
apply is an Efficiency aid under the strict precedence Security > Correctness
> Soundness > Efficiency; no fix may ever weaken anything above Efficiency.

## Command model

Every fenced block in this document is an illustrative design shape — the
command surface, schemas, and code sketches specified here do not exist until
the implementation plan below lands; none is runnable today.

```
ipe health              # report; on a TTY, per-item consent prompts follow
ipe health --yes | -y   # report + apply every fix, no prompts (provisioning/CI)
ipe health --plain      # unframed scriptable text; data only; never mutates
ipe health --json       # structured output; data only; never mutates
```

Human-vs-machine is TTY-driven, not flag-driven, reusing the conventions of
`src/ipe-cli/src/style.rs` (`IsTerminal`, `Palette::for_stream`, `gutter`,
`frame`) and `src/ipe-cli/src/cli_args.rs` (`OutputFormat`, `split_format`):

- **Interactive TTY** (stdin *and* stdout are terminals): print the framed,
  guttered, grouped report; then, for each fixable item, show the preview and
  ask `[Y/n]` with **default Yes**. Per-item prompts keep consent honest — a
  user can accept "install mold" while declining "edit my cargo config".
- **`--yes` / `-y`**: the same apply pass without prompts. Every preview is
  still printed before its apply — transparency is preserved, only the
  keystroke is waived. Intended for provisioning scripts and CI images.
- **`--plain` / `--json`**: pure data, flush-left, never framed, never
  coloured, **never mutating** — the CLI's existing machine-output
  convention. Combining either with `--yes` is a usage error (a machine mode
  that mutates is a contradiction; refuse rather than pick a winner).
- **Non-TTY without flags** (piped/redirected): the report and the exit code
  only. Never prompt into a pipe. A trailing hint names the two ways to get
  fixes applied: run interactively, or pass `--yes`.

Wiring: a new `src/ipe-cli/src/health.rs` module; one dispatch arm in
`run_cli` (`src/ipe-cli/src/lib.rs`, the `match args.split_first()` table)
via `with_help_on_misuse("health", health::run_health(rest))`; one entry in
the `COMMANDS` table of `src/ipe-cli/src/help.rs`. Format flags parse through
`cli_args::split_format` exactly like `capabilities` / `version` / `diff`;
`--yes`/`-y` parses in the health-specific tail with the same
reject-duplicates discipline (`set_once` style).

### Report structure

Six groups, fixed order: **Toolchain, Linker, Cache, Target, Sandbox, Disk.**
Each check renders one line: a status glyph, the check name, a short detail
(what was found, where), and — when applicable — a one-line suggestion.

Statuses:

| Status | Meaning | Glyph |
|---|---|---|
| `ok` | present and correctly configured | `glyph::OK` (green) |
| `warn` | usable but improvable; a fix exists | `!` (yellow) |
| `missing` | absent or broken | `glyph::FAIL` (red) |
| `unknown` | the check cannot currently be performed; explicitly labelled, never rendered as ok | `?` (dim) |

`warn`/`unknown` need two glyphs `style.rs` does not yet carry. They join the
`glyph` module as SSOT constants; whether the installer must mirror them is
decided by the existing drift test's scope (`tests/install_style_drift.rs`)
— if the installer never renders them, they are CLI-only constants and the
drift test is untouched.

### Exit-code policy

- **0** — no *critical* check is `missing` (warn/unknown do not fail the
  command; declined fixes on non-critical items do not fail it either).
- **non-zero** — at least one critical check is `missing`. Implemented as a
  typed `CliError::HealthCritical { missing: Vec<CheckId> }` added to the
  `main.rs` pass-through list (the variants that render their own complete
  screen), so the report prints once and the process exits `FAILURE`.

When fixes were applied, detection re-runs after the apply pass and the exit
code reflects the **post-apply** state.

### `--plain` shape

One record per check, flush-left, tab-separated, stable field order:

```
<group>\t<check-id>\t<status>\t<critical|optional>\t<detail>
```

### `--json` shape

A stable, versioned schema (the convention set by `capabilities --json`):

```json
{
  "health": 1,
  "platform": { "os": "linux", "arch": "x86_64" },
  "groups": [
    { "name": "toolchain",
      "checks": [
        { "id": "rustc-cargo", "status": "ok", "critical": true,
          "detail": "cargo 1.88 at /home/u/.cargo/bin/cargo",
          "suggestion": null, "fix_available": false } ] }
  ],
  "critical_missing": 0
}
```

`--plain`/`--json` are produced by the same check evaluation as the human
report — one detection pass, three renderers.

## Checks

Every check is a **read-only** detector: a pure function over an injected
environment probe (PATH lookup, file reads, command runner), so each is unit
testable with a mocked environment on any host. Detectors never mutate; only
the apply engine does.

| Id | Group | Detects | Method | Fix | Critical |
|---|---|---|---|---|---|
| `ipe-version` | Toolchain | running `ipe` vs the latest release | running side: `CARGO_PKG_VERSION`. Latest side: **no release/version feed exists yet** → status `unknown`, labelled "latest-version check unavailable" — never fake-green | none offered; the detail names `ipe upgrade` (POSIX, shipped) as the manual refresh | no |
| `rustc-cargo` | Toolchain | Rust toolchain present + version floor | reuse `src/ipe-cli/src/toolchain.rs` — the pure `resolve` over PATH + `known_install_dirs`, distinguishing `Disposition::NotInstalled` from `NotOnPath { found_in }`; then `cargo --version` / `rustc --version` against the floor implied by the workspace edition (`rust-toolchain.toml` channel = stable) | suggestion-only: the rustup install command (or PATH export for `NotOnPath`) is printed, never executed (see the pipe-to-shell rule) | **yes** |
| `runtime-crate` | Toolchain | the runtime dependency crate resolvable | run the same typed resolution the build path uses (`RuntimeCrate`: name + exact version verified), over the home-layout contract below — `$IPE_RUNTIME_DIR`, the materialized `$IPE_HOME` copy, the in-repo walk | when the home layout exists but its materialized runtime is absent/stale: offer the layout's own re-materialization (writes only under `$IPE_HOME`); otherwise suggestion-only | **yes** |
| `linker-tool` | Linker | a fast linker available for this OS/arch | Linux: `mold` (else `ld.lld`) on PATH; macOS: `ld.lld` (Homebrew llvm) on PATH, noting recent Xcode's default is already fast; Windows: bundled `rust-lld` (rustc version probe); FreeBSD: lld **is** the system default → `ok` with nothing to do | consented install per the platform matrix | no |
| `linker-config` | Linker | cargo wired to the fast linker | parse `~/.cargo/config.toml` for the per-target `rustflags` link-arg line (and honor an equivalent `RUSTFLAGS` env as configured); skipped (`ok`, "system default") on FreeBSD | consented **config edit** to `~/.cargo/config.toml`: diff shown, backup taken, idempotent | no |
| `sccache-tool` | Cache | `sccache` installed | `sccache --version` via PATH lookup | consented install (`cargo install sccache` or the platform package) | no |
| `sccache-config` | Cache | cargo wired to sccache | `[build] rustc-wrapper = "sccache"` in `~/.cargo/config.toml`, or `RUSTC_WRAPPER` set | consented config edit, same applier as `linker-config` | no |
| `shared-target` | Target | the ipe-managed shared `CARGO_TARGET_DIR` opted in | read ipe's own config (`$IPE_HOME/config.toml`) for the opt-in; report the resolved `$IPE_HOME/target/<key>` path and its size when on | the health-offered S2 setup (section below); writes only under `$IPE_HOME` | no |
| `sandbox` | Sandbox | the FFI build-jail / run-jail prerequisites for this platform | Linux: `bwrap` + `prlimit` on PATH (the exact tools `run_jail::RunJailTools` needs; the refusal message in `src/compiler/sandbox/src/run_jail.rs` names them); macOS: `sandbox-exec` (base system); Windows: Job Object + AppContainer arm is built-in — nothing installable; FreeBSD: `jail(8)` is base system — detail notes the privilege it requires | Linux: consented install of `bubblewrap` per the matrix; elsewhere informational | no — pure programs never need the jail, and a native-bearing run without one is already fail-closed at run time; health surfaces it early rather than gating on it |
| `disk` | Disk | free space where the shared target / build caches live | filesystem free-space probe at `$IPE_HOME` (statvfs on unix, `GetDiskFreeSpaceExW` on Windows, behind one small platform module) | none auto; suggestion names the biggest ipe-owned directories to reclaim manually | `warn` below a comfortable threshold (a shared-target epoch is 1–3 GB); `missing` + **critical** only below a hard floor where builds will fail |

**Deliberate exclusion — cranelift.** `docs/rust-perf-improvement.md` also
covers the cranelift debug backend, but it requires the nightly toolchain and
an `[unstable]` cargo table. Health never configures nightly-only or unstable
features; the perf doc remains the manual path. A check that cannot suggest a
stable fix does not appear.

### The `$IPE_HOME` contract health consumes

Health builds on the toolchain home layout established by the runtime
install-resolution work (the S3 install story): `ipe_home()` resolves
`$IPE_HOME` when set, else `~/.ipe`; the runtime source crate is materialized
under it and the typed `RuntimeCrate` resolver finds it there. Health adds
exactly two things to that layout, both created only on consent:

```
$IPE_HOME/
  config.toml        # ipe's own user-level config (the shared-target opt-in)
  target/<key>/      # the shared CARGO_TARGET_DIR epochs
```

Health does not define the layout; it reads the same resolution API the build
path uses, so the two can never disagree about where the runtime lives.

## Platform matrix

The suggestion and install command shown are selected by detected OS/arch —
a Linux user never sees a `brew` line. Install commands come from each tool's
official documentation (linked); the recipes match the verified ones in
`docs/rust-perf-improvement.md`.

| Tool | Detect | Linux | macOS | Windows | FreeBSD | Docs |
|---|---|---|---|---|---|---|
| rustup / cargo | `toolchain.rs` resolve + `cargo --version` | `curl … https://sh.rustup.rs \| sh` — **shown, never run** | same | `winget install Rustlang.Rustup` — shown, never run | `pkg install rust` or rustup — shown, never run | <https://rustup.rs> |
| mold | `mold --version` | `apt-get install mold` / `dnf install mold` / `pacman -S mold` (distro detected via `/etc/os-release`) | not available (mold does not target macOS → lld) | n/a (rust-lld bundled) | `pkg install mold` | <https://github.com/rui314/mold> |
| lld | `ld.lld --version` | distro `lld` package (fallback when mold unavailable) | `brew install llvm` | bundled `rust-lld` — detect only | system default — nothing to do | <https://lld.llvm.org/> |
| sccache | `sccache --version` | `cargo install sccache` (runnable) or distro package (shown) | `brew install sccache` | `winget install Mozilla.sccache` | `pkg install sccache` | <https://github.com/mozilla/sccache> |
| bubblewrap | `bwrap --version` | `apt-get install bubblewrap` / `dnf install bubblewrap` / `pacman -S bubblewrap` | n/a (`sandbox-exec` in base) | n/a (built-in jail arm) | n/a (`jail(8)` in base) | <https://github.com/containers/bubblewrap> |
| prlimit | `prlimit --version` | util-linux — present on effectively every distro; named if absent | n/a | n/a | n/a | util-linux |

**Elevation rule.** Health never elevates and never spawns `sudo`. Commands
that run as the invoking user (`cargo install`, `brew`, `winget`, `pkg` as
root on a root-administered box) are runnable on consent; commands that
require root on a normal desktop (`apt-get`, `dnf`, `pacman`) are **shown**
with a "run this yourself" note instead of executed. The preview line is the
suggestion either way, so declining costs the user nothing but a paste.

## The apply engine

### The typed `Fix` model

```rust
enum Status { Ok, Warn, Missing, Unknown }

struct Check {
    id: CheckId,               // closed enum, one per row above
    group: Group,              // Toolchain | Linker | Cache | Target | Sandbox | Disk
    status: Status,
    critical: bool,
    detail: String,
    fix: Option<Fix>,          // present only when a real, tested fix exists
}

enum Fix {
    /// A structured edit to a TOML config file.
    ConfigEdit { target: ConfigTarget, edit: TomlEdit },
    /// Run one program with an explicit argv (no shell).
    Install { argv: Vec<OsString>, docs_url: &'static str },
    /// A setup entirely inside $IPE_HOME (shared target, re-materialization).
    HomeSetup(HomeSetup),
}

/// The ONLY files a ConfigEdit can name — out-of-scope paths are
/// unrepresentable, not merely rejected.
enum ConfigTarget {
    IpeConfig,        // $IPE_HOME/config.toml
    CargoUserConfig,  // ~/.cargo/config.toml — an explicit, per-item-approved edit
}
```

Every `Fix` satisfies one contract:

- `preview()` — exactly what will change: a unified diff for a `ConfigEdit`
  (current file → proposed file), the exact argv line for an `Install`, the
  created paths for a `HomeSetup`. Shown **before** any prompt and before any
  `--yes` apply.
- `already_applied()` — the idempotency check: re-runs the corresponding
  detector. Applying an already-applied fix is a visible no-op ("already
  configured"), never a double-append.
- `apply()` — performs the change and returns a typed outcome; the check is
  re-detected afterwards so the final report shows reality, not intent.

### The consent loop

Runs only when stdin **and** stdout are terminals (the `IsTerminal` path).
For each check with a `fix`, in report order:

```
  ! mold is installed but cargo is not configured to use it
    The fix edits ~/.cargo/config.toml:

      [target.x86_64-unknown-linux-gnu]
    + rustflags = ["-C", "link-arg=-fuse-ld=mold"]

    A backup will be written to ~/.cargo/config.toml.ipe-health.bak
  Apply? [Y/n]
```

- Empty line or `y`/`yes` → apply (**default Yes**).
- `n`/`no` → skip, report "skipped", continue to the next item.
- EOF or a read error → treated as **no** for this and all remaining items
  (a broken stdin must never authorize a mutation). This is deliberately
  stricter than the visual default: default-Yes needs a live keystroke
  stream; silence from a closed pipe is not consent.

The existing `read_yes_no` (`src/ipe-cli/src/lib.rs`) defaults to No; health
adds a default-Yes sibling with the EOF-declines rule rather than changing
the shared helper — `ipe fix` edits the user's source (default-No is right
there), health previews an exact, reversible, backed-up change (default-Yes
is right here, and it is the command's specified contract).

`--yes` walks the same loop with the prompt replaced by an "applying" line.
The previews still print; the transcript of a `--yes` run is a complete audit
of what changed.

### The config-edit applier

- **Structured, not textual**: the edit is expressed against the parsed TOML
  (set `[build] rustc-wrapper`, set the per-target `rustflags` array),
  applied with a comment-and-formatting-preserving TOML editor. Idempotency
  is semantic — the key already holding the desired value is `already_applied`.
- **Conflict honesty**: a key that exists with a *different* value (the user
  already has a `rustc-wrapper`, or custom `rustflags`) is never overwritten
  silently — the check reports `warn` with the found value and the fix
  demotes to suggestion-only. Health augments configs; it does not fight
  their owner.
- **Backup, then atomic write**: the original is copied to
  `<file>.ipe-health.bak` (a numbered sibling if one exists — an older
  backup is never clobbered), the new content goes through the existing
  temp-file + rename `write_atomic` path, and the result is re-parsed; a
  write whose readback does not parse is rolled back from the backup.
- **Scope**: writes land only under `$IPE_HOME`, or in the one explicitly
  named, per-item-approved user file (`~/.cargo/config.toml`). The
  `ConfigTarget` enum makes any other destination unrepresentable; a runtime
  guard asserts the resolved path is the enum's own resolution before any
  write, as defense in depth.

### The install runner

- Spawns the exact previewed argv with `Command::new(program).args(…)` —
  **no shell**, no string interpolation, no environment-driven indirection.
- Stdio is inherited so the user watches their own package manager run.
- **Never pipe-to-shell**: anything whose official install is `curl … | sh`
  (rustup, the ipe installer itself) is print-only with the official URL.
  Health may run package managers; it never becomes one.
- A non-zero install exit marks the item "fix failed", keeps going, and
  surfaces in the final summary; it never aborts the remaining consented
  items and never affects the read-only findings.

## The shared-target setup

This is the shared build-once target of
`precompiled-runtime-and-shared-target.md` (S2) delivered as an **opt-in
health setup** rather than a compiler default — the non-invasive resolution
of the invasiveness concern recorded there. The mechanism is unchanged; only
the enablement moves.

- **What the fix does** (all under `$IPE_HOME`, previewed as a diff + created
  paths): writes `[build] target = "shared"` into `$IPE_HOME/config.toml`
  and creates `$IPE_HOME/target/`.
- **The key**: `$IPE_HOME/target/<epoch>` where `<epoch>` reuses the
  `derive_epoch` derivation in `src/ipe-cli/src/cache.rs` under its own
  domain tag — compiler revision × rustc toolchain fingerprint. The resolved
  feature set is deliberately not in the key: cargo hashes each unit's
  features into its fingerprint, so differently-featured builds coexist in
  one directory and share every unit whose resolved features coincide (the
  coexistence argument, verified empirically in the S2 design). If
  real-world thrash is ever observed, the retreat is the finer
  `(epoch, feature-set-hash)` key — one function.
- **How the build path consumes it**, first match wins:
  1. an explicit `CARGO_TARGET_DIR` in the user's environment — always
     respected, as the CLI already honors today;
  2. project `ipe.toml` `[build] target = "local" | "shared" | "<path>"`;
  3. `IPE_TARGET_DIR=<path>` / `IPE_TARGET=local` environment override;
  4. `$IPE_HOME/config.toml` `[build] target = "shared"` — **the health
     opt-in**;
  5. default: per-project `out/rust/target` (local), exactly as today.
- `ipe` sets `CARGO_TARGET_DIR` only in the environment of the child `cargo`
  process it spawns. Nothing of the user's own configuration is written for
  this — not `~/.cargo/config.toml`, not shell profiles.
- **Reversible**: set `target = "local"` in either config layer, or delete
  the key; the check's `ok` detail names the active path, its on-disk size,
  and the revert line. Health's fix only ever enables; disabling is a
  one-line edit the detail spells out.
- Stale-epoch reclaim is reported by the disk check (sizes per epoch);
  automated pruning is a separate, unadvertised-until-shipped follow-up.

## Honesty gates

Per the house rule — never advertise unimplemented:

- **"Run at install"** is future work: there is no packaging pipeline to hook
  yet. For now health is a manual command; the shell installer gains a
  closing "next: run `ipe health`" hint only when that wiring actually
  exists. Nothing in help or docs promises install-time execution.
- **The latest-version check** needs a release/version feed that does not
  exist. It ships as a clearly-labelled `unknown` ("latest-version check
  unavailable") — visible, honest, never fake-green, never silently dropped.
  When a feed exists, the check upgrades in place.
- **Suggestions name only shipped commands.** The disk suggestion points at
  concrete directories, not at a cache-cleaning subcommand that has not
  shipped; the version detail names `ipe upgrade` because it is implemented.
- A `fix` field exists only where the applier is real and tested; a check
  without a working fix renders its suggestion as prose, with nothing to
  consent to.

## Security

The apply engine is a trust surface — it edits user configuration and runs
installers. Its load-bearing properties:

- **Consent and transparency are the mechanism, not decoration**: every
  mutation is previewed exactly (diff or argv) before consent, per item;
  machine modes cannot mutate by construction (the apply engine is only
  reachable from the interactive and `--yes` paths — the renderer for
  `--plain`/`--json` never constructs it).
- **No security setting is ever weakened to optimize.** Health never touches
  the sandbox posture: it installs jail prerequisites, and it will never
  suggest, set, or preview an unsandboxed override or any confinement
  bypass.
- **Write scope is typed**: `ConfigTarget` admits exactly `$IPE_HOME` files
  and the explicitly-approved `~/.cargo/config.toml`; anything else is
  unrepresentable, with a runtime path assertion as depth.
- **No shell, no pipe-to-shell, no elevation**: installs are direct argv
  spawns of the previewed command; `curl | sh` installers are print-only;
  `sudo` is never spawned.
- **No silent loss**: backup before edit, atomic write, parse-verified
  readback, rollback on failure.
- **Fail-closed prompting**: EOF/read errors decline; a non-TTY stream is
  never prompted.

The engine requires a **security-soundness-guardian review before merge**
(the standing rule for trust-surface work), with the consent loop, the two
appliers, and the scope guard as the review's named subjects.

## Implementation plan

Test-first, ordered, each phase independently landable and green (fmt,
clippy, nextest). Detection is a pure core over an injected environment
probe, so every phase's tests run hermetically on any host.

1. **Substrate — command, model, renderers, exit codes.**
   *Failing tests first*: fixture check-sets render deterministically in
   human/plain/json modes (snapshots); `--plain --json` and `--yes --json`
   rejected via the `split_format` discipline; exit code 0 vs
   `HealthCritical` mapping; non-TTY runs print the hint and never read
   stdin. *Then*: `health.rs` with `Check`/`Status`/`Group`/`CheckId`, the
   three renderers on `style.rs`, the `run_cli` arm, the `CliError` variant +
   `main.rs` pass-through, the two new glyphs.
2. **Read-only detectors, per platform.**
   *Failing tests first*: one test module per detector with a mocked probe —
   PATH hit/miss, `Disposition::NotOnPath`, config-present/absent/conflicting,
   FreeBSD lld-default and jail-base cases, disk thresholds, the `unknown`
   version check. *Then*: the probe trait + real impls (PATH lookup, TOML
   read, version-command runner, statvfs/Windows free-space), each detector
   as a pure function, wired into the report.
3. **The apply engine.**
   *Failing tests first*: idempotency (second apply is a no-op),
   backup-created + numbered-backup-preserved, refusal on `n`, default-Yes on
   empty line, EOF declines all remaining, no prompt into a pipe, conflict
   demotes to suggestion-only, diff preview equals the applied change,
   out-of-scope write unreachable (type-level) + the runtime guard trips in a
   forged-path test, atomic rollback on parse-failing readback, install
   runner passes argv verbatim with no shell. *Then*: the `Fix` model, the
   consent loop, the TOML applier, the install runner, the `--yes` path.
   **Guardian review gates this phase's merge.**
4. **The shared-target setup.**
   *Failing tests first*: epoch-key derivation (reusing the `derive_epoch`
   components, own domain tag), the five-layer precedence chain resolved in
   order, opt-in honored by the build path's child-cargo environment,
   revert honored, coexistence smoke (two projects, one epoch dir, no shared
   dep compiled twice). *Then*: `$IPE_HOME/config.toml` read/write, the
   target-dir resolution module, the health check + `HomeSetup` fix.
5. **Docs and help.**
   `help.rs` `COMMANDS` entry + section placement; the command's `--help`
   page; cross-links from `docs/rust-perf-improvement.md` ("health can set
   these up for you") and the sandbox docs; installer hint only if the
   install-time wiring lands. Includes correcting the stale
   `ipe health --fix` phrase in the `write_atomic` doc comment
   (`src/ipe-cli/src/lib.rs`) to name the real caller.

## Risks

- **Editing `~/.cargo/config.toml`** is the highest-trust action: exotic
  user configs, includes, or unusual formatting could confuse a structured
  editor. Mitigations: comment-preserving TOML editing, conflict-demotes-to-
  suggestion, diff preview, backup, atomic write, parse-verified readback —
  and the user can always decline and paste the snippet themselves.
- **Detection false negatives**: a linker configured via `RUSTFLAGS`, a
  project-local `.cargo/config.toml`, or a wrapper script can make a
  configured machine look unconfigured. Health checks the user-level file
  and the documented env vars and says exactly where it looked; a `warn`
  with an accurate "found X at Y" beats a guessed `ok`.
- **The elevation rule** limits Linux auto-install usefulness (apt/dnf/
  pacman are print-only). Accepted: a health that runs `sudo` is a worse
  trade than a paste.
- **Sequencing**: the runtime-crate check and the `$IPE_HOME` layout consume
  the install-resolution work; until it lands, health's Toolchain group
  codes against that contract and its tests mock the layout.
