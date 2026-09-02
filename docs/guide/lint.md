# Linting

`ipe lint` runs extensible static analysis over your source. It reasons about
code that already type-checks: the compiler enforces what *must* be true
(soundness — it may only reject), and the linter suggests what *should* be true
by convention (idiom, consistency, safety-by-convention). Every finding is
advisory and suppressible, and the same rules surface three ways — on the CLI,
live in the editor through the LSP, and in CI.

Every command and configuration on this page has been run as written.

## Running it

```sh
# Lint the current project:
ipe lint

# Lint a single file or a directory:
ipe lint src/Main.ipe

# Apply every machine-applicable fix and report what changed:
ipe lint --fix
```

A finding at or above the configured gate severity exits non-zero, so `ipe lint`
drops straight into CI as a gate. `ipe lint --help` lists exactly the rules that
ship — nothing it prints is unimplemented.

## Why a linter, not more compiler

`connect : String -> Int -> Task Error Conn` is **sound** — the type checker
must accept it. Whether *that* `Int` is a port that should be `Ipe.Net.Port` is
a contextual, heuristic judgement (it reads the parameter name, the domain, how
siblings are typed). A sound type system knows only universal invariants ("once
you *have* a `Port`, it is in range"); it cannot know you *should have chosen*
`Port` here. That gap — from "sound" to "idiomatic / safe-by-convention" — is the
linter's territory. It is what lets Ipê keep a small, refinement-free core and
still drive the ecosystem toward "invalid states unrepresentable".

## The rules

| Rule | What it flags | Auto-fix |
| --- | --- | --- |
| `prim-param` | An exported signature takes a bare primitive (`Int`, `String`) at an API edge where a domain newtype fits (`Ipe.Net.Port`, `Ipe.Url.Url`). | advisory |
| `adjacent-bools` | Two or more adjacent `Bool` parameters that call sites cannot tell apart (`render True False` — which flag is which?). | advisory |
| `wrapper-consistency` | A parameter that sibling exported APIs wrap as a newtype, but one signature leaves bare. | advisory |
| `unsafe-convention` | A call to an `unsafe*` / `.Unsafe` escape hatch, surfaced so its use is deliberate and reviewed. | advisory |
| `prefer-pipeline` | A nested call chain that reads clearer left-to-right as a `\|>` pipeline (`f (g x)` → `x \|> g \|> f`). | ✅ `--fix` |

An **advisory** rule reports and teaches but never rewrites your code: its remedy
would change an exported signature and thread every call site, which is a design
decision, not a mechanical edit. `prefer-pipeline` is the exception — `x |> f`
desugars to exactly `f x`, so its rewrite is provably equivalent and `--fix`
applies it safely. Re-running `ipe lint --fix` is idempotent: it reports "no
machine-applicable fixes" once every fix has landed.

## Configuring it — `lint.ipe`

Configuration is Ipê-native. A `lint.ipe` next to your `package.ipe` declares a
single `lint` value, built in the same builder style as the manifest (not TOML):

```ipe
module Lint exposing (lint)

lint =
    Lint.config
        |> Lint.deny "adjacent-bools"
        |> Lint.allow "prim-param"
        |> Lint.gate "deny"
```

- `Lint.allow "<rule>"` — silence a rule entirely.
- `Lint.warn "<rule>"` — report it without failing the gate.
- `Lint.deny "<rule>"` — report it and fail the gate when it survives.
- `Lint.gate "allow" | "warn" | "deny"` — the severity at or above which a
  surviving finding exits non-zero (default `deny`).

The file is *read*, never evaluated — the reader walks the parsed `lint` binding
and recognises each `Lint.*` builder by name. An unknown rule name fails closed
with a `lint.ipe:line:col` message, so a typo is a clear error, never a silent
no-op.

## Suppressing one site

Silence a single occurrence with a source comment on the line, or on the line
directly above it:

```ipe
-- ipe-lint: allow adjacent-bools
render : Bool -> Bool -> String
```

Use `-- ipe-lint: allow all` to silence every rule for that site. Suppression is
by rule name — there are no numeric codes to memorise.

## In the editor

The LSP surfaces lint findings as diagnostics alongside the compiler's own, so
the same advice you get from `ipe lint` appears live as you type — the way clippy
flows through rust-analyzer. See [editor integration](../topics/editor-integration.md)
for setup.
