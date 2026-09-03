Status: Accepted
Date: 2026-09-02

# 0066. The dev-loop speed lever is the data-patch tier

## Context

The inner development loop — edit source, see the result — has a performance
floor set by how much the change forces to recompile. Several architectural
levers could lower it, and they are not equally good; some help one edit class
and actively hurt the dominant one. The dominant edit class is view and
appearance changes.

A measured comparison of the candidate levers, on a common view edit:

- **Data-patch (compile nothing).** The running program reads a data table; a
  dev edit ships a new table over the live socket. Compiles nothing.
- **Native incremental recompile.** Already near its own floor; that floor
  decomposes into codegen, link, fixed incremental overhead, and a
  monomorphization walk, none of which an architectural change short of
  "compile nothing" meaningfully removes.
- **Splitting the app into core and view crates.** *Worse* on view edits;
  helps only logic edits on heavy apps, and it slows the very edit class that
  dominates.
- **Alternative codegen backends for faster incremental builds.** The codegen
  delta is too small to matter against the fixed overheads.
- **A separate view-host runtime.** Removes only the final binary relink; needs
  a large new runtime subsystem to buy that.

Nothing architectural short of compiling nothing beats the incremental floor
meaningfully, and only the data-patch tier compiles nothing.

## Decision

**Invest the dev-loop speed effort in the data-patch (program-as-data) tier.**
It is the only path that both beats the recompile floor by roughly an order of
magnitude *and* speeds the dominant edit class (view/appearance) — the same
class the crate split slows.

The competing levers are shelved with their reasons recorded so the question is
not re-litigated:

- **Crate split** — shelved: net-negative for the dominant UI-iteration edit
  class, and it addresses only logic edits on heavy apps.
- **Separate view-host runtime** — shelved: removes only the relink, at the cost
  of a whole new runtime subsystem.
- **Alternative incremental-codegen backends** — shelved: codegen delta too
  small to move the floor.

The incremental recompile path remains the correct *fallback* for edits the
data-patch tier cannot cover (genuinely new compiled logic); it is already near
its floor and is not the place to invest for speed.

## Consequences

Dev-loop work is directed at moving more of a program into patchable data (the
mechanism is its own decision), not at re-slicing the crate graph or swapping
codegen backends. A future proposal to split the app crates or add a view-host
runtime *for dev-loop speed* must first overturn the measured finding that they
do not beat the data-patch tier on the dominant edit class — the measurement,
not intuition, is the standard.

The invariant behind the choice: the dominant edit class is view/appearance, and
the only way to beat the recompile floor on it is to compile nothing. If that
premise changes — if logic edits on heavy apps become dominant — the shelved
levers (the crate split in particular) are the ones to reconsider.
