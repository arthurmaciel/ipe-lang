# iced-counter — an Iced binding spike

A minimal [Iced](https://iced.rs) counter, mapped onto Ipê's TEA over the REAL
crates.io `iced` crate (auto-FFI-bound, shim-free). Iced's Elm architecture is
Ipê's, so each piece maps to one `[rust.provide.*]` form in `ipe.toml`:

| Iced piece | Ipê shape | `[rust.provide.*]` form | Status |
|------------|-----------|-------------------------|--------|
| `Model` (`Counter`) | a struct | `[[rust.provide.struct]]` | emitted, cargo-builds |
| `Message` (`Increment`/`Decrement`) | an enum | `[[rust.provide.enum]]` | emitted, cargo-builds |
| `update : Message -> Model -> Model` | a sync closure | `[[rust.provide.closure]]` | scalar subset only |
| `view : Model -> Element Message` | a sync closure | `[[rust.provide.closure]]` | blocked (opaque return) |

## What binds today (the SEAL that holds)

`ipe install` runs the sandboxed inspector over `iced = 0.12.1` and merges the
`[rust.provide.*]` decls into the generated `_bindings.rs`. For the Model + the
Message, the driver emits **real, self-contained Rust**:

```rust
#[derive(Clone, Default)]
pub struct Counter { pub value: i64 }
pub fn iced_counter_new(arg0: i64) -> Counter { Counter { value: arg0 } }

#[derive(Clone, Debug)]
pub enum Message { Increment, Decrement }
pub fn iced_message_new_increment() -> Message { Message::Increment }
pub fn iced_message_new_decrement() -> Message { Message::Decrement }
```

These definitions **cargo-build against real Iced 0.12.1 as a working
`iced::Sandbox` counter** — the emitted `Counter` is the `Sandbox` Model, the
emitted `Message` is its `type Message`, and a real `update`/`view` folds over
them. (A GUI need not run headless; the cargo-build is the SEAL proof. See the
spike's build log.) The `Debug` derive is in the allowlist precisely because
Iced's `Sandbox::Message: Debug` bound requires it — `Debug` is total for every
closed carrier, so it carries no IEEE-754 hazard (unlike `Eq`/`Ord`/`Hash`).

## The exact remaining block (why `Main.ipe` is a placeholder)

The Ipê-side **forwarder plumbing** for provide-defined types is not wired yet.
The FFI interface admits the emitted `_bindings.rs` definitions, but it does NOT
yet surface:

* an Ipê-held opaque nominal (`type Counter` / `type Message`) plus a forwarder
  the Ipê program can call to construct one, and
* a way to hand a boxed Ipê closure (`update`/`view`) to Iced's `run` entrypoint.

So the driver's counter loop cannot yet be *entered* from Ipê. This is the same
gap the neighbouring `bevy-game` example documents for `Component`/system-fn.
`Main.ipe` therefore names the surface the bindings define but does not drive
the loop.

Two further Iced-specific gaps sit on top of that plumbing:

* **Opaque-return closures.** `view` returns `Element<Message>` — an opaque,
  lifetime-parameterised handle. The sync closure adapter only lifts scalar /
  `Result`/`Option` returns today; an opaque return needs the opaque-map +
  boxed-closure-as-Ipê-value plumbing.
* **`provide.struct`/`provide.enum` opaque fields/payloads.** A field or variant
  payload of a crate-opaque type (`Element`, `Command`) over-drops at decode
  until the opaque-map is threaded into the definition emitter.

All three are filed to the FFI backlog (see the PR body).

## Regenerating the bindings

The workflow below is the standard provide-surface flow (as used by
`bevy-game`). It is shown for reference: `ipe install` emits the
`_bindings.rs` shown above, but `ipe build` currently stops at the forwarder gap
documented above — the emitted definitions compile, but `Main.ipe` cannot yet
drive the Iced loop. The definitions' cargo-build against real Iced was proven
directly by the spike (a real `iced::Sandbox` around the verbatim emitter
output), not by the `ipe build` path.

```
cd examples/iced-counter
ipe install --yes --allow-build-scripts   # sandboxed; writes .ipe/cache/ffi/rust (gitignored)
ipe build                                 # blocked at the forwarder gap (see above)
```
