The command-line flags several `ipe` commands share, described once. A command page lists one with `- @<flag>`.

- `[--accept-risks]` — accept every disclosed .Unsafe escape-hatch import and proceed without prompting
- `[--allocator <auto|system|dlmalloc|talc|mimalloc>]` — select the global allocator (default: auto)
- `[--allow-slow-allocator]` — permit an allocator known to be slow for the target
- `[--cfree]` — build without linking any C code (incompatible with allocators that require C, e.g. mimalloc)
- `[--emit-permissions <ios|macos|android>]` — read-only: print the OS-permission declarations the app's accepted web capabilities derive on the platform, and build nothing
- `[--json]` — emit each diagnostic as a stable JSON object (one per line) instead of the human layout
- `[--out <dir>]` — write the emitted project to <dir>
- `[-q|--quiet]` — suppress progress chatter; only warnings and errors
- `[--runtime <dir>]` — vendor the Ipê runtime from <dir>
- `[--static]` — produce a statically linked binary
