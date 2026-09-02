Status: Accepted
Date: 2026-09-02

# 0063. Dev-loop Model-state reset escape hatches

## Context

`ipe watch` persists the running Model to a dev session store so a rebuild
preserves live state. An additive-superset algorithm (`web::additive::reconstruct`)
splices the checkpoint onto the new binary's `init` value when the Model gains
new fields; any other change (field removal, rename, retype) forces a clean
`init`. Both paths are fail-closed: a splice happens only on a proven additive
superset, and even then only if the merged object decodes strictly.

Two situations call for a forced reset:

1. **`ipe watch --reset-state`**: the developer wants every returning session
   to start from a clean `init` for an entire binary's lifetime — e.g. after
   a deliberate database-schema migration or when debugging init-path behaviour.
2. **Debugger "reset to init"**: the developer is using the TUI or web debugger
   overlay and wants to restart the recorded history and the live session from
   a clean `init` without restarting the server.

The challenge is providing these escapes without weakening the preservation
guarantee for the normal path. The checkpoint is server-side (never
client-forgeable); the splice algorithm is bounded, strict, and fail-closed; the
reset paths must be equally fail-closed.

## Decision

### `ipe watch --reset-state`

`WatchOptions` gains a `reset_state: bool` field (default `false`). When set,
`child_env` injects `IPE_WEB_RESET_STATE=1` into the child binary's environment.
The runtime gate `reset_state_from_env()` reads this variable once per request
and — when truthy — skips `get_reconstructing` entirely, treating every incoming
session cookie as a miss and forcing a fresh `init`. The flag is fail-closed: any
value other than `"1" | "true" | "yes" | "on"` leaves the additive algorithm in
place.

`IPE_WEB_RESET_STATE` is registered in the `ENV_VARS` table under the `Web`
subsystem as `Tunable`. It is a dev-loop variable; the CLI only sets it via
`--reset-state`, which itself is a dev command.

### Debugger "reset to init"

`RecordBuffer::reset_to_init(init)` clears the step log and replaces the base
with `init`. `History::reset_to_init` and `TuiDebugger::reset_to_init` delegate
to it, with `TuiDebugger` additionally clearing the scrub index (returning to
live mode).

The TUI key **Ctrl-R** maps to `RESET_KIND/RESET_VALUE` constants (mirroring the
existing Ctrl-T / Ctrl-Left / Ctrl-Right constants). The web overlay gains a
"↺ reset" button (`data-ipe-dbg-reset`) that POSTs to `/_ipe/debug/reset` (a new
debugger-feature-gated endpoint); on success the page reloads.

The `/_ipe/debug/reset` handler builds a fresh `init` from the current request
context (identical to the cold-start path), replaces the live session's model and
history, and returns 200. The CSRF middleware protects it. It is registered only
when the `debugger` feature is active.

### What was considered and rejected

**Client-side checkpoint deletion**: A client cannot forge or delete a
server-side session row. Any client-driven reset would need an authenticated
endpoint — identical complexity to `/_ipe/debug/reset` — with no benefit.

**Environment variable alone (no `--reset-state` flag)**: `IPE_WEB_RESET_STATE`
could be set by hand. Keeping a named flag makes the intent explicit in the
watch command and ensures the variable is documented and auditable.

**Resetting the file-store row on flag**: Deleting the persisted checkpoint on
reset would also work but is unnecessary — the runtime gate prevents the row from
being read, and the row is overwritten on the next `set` call.

## Consequences

The additive-preservation algorithm is unchanged. The two reset paths are strictly
additive escape hatches layered on top of it.

Invariants that must continue to hold:

- `reset_state_from_env()` is fail-closed: an absent or unrecognised value must
  never trigger a reset.
- `/_ipe/debug/reset` is protected by CSRF and is gated behind `#[cfg(feature =
  "debugger")]`; it must never appear in a release build.
- `IPE_WEB_RESET_STATE` must never be set by `child_env` on the non-`--reset-state`
  path.
- `RecordBuffer::reset_to_init` must not fire any `update` call or `Cmd`.
