# Index entry schema — `packages/<name>.toml`

This is the canonical schema for one package's entry in the curated index. It is
the source of truth; the resolver's parser (`src/ipe-cli/src/index.rs`) and the
publish renderer (`src/ipe-cli/src/publish.rs`) both implement it, and
`ipe package validate-entry` validates a file against it by reusing that parser.
Where the prose here and the parser ever disagree, this document governs and the
divergence is a bug to fix in the parser.

## Layout

The index is a git repository. Every package has exactly one entry file at:

```
packages/<name>.toml
```

The **file stem is the authoritative package name** — the resolver reads
`packages/http-extras.toml` as the package `http-extras`. A top-level `name` key
inside the file is informational and, if present, must match the stem.

## Entry format

A top-level header followed by one `[[version]]` array-of-tables block per
published version:

```toml
name = "http-extras"          # informational; must equal the file stem
publisher = "arthurmaciel"    # required — the publishing account

[[version]]
version = "1.2.0"
source = "https://github.com/arthurmaciel/http-extras"
rev = "9f2c7b1e0a4d5c6f8b2a1e3d4c5b6a7f8e9d0c1b"
sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
capabilities = ["network"]
```

Comments (`#`) and blank lines are ignored. An unrecognised key is ignored
(forward-compatible), but a malformed *known* value is a hard error — the schema
is fail-closed on the fields it defines.

## Top-level fields

| Field | Required | Type | Meaning |
| --- | --- | --- | --- |
| `name` | no | string | Informational; the authoritative name is the file stem. |
| `publisher` | **yes** | string | The publishing account. Provenance for the entry. An entry with no `publisher` is rejected. |

## Per-`[[version]]` fields

Every field below is **required**; a version missing any one is rejected.

| Field | Type | Meaning |
| --- | --- | --- |
| `version` | semver string | The exact published version. Must parse as [semver](https://semver.org). Ipê is pre-1.0, so `major` is reserved. |
| `source` | string | The source repository URL, fetched with `git`. |
| `rev` | string | The exact commit fetched — a version pins an immutable revision, never a moving branch. |
| `sha256` | hex string | The sha256 of the source tree at `rev`. The resolver trusts a fetched tree only when its hash equals this (verify-before-trust). This is the integrity anchor: a swapped source tree fails the check. |
| `capabilities` | array of strings | The capability set the publisher declared for this version, surfaced for consent at `ipe add`. May be empty (`[]`) for a pure package with no effects. |

### `EntryVersion` — the parsed value

Each `[[version]]` block parses into a typed `EntryVersion`
(`src/ipe-cli/src/index.rs`): `version: semver::Version`, `source: String`,
`rev: String`, `sha256: String`, `capabilities: BTreeSet<Capability>`. A version
that parses is one the resolver will accept.

## Capability vocabulary

`capabilities` entries are drawn from a **closed** vocabulary shared with the
compiler's kernel registry (`src/compiler/kernels/src/capability.rs`). An unknown
name is a hard rejection — a typo can never become a silently-dropped capability
the user is then not warned about. The wire names are:

| Wire name | Capability |
| --- | --- |
| `network` | Outbound/inbound network access. |
| `filesystem` | Reading or writing the filesystem. |
| `database` | Structured database access. |
| `env` | Reading/writing process environment (env vars, argv). |
| `subprocess` | Spawning or controlling a child process. |
| `clock` | Reading wall-clock/monotonic time, sleeping, timers. |
| `random` | Drawing non-deterministic randomness. |
| `native-ffi` | Crossing into native `Rust.` code — the signal that the capability set cannot be inferred from Ipê alone. |

## Ordering and immutability

- Versions are re-rendered in ascending semver order when publish appends a new
  one; the resolver scans all of them for the highest match rather than relying on
  file order.
- A published version is **immutable**: republishing an existing `version` is a
  refusal, never a silent overwrite. A change to already-published bytes is
  therefore only possible under a new version number, which the enforced-semver
  gate then classifies.
