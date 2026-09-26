Build the production artifact — optimised, Debug.* gated. Native-bearing apps get a jailed bundle; pure-native apps get a plain optimised binary; `web desktop|ios|android` produces a production app bundle; `--target wasm` produces a production browser bundle.

```
ipe release [<path>] [<shape>] [<runtime>] [<host>]
```

## Arguments

A source file, a project directory, or a package.ipe (default: the current project). shape/runtime/host are the delivery grammar shared with `build`: a `desktop`/`ios`/`android` host lays out the production app bundle for that host (a self-contained desktop bundle, or a native mobile system-webview shell). With no delivery args, `release` builds every delivery declared in package.ipe (`build` builds only the default one). Signing is release-time env, never in package.ipe.

## Options

- `[--out <dir>]` — write the artifact to <dir> (default: release/)
- `[--target wasm|<triple>]` — produce a browser bundle (`wasm`) or a musl-static binary for <triple> (default: x86_64-unknown-linux-musl)
- @--emit-permissions
- `[--runtime <dir>]` — vendor the Ipê runtime source from <dir>
- `[--bundle]` — native-bearing only: multi-file opt-out — wrapper + app + profile as siblings (app binary can be run directly, bypassing the sandbox)
- `[--embed]` — native-bearing only: default single self-jailing binary (app + profile fused into wrapper)
- `[--capabilities] [--plain|--json]` — print the inferred capability model for the app without building
