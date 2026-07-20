# Hello World

The simplest Ipê programme — prints a greeting to stdout. It anchors the
static-compilation gate (`--static` musl build, run in a `scratch` container)
and the release smoke-test, so it stays a first-party, dependency-free example.

## Build & run

```bash
ipe run ipe.toml
```
