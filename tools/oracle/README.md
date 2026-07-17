# Go-reference oracle binary

The equivalence oracle (`scripts/equivalence-checks/examples-sweep.sh`, EQUIVALENCE column) diffs skyc's
emitted-Rust **runtime output** against the reference Haskell `sky` compiler's
**Go-backend runtime output**. For that diff to mean "parity," the reference
must be pinned to our **port target version**, not whatever stale `sky` happens
to be on `PATH`.

## Pinned binary

Place the reference compiler here:

    tools/oracle/bin/sky              # sky-linux-x64 release, executable
    tools/oracle/bin/sky-ffi-inspect  # bundled inspector (Tier-2 FFI use)

Current pin: **v0.17.3** (`sky-linux-x64.tar.gz` GitHub release asset).
`tools/oracle/bin/` is `.gitignore`d — the ~40 MB binaries are fetched, not
committed. `sky --version` must print the pinned version.

## Resolution order (build_go)

1. `$IPE_GO_BIN` (explicit override)
2. `tools/oracle/bin/sky` (this pinned binary)
3. `sky` on `PATH` (fallback; may be stale — a version-skew warning risk)

## Why pin

The installed `/usr/local/bin/sky` was v0.16.29 while the port targets v0.17.x.
The oracle compares runtime output (stdout / rendered HTML), which is stable
across most of 0.16→0.17 — but v0.17-**changed stdlib semantics** (rune-based
`String.dropLeft`, `*Utc` companions, …) would diff falsely and read as *our*
bug. Pinning the reference to the port target removes that class of false
negative. We cannot build the Haskell `sky` from source on this host
(GHC 8.8.4; needs 9.4.8 + cabal), so the release binary is the reference.
