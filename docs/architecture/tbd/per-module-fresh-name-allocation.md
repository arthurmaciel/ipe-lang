# Per-module fresh-name allocation in `ipe_lower`

## Why fresh-name numbering blocks per-module lowering

`ipe_lower::lower` (`src/compiler/lower/src/lib.rs`) mints seven fresh-symbol
pools before the lowering walk. The obstacle to a per-module `lower_module`
query is that some pool names are numbered from **whole-program** quantities:
lowering a module in isolation would number its fresh names differently than
lowering it inside the linked whole program, changing emitted bytes. The
golden-oracle SEAL pins those exact names (`param_patterns` locks
`arg_0 … arg_9`; `firstclass_curried` locks `eta_1 eta_2` in emitted Rust), so
any renumbering is a regression, not a re-bless.

## The two naming disciplines, precisely

The pools split into two kinds with very different whole-program coupling.

### The position-indexed pools (`eta_` / `cap_`) — already per-module-deterministic

`eta_` (`eta_params`) and `cap_` (`cap_params`) are consumed at a **scope-local
position**, never through a monotonic cursor:

- `self.eta_params.get(i)` where `i` is the eta-lambda's own parameter position
  (`eta_pad.len()`, the ctor-arg index, the promotion-shim offset) — see
  `lower.rs` around the `eta_expand_partial*` and ctor/promotion sites.
- `self.cap_params.get(cap_cursor)` where `cap_cursor` is a **per-site local**
  counter, reset per capture group.

The string a position receives is `format!("eta_{n}")` / `format!("cap_{n}")`
from `Interner::fresh_symbols`, whose suffix `n` is a per-prefix counter
starting at `0` that skips only names already interned (all user identifiers).
So the string for local position `i` is a function of **`(prefix, i,
set-of-interned-identifiers)` only** — never of pool *size*, never of symbol
numbering. The pools are sized `max(max_def_arity(m), 16)` from the whole
program, but that size is **byte-neutral**: it controls only *how many*
symbols are minted, not *which string* any position gets (every consume site
fails closed as a `bug` if the pool is too small — never a silent reuse).

Consequence: **these pools are already per-module-deterministic in their emitted
names.** Lowering a module in isolation and sizing its eta/cap pool by that
module's own `max_def_arity` (floor 16) yields byte-identical output, because
`eta_0 … eta_k` are the same strings regardless of pool length.

### The monotonic-cursor pools (`arg_` / `anyp_` / `destr_thunk_` / `ncons_` / `nstrlit_`) — the genuine whole-program dependency

`arg_` (`param_binders`), `anyp_` (`any_param_binders`), `destr_thunk_`
(`destructure_thunk_binders`), `ncons_` (`nested_cons_binders`), and `nstrlit_`
(`nested_strlit_binders`) are handed out through a single **module-global
monotonic cursor** (`param_cursor` etc.) in the order `Lowerer::run` walks
`m.defs`, each def in a fixed pre-order (`count_destructure_param_sites`'s
`walk_expr` shape). The string a site receives is `arg_<global_index>`, where
`global_index` = (count of qualifying sites in **every earlier def**) + (its
local rank within its own def).

`link::link` builds `m.defs` by `defs.extend(m.defs)` **module by module in the
`modules`-vec order**, each module's defs in source order. So `m.defs` is
partitioned into contiguous, home-grouped runs. The global index of the first
site in module *M* is therefore the **prefix sum** of qualifying-site counts
over every module before *M* in that order.

This is exactly what breaks per-module isolation: module *M*'s first `arg_`
site is `arg_(prefix_sum_before_M)` in the whole program but `arg_0` in
isolation. **These monotonic-cursor pools are where a naive per-module scheme
drifts goldens.**

## The deterministic per-module scheme

Name every monotonic-cursor site as

```
name(site) = "<prefix>" ++ (module_base_offset(home_of_site) + local_index(site))
```

where

- `local_index(site)` is the site's rank in the **pre-order traversal of its own
  module's defs** (in source order) — a per-module-stable input, identical
  whether the module is lowered alone or inside the linked program;
- `module_base_offset(home)` is the prefix sum of per-module qualifying-site
  counts over every module ordered before `home`, one value per (prefix, home).

The offset table is the *only* whole-program input, and it is a small
`BTreeMap<home, [usize; 5]>` (five monotonic-cursor prefixes) derivable from the
module set + each module's own count — never from symbol numbering or interner
state.

The position-indexed `eta_` / `cap_` pools need no offset: size each module's
pool by its own `max_def_arity` (floor 16).

### Byte-identity proof (whole-program path)

On the whole-program path the offset table is computed as the prefix sum over
the *same* home-grouped, source-ordered `m.defs` the current cursor already
walks. By construction, for every site,

```
module_base_offset(home) + local_index(site)
  = (sites in all earlier defs)          // prefix sum over earlier modules
  + (rank within this def, accumulated)  // local pre-order rank
  = current global cursor value at that site.
```

So every monotonic-cursor name is unchanged and the position-indexed names were
never size-dependent — the emitted bytes are identical. The
`clean_vs_incremental_parity` suite (warm vs cold, whole golden corpus) plus
every `emits_byte_identical_main_rs` golden is the machine-checked witness.

### Per-module isolation argument

Under this scheme, a `lower_module(home)` query needs only `home`'s own AST
(for `local_index`) and its `module_base_offset` (five integers). The offset is
a pure function of the linked module set's *shape* (which modules, in what
order, each module's own site count) — not of any module's *body text*. So a
body-only edit that preserves site counts leaves every module's offset
unchanged, and only the edited module's `lower_module` re-executes. An edit
that changes a module's site count shifts the offsets of all later modules,
correctly invalidating exactly their `lower_module` memos — no stale-cache
hole. For that query, the offset table is a tracked salsa query; the naming is
expressible as `base + local` without drift.

## What is wired

The position-indexed decoupling is wired. The `eta_` / `cap_` pools are sized
through an explicit per-module maximum (`max over modules of
max_def_arity(module)`, floor `MAX_CALLEE_ARITY`) rather than a bare
whole-program `max_def_arity(m)` call. The two are numerically equal — `max`
over the whole is the `max` of the per-module maxima — so the change is provably
byte-identical, and it lands the home-grouping primitive (`defs_by_home`) that
the monotonic-cursor offset table is built on, exercised end to end by the
parity suite.

The monotonic-cursor two-level cursor (`module_base_offset + local_index`) is
**specified above but not yet wired**, because computing and threading the
offset table is inseparable from the `lower_module` boundary it exists to serve:
on the whole-program path it is a numeric no-op (it reproduces the current
cursor), so wiring it without that query would add machinery with no observable
behaviour and no independent test surface until a per-module query consumes it.
It ships with the `lower_module` query, gated by the same parity suite, against
this spec.

## Gate

- `clean_vs_incremental_parity.rs` (warm-vs-cold, whole golden corpus) green.
- Every `emits_byte_identical_main_rs` / seal golden unchanged.
- `cargo build --workspace`, `clippy -D warnings`, `fmt --check` clean.
