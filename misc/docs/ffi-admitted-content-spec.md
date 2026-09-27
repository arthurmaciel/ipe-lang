# Spec — FFI trust boundary: admit once as held content, consume only admitted content

Crates: `ipe-cli` (`ffi.rs`, `owner_trust.rs`, `audit.rs`), `ipe_ffi`
(`driver.rs`). Principles: Security (1), defend in depth, parse-don't-validate.
Lane tier: high (trust boundary). SCEF-flag: yes.

## Class-closing property

Every FFI input that crosses from the filesystem into a build — a discovered
`package.ipe`, an FFI cache catalog, a wrapper crate's source tree — is
**admitted once** into a typed value that *owns the bytes* that were checked
(`AdmittedManifest { path, text }`, `AdmittedCatalog`, `AdmittedWrapper {
tree_hash, held_dir }`). Every consumer takes the admitted value; no consumer
re-reads by path. The FFI scope is computed once from a `ProjectRoot` and
threaded, never re-derived from a blame path.

Without it, each check-then-reopen site is a TOCTOU and each re-derivation a
second, driftable scope.

## Why the instances exist

- `admit_discovered_manifest` (`src/ipe-cli/src/owner_trust.rs:167`/`:200` on
  in-flight `origin/lane/2944-ffi-cache-trust`; absent on main) returns `()`;
  callers then re-read the manifest by path (#3008).
- `enforce_wrapper_capabilities` (`src/ipe-cli/src/ffi.rs:1452` main;
  `:1431` lane/2944, `:1496` lane/2959) and `audit.rs` read wrapper sources by
  path after the check (#3008).
- Wrapper crate capability scan runs at `ipe ffi install`; the build compiles
  the live `path =` dependency — the scanned bytes and the compiled bytes are
  different objects (#3012).
- `src/compiler/ffi/src/driver.rs:1044`: raw unbounded
  `std::fs::read_to_string(p)` of the legacy catalog (#2951 item 7).
- `watch.rs` (~1174, ~1297) calls `prepare_ffi(sources, &resolved.blame_path)`
  — a second scope computation beside the LSP's `ProjectRoot` (#2951 item 2).
- `find_cache_root` (`ffi.rs:73`) walks ancestors unbounded (#2923 item 1 /
  #2943).

## Members

| Issue | Status | Verified by |
|---|---|---|
| #3008 manifest admitted as held text; wrapper/audit reads | real; code is on in-flight lane/2944 | `git grep admit_discovered_manifest` only on lane/2944 |
| #3012 wrapper scanned at install, build compiles live dep | real | `enforce_wrapper_capabilities` scans at install only (main + lane/2959) |
| #2951 items 2, 3, 4, 6, 7 (ProjectRoot threading, loose entry read direct, bounded legacy read) | real | `driver.rs:1044`; watch `prepare_ffi` blame path |
| #2923 item 1 (FFI cache walk escapes to `/`) | real on main; fix on stranded lsp-loose | `ffi.rs:73` |
| #3009 Windows owner-SID check | real, FEATURE (non-unix refuses unconditionally — fail-closed, sound) | lane/2944 `cfg(not(unix))` arm |
| #3010 escape hatch for planted ancestor / loose-perm mounts | LIMIT (policy) | — |

Related in-flight (do not duplicate): #2944 (lane/2944-ffi-cache-trust),
#2959 (lane/2959-wrapper-jail). This spec starts after both land.

## Implementation plan

1. `AdmittedManifest { path: ProvenPath, text: String }` — produced only by
   `admit_discovered_manifest`, which opens `O_NOFOLLOW`, `fstat`s owner +
   mode + regular-file on the fd, reads capped (`SMALL_FILE_READ_CAP`) from the
   same fd. Private constructor. Every caller that today re-reads takes
   `&AdmittedManifest` (parse-don't-validate).
2. `AdmittedWrapper`: at install, copy the scanned tree into an ipe-owned
   held dir under the FFI cache (or record a content hash over the scanned
   files). At build, the emitted `Cargo.toml` `path =` points at the ipe-owned
   copy; or the hash is re-verified on held fds before cargo runs. Prefer the
   copy: the compiled bytes then *are* the scanned bytes.
3. Legacy catalog read (`driver.rs:1044`) goes through the capped, typed
   reader (`FFI_CACHE_READ_CAP`), `FfiPrepError::CatalogTooLarge`.
4. Thread `ProjectRoot` into `prepare_ffi`; delete the blame-path overload.
5. Bound `find_cache_root` with the same ceiling as the manifest walk
   (`source-read-held-handle-spec.md` step 4) — one ancestor-walk function.
6. Windows (#3009): owner-SID + DACL check through the `windows` crate's safe
   API only (`unsafe` forbidden); until then the unconditional refusal stays.

## Prove the refusals

- manifest swapped between admission and consumption (test hook) → the
  consumer sees the admitted text, not the swap;
- wrapper source edited after `ipe ffi install` → build refused (or builds
  the scanned copy; assert the edit is not compiled);
- legacy catalog one byte past cap → typed refusal;
- cache root walk past the ceiling → not consulted;
- Windows: manifest owned by another SID → refused (Windows CI).

## LIMIT

#3010 asks for an opt-out (like git `safe.directory`) for planted ancestor
manifests and 0777 mounts (WSL drvfs, uid-mapped mounts). Security says
fail-closed by default; ease-of-use says a documented stop. Decision needed:
(a) no escape hatch, improve error text only; (b) an explicit allowlist in the
user config, never in the project. Not specced until decided.

Lane tier: high. Guardian: required.
