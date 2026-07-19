Status: Accepted

# 0014. Kernel robustness: blocking-work offload, single-pass read-limit, per-frame CLI separator

## Context

Several runtime kernels had latent robustness bugs surfaced by concurrent or
adversarial use: synchronous blocking work on the async reactor, a TOCTOU race
in a bounded read, and a missing output separator between CLI render frames.

## Decision

- **Offload blocking work to `tokio::spawn_blocking`.** The four compression
  kernels (`gzip`/`gunzip`/`zstdCompress`/`zstdDecompress`) computed inline on
  the caller's thread; file kernels ran blocking `std::fs` syscalls directly in
  their async bodies — both starve the reactor under concurrent load. Extract
  each into a private `*_sync` helper and route the public kernel through
  `tokio::task::spawn_blocking`, generalizing the pattern auth already used for
  bcrypt. Rejected: wrapping sync work in `std::future::ready()` — it defers
  nothing, the work still runs inline on first poll. `compression.rs` is already
  tokio-gated (unconditional offload); `file.rs` is not, so the offload is
  `#[cfg(feature = "tokio")]` with an inline fallback reachable only from
  test-only narrow-feature builds (all real generated projects have tokio).

- **Eliminate the `readFileLimit` TOCTOU by reading `cap+1` in one pass.**
  `file_read_file_limit` stat-checked size then `take(cap)`, so a concurrent
  append could pass the stale check while `take(cap)` silently truncated —
  returning `Ok(<cap bytes>)` where the contract promised `Err`. Drop the
  `metadata()` stat entirely; read `cap + 1` bytes and check the actual count
  post-read (the same idiom as the decompression-bomb guards). One syscall
  sequence, nothing left to race.

- **Terminate every CLI render frame with a newline at its call site.**
  `cli_program` appended a single trailing newline only after the loop exited,
  so consecutive renders read as `"lines: 0lines: 1"`. Move the `"\n"` to every
  render call site (initial + each loop body) and delete the redundant post-loop
  newline.

## Consequences

- **Invariants that must keep holding:** a single-threaded (current_thread)
  executor must still make progress on concurrent work during a blocking op — a
  ticker started before the op must observe ticks > 0 by the time it completes.
  A file that grows between start and read completion is reliably detected as
  over-limit; boundary is `read as u64 > cap` (exactly `cap` succeeds, one over
  errors); the error message intentionally omits an exact byte count to keep the
  bounded read bounded. Every `view(model)` render is followed by `"\n"` before
  any other output, so consecutive frames always land on separate lines.
