# Upstream example corpus

[![examples sweep](https://github.com/arthurmaciel/ipe-lang/actions/workflows/examples-sweep.yml/badge.svg)](https://github.com/arthurmaciel/ipe-lang/actions/workflows/examples-sweep.yml)

These examples are cloned from the [Sky project](https://github.com/anzellai/sky)
and serve as a reference for our runtime.

Each CI examples sweep refreshes this directory from the current upstream Sky
corpus, applies the Ipê-adaptation patches in [`ipe-patches/`](ipe-patches/),
and builds and runs each one against the Ipê compiler. The badge above reflects
that sweep's pass/failure — a live proof that Ipê builds and runs the real
upstream corpus.

## License & copyright

The Sky project is licensed under the **Apache License, Version 2.0**. The
example sources mirrored here retain their original Sky copyright and license;
they are included under the terms of that license. This project's own
contribution is the adaptation layer under `ipe-patches/`, licensed under the
same Apache License, Version 2.0 as the rest of Ipê (see the repository-root
`LICENSE` and `NOTICE`).
