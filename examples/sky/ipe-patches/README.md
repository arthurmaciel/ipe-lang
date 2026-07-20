# `examples/sky/ipe-patches/`

Per-example semantic-delta patches, applied by `scripts/lib/mirror.sh` AFTER the
shared `rename-map.tsv` token rewrite. One optional `<name>.patch` per example
(a `patch -p1` unified diff, relative to the mirrored example root).

A patch lives here ONLY when the shared token rewrite cannot produce a
buildable-and-runnable Ipê example on its own. Every example today builds from
the token rewrite alone, so this directory holds no patches — it is the tracked
slot for the day one is genuinely needed. A patch that fails to apply is a RED
sweep row, never silently ignored; a behavioural gap a patch would paper over is
filed in `../BLOCKERS.md` instead.
