# Faster Ipê builds

`ipe build` and `ipe run` compile an emitted **Rust** project, so most of the
wall-clock is `rustc` plus linking. Three optional, per-machine tools cut that
substantially. None is required and none is shipped in your program — you install
them on your development machine, point cargo at them once, and every `ipe build`
gets faster.

| Tool | What it does | Best for |
|---|---|---|
| [**sccache**](https://github.com/mozilla/sccache) | caches compiled crates across builds and projects | skipping recompiles of unchanged dependencies |
| [**mold**](https://github.com/rui314/mold) / [**lld**](https://lld.llvm.org/) | a much faster linker | the edit → rebuild loop (linking dominates incremental relinks) |
| [**cranelift**](https://github.com/rust-lang/rustc_codegen_cranelift) | a `rustc` backend that generates *debug* code ~2–5× faster than LLVM | fast `ipe build` / `ipe run` during development (debug only) |

> The recipes below were verified on Linux x86-64. The per-platform **install**
> commands follow each tool's own documentation (linked above) — check those
> pages for your exact platform and version. The `~/.cargo/config.toml` snippets
> are platform-agnostic except where a target triple is shown.

## Quick start (all platforms)

Put the shared settings in your global cargo config, `~/.cargo/config.toml`, so
they apply to every emitted project:

```toml
# ~/.cargo/config.toml
[build]
rustc-wrapper = "sccache"        # compilation cache
```

Then add a linker for your platform (next section) and — if you want the fastest
debug builds — cranelift. You can combine all three.

## Install per platform

Ipê supports Linux, macOS, Windows, and FreeBSD. Install the tools with your
platform's package manager:

```sh
# Linux — Debian / Ubuntu
sudo apt-get install -y mold clang
cargo install sccache            # or: sudo apt-get install -y sccache

# Linux — Fedora
sudo dnf install -y mold clang sccache

# Linux — Arch
sudo pacman -S mold clang sccache

# macOS (Homebrew) — use lld; mold does not target macOS
brew install sccache llvm        # llvm provides ld.lld

# Windows — rust ships `rust-lld`; just add sccache
winget install Mozilla.sccache   # or: cargo install sccache

# FreeBSD
pkg install sccache mold llvm
```

cranelift is a rustup component (nightly), the same on every platform:

```sh
rustup toolchain install nightly
rustup component add rustc-codegen-cranelift-preview --toolchain nightly
```

## A faster linker

Linking runs on almost every rebuild, so a fast linker is often the biggest win.
Add the block for your platform to `~/.cargo/config.toml`:

```toml
# Linux — mold
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-fuse-ld=mold"]

# macOS — lld (from `brew install llvm`)
[target.aarch64-apple-darwin]
rustflags = ["-C", "link-arg=-fuse-ld=lld"]

# Windows (MSVC) — the bundled rust-lld (needs a recent Rust)
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "linker-features=+lld"]

# FreeBSD — lld is the system default linker; usually nothing to configure
```

> On older GCC (< 12) `-fuse-ld=mold` may be unsupported; use `mold -run cargo
> build`, or switch the compiler to clang. On recent Xcode, Apple's default
> linker is already fast, so lld on macOS is optional.

## cranelift — fastest debug builds

cranelift skips LLVM's optimizer, so it only helps **debug** builds (`ipe build`
/ `ipe run`), never `--release`. It needs the nightly toolchain. Add to
`~/.cargo/config.toml`:

```toml
[unstable]
codegen-backend = true

[profile.dev]
codegen-backend = "cranelift"
```

and build with nightly:

```sh
cargo +nightly build            # ipe drives cargo; select nightly via rustup override
```

Or, without editing config, per-invocation:

```sh
CARGO_PROFILE_DEV_CODEGEN_BACKEND=cranelift cargo +nightly build -Zcodegen-backend
```

## Combining them

The three are independent and stack: sccache caches compilation, the linker
speeds the final link, cranelift speeds debug codegen. A full development
`~/.cargo/config.toml` on Linux:

```toml
[build]
rustc-wrapper = "sccache"

[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-fuse-ld=mold"]

[unstable]
codegen-backend = true

[profile.dev]
codegen-backend = "cranelift"
```

Keep cranelift out of your `--release` path (it does not optimize), and remember
`[unstable]` requires the nightly toolchain.
