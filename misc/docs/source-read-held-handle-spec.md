# Spec — one bounded, typed, held-handle source read

Crates: `ipe-cli` (`io_bounded`, `driver/build_pipeline`, `watch`, `project`),
`ipe_lsp_server` (`loader`, `lsp`). Principles: Security (1), Soundness (3,
bounded by construction). Lane tier: high (trust boundary).

## Class-closing property

Every byte of `.ipe` source, package manifest, or cache file that the
compiler, `ipe watch`, or the LSP consumes arrives through ONE read path:
- opened relative to a held parent directory handle, `O_NOFOLLOW`,
  non-blocking, file type proven regular on the opened fd (`fstat`), size
  capped by a typed limit error;
- reached only through a bounded discovery (the import closure for a loose
  file, the package tree for a package — never an unbounded directory crawl
  or unbounded ancestor walk);
- refused with one typed error (`CliError::SourceRefused { path, cause }`)
  that every consumer (build, watch, LSP) maps exhaustively.

With that property, no instance of "a read that follows a link / blocks on a
FIFO / reads unbounded / guesses the scope / renders an untyped error" is
representable.

## Why the instances exist (origin/main 983fdc404)

- `src/ipe-cli/src/io_bounded.rs:68` `read_to_string_capped`: plain
  `File::open` (follows symlinks, blocks on a FIFO, no file-type check). Cap is
  enforced (`take(max+1)`, `SOURCE_READ_CAP` at `:29`), nothing else is.
- `src/ipe-cli/src/driver/build_pipeline.rs:616` `find_manifest_for_ipe_file`:
  walks every ancestor to `/` looking for `package.ipe`; no ceiling.
- `build_with_sibling_discovery*` is still the loose-file build entry
  (`commands.rs` 801, 1092, 1292, 1377, 1469, 2718; `commands_pkg.rs:1366`;
  mirrored in `watch.rs:497`/`:563`) — scope is "the file's directory",
  not "the file's import closure".
- `src/lsp/server/src/loader.rs:40`: `pub struct LoadError { pub detail: String }`
  — untyped; the LSP classifies a load failure from prose.
- Absent on main: `SourceRefused`, `loose_file` module, `resolve_loose_file`,
  `CacheScope`, `ProjectRoot` threading, typed `LoadError`.

The fixes for most members were written and reviewed but never landed: they sit
on `origin/integration/lsp-loose` (71 ahead / 110 behind main, no PR ever
opened), with the same commits also on `lane/2863-lsp-loose-file`,
`lane/2920-loose-file-ssot`, `lane/2923-lsp-loose-residuals`,
`lane/2929-source-read-ssot`, `x/lsp-loose`, `x/2920`, `x/2929`. The issues
stay open because the code never reached main — not because the premise is
wrong.

## Members

| Issue | Status on main | Verified by |
|---|---|---|
| #2863 LSP loose file crawls parent dir | real; fix on lsp-loose | no `resolve_loose_file` on main; lane commit present |
| #2920 loose-file build/watch crawls recursively | real; fix on lsp-loose | `build_with_sibling_discovery*` call sites above |
| #2923 (items 2–4: `CacheScope`, case refusal, device names, typed `LoadError`) | real; fix on lsp-loose | `loader.rs:40` untyped; `CacheScope` absent |
| #2929 source reads: non-blocking regular-file open, `SourceRefused` | real; fix on lsp-loose | `io_bounded.rs:68` plain open |
| #2939 watch rescope R1–R4 | partly on lsp-loose (rescope follows closure); R2 mode-change, R3 failed-watch record still open | lane diff |
| #2943 bounded manifest walk-up, handle-held spelling check, exhaustive `LoadError` | real | `build_pipeline.rs:616`; LSP `load_error` has no `SourceRefused` arm |
| #2946 bounded cache read, per-level handles, refuse symlinked modules, one discovery for watch count | real | cache read uncapped on lsp-loose; intermediate levels by path |
| #2948 items 2–4 (capped raw reads, lstat error mapping) | item 1 FIXED (`api_surface.rs:99` `DiffError::Source(Box<CliError>)`, commit ee1cbaeb4, issue closed); items 2–4 fold here | `git grep` |

## Implementation plan

1. **Land the stranded work.** Rebase `integration/lsp-loose` onto current
   main (110 behind; expect conflicts in `commands.rs`, `watch.rs`, `ffi.rs`
   against the #2944/#2959 lanes). Keep only the source-read commits; the
   #2930 (warnings homing), #2955 (capture obligations), #2861, #2862 commits
   ride separately (see `diagnostic-homing-spec.md`,
   `auto-trait-classification-spec.md`; #2861 c090e5f81 and #2862 bfc20f2d9
   apply cleanly to main as-is).
2. **One opener.** Replace `read_to_string_capped(path, max)` with
   `read_source_at(dir: &HeldDir, name: &ModuleSegment, cap: ReadCap)` —
   `openat(O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC)`, `fstat` → regular file or
   `SourceRefused::NotRegular`, capped read → `SourceRefused::TooLarge`.
   `ReadCap` is an enum over the three existing caps (no ad-hoc numbers).
   Make the path-taking form `#[cfg(test)]` or delete it.
3. **Per-level handles** for the whole walk (`output_dir/held/unix.rs`
   already has `openat`/`mkdirat` primitives — reuse, do not duplicate). The
   case-spelling check (`is_spelled_on_disk`) runs on the held parent's
   listing, so the name checked is the name opened (#2943 TOCTOU).
4. **Bounded ancestor walk.** `find_manifest_for_ipe_file` stops at the first
   VCS root, the user's home, or a depth ceiling (declared const), whichever
   comes first. See LIMIT below for the policy on a foreign ancestor manifest.
5. **Symlinked module → typed refusal**, never a silent skip (#2946).
6. **One discovery** feeds build, watch count limit, and watch scope (#2946,
   #2939 R1/R2): watch derives its registered set from the same closure the
   build read; a mode change (manifest appears/disappears) forces full
   rescope; a failed `watch()` is surfaced, never recorded as watched (R3).
7. **Typed `LoadError`** in the LSP: an enum mirroring `CliError`'s source
   variants; `load_error` matches exhaustively (no `_`), `SourceRefused` →
   Refuse.

## Prove the refusals

- symlinked `.ipe` module → `SourceRefused::Symlink` (build, watch, LSP);
- FIFO named `Main.ipe` → refused without blocking (test has a timeout
  bound);
- a file one byte past each cap → `TooLarge`;
- an intermediate directory swapped for a symlink between two walk steps
  (test hook) → not followed;
- case-mismatched import on a case-insensitive FS → refused;
- Windows device name (`Aux.ipe`, `CON.ipe`) as entry → refused;
- a loose file in a directory with 10k unrelated files → only the import
  closure is read (count assertion);
- a planted ancestor `package.ipe` beyond the ceiling → not consulted.

## LIMIT

#2943 bullet 1 / #3010 overlap: what an ancestor `package.ipe` outside the
user's tree means (refuse / ignore / opt-in like git `safe.directory`) is a
maintainer policy decision. Decision needed: walk ceiling = VCS root, or
VCS root + explicit opt-in list. The spec implements the ceiling; the
opt-in is out of scope until decided.

Lane tier: high. Guardian: required (trust boundary, TOCTOU).
