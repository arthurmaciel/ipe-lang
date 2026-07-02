The Rust backend development is guided by the following principles:

1. **Security** — generated code and runtime must give an attacker no foothold: no injection (SQL, shell, path, header, log), no secret leakage into logs or errors, no authentication or CSRF bypass, no timing oracle on a secret comparison, and no unbounded resource a remote party can exhaust. When a program handles untrusted input, the safe outcome must be the only reachable outcome.

2. **Correctness** — the backend must produce the right answer: for the same well-typed Sky program and the same input, the Rust output must match the Go reference's observable behaviour (ideally byte-for-byte), and any deliberate divergence must be documented rather than silently wrong. A program that compiles and runs but yields a value different from what the language semantics specify is incorrect.

3. **Soundness** — a well-typed Sky program must never be able to trigger a runtime failure in the generated Rust: no panic, no .unwrap()/.expect() blowup, no out-of-bounds index, no integer-overflow abort, no unchecked downcast, and no undefined behaviour. Where correctness is "the result is right," soundness is the stronger structural guarantee that "no input can make the program fall over" — the type system's promise is honoured all the way down to the binary.

4. **Efficiency** — within the bounds set by the three principles above, the code should be fast and lean: no needless allocation or cloning, no re-computation on hot paths, no O(n²) where O(n) is trivial, and a small binary and memory footprint. Efficiency is pursued only after security, correctness, and soundness, never by trading one of them away.

5. **Completeness** — the backend should cover as much of the Sky language and standard library as possible, so that real programs build and run without hitting an "unsupported" wall. A missing kernel or unimplemented feature is a completeness gap; it is a legitimate, documented limitation rather than a bug, but the goal is to keep shrinking that set.

6. **Readability** — The code (both the Haskell codegen and the generated Rust) should be clear, well-named, and maintainable, so the next person — human or agent — can understand and safely change it. It ranks last only in the sense that a readable name is never allowed to break correctness or a clean abstraction is never allowed to open a soundness hole; everything else being equal, the clearer form wins.
  
**The ordering is a strict tie-breaker, not a weighting**: whenever two principles conflict at a specific decision, the higher-numbered one yields to the lower — a faster path that opens a soundness hole is rejected, a more readable form that breaks correctness is rejected — so a lower principle can never justify compromising a higher one.

## The two fundamental rules

Independent of and beneath the ranked principles, every design and every code pass must obey two non-negotiable laws:

- **Parse, don't validate.** Convert untrusted or untyped input into a precise typed value at the boundary, once, so downstream code cannot re-encounter the unvalidated form. A function that takes a broad type and re-checks it everywhere is a smell; the check should happen once and produce a narrower type that makes the checked property structurally true thereafter. In this compiler: foreign/JSON/config values enter through a typed decode point; error channels are typed (`Diagnostic`/`Error`), never `String`.

- **Make invalid states unrepresentable.** Encode invariants in the types so an illegal combination cannot be constructed at all, rather than relying on a runtime guard or convention. Prefer a sum type over a bool-pair that admits impossible combinations; prefer an exhaustive `match` (no wildcard that silently swallows a new variant) so an unhandled case is a compile error; prefer a smart constructor over a public field that could hold an out-of-range value. In this compiler specifically: a kernel that the resolver recognises but the type-scheme table does not cover must be a compile-time error, never a silent flexible type variable that lets an ill-typed program pass the type-checker and fail only at the downstream Rust build (the "exit-0-then-cargo-fail" class).

These two rules are the structural machinery by which the ranked principles — especially Soundness and Correctness — are actually achieved: the ordering says *what wins in a conflict*; these two rules say *how you build code that doesn't create the conflict*.
