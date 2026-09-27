# Spec — every write target is a proven, claimed area; reserved names from one table

Crates: `ipe-cli` (`output_dir.rs`, `output_dir/held`, `driver/build_pipeline.rs`,
`doc.rs`, `audit.rs`). Principles: Security (1), make invalid states
unrepresentable, SSOT. Lane tier: high (trust boundary). SCEF-flag: yes.

## Class-closing property

No compiler command writes to a directory except through a value that proves
it: `ProvenOutPath` (parent steps proven on held handles from a typed `Cwd`),
then `OwnedDir::claim` / `AreaClaim` (ownership marker on the held fd). Every
writer (`ipe build` emitted project, `ipe doc`, `ipe init`, cache) takes the
claim type, never a `Path`. Reserved component names (`.ipe`, `target`, …) come
from ONE table that the claim check and the audit both read.

## Why the instances exist (origin/main 983fdc404)

- `output_dir.rs:1227` `prove_parent_steps_from(raw, cwd)` takes `cwd` as a
  bare path — any caller can pass a non-cwd (#3006 Cwd newtype).
- `build_pipeline.rs:1849` `write_emitted_project` and `doc.rs:2556`
  `OwnedDir::claim(out)` are two claim entry points with different proofs
  (#3004, #3007).
- `output_dir.rs:65` `const ALL: [Self; 2]` is hand-synced with the enum;
  `audit.rs:807` repeats `".ipe"` in `CACHE_COMPONENTS` (#2866). Fix on
  `lane/2866-reserved-names` — unmerged, conflicts.
- Held `DirId` on Windows is not the 128-bit `FILE_ID_INFO` (#2934 item 4).

## Members

| Issue | Status | Verified by |
|---|---|---|
| #3004 emitted-project write through the claim | real | `build_pipeline.rs:1849` |
| #3007 `ipe doc` claim unified with build claim | real | `doc.rs:2556` |
| #3006 (`Cwd` newtype part) | real | `output_dir.rs:1227` |
| #2986 area claim residuals | real | same sites |
| #2866 reserved names from one table | real; fix stranded lane/2866-reserved-names | `output_dir.rs:65`, `audit.rs:807` |
| #2934 item 4 (Windows 128-bit `DirId`) | real, Windows only | `held.rs` `DirId` is `(dev, ino)` |

## Implementation plan

1. `Cwd` newtype, constructed once from `std::env::current_dir` at CLI entry;
   `prove_parent_steps_from(raw, &Cwd)`.
2. One `AreaClaim::claim(ProvenOutPath, Purpose) -> Result<AreaClaim, ClaimError>`;
   `write_emitted_project`, `doc`, `init` take `&AreaClaim`. Delete the
   path-taking claim.
3. Rebase `lane/2866-reserved-names`: `ReservedComponent` enum with
   `strum`-free exhaustive `ALL` derived by a `const` match check (adding a
   variant without listing it breaks the build); `CACHE_COMPONENTS` reads it.
4. Windows `DirId`: `GetFileInformationByHandleEx(FileIdInfo)` via the safe
   `windows` crate API (no `unsafe` in ipe code), `(volume_serial, u128)`.

## Prove the refusals

- output dir whose parent is a symlink swapped mid-proof → refused;
- `ipe doc --out` into an unclaimed non-empty dir → refused, same variant as
  build;
- output named `.ipe` or `target` → refused (table test over every
  `ReservedComponent`);
- claim marker owned by another uid → refused.

Lane tier: high. Guardian: required.
