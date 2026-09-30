# Vendored ACE editor

The playground page holds a launch token that authorises code execution on
the operator's machine, so it runs no script from another origin. These are
unmodified files from the `ace-builds` npm package, version **1.35.4**,
directory `src-min-noconflict/`:

- `ace.js` — the editor core;
- `ext-themelist.js` — the theme list the switcher offers;
- `mode-haskell.js` — the highlighting mode the editor uses;
- `theme-<name>.js` — one per theme `ext-themelist.js` lists, which ACE loads
  lazily from this directory.

Source tarball: `https://registry.npmjs.org/ace-builds/-/ace-builds-1.35.4.tgz`,
npm integrity
`sha512-r0KQclhZ/uk5a4zOqRYQkJuQuu4vFMiA6VTj54Tk4nI1TUR3iEMMppZkWbNoWEgWwv4ciDloObb9Rf4V55Qgjw==`.
`SHA256SUMS` records each file's digest (`sha256sum -c SHA256SUMS`, checked
by `tools/scripts/lib/playground-verify.mjs`). `LICENSE` is ACE's BSD license,
from the same tarball.

To change the version or the file set: extract the new tarball's
`src-min-noconflict/` files listed above, regenerate `SHA256SUMS`, and update
the version and integrity here.
