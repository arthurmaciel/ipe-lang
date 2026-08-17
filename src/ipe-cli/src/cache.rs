//! The on-disk build cache.
//!
//! Decision record: `docs/adr/0032-salsa-incremental-compilation-phase1.md`.
//!
//! Everything in-process is memoized, but nothing survives ACROSS process
//! invocations — every `ipe build` starts a cold [`ipe_db::IpeDatabase`].
//! This module closes that gap for the coarse, whole-project granularity
//! that genuinely exists (`ipe_db::emit_project`'s output — see this
//! module's own doc section below for why that is a deliberate, documented
//! divergence from the design doc's literal "persist per-module lowered IR"
//! wording).
//!
//! ## What is cached, and why not literally "lowered IR"
//!
//! The design doc's Option-B locks in persisting `ipe_ir` (the lowered IR)
//! to `.ipe/lowered/`. `ipe_lower::lower` always produces exactly ONE
//! whole-program [`ipe_ir::Program`], so "per module" was never on the
//! table. The DEEPER blocker for persisting `ipe_ir::Program` itself,
//! specifically:
//! every [`ipe_intern::Symbol`] embedded in the IR (`Var`, `Ctor`, record
//! field names, `IrType::Generic`, …) is a raw index into THIS process's
//! [`ipe_intern::Interner`] — meaningless, and NOT merely "differently
//! numbered", in a fresh process with a fresh, empty interner. Making that
//! sound requires a relocation pass: serialize every embedded `Symbol` as
//! its resolved STRING, and on load, re-intern each string into the
//! CURRENT process's interner and rewrite every `Symbol` occurrence to the
//! newly-assigned id — a walker over every `Symbol`-carrying site in
//! `ipe_ir::ir` (far more sites than [`ipe_db::program_metadata`]'s
//! `Ctor`-only walk touches: `Var`, `CloneVar`, `Access`, record field
//! keys, `FuncSig` params/generics, `EnumDef`/`TypeDef` fields, …) plus
//! full `serde` coverage across ~20 IR types. That is a genuine, multi-
//! session redesign, not a corner to cut.
//!
//! **What ships instead**: [`ipe_backend::EmittedProject`] — the output of
//! [`ipe_db::emit_project`] — is cached. It is pure `String` data (no
//! `Symbol`, no interner dependency whatsoever: `RelPath` wraps a `String`,
//! `files` maps `RelPath -> String`, `cargo_toml` is a `String`), so it
//! serializes and deserializes losslessly with zero cross-process identity
//! risk. The practical win is AT LEAST as large as literal IR caching would
//! give for `ipe build`'s actual use case (a cold-start cache hit skips
//! parse -> canon -> link -> infer -> lower -> emit ENTIRELY, not just
//! infer -> lower -> emit), at the cost of not serving a hypothetical
//! future interpreter tier that wants to consume `ipe_ir` directly (design
//! doc §"Why `ipe_ir` is the cut-point") — that tier does not exist yet,
//! so the cost is paid by nobody today. This divergence is deliberate and
//! recorded here, not silently substituted.
//!
//! ## Content address (the cache KEY)
//!
//! [`compute_project_key`] hashes, with explicit length-prefixed framing
//! (never delimiter-joined — a delimiter that can appear inside a module
//! segment or source text would make two distinct projects collide) so
//! there is no ambiguity between e.g. `[["AB"], ["C"]]` and `[["A"], ["BC"]]`:
//!
//! - the entry module path,
//! - the SQL driver ([`ipe_backend_rust::DbDriver`]),
//! - every in-scope module's path, trust origin (injected stdlib vs. user
//!   source — the module-IDENTITY axis the design doc's cache-key-
//!   completeness note calls out: an add/delete/rename of a module MUST
//!   yield a different key, never a stale hit), and full source text.
//!
//! `blame_path` (diagnostic-only) and the vendored runtime tree are
//! deliberately NOT part of the key: neither affects [`EmittedProject`]'s
//! content (blame only shapes error rendering on a FAILED compile, which is
//! never cached; the runtime tree is copied by `write_emitted_project`
//! independently of the cache, exactly as it always was).
//!
//! ## Version epoch (toolchain refuse-don't-guess)
//!
//! [`derive_epoch`] hashes the CURRENTLY RUNNING `ipe` binary's own bytes
//! (`compiler_revision()`, matching the design doc's row verbatim: "content
//! hash seeded from the `ipe` binary's own build hash") together with the
//! active `rustc`'s `-vV` output (`toolchain_fingerprint()`). The epoch is a
//! DIRECTORY PREFIX (`<cache_root>/<epoch>/<key>.json`), not a value
//! compared after a hit — so "refuse, don't guess" is achieved BY
//! CONSTRUCTION, the same mechanism the design doc's FFI cache uses ("stale
//! entry has a different address -> unreachable miss", H1/H4 in the hazard
//! ledger): a `cargo build`/`cargo install` of `ipe` OR a `rustup update`
//! moves every subsequent build to a DIFFERENT directory, so entries from
//! the old compiler/toolchain pairing are never even looked up, let alone
//! trusted. There is nothing to "refuse" at lookup time because the stale
//! entries are structurally unreachable.
//!
//! Either probe failing (no `current_exe`, no `rustc` on `PATH`) disables
//! the cache for that invocation ([`derive_epoch`] returns `None`) — never a
//! guess, never a build failure: a compile just runs uncached, exactly as
//! every build did before this module existed.
//!
//! **Not yet ported**: `ipe watch`'s specific mid-session UX (hard-refuse a
//! REBUILD with `toolchain changed (was A, now B) — restart 'ipe watch'`
//! while keeping the last-good binary alive) needs a live watch session to
//! refuse INTO. The sound foundation that UX builds on is the version-epoch
//! gate itself.
//!
//! ## Advisory semantics (never a build failure)
//!
//! Every cache operation is best-effort: a missing directory, a corrupt
//! entry, or a write failure (permissions, full disk) is treated as "cache
//! unavailable for this build" and silently falls through to a full
//! compile — matching the design doc's own "Entries are advisory: hash
//! miss -> recompute, corrupt entry -> discard." A cache-write failure
//! after a SUCCESSFUL compile must never turn that success into a reported
//! build failure.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ipe_backend::EmittedProject;
use ipe_backend_rust::DbDriver;
use ipe_intern::{Interner, SerdeInternerGuard};
use ipe_ir::Program;
use sha2::{Digest, Sha256};

