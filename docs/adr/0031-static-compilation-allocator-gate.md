Status: Accepted

# 0031. Static compilation and allocator selection

## Context

The emitted Rust crate that `ipe build` produces must be distributable as a
fully-static, self-contained binary so that Ipê-compiled apps run on any Linux
target without a glibc version dependency or a dynamic loader. The naive static
path (`target-feature=+crt-static` + `x86_64-unknown-linux-musl`) has a known
cliff: `libc`'s malloc/free stub in musl is single-threaded; tokio's runtime
needs a thread-safe allocator. Three allocator choices cover the realistic set:
system default (musl built-in, acceptable for dev), `dlmalloc` (pure-Rust,
thread-safe, zero C dependency, the default for static release), and `mimalloc`
(high-throughput, C-backed, opt-in).

`talc` (arena-based, zero-syscall) was evaluated and rejected: its
`ClaimOnOom::new` is an `unsafe const fn` requiring a real static arena with a
hard heap cap — emitted `unsafe` plus an arbitrary memory ceiling, both of which
violate the generated-code soundness posture.

## Decision

`ipe build --static [--allocator dlmalloc|mimalloc|talc]` controls the static
path. The emitted `Cargo.toml` gains a `[profile.release]` `lto` + `codegen-units=1`
block and a `.cargo/config.toml` `rustflags = ["-C", "target-feature=+crt-static"]`
for the musl target.

`dlmalloc` is the default — pure-Rust, thread-safe, no C, proven on the full dep
graph (tokio/reqwest/ring/libzstd all link clean). `mimalloc` is an opt-in for
throughput-critical apps; it requires the host `cmake`/CC. `talc` parses (the
enum stays closed) but resolves to a typed refusal: "talc requires an arena
design that passes the no-unsafe gate; use dlmalloc instead."

The static path is verified in CI (`static.yml`) end-to-end: emit → `cargo build
--target x86_64-unknown-linux-musl` → `file` + `ldd` assertions (static-ness
asserted, never assumed) → execution inside a `scratch` container (nothing in
filesystem but the binary — a hidden dynamic dep fails loudly here) → `cargo audit`
over the emitted lockfile.

## Consequences

- `--allocator talc` is permanently a typed refusal until an arena design passes
  the no-unsafe gate; the refusal message explains why and names the alternative.
- The static path adds `musl-gcc` / `x86_64-unknown-linux-musl` as a CI dep; the
  normal dev path (dynamic linking) is unaffected.
- Supply-chain: static linking freezes every dep into the artifact; the CI
  `cargo audit` gate on the emitted lockfile is therefore load-bearing (a dep
  advisory that would normally be indirect becomes directly bundled).
- The emitted `.cargo/config.toml` applies only when cargo is invoked from the
  emitted crate's directory; a `CARGO_TARGET_DIR` override in the CI sweep must
  be set from that same directory context.
