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
`LICENSE` is ACE's BSD license, from the same tarball. Every file in this
directory is served under `/static/vendor/ace/`, so `SHA256SUMS` records the
digest of each one but itself, this README and `LICENSE` included
(`sha256sum -c SHA256SUMS`; `tools/scripts/lib/playground-verify.mjs` refuses
a file it does not record).

To change the version or the file set: extract the new tarball's
`src-min-noconflict/` files listed above, update the version and integrity
here, and regenerate `SHA256SUMS`.