/// Domain-separation tag for the content-address hash — bumped whenever the
/// key's ingredient set changes shape (never for a value change within the
/// same shape; that is what the hash itself captures).
const KEY_TAG: &[u8] = b"ipec-build-cache-key-v2";

/// Domain-separation tag for the version-epoch hash.
const EPOCH_TAG: &[u8] = b"ipec-build-cache-epoch-v1";

/// Hash `bytes` into `hasher` with an explicit little-endian length prefix,
/// so two distinct inputs can never concatenate into the same byte stream
/// (the classic delimiter-collision hazard: `["AB", "C"]` vs `["A", "BC"]`
/// must hash differently, and would if segments were simply joined).
fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    hasher.update(len.to_le_bytes());
    hasher.update(bytes);
}

fn update_str(hasher: &mut Sha256, s: &str) {
    update_len_prefixed(hasher, s.as_bytes());
}

/// Domain-separation tag for the source-tree content hash — the integrity check
/// a resolved package is verified against (`crate::resolve`).
const TREE_TAG: &[u8] = b"ipe-source-tree-v1";

/// Compute a sha256 over the content of the directory tree rooted at `root`,
/// deterministically over `(relative_path, file_bytes)` pairs sorted by path.
///
/// This is the content integrity check a fetched package is verified against:
/// the hash the index pins equals `hash_tree` over the source the publisher
/// registered, so a mismatch means the fetched bytes are not that source. The
/// `.git` directory is excluded so the hash is of the source tree itself, not of
/// git's own bookkeeping (which varies across clones of the same revision). Each
/// path and its bytes are length-prefixed, so no rearrangement of files can
/// collide (the delimiter-collision hazard, as in [`update_len_prefixed`]).
///
/// # Errors
/// Returns the failing path and its [`std::io::Error`] if the tree cannot be
/// walked or a file cannot be read.
pub fn hash_tree(root: &Path) -> Result<String, (PathBuf, std::io::Error)> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(root, root, &mut files)?;
    // Sort by the relative path so the hash is independent of directory-read
    // order (which the OS does not guarantee).
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    update_len_prefixed(&mut hasher, TREE_TAG);
    let count = u64::try_from(files.len()).unwrap_or(u64::MAX);
    hasher.update(count.to_le_bytes());
    for (rel, abs) in &files {
        update_str(&mut hasher, rel);
        let bytes = fs::read(abs).map_err(|e| (abs.clone(), e))?;
        update_len_prefixed(&mut hasher, &bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Depth-first collect every regular file under `dir` as `(relative_path,
/// absolute_path)`, with the relative path expressed in forward slashes so the
/// hash is identical across platforms.
///
/// Hidden (dot-prefixed) directories are skipped. These hold VCS and local
/// tooling metadata — `.git`, a code indexer's `.tokensave`, an editor's
/// `.vscode`/`.idea` — never a package's published source, which lives in named
/// directories. Excluding them keeps the content hash a function of the source
/// alone, so a fetched checkout hashes identically no matter what local tools
/// have dropped a scratch directory into it.
///
/// Symlinks are rejected with an `InvalidInput` error. A published package
/// source must contain only plain files and directories; symlinks cannot be
/// integrity-checked safely without following them (which opens TOCTOU and
/// path-escape hazards), so we fail-closed rather than silently omitting them
/// from the hash (which would leave them invisible to the integrity check).
fn collect_files(
    base: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), (PathBuf, std::io::Error)> {
    let entries = fs::read_dir(dir).map_err(|e| (dir.to_path_buf(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| (dir.to_path_buf(), e))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| (path.clone(), e))?;
        if file_type.is_symlink() {
            return Err((
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "symlinks are not permitted in a published package source tree",
                ),
            ));
        } else if file_type.is_dir() {
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            collect_files(base, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
        }
    }
    Ok(())
}

/// Compute the content-address key for one build: a pure function of every
/// input that determines [`EmittedProject`]'s bytes (see the module doc's
/// "Content address" section for exactly what is, and is not, included).
///
/// `sources` is the driver's `module_path -> (fs_path, text)` map AFTER
/// [`crate::project::inject_compiled_std_closure`] has run (so it already
/// includes the injected stdlib closure's text) — the fs path itself is
/// deliberately unhashed, matching [`ipe_db::SourceFile`]'s own input shape
/// (module path + text + origin; never the on-disk path).
#[must_use]
pub fn compute_project_key(
    sources: &BTreeMap<Vec<String>, (PathBuf, String)>,
    injected: &BTreeSet<Vec<String>>,
    entry_path: &[String],
    db_driver: DbDriver,
    target: ipe_ir::Target,
    wasm_public_env: &[String],
    production: bool,
) -> String {
    let mut hasher = Sha256::new();
    update_len_prefixed(&mut hasher, KEY_TAG);
    // The target changes the emitted manifest/entry shape — a native-keyed
    // entry must never serve a wasm build (or vice versa).
    update_len_prefixed(&mut hasher, format!("{target:?}").as_bytes());

    // A PRODUCTION build (`--optimize`) rejects any `Debug.*` use (IPE-L0140),
    // so its outcome differs from a development build for a Debug-using program
    // (error vs emitted project). Keying on it keeps the two builds' cache
    // entries disjoint — a dev-cached project is never served to `--optimize`,
    // and vice versa. (For a Debug-free program the emitted bytes are identical
    // either way; the extra key bit only costs a one-time cold entry.)
    hasher.update([u8::from(production)]);

    let entry_len = u64::try_from(entry_path.len()).unwrap_or(u64::MAX);
    hasher.update(entry_len.to_le_bytes());
    for segment in entry_path {
        update_str(&mut hasher, segment);
    }

    let driver_tag: u8 = match db_driver {
        DbDriver::Sqlite => 0,
        DbDriver::Postgres => 1,
    };
    hasher.update([driver_tag]);

    // `[wasm] publicEnv` only affects the final emit stage (the generated
    // `env_public.rs`), the same class of input `db_driver` is — see this
    // fn's sibling [`compute_ir_key`], which deliberately excludes both.
    let public_env_len = u64::try_from(wasm_public_env.len()).unwrap_or(u64::MAX);
    hasher.update(public_env_len.to_le_bytes());
    for name in wasm_public_env {
        update_str(&mut hasher, name);
    }

    // `BTreeMap` iteration is already sorted by key — deterministic across
    // runs and independent of insertion order.
    let sources_len = u64::try_from(sources.len()).unwrap_or(u64::MAX);
    hasher.update(sources_len.to_le_bytes());
    for (path, (_fs_path, text)) in sources {
        let path_len = u64::try_from(path.len()).unwrap_or(u64::MAX);
        hasher.update(path_len.to_le_bytes());
        for segment in path {
            update_str(&mut hasher, segment);
        }
        let origin_tag: u8 = u8::from(injected.contains(path));
        hasher.update([origin_tag]);
        update_str(&mut hasher, text);
    }

    hex::encode(hasher.finalize())
}

/// Domain-separation tag for the lowered-IR content-address key —
/// distinct from [`KEY_TAG`] because this tier's key excludes `db_driver`
/// (see [`compute_ir_key`]'s doc for why).
const IR_KEY_TAG: &[u8] = b"ipec-build-cache-ir-key-v1";

/// Compute the content-address key for the lowered-IR cache tier:
/// a pure function of every input that determines
/// [`ipe_db::lower_program`]'s output.
///
/// Deliberately NARROWER than [`compute_project_key`]: `db_driver` only
/// affects the FINAL emit stage (`ipe_db::emit_project` reads
/// `config.db_driver`), never `linked_program`/`typecheck`/`lower_program`
/// (see `docs/architecture/salsa-incremental-compilation-2026-07-11.md`
/// §11.2/§13) — so an IR-tier key that included it would over-invalidate: a
/// `[database] driver` edit in `ipe.toml` would needlessly miss a perfectly
/// reusable `Program`, even though the ONLY thing that changed is read by
/// the emit stage this tier deliberately sits upstream of.
#[must_use]
pub fn compute_ir_key(
    sources: &BTreeMap<Vec<String>, (PathBuf, String)>,
    injected: &BTreeSet<Vec<String>>,
    entry_path: &[String],
    target: ipe_ir::Target,
) -> String {
    let mut hasher = Sha256::new();
    update_len_prefixed(&mut hasher, IR_KEY_TAG);
    // The IR itself is target-independent, but the fast path re-emits from a
    // cached Program WITHOUT re-running canonicalisation — keying on target
    // keeps the wasm Layer-1 gate (which runs at canon) unskippable.
    update_len_prefixed(&mut hasher, format!("{target:?}").as_bytes());

    let entry_len = u64::try_from(entry_path.len()).unwrap_or(u64::MAX);
    hasher.update(entry_len.to_le_bytes());
    for segment in entry_path {
        update_str(&mut hasher, segment);
    }

    let sources_len = u64::try_from(sources.len()).unwrap_or(u64::MAX);
    hasher.update(sources_len.to_le_bytes());
    for (path, (_fs_path, text)) in sources {
        let path_len = u64::try_from(path.len()).unwrap_or(u64::MAX);
        hasher.update(path_len.to_le_bytes());
        for segment in path {
            update_str(&mut hasher, segment);
        }
        let origin_tag: u8 = u8::from(injected.contains(path));
        hasher.update([origin_tag]);
        update_str(&mut hasher, text);
    }

    hex::encode(hasher.finalize())
}

/// The whole-project content hash of the CURRENTLY RUNNING `ipe` binary's
/// bytes — the design doc's `compiler_revision()`. `None` when the running
/// executable cannot be located or read (never a hard error: the cache is
/// simply unavailable for this invocation).
fn compiler_revision_hash() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let bytes = fs::read(&exe).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(hex::encode(hasher.finalize()))
}

/// The active `rustc`'s `-vV` output, hashed — the design doc's
/// `toolchain_fingerprint()`. `None` when `rustc` is not on `PATH` or exits
/// non-zero.
fn toolchain_fingerprint_hash() -> Option<String> {
    let output = std::process::Command::new("rustc")
        .arg("-vV")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(&output.stdout);
    Some(hex::encode(hasher.finalize()))
}

/// Derive the version-epoch directory name for this process, or `None` when
/// either probe is unavailable (cache disabled for this invocation — see
/// the module doc's "Advisory semantics" section).
#[must_use]
pub fn derive_epoch() -> Option<String> {
    let compiler_revision = compiler_revision_hash()?;
    let toolchain = toolchain_fingerprint_hash()?;
    let mut hasher = Sha256::new();
    update_len_prefixed(&mut hasher, EPOCH_TAG);
    update_str(&mut hasher, &compiler_revision);
    update_str(&mut hasher, &toolchain);
    Some(hex::encode(hasher.finalize()))
}

/// The default cache root for a build writing to `out_dir`, honouring the
/// `IPE_BUILD_CACHE` / `IPE_BUILD_CACHE_DIR` environment overrides.
///
/// - `IPE_BUILD_CACHE=0` (also `off` / `false`) disables the cache entirely.
/// - `IPE_BUILD_CACHE_DIR=<path>` overrides the default location.
/// - Otherwise: `<out_dir>/.ipe-cache` — colocated with the build output
///   so `rm -rf <out_dir>` (the existing "force a clean rebuild" ritual)
///   also resets the cache, with no new mental model to learn.
#[must_use]
pub fn env_cache_dir(out_dir: &Path) -> Option<PathBuf> {
    if matches!(
        std::env::var("IPE_BUILD_CACHE").as_deref(),
        Ok("0" | "off" | "false")
    ) {
        return None;
    }
    if let Ok(dir) = std::env::var("IPE_BUILD_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(out_dir.join(".ipe-cache"))
}

fn entry_file_path(cache_root: &Path, epoch: &str, key: &str) -> PathBuf {
    cache_root.join(epoch).join(format!("{key}.json"))
}

/// Look up a cached [`EmittedProject`] for `key` under `epoch`. Every
/// failure mode (missing file, unreadable, corrupt JSON, an entry that
/// deserializes but fails `RelPath`'s validation) is a plain cache MISS —
/// `None`, never an error, matching "corrupt entry -> discard".
#[must_use]
pub fn try_load(cache_root: &Path, epoch: &str, key: &str) -> Option<EmittedProject> {
    let path = entry_file_path(cache_root, epoch, key);
    let bytes = fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Best-effort store of a successfully compiled [`EmittedProject`] under
/// `key`/`epoch`. Every failure (directory creation, serialize, write,
/// rename) is silently swallowed — a cache-write failure must never turn a
/// successful build into a reported failure. Writes atomically (tmp file +
/// rename) so a concurrent reader (a second `ipe build` racing this one)
/// never observes a partially-written entry; a torn read is impossible, a
/// missing-then-appearing file is the only visible race, which `try_load`
/// already treats as an ordinary miss.
///
/// The tmp file name is suffixed with this process's PID (mirroring
/// `write_atomic`'s existing convention) so two CONCURRENT `ipe build`
/// invocations computing the SAME key never write to the same tmp path —
/// without that, two racing writers could interleave into one file before
/// either renamed it, corrupting the entry a third reader might load in
/// between (a real hazard the single shared-name `entry.json.tmp` form
/// would have had, distinct from the deliberately-tolerated "a fresh entry
/// was written between my miss-check and now" race).
pub fn store(cache_root: &Path, epoch: &str, key: &str, project: &EmittedProject) {
    let path = entry_file_path(cache_root, epoch, key);
    let Some(dir) = path.parent() else {
        return;
    };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(json) = serde_json::to_vec(project) else {
        return;
    };
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if fs::write(&tmp, &json).is_err() {
        let _ = fs::remove_file(&tmp);
        return;
    }
    if fs::rename(&tmp, &path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

// ---------------------------------------------------------------------------
// The lowered-IR cache tier
// ---------------------------------------------------------------------------
//
// Sits ONE STAGE EARLIER than the `EmittedProject` tier above: a hit here
// skips parse -> canon -> link -> infer -> lower ENTIRELY (no
// `ipe_db::IpeDatabase` is even constructed — see `compile_modules_observed`
// in `crate::lib`), running only `RustBackend::emit` over the recovered
// `ipe_ir::Program` before falling through to the SAME
// `write_emitted_project`/tier-1-`store` path a full pipeline run uses. A
// hit here is therefore a smaller win than an `EmittedProject`-tier hit
// (emit still runs), but covers the case an `EmittedProject`-tier miss does
// NOT: a `db_driver`-only edit (SQL driver flip in `ipe.toml`), where the
// SAME `Program` this tier caches is still exactly reusable even though the
// `EmittedProject` tier's key (which folds in `db_driver`) misses.
//
// **The relocation pass.** `ipe_ir::Program` embeds `ipe_intern::Symbol`
// pervasively — a raw index into the WRITING process's interner, meaningless
// against any other. `ipe_intern::Symbol`'s `serde` impls close this by
// serialising the symbol's resolved STRING and re-interning it into an
// AMBIENT interner installed via `SerdeInternerGuard::install` (see that
// type's own module doc for the full design + the cross-process id-drift
// proof). Every (de)serialize call in this section installs a guard around
// exactly one `Program` (de)serialize call, so a `Program` deserialized here
// behaves identically to a fresh `ipe_lower::lower` output IN THE CALLING
// PROCESS's interner — never a raw-id mismatch.
//
// **Security.** `ipe_intern::Symbol::deserialize` validates every embedded
// string through `ipe_intern::is_valid_symbol_text` before interning it —
// closing the SAME class of hole `RelPath`'s hand-written `Deserialize`
// closes for path traversal, applied to identifier text instead of paths
// (a poisoned symbol string could otherwise splice arbitrary Rust source
// into the next `RustBackend::emit` call, since the backend trusts an
// interned string verbatim when emitting identifiers). `ipe_ir::Match`
// similarly carries a hand-written `Deserialize` that re-validates through
// `Match::new_flat`'s structural backstop rather than trusting the arm list
// verbatim. Every failure mode here — corrupt JSON, a poisoned `Symbol`
// string, a malformed `Match` — is `None`/silently-swallowed, the SAME
// "corrupt entry -> discard" contract the `EmittedProject` tier established.

fn ir_entry_file_path(cache_root: &Path, epoch: &str, key: &str) -> PathBuf {
    cache_root.join(epoch).join(format!("{key}.ir.json"))
}

/// Look up a cached lowered [`Program`] for `key` under `epoch`, relocating
/// every embedded `Symbol` into `interner` (the RELOCATION PASS — see this
/// section's module doc). Every failure mode (missing file, unreadable,
/// corrupt JSON, a `Symbol` text that fails `ipe_intern::is_valid_symbol_text`,
/// a `Match` arm list `Match::new_flat` rejects) is a plain cache MISS —
/// `None`, never an error — matching the `EmittedProject` tier's contract
/// exactly ([`try_load`]).
#[must_use]
pub fn try_load_ir(
    cache_root: &Path,
    epoch: &str,
    key: &str,
    interner: &Arc<Mutex<Interner>>,
) -> Option<Program> {
    let path = ir_entry_file_path(cache_root, epoch, key);
    let bytes = fs::read(&path).ok()?;
    let _guard = SerdeInternerGuard::install(Arc::clone(interner));
    serde_json::from_slice(&bytes).ok()
}

/// Best-effort store of a successfully lowered [`Program`]. Same
/// advisory/atomic-write contract as [`store`]: every failure (directory
/// creation, resolve/serialize, write, rename) is silently swallowed — a
/// cache-write failure must never turn a successful build into a reported
/// failure. PID-suffixed tmp name for the same concurrent-writer safety
/// [`store`]'s own doc explains.
pub fn store_ir(
    cache_root: &Path,
    epoch: &str,
    key: &str,
    program: &Program,
    interner: &Arc<Mutex<Interner>>,
) {
    let path = ir_entry_file_path(cache_root, epoch, key);
    let Some(dir) = path.parent() else {
        return;
    };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let json = {
        let _guard = SerdeInternerGuard::install(Arc::clone(interner));
        serde_json::to_vec(program)
    };
    let Ok(json) = json else {
        return;
    };
    let tmp = path.with_extension(format!("ir.json.{}.tmp", std::process::id()));
    if fs::write(&tmp, &json).is_err() {
        let _ = fs::remove_file(&tmp);
        return;
    }
    if fs::rename(&tmp, &path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestSources = BTreeMap<Vec<String>, (PathBuf, String)>;

    fn sample_sources() -> (TestSources, BTreeSet<Vec<String>>) {
        let mut sources = BTreeMap::new();
        sources.insert(
            vec!["Main".to_owned()],
            (
                PathBuf::from("Main.ipe"),
                "module Main exposing (main)\n".to_owned(),
            ),
        );
        sources.insert(
            vec!["Ipe".to_owned(), "Basics".to_owned()],
            (
                PathBuf::from("<embedded>"),
                "module Ipe.Basics exposing (x)\nx = 1\n".to_owned(),
            ),
        );
        let injected = BTreeSet::from([vec!["Ipe".to_owned(), "Basics".to_owned()]]);
        (sources, injected)
    }

    fn entry() -> Vec<String> {
        vec!["Main".to_owned()]
    }

    #[test]
    fn key_is_deterministic() {
        let (sources, injected) = sample_sources();
        let a = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        let b = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        assert_eq!(a, b, "same inputs must hash to the same key");
    }

    #[test]
    fn key_changes_with_source_text() {
        let (mut sources, injected) = sample_sources();
        let base = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        if let Some(main) = sources.get_mut(&vec!["Main".to_owned()]) {
            main.1.push_str("\n-- comment\n");
        }
        let edited = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        assert_ne!(base, edited, "a body edit must change the key");
    }

    #[test]
    fn key_changes_with_db_driver() {
        let (sources, injected) = sample_sources();
        let sqlite = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        let postgres = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Postgres,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        assert_ne!(sqlite, postgres, "the SQL driver is part of the key");
    }

    /// A `--optimize` (production) build rejects any `Debug.*` use (IPE-L0140),
    /// so its outcome differs from a development build for a Debug-using
    /// program. The key must separate the two so a dev-cached project is never
    /// served to `--optimize` (or vice versa) — the tier-1 proof of the emit
    /// demand's production gate.
    #[test]
    fn key_changes_with_production() {
        let (sources, injected) = sample_sources();
        let dev = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        let prod = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            true,
        );
        assert_ne!(dev, prod, "the production flag is part of the key");
    }

    /// `[wasm] publicEnv` only affects the final emit stage (the generated
    /// `env_public.rs`) — same class of input as `db_driver` (see this
    /// module's `compute_project_key` doc) — so a cached `EmittedProject`
    /// entry must never serve a stale `env_public.rs` after a `publicEnv`
    /// edit; this test is the tier-1 (project-key) proof of that.
    #[test]
    fn key_changes_with_wasm_public_env() {
        let (sources, injected) = sample_sources();
        let empty_allowlist = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        let with_allowlist = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &["API_BASE_URL".to_owned()],
            false,
        );
        assert_ne!(
            empty_allowlist, with_allowlist,
            "the [wasm] publicEnv allowlist is part of the key"
        );
    }

    #[test]
    fn key_changes_with_entry_path() {
        let (sources, injected) = sample_sources();
        let a = compute_project_key(
            &sources,
            &injected,
            &["Main".to_owned()],
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        let b = compute_project_key(
            &sources,
            &injected,
            &["Other".to_owned()],
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        assert_ne!(a, b, "the entry module path is part of the key");
    }

    #[test]
    fn key_changes_with_module_add_and_remove() {
        let (sources, injected) = sample_sources();
        let base = compute_project_key(
            &sources,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );

        let mut added = sources.clone();
        added.insert(
            vec!["Extra".to_owned()],
            (
                PathBuf::from("Extra.ipe"),
                "module Extra exposing (y)\ny = 2\n".to_owned(),
            ),
        );
        let with_extra = compute_project_key(
            &added,
            &injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        assert_ne!(base, with_extra, "adding a module must change the key");

        let mut removed = sources;
        removed.remove(&vec!["Ipe".to_owned(), "Basics".to_owned()]);
        let mut injected_without = injected;
        injected_without.remove(&vec!["Ipe".to_owned(), "Basics".to_owned()]);
        let without_basics = compute_project_key(
            &removed,
            &injected_without,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        assert_ne!(
            base, without_basics,
            "removing a module must change the key"
        );
    }

    #[test]
    fn key_changes_with_module_origin() {
        // Same path + text, different trust origin (injected vs. user) —
        // the design doc's module-identity axis. This can't happen through
        // the real driver (a path is injected or it isn't), but the key
        // function must still be sensitive to it: origin affects
        // canonicalisation (IPE-N0025), and the module doc's own "when in
        // doubt, include it" principle applies.
        let (sources, _injected) = sample_sources();
        let no_injection: BTreeSet<Vec<String>> = BTreeSet::new();
        let all_injected: BTreeSet<Vec<String>> = sources.keys().cloned().collect();
        let a = compute_project_key(
            &sources,
            &no_injection,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        let b = compute_project_key(
            &sources,
            &all_injected,
            &entry(),
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        assert_ne!(a, b, "the trust-origin flag is part of the key");
    }

    #[test]
    fn key_is_delimiter_collision_safe() {
        // ["AB", "C"] and ["A", "BC"] must NOT collide even though a naive
        // "join with no delimiter" scheme would produce the same bytes.
        let mut left = BTreeMap::new();
        left.insert(
            vec!["AB".to_owned(), "C".to_owned()],
            (PathBuf::from("x"), String::new()),
        );
        let mut right = BTreeMap::new();
        right.insert(
            vec!["A".to_owned(), "BC".to_owned()],
            (PathBuf::from("x"), String::new()),
        );
        let empty: BTreeSet<Vec<String>> = BTreeSet::new();
        let a = compute_project_key(
            &left,
            &empty,
            &[],
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        let b = compute_project_key(
            &right,
            &empty,
            &[],
            DbDriver::Sqlite,
            ipe_ir::Target::Native,
            &[],
            false,
        );
        assert_ne!(a, b, "differently-segmented module paths must not collide");
    }

    #[test]
    fn store_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("ipe-cache-test-{}", std::process::id()));
        let cache_root = dir.join("cache-root-round-trip");
        let mut files = BTreeMap::new();
        files.insert(
            ipe_backend::RelPath::new("src/main.rs").expect("valid path"),
            "fn main() {}".to_owned(),
        );
        let project = EmittedProject {
            files,
            cargo_toml: "[package]\nname = \"x\"\n".to_owned(),
        };

        assert!(
            try_load(&cache_root, "epoch-a", "key-a").is_none(),
            "empty cache misses"
        );
        store(&cache_root, "epoch-a", "key-a", &project);
        let loaded = try_load(&cache_root, "epoch-a", "key-a");
        assert_eq!(loaded, Some(project));

        // A different epoch or key must NOT see the stored entry — the
        // version-epoch/content-address separation is structural.
        assert!(try_load(&cache_root, "epoch-b", "key-a").is_none());
        assert!(try_load(&cache_root, "epoch-a", "key-b").is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_load_treats_corrupt_entry_as_a_miss() {
        let dir =
            std::env::temp_dir().join(format!("ipe-cache-test-corrupt-{}", std::process::id()));
        let cache_root = dir.join("cache-root-corrupt");
        let path = entry_file_path(&cache_root, "epoch", "key");
        fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir must succeed");
        fs::write(&path, b"not valid json at all {{{").expect("write must succeed");

        assert!(
            try_load(&cache_root, "epoch", "key").is_none(),
            "corrupt entry must be discarded as a miss, never a panic or error propagation"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_load_treats_a_poisoned_relpath_entry_as_a_miss() {
        // Even a syntactically-valid JSON document with a semantically
        // unsafe `RelPath` key must be discarded, not partially trusted —
        // proven at the `ipe_backend` level (`emitted_project_deserialize_
        // rejects_a_poisoned_key`); this test proves the cache layer
        // inherits that rejection via `.ok()` rather than accidentally
        // routing around it.
        let dir =
            std::env::temp_dir().join(format!("ipe-cache-test-poison-{}", std::process::id()));
        let cache_root = dir.join("cache-root-poison");
        let path = entry_file_path(&cache_root, "epoch", "key");
        fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir must succeed");
        fs::write(
            &path,
            br#"{"files":{"../../etc/passwd":"pwned"},"cargo_toml":""}"#,
        )
        .expect("write must succeed");

        assert!(try_load(&cache_root, "epoch", "key").is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_cache_dir_respects_disable_and_override() {
        // Pure-function shape avoided here on purpose: `env_cache_dir` reads
        // process env, so this test only checks the UNSET default (the
        // env-mutation cases are exercised end-to-end in
        // `crates/ipe/src/lib.rs`'s cache integration tests via the
        // explicit-cache-dir seam, never via `std::env::set_var` — see that
        // module's doc for why).
        let out_dir = Path::new("/tmp/ipe-cache-dir-does-not-need-to-exist");
        let default = env_cache_dir(out_dir);
        assert_eq!(default, Some(out_dir.join(".ipe-cache")));
    }

    // -----------------------------------------------------------------
    // The lowered-IR cache tier
    // -----------------------------------------------------------------

    #[allow(clippy::too_many_lines)] // exhaustive `Module` literal (every `uses_*` flag)
    fn sample_ir_program(i: &mut Interner) -> ipe_diagnostics::DResult<Program> {
        use ipe_ir::{
            Arm, CallPin, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, Match, ModPath,
            Module, OnFormKind, Pat, TypeDef, Variant,
        };

        let msg_ty = i.intern("Msg")?;
        let inc = i.intern("Increment")?;
        let dec = i.intern("Decrement")?;
        let main_sym = i.intern("main")?;
        let main_mod = i.intern("Main")?;
        let msg_param = i.intern("msg")?;

        let body = Expr::Match(Match::new(
            Expr::Var(msg_param),
            vec![
                Arm::new(
                    Pat::Ctor {
                        home: ModPath(vec![]),
                        ty: msg_ty,
                        variant: inc,
                        args: vec![],
                    },
                    Expr::Call {
                        callee: Callee::Kernel(KernelFn::IoPrintln),
                        args: vec![],
                        pin: CallPin::None,
                        on_form: OnFormKind::NotForm,
                    },
                ),
                Arm::new(
                    Pat::Ctor {
                        home: ModPath(vec![]),
                        ty: msg_ty,
                        variant: dec,
                        args: vec![],
                    },
                    Expr::Call {
                        callee: Callee::Kernel(KernelFn::IoPrintln),
                        args: vec![],
                        pin: CallPin::None,
                        on_form: OnFormKind::NotForm,
                    },
                ),
            ],
            &[inc, dec],
        )?);

        Ok(Program {
            imports_unsafe_submodule: false,
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    home: ModPath(vec![]),
                    name: msg_ty,
                    type_params: vec![],
                    variants: vec![
                        Variant {
                            name: inc,
                            fields: vec![],
                        },
                        Variant {
                            name: dec,
                            fields: vec![],
                        },
                    ],
                })],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: main_sym,
                    home: ModPath(vec![]),
                    type_params: vec![],
                    row_params: vec![],
                    params: vec![(msg_param, IrType::Generic(msg_param))],
                    ret: IrType::Unit,
                    body,
                }],
                entry: Some(FuncId::from_raw(0)),
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_http: false,
                uses_config: false,
                uses_compression: false,
                uses_csv: false,
                uses_cache: false,
                uses_encoding: false,
                uses_regex: false,
                uses_uuid: false,
                uses_random: false,
                uses_log: false,
                uses_decimal: false,
                uses_char_category: false,
                uses_crypto_core: false,
                uses_secret: false,
                uses_json: false,
                uses_crypto: false,
                uses_jwt: false,
                uses_url: false,
                uses_ui: false,
                uses_web: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_time: false,
                uses_env_public: false,
                uses_debug: false,
                uses_ffi: false,
                uses_async_runtime: false,
            }],
        })
    }

    #[test]
    fn compute_ir_key_is_deterministic_and_excludes_db_driver() {
        let (sources, injected) = sample_sources();
        let a = compute_ir_key(&sources, &injected, &entry(), ipe_ir::Target::Native);
        let b = compute_ir_key(&sources, &injected, &entry(), ipe_ir::Target::Native);
        assert_eq!(a, b, "same inputs must hash to the same IR key");

        // Unlike `compute_project_key`, `compute_ir_key` must be blind to
        // `db_driver` entirely — it isn't even a parameter, so there is
        // nothing to vary here; this test pins the SIGNATURE difference
        // itself (a `db_driver`-only rebuild reuses the SAME IR key).
        assert_eq!(
            compute_ir_key(&sources, &injected, &entry(), ipe_ir::Target::Native),
            a,
            "compute_ir_key has no db_driver parameter to vary"
        );
    }

    #[test]
    fn compute_ir_key_changes_with_source_text() {
        let (mut sources, injected) = sample_sources();
        let base = compute_ir_key(&sources, &injected, &entry(), ipe_ir::Target::Native);
        if let Some(main) = sources.get_mut(&vec!["Main".to_owned()]) {
            main.1.push_str("\n-- comment\n");
        }
        let edited = compute_ir_key(&sources, &injected, &entry(), ipe_ir::Target::Native);
        assert_ne!(base, edited, "a body edit must change the IR key");
    }

    #[test]
    fn ir_store_and_load_round_trip_within_one_interner() -> ipe_diagnostics::DResult<()> {
        let mut plain = Interner::new();
        let program = sample_ir_program(&mut plain)?;
        let interner = Arc::new(Mutex::new(plain));

        let dir = std::env::temp_dir().join(format!("ipec-ir-cache-test-{}", std::process::id()));
        let cache_root = dir.join("cache-root-ir-round-trip");
        let _ = fs::remove_dir_all(&dir);

        assert!(
            try_load_ir(&cache_root, "epoch-a", "key-a", &interner).is_none(),
            "empty cache misses"
        );
        store_ir(&cache_root, "epoch-a", "key-a", &program, &interner);
        let loaded = try_load_ir(&cache_root, "epoch-a", "key-a", &interner);
        assert_eq!(loaded, Some(program));

        // Different epoch/key must not see the entry — same structural
        // separation as the `EmittedProject` tier.
        assert!(try_load_ir(&cache_root, "epoch-b", "key-a", &interner).is_none());
        assert!(try_load_ir(&cache_root, "epoch-a", "key-b", &interner).is_none());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    /// **Cross-process id-drift proof, at the on-disk cache boundary.**
    /// Stores a `Program` written through one interner, then loads it
    /// through a COMPLETELY DIFFERENT, differently-polluted interner (the
    /// scenario a real `ipe build` -> `ipe build` sequence produces: a
    /// fresh `Interner::new()` per invocation). Asserts the relocated
    /// `Program`'s structural content (via `ipe_ir::pretty::pretty`,
    /// resolved-name comparison — not raw `Symbol` equality, which is not
    /// expected to survive the boundary) matches a Program built fresh in
    /// the reader's own, unrelated interner.
    #[test]
    fn ir_cache_hit_survives_cross_process_symbol_id_drift() -> ipe_diagnostics::DResult<()> {
        let dir = std::env::temp_dir().join(format!("ipec-ir-cache-drift-{}", std::process::id()));
        let cache_root = dir.join("cache-root-drift");
        let _ = fs::remove_dir_all(&dir);

        // "Process A" (the writer): noise, then build + store.
        let mut interner_a = Interner::new();
        for noise in ["foo", "bar", "baz", "qux"] {
            interner_a.intern(noise)?;
        }
        let program_a = sample_ir_program(&mut interner_a)?;
        let interner_a = Arc::new(Mutex::new(interner_a));
        store_ir(&cache_root, "epoch", "key", &program_a, &interner_a);

        // "Process B" (the reader): DIFFERENT noise, different count/order.
        let mut interner_b = Interner::new();
        for noise in ["zzz_1", "zzz_2"] {
            interner_b.intern(noise)?;
        }
        let interner_b = Arc::new(Mutex::new(interner_b));
        let program_b =
            try_load_ir(&cache_root, "epoch", "key", &interner_b).expect("must be a cache hit");

        // "Process C" (ground truth): independent construction, never
        // touches the cache at all.
        let mut interner_c = Interner::new();
        interner_c.intern("unrelated")?;
        let program_c = sample_ir_program(&mut interner_c)?;

        let dump_b = {
            let guard = interner_b
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ipe_ir::pretty(&program_b, &guard)
        };
        let dump_c = ipe_ir::pretty(&program_c, &interner_c);
        assert_eq!(
            dump_b, dump_c,
            "an IR-cache entry loaded through a differently-polluted \
             interner must be structurally/name-identical to a fresh, \
             never-cached construction"
        );
        assert!(dump_b.contains("Increment") && dump_b.contains("main"));

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn ir_try_load_treats_corrupt_entry_as_a_miss() {
        let dir =
            std::env::temp_dir().join(format!("ipec-ir-cache-corrupt-{}", std::process::id()));
        let cache_root = dir.join("cache-root-corrupt");
        let path = ir_entry_file_path(&cache_root, "epoch", "key");
        fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir must succeed");
        fs::write(&path, b"not valid json at all {{{").expect("write must succeed");

        let interner = Arc::new(Mutex::new(Interner::new()));
        assert!(
            try_load_ir(&cache_root, "epoch", "key", &interner).is_none(),
            "corrupt entry must be discarded as a miss, never a panic or error propagation"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// A poisoned `Symbol` text (would-be Rust-injection payload) inside an
    /// otherwise-valid-JSON IR cache entry must be rejected as a whole-entry
    /// miss — proving the on-disk boundary inherits `ipe_intern::Symbol`'s
    /// deserialize-time validation rather than accidentally routing around
    /// it (the same class of proof `try_load_treats_a_poisoned_relpath_
    /// entry_as_a_miss` gives for the `EmittedProject` tier).
    #[test]
    fn ir_try_load_treats_a_poisoned_symbol_entry_as_a_miss() {
        let dir = std::env::temp_dir().join(format!("ipec-ir-cache-poison-{}", std::process::id()));
        let cache_root = dir.join("cache-root-poison");
        let path = ir_entry_file_path(&cache_root, "epoch", "key");
        fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir must succeed");
        // A syntactically-valid `Program` shape whose module name embeds an
        // injection-shaped payload instead of a legal identifier.
        fs::write(
            &path,
            br#"{"modules":[{"name":["x; std::process::exit(1); //"],"types":[],"funcs":[],"entry":null,"records":[],"uses_tea":false,"uses_server":false,"uses_ui":false,"uses_web":false,"uses_tui":false,"uses_webview":false,"uses_css":false,"uses_auth":false}]}"#,
        )
        .expect("write must succeed");

        let interner = Arc::new(Mutex::new(Interner::new()));
        assert!(
            try_load_ir(&cache_root, "epoch", "key", &interner).is_none(),
            "a poisoned Symbol text must be rejected — whole entry discarded, never partially trusted"
        );
        // The poisoned text must never have reached the interner.
        let resolved: Option<String> = {
            let guard = interner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .resolve(ipe_intern::Symbol::from_raw(0))
                .map(str::to_owned)
        };
        assert!(
            resolved.is_none(),
            "a rejected symbol text must never be interned"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ir_env_extension_does_not_collide_with_emitted_project_tier() {
        // The two tiers must write to DIFFERENT files under the same
        // `(cache_root, epoch, key)` triple — proven structurally rather
        // than by inspection, so a future accidental filename collision
        // (one tier silently overwriting the other) is caught immediately.
        let cache_root = Path::new("/tmp/x");
        let a = entry_file_path(cache_root, "epoch", "key");
        let b = ir_entry_file_path(cache_root, "epoch", "key");
        assert_ne!(
            a, b,
            "the EmittedProject and lowered-IR tiers must use distinct file paths"
        );
    }

    /// A fetched tree containing a symlink must be rejected — the integrity
    /// hash must never silently omit a tree entry.
    #[test]
    #[cfg(unix)]
    fn hash_tree_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "ipe-cache-test-symlink-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create base");
        std::fs::write(base.join("file.ipe"), b"hello").expect("write file");
        symlink("/etc/passwd", base.join("link.ipe")).expect("create symlink");

        let result = hash_tree(&base);
        let _ = std::fs::remove_dir_all(&base);

        assert!(
            result.is_err(),
            "hash_tree must reject a tree containing a symlink"
        );
        let (_, err) = result.unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::InvalidInput,
            "symlink rejection must use InvalidInput"
        );
    }

    /// A tree with only plain files hashes normally and produces the same
    /// result on a second call (deterministic, no symlinks → no rejection).
    #[test]
    fn hash_tree_plain_tree_is_deterministic() {
        let base = std::env::temp_dir().join(format!(
            "ipe-cache-test-plain-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create base");
        std::fs::write(base.join("Main.ipe"), b"module Main").expect("write file");

        let h1 = hash_tree(&base).expect("plain tree hashes ok");
        let h2 = hash_tree(&base).expect("plain tree hashes ok second time");
        let _ = std::fs::remove_dir_all(&base);

        assert_eq!(h1, h2, "hash_tree must be deterministic for plain trees");
    }
}
