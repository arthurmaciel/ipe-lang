# Principles Audit — 2026-07-02

Merged ledger from seven whole-codebase guardian audit partitions: the exit-0
UI-kernel surface, the exit-0 non-UI stdlib surface, the PARSE-DON'T-VALIDATE
rule, the MAKE-INVALID-STATES-UNREPRESENTABLE rule, panic-soundness (principle
3), runtime-soundness (session/render/coerce), and a broad six-principle pass
(security / efficiency / completeness / readability).

Principle order (strict tie-breaker, from `PRINCIPLES.md`):
1 Security · 2 Correctness · 3 Soundness · 4 Efficiency · 5 Completeness ·
6 Readability.

Two fundamental rules:
- **PARSE, DON'T VALIDATE** — decode once at the boundary into a narrow typed
  value; typed error channels (`Diagnostic` / `Error`), never `String`; no
  validate-then-use.
- **MAKE INVALID STATES UNREPRESENTABLE** — sum types over bool-pairs,
  exhaustive match with no silent wildcard, smart constructors. A
  resolved-but-unschemed kernel must be a **compile error**, not a flexible
  `Ty::Var`.

The dominant material result across all seven partitions is a single
architectural root: kernel handling is spread over three independently
hand-maintained tables — canon `install_prelude_qualifiers` (what resolves),
`lower_callee` (what emits), `constrain::kernel_ty` (what gets a type scheme) —
with no exhaustiveness link between them, and `kernel_ty` **fails open**
(`_ => Ty::Var(u32::MAX)`, `constrain.rs:3984`) where `lower_callee` and
`naming::kernel_name` **fail closed**. Every kernel that resolves and emits but
carries no scheme therefore type-checks against anything: `skyc` exits 0 on an
ill-typed program and the emitted Rust fails `cargo build`. This is the
exit-0-then-cargo-fail class the fundamental rules explicitly forbid, and it is
the shared cause behind the M4c/M4d/M7/IntDiv holes in the memory ledger. It is
tracked as **Task #45**.

The security surfaces are otherwise sound and worth stating: SQL identifier
interpolation goes through a genuine `SqlIdent` smart constructor
(`db.rs:1097`); response headers fail closed at the axum boundary (a CRLF value
turns the whole response into a 500, never an injected header —
`server.rs:575`); `<style>` breakout XSS is closed by a total, fixpoint
`strip_style_close` on every user CSS fragment; HTML escaping matches Go's
`html.EscapeString` byte-for-byte; the console proxy strips the parent
`Authorization` secret before forwarding to the unauth'd child. The compiler
crates are panic-clean: `sky_types` has zero non-test unwrap/expect/panic/index
sites, and the backend routes internal invariant misses through
`Diagnostic::CompilerBug` (a `Result`) rather than panicking.

---

## 1. Exit-0-then-cargo-fail — closed inventory

Every kernel below is resolved by `lower_callee` and emitted as a concrete Rust
call by the backend, but has **no scheme arm** in `constrain::kernel_ty`, so it
falls to `constrain.rs:3984 _ => Ty::Var(u32::MAX)` and type-checks against any
argument. `skyc` exits 0; `cargo build` fails (E0308 / E0277 / E0282). The list
is finite and closed — this turns the whack-a-mole into a bounded inventory.

Reachability: canon registers every kernel as an auto-qualifier (the documented
no-import path), which produces `VarKernel` → `kernel_ty`. An explicit
`import Sky.Core.X as X` binds `VarHome::TopLevel` → the typed stdlib
signature → **safe**. So the hole is exactly the no-import auto-qualifier path,
**except** the six families that have no embedded stdlib `.sky` module at all
(Encoding / JsonEnc / JsonDec / JsonDecP / Jwt / Uuid) — those have no typed
path under any import and are unconditionally holed.

| Family (qualifier) | Count | Members | file:line | Class | Sev | Fix |
|---|---|---|---|---|---|---|
| `Html.*` | ~85 | render, toString, escapeHtml, escapeText, escapeAttr, attrToString, text, raw, node, voidNode, doctype, styleNode, titleNode, htmlNode, headNode, title, div, header(Node), span, a, link(Node), button, p, h1–h6, pre, code(Node), strong, em, small, nav, section, article, footer(Node), main(Node), aside, ul, ol, li, table, thead, tbody, tfoot, tr, th, td, textarea, select, option, label, form, fieldset, legend, blockquote, figure, figcaption, details, summary, dialog, video, audio, canvas, iframe, progress, meter, script, body, input, img, br, hr, meta, area, base, col, embed, source, track, wbr | scheme miss `constrain.rs:3984`; resolve `lower.rs:3929-4021`; emit `emit_expr.rs:1182-1878` | exit0 | High | Add `(Some("Html"), Some(name))` arms in lockstep with the resolve set; container tags `List (Attribute msg) -> List (Html msg) -> Html msg`, void tags `List (Attribute msg) -> Html msg`, escaping sinks `String -> String`. Security-adjacent: escapeHtml/escapeAttr/render/raw are the XSS-guard sinks and currently carry zero arg discipline. |
| `Ui.*` | 36 | none, text, html, spacing, padding, paddingXY, width, height, centerX, centerY, alignLeft, alignRight, alignTop, alignBottom, pointer, clip, clipX, clipY, scrollbars, scrollbarX, scrollbarY, gridColumns, px, fill, content, shrink, fillPortion, vh, vw, minimum, maximum, rgb, rgba, white, black, transparent | `constrain.rs:3984`; resolve `lower.rs:3936-3977`; emit `emit_expr.rs:1252-1592` | exit0 | High | Add arms mirroring the resolve set. Requires adding `Length` and `Color` nominal builtins to `Builtins`. Nullary ones (centerX, fill, white, none) are additionally mis-typeable outside attr context (`let n = Ui.centerX in n + 1`). |
| `Background.*` | 2 | color, image | `constrain.rs:3984`; resolve `lower.rs:3979-3990`; emit `emit_expr.rs:1595-1718` | exit0 | High | color `Color -> Attribute msg`; image `String -> Attribute msg`. Depends on the `Color` builtin. |
| `Border.*` | 3 | width, rounded, color | as above | exit0 | High | width/rounded `Int -> Attribute msg`; color `Color -> Attribute msg`. |
| `Font.*` | 5 | size, color, family, bold, italic | as above | exit0 | High | size `Int -> Attribute msg`; color `Color -> Attribute msg`; family `String -> Attribute msg`; bold/italic `Attribute msg`. |
| `String.*` | 33 | append, casefold, concat, contains, dropLeft, dropRight, endsWith, equalFold, fromChar, fromList, isEmail, isEmpty, isUrl, join, length, lines, padLeft, padRight, repeat, replace, reverse, slice, split, startsWith, toFloat, toInt, toList, toLower, toUpper, trim, trimEnd, trimStart, words | `constrain.rs:3984`; resolve `lower.rs:3540-3572` | exit0 | High | Add monomorphic arms cross-checked against `runtime/src/sky_runtime/string.rs`. PROVEN live: `String.length 42` → skyc exit 0 → `string_length(42)` → E0308. Import escape hatch exists. |
| `Char.*` | 8 | isAlpha, isDigit, isLower, isUpper, toLower, toUpper, toCode, fromCode | `constrain.rs:3984` (zero Char arms); resolve `lower.rs:3574-3581` | exit0 | High | isAlpha/isDigit/isLower/isUpper `Char -> Bool`; toLower/toUpper `Char -> Char`; toCode `Char -> Int`; fromCode `Int -> Char`. Import escape hatch exists. |
| `Crypto.*` | 17 | sha256, sha512, sha1, md5, hmacSha256, hmacSha512, rsaSha256Sign, rsaSha256Verify, constantTimeEqual, aesGcmEncrypt, aesGcmDecrypt, chacha20Encrypt, chacha20Decrypt, aesKeyFromPassword, chachaKeyFromPassword, randomBytes, randomToken | `constrain.rs:3984` (zero Crypto arms); resolve in `lower.rs` | exit0 | High | Match `runtime/src/sky_runtime/crypto.rs`. PROVEN live: `Crypto.sha256 42` → skyc exit 0 → `crypto_sha256(42)` → E0308. Security-adjacent: wrong-typed key/message to HMAC/AEAD should be a clean Sky type error at the crypto boundary. Import escape hatch exists. |
| `Encoding.*` | 6 | base64Encode, base64Decode, urlEncode, urlDecode, hexEncode, hexDecode | `constrain.rs:3984`; **no stdlib module** (`stdlib.rs` MODULES omits it) | exit0 | High | `String -> String` / `String -> Maybe String`. **No import escape hatch — unconditionally holed.** |
| `JsonEnc.*` | 8 | string, int, float, bool, null, list, object, encode | `constrain.rs:3984`; **no stdlib module** | exit0 | High | Build `Value` with concrete arg types; `list : (a -> Value) -> List a -> Value`; `encode : Int -> Value -> String`. **No import escape hatch.** |
| `JsonDec.*` | 17 | string, int, float, bool, decodeString, field, at, index, list, map, andThen, succeed, fail, oneOf, map2, map3, map4 | `constrain.rs:3984`; **no stdlib module** | exit0 | High | `Decoder`-typed combinators mirroring the `Db.Decode` arms at `constrain.rs:3192-3268`. Polymorphic combinators (map/succeed/andThen) risk E0282 ambiguity rather than E0308. **No import escape hatch.** |
| `JsonDecP.*` | 4 | required, optional, custom, requiredAt | `constrain.rs:3984`; **no stdlib module** | exit0 | High | Pipeline combinators over `Decoder`. **No import escape hatch.** |
| `Jwt.*` | 4 | encodeHs256, decodeHs256, encodeRs256, decodeRs256 | `constrain.rs:3984`; **no stdlib module** | exit0 | High | encode/decode arity-2 (key, payload). **No import escape hatch.** |
| `Uuid.*` | 3 | v4, v7, parse | `constrain.rs:3984`; **no stdlib module** | exit0 | High | v4/v7 `String` (arity-0); parse `String -> Maybe String`. **No import escape hatch.** |
| `Webview.app` | 1 | app | resolve `lower.rs:4049`; unschemed `constrain.rs:3984`; emit stub `emit_expr.rs:2053` | stub-reachable | Low | Fail-closed **today** only because emit returns `Diagnostic::CompilerBug`; the moment Webview emit is wired (Phase 2) without adding the scheme, this becomes a live exit-0 hole. Add the `(Some("Webview"), Some("app"))` cfg-record scheme in the same change; convert the emit stub to `unsupported(...)` so the user sees a feature-gap, not a CompilerBug. |

**Total exit-0 holes: ≈231 `(qualifier, name)` pairs across 14 families**
(~131 UI + 100 non-UI stdlib), plus **1 fragile stub-reachable** kernel
(`Webview.app`). The single structural fix (Task #45) closes the entire class at
once — see §3 F1.

Prioritise the six no-stdlib-module families (Encoding / JsonEnc / JsonDec /
JsonDecP / Jwt / Uuid, 42 kernels) first: they have no import escape hatch and
are holed under all user actions.

Positive lockstep facts verified this pass, worth preserving:
- For the non-UI surface, `kernel_ty` schemes ⊆ `lower_callee` resolves — there
  is no schemed-but-unresolved mismatch in that direction.
- `lower_callee`'s fallthrough is correctly fail-closed
  (`SKY-L0108`, `lower.rs:4052`), so a kernel `lower` does not resolve errors
  non-zero rather than leaking to cargo.
- `naming::kernel_name` is an exhaustive `match` over `KernelFn` (404 arms, no
  wildcard) — adding a `KernelFn` forces a name.
- Math min/max, sqrt/trig/log, Dict/Set/Bytes/List/Task/Db/Http/System/Time/
  Random/File/Server/Middleware/RateLimit/Cmd/Sub/Io/Maybe/Result are correctly
  schemed — **not** holes.

---

## 2. Severity roll-up

Deduped distinct findings: **22** (the constrain fallback root is counted once,
under invalid-states, and cross-referenced from the parse and exit-0 sections).

| Class | Critical | High | Medium | Low | Total |
|---|---|---|---|---|---|
| exit0-cargo-fail (closed-list families) | 0 | 2 | 0 | 1 | 3 |
| parse-dont-validate | 0 | 1 | 1 | 3 | 5 |
| invalid-states | 1 | 1 | 1 | 1 | 4 |
| panic-soundness | 1 | 0 | 0 | 3 | 4 |
| security | 0 | 0 | 1 | 0 | 1 |
| efficiency | 0 | 0 | 0 | 0 | 0 |
| completeness | 0 | 0 | 2 | 3 | 5 |
| readability | 0 | 0 | 1 | 0 | 1 |
| **Total** | **2** | **4** | **6** | **11** | **22** |

Note: the two exit-0 High rows in the closed-list class (F4 UI, F5 stdlib) are
family-level umbrellas over the ≈231 individual holes in §1; they share the
structural root F1 (invalid-states, Critical). The panic-soundness `ffi_polyfills`
finding (F17) is co-classified parse-dont-validate; counted once under
panic-soundness.

---

## 3. Findings by class

### 3.1 Make-invalid-states-unrepresentable

| file:line | Sev | Title | Fix |
|---|---|---|---|
| `constrain.rs:3984` (fn `kernel_ty` @1868; caller `constrain_var_kernel` 1422-1441) | **Critical** | **F1 (root).** Resolved-but-unschemed kernel returns a flexible `Ty::Var(u32::MAX)` instead of a compile error. `instantiate` mints one fresh flex var, so the callee unifies with any `args -> R`; arguments are never checked. This is the exact anti-pattern the fundamental rules name, and the generative source of the whole exit-0 class (§1) plus the historical Math.min/max, Set/Dict, generic-`==` holes. | Make `kernel_ty` return `Option<Ty>` / `DResult<Ty>`; delete the `_ =>` fallback; a miss for a `lower_callee`-resolved name raises a hard `Diagnostic`. Structurally: resolve kernel identity once into `KernelFn` in a shared resolver consumed by canon + constrain + lower, then make `kernel_ty` an exhaustive `match KernelFn` with no wildcard, so a variant added without a scheme is a non-exhaustive-match compile error. **Task #45.** |
| `env.rs:181` · `lower.rs:3540-4052` · `constrain.rs:1868` | High | **F3.** Three hand-maintained kernel registries with asymmetric failure: `lower_callee` and `naming::kernel_name` fail closed, `kernel_ty` fails open. Nothing asserts the three sets agree, so any future kernel silently reopens the hole for whichever table the author forgets. A kernel present in canon + lower but absent from constrain (every Char kernel) bypasses the checker yet lowers correctly — silent unsoundness, not a clean gap. | Single registry (F1 fix). If deferred, add a golden test asserting the three sets are identical (see F12) **and** flip constrain's fallback to fail-closed so the type checker is never the lenient one. |
| `emit_expr.rs:920, 1021, 2077` (predicates `ir.rs:1882/1939/1969/2014/2110/2120/2127`) | Medium | **F8.** Backend kernel-emit dispatch keys off runtime `is_tea()`/`is_server()`/`is_ui()` `matches!`-lists (default false) + a trailing `_ => CompilerBug`, not one exhaustive match. A newly-added `KernelFn` mis-categorised (omitted from the family's `matches!` list) returns false everywhere, skips all category emitters, and can fall through to the plain-kernel path — emitting a wrong runtime call with no error. Softened form of the CLAUDE.md explicit-walker-arm rule. | Replace the boolean `is_*()` predicates with one `const fn category(self) -> KernelCategory` implemented as an exhaustive `match` (no wildcard). Adding a variant becomes a compile error until classified. |
| `constrain.rs:512-525` (`_ => BinopClass::Poly` @523) | Low | **F13.** `classify_binop` matches raw operator-name bytes and defaults to `Poly` (an unconstrained `a -> a -> a` with no Number/Order/Equality obligation). Safe today (closed operator set, `Poly` deliberately catches `::`), but a future operator kernel forgotten here silently gets an obligation-free scheme — the F1 shape in miniature. | Match over a closed `BinOpKind` enum produced by canon at desugar time, exhaustive, no wildcard. |

### 3.2 Parse-don't-validate

| file:line | Sev | Title | Fix |
|---|---|---|---|
| `ast.rs:132` (`VarKernel { module: Symbol, name: Symbol }`) | High | **F2.** Kernel identity is never parsed — it is carried as an unresolved `(Symbol, Symbol)` and re-inspected as `&str` in three separate tables (canon admit-list 105 Ui names, lower 387 `KernelFn`, constrain 173 arms). The canonical stringly-typed / validate-then-use smell; the three tables provably diverge (49 Ui in lower vs 3 in constrain) and that divergence is the mechanism behind F1/§1. | Parse `(Symbol, Symbol) -> Result<KernelFn, Diagnostic>` exactly once (canon/resolve or a shared `sky_ir::kernels`); carry the resolved `KernelFn` on the canon node. constrain/lower/backend then each map `KernelFn` via exhaustive match. |
| `lower.rs:1953` | Medium | **F9.** `Attribute msg` exists in both Std.Ui and Std.Html; lower disambiguates the two IR constructors by substring-scanning the module path for `"Html"` (`module.iter().any(|s| resolve(*s) == Some("Html"))`). Nominal type identity recovered by a substring literal rather than a distinct interned symbol; a user module `Foo.Html` or a re-export misclassifies the constructor. | Give `Std.Ui.Attribute` and `Std.Html.Attribute` distinct pre-interned type-constructor symbols in `Builtins` and match on symbol identity. |
| `constrain.rs:1423` | Low | **F14.** Math.min/max special-cased via `matches!(resolve(module), Some("Math")) && matches!(resolve(name), Some("min"|"max"))` before the scheme table; Set/Dict key-obligation similarly. Re-stringified-symbol guards, structurally the F1/F2 smell; each duplicates the `(module,name) -> behaviour` mapping by hand. | Fold into shared `KernelFn` resolution (F2): attach obligation kind (Comparable / comparable-key / none) to the variant or its scheme metadata. |
| `constrain.rs:3719-3721, 3765-3768` | Low | **F16.** Lockstep defect: `kernel_ty` schemes `paragraph`, `textColumn`, `onKeyPress` that `lower_callee` never resolves (they hit SKY-L0108). Net-safe (constrain accepts, lower rejects → skyc errors), but exactly the scheme↔lower divergence the codebase relies on staying in lockstep, pointing the safe way here and the unsafe way in §1. `onKeyPress` is additionally absent from canon's name lists — unreachable dead code. | Either add the matching lower arms (and expose `onKeyPress` in `env.rs` if intended) or drop the dead scheme fragments. The shared `KernelFn` source (F1/F2) makes the divergence unrepresentable. |
| `project.rs:385` (also readability) | Low | **F15.** Emitted `Cargo.toml` assembled by string-surgery (`.find` / `.replacen`) over a structured TOML template. Fail-closed (missing anchor → `CompilerBug`) and input is compiler-controlled, but it is validate-then-mutate-a-string over a format with a typed representation; reordering the golden's feature list silently changes behaviour. | Parse the base manifest into `toml_edit::Document` once, mutate the feature array + `[dependencies]` table structurally, re-serialise. Opportunistic, non-blocking. |

### 3.3 Panic-soundness (principle 3)

| file:line | Sev | Title | Fix |
|---|---|---|---|
| `lower.rs:4089` (emit `emit_expr.rs:42, 2205-2217`; dead helper `runtime/.../math.rs:48, 55`) | **Critical** | **F6.** Integer `//` (`idiv`) lowers to `BinOp::Div` and emits raw `i64 / i64`, which panics on `x // 0` (div-by-zero) and `i64::MIN // -1` (overflow) regardless of `overflow-checks=false`. `5 // 0` panics; Elm/Sky and the Go backend make `//` **total** (`5 // 0 == 0`). The correct total helpers `sky_int_div`/`sky_int_rem` already exist but are **dead code** — lowering bypasses them. `constrain.rs:517` already classifies `idiv` distinctly, so the type info to split the paths is present. Blast radius bounded (panic classifier + task join catch it → classified exit 1), but a controlled abort is still a runtime failure from well-typed Sky and diverges observably from Go (0 vs abort). | Split IR `BinOp` into `IntDiv` (integer) and `Div` (float). Map `"idiv" => IntDiv`, `"fdiv" => Div`. Emit `IntDiv => sky_runtime::math::sky_int_div(l, r)` (call, not infix); keep `Div => (l / r)` for f64. Delete the false parity comment at `lower.rs:4086-4088`. Golden cases: `5 // 0`, runtime-valued `x // 0`, `minInt // -1`. Route any future `%`/`remainderBy` through `sky_int_rem`. |
| `ffi_polyfills.rs:27-35, 50-64` (also parse-dont-validate) | Low | **F17.** `Ffi.callPure`/`Ffi.callTask` with a non-literal kernel name or non-literal args list falls through the static-dispatch peephole to a `panic!` in the emitted binary. Well-typed Sky (`String -> List a -> b`) reaches it. Documented, fail-loud, confined to the FFI escape hatch (least-trusted boundary), ledger-#3 ACCEPTED — but per parse-don't-validate the dynamic-dispatch shape should be rejected once at lower with a typed `Diagnostic`, not deferred to a runtime panic. | Add a lower-stage gate: non-peephole `Ffi.call*` shape emits a fail-closed `Diagnostic` (new SKY-L code). Keep the polyfill panic as an unreachable backstop, or drop it. |
| `tui/layout.rs:355` (pad sites 394, 404) | Low | **F18.** `set_width` pads with `" ".repeat(w - used)`; a near-`usize::MAX` `w` allocates an enormous String → OOM. Not presently reachable (all ~6 call sites derive `w` through resolvers clamping to `MAX_CELLS=100_000`), but soundness here is a whole-program invariant spread across six sites; one future unclamped caller reintroduces the OOM. | Make the invariant local: `let w = w.min(MAX_CELLS);` as the first line of `set_width` (and any sibling `" ".repeat`-ing a caller width). Per-site clamps become defense-in-depth. |
| `live/mod.rs:615` | Low | **F19.** `drive_session` calls user `update(msg, model)` with no `catch_unwind`. The thesis makes it unreachable, but the request path at `live/mod.rs:1749` already installs per-request panic recovery for exactly this reason. Runs inside `tokio::spawn` so a panic does not abort the process, but the effect is a silent per-session wedge with no warn (the metric calls after it never run). Asymmetric with the request path that converts a panic to a 500. | Wrap in `catch_unwind(AssertUnwindSafe(|| update(msg, model)))` (model already cloned, lock released). On `Err`, emit a structured warn via the panic classifier + a 4-byte errId, then `continue` — keeping the session alive and observable. |

### 3.4 Security

| file:line | Sev | Title | Fix |
|---|---|---|---|
| `ui/render.rs:236-335` (AttrFontFamily 242, AttrFontDecoration 254, AttrFontAlign 263, AttrBorderStyle 303, AttrOverflow 321, AttrTransition 325, AttrAnimation 328; AttrBgImage 269) | Medium | **F7.** Several typed String-valued CSS attrs are emitted into inline `style=""` via `format!("prop:{val}")` **without** the `SafeCssValue` whole-string gate that the generic user-CSS path (`_ =>` 226) and `AttrBgGradient` (282) already use. `AttrBgImage` guards only `is_dangerous_url_scheme` (scheme-prefix) — it does not scan for `)` or `;`. Values flow from Model (Font.family, Border.style, Background.image). The enclosing `style="..."` is `escape_attr`-escaped, so `"`/`<`/`>` are neutralised (no attribute breakout, no script-exec), but `;` and `)` are **not** escaped → CSS-declaration injection (full-viewport overlay/clickjacking, network beacon via `url()`). Confined to CSS integrity, not script XSS. Exactly the make-invalid-states asymmetry the `SafeCssValue` newtype was built to close. | Route every String-valued CSS attr through `SafeCssValue::parse` before pushing, dropping on failure, like the `_ =>` and `AttrBgGradient` paths (`SafeCssValue` permits font stacks / border keywords / transition shorthands, bans the breakout set). For `AttrBgImage`, wrap the assembled `url(...)` in `SafeCssValue::parse` or reject values containing `)` / `;`. |

### 3.5 Efficiency

No material efficiency findings this pass. The reviewed runtime hot paths
(String/Bytes `.get()`+clamp, `string_repeat` 64 MiB cap, layout `MAX_CELLS`
clamp, sqlite parameterized queries, `SkyTuple2` TEA fast-path) are bounded and
sound.

### 3.6 Completeness

| file:line | Sev | Title | Fix |
|---|---|---|---|
| `lower.rs:4052` (SKY-L0108 catch-all; canon `env.rs:892-917`) | Medium | **F11.** All of `Std.Html.Attributes` (Attr.class/id/href/style/attribute/boolAttribute/type_/name/value/placeholder/src/alt/for_/checked/disabled/readonly/required/multiple/selected/autofocus/tabindex/noAttr — 22 names) is canon-reachable but lower-unresolved → SKY-L0108. Fail-closed (not exit-0), but plain HTML attributes are entirely unusable — any real `Html.div [Attr.class "x"] [...]` is rejected. **Task #46.** | Wire the Attr kernels in `lower.rs` **and** add matching `kernel_ty` schemes in the **same change** (lockstep, so wiring does not open a fresh exit-0 hole): most `String -> Attribute msg`; `attribute : String -> String -> Attribute msg`; `boolAttribute : String -> Bool -> Attribute msg`; checked/disabled/… `Bool -> Attribute msg`; tabindex `Int -> Attribute msg`; noAttr `Attribute msg`. |
| `env.rs:181` · `lower.rs:3532` · `constrain.rs:1868` | Medium | **F12.** No regression test cross-references the three kernel surfaces — the mechanism by which the exit-0 class keeps reopening (Math.min/max, Set/Dict, generic `==` were each patched individually). | Unit/golden test: for every `(q,n)` that `lower_callee` resolves to `Callee::Kernel`, assert `kernel_ty` returns a non-fallback scheme (and vice-versa). Fail the build on drift. Combined with the `Option`-returning `kernel_ty` (F1), makes "wired kernel without a scheme" unrepresentable. |
| `lower.rs:4052` (canon `env.rs:622-935`) | Low | **F20.** Ui layout/nearby/pseudo/responsive/input-helper names + Background/Border/Font extras + `Event.onSubmit` are canon-exposed (and advertised as shipped Std.Ui surface in CLAUDE.md) but lower-unresolved → SKY-L0108. Safely fail-closed; a completeness/consistency gap. Includes paragraph, textColumn, above/below/onLeft/onRight/inFront/behind, onSubmit, onFile, htmlAttribute, mediaQuery, breakpoint, aspectRatio*, onPseudo, hover/focus/…, paddingEach, image, link, button, input, form; Background hoverColor/…/linearGradient; Border widthEach/solid/…/shadow; Font weight/…/monospace. | Wire incrementally in lower + `kernel_ty` lockstep as the UI surface is completed. Never add a name to lower without a matching scheme in the same commit. |
| `emit_expr.rs:2053` (resolve `lower.rs:4049`; unschemed `constrain.rs:3984`) | Low | **F21.** `Webview.app` resolved by lower, unschemed in constrain — saved from exit-0 only by the emit-stage `CompilerBug` stub (surfaces as internal-error, not a clean feature-gap). Fragile: wiring emit (Phase 2) without adding the scheme makes it a live exit-0 hole. (Also §1, last row.) | Add the `(Some("Webview"), Some("app"))` cfg-record scheme when wiring emit; convert the stub to `unsupported(...)`. |
| `db.rs` (whole file; `grep tenant` = 0 in `runtime/src/sky_runtime`) | Low | **F22.** CLAUDE.md advertises runtime tenant-prefix SQL enforcement (v0.16.6 `HubStoreReaderWithTenant`, "refuses cross-tenant reads") as a shipped security property, but the Rust runtime has no `tenant` symbol; `fetch_all_routed` etc. route only pool-vs-transaction, no tenant WHERE gate. Legitimate port-in-progress gap, but an operator could deploy a multi-tenant app on the Rust backend believing the SQL gate is active. | Port the tenant-prefix enforcement into the routed query helpers before the Rust backend is offered for multi-tenant use, **or** add an explicit "NOT YET PORTED" line to the Rust-backend docs so the isolation guarantee is not silently assumed. Track alongside the hub port. |

### 3.7 Readability

| file:line | Sev | Title | Fix |
|---|---|---|---|
| `constrain.rs:3980-3983` | Medium | **F10.** The comment on the `Ty::Var(u32::MAX)` fallback is self-contradictory: it claims the constant id "only needs to differ between the two `Ty::Var` arms … which a constant id trivially satisfies" — a constant id makes the two occurrences identical, not different. A maintainer trusting it could assume the fallback safely handles multi-variable kernel shapes. Will bite the post-completion de-abbreviation pass. | Rewrite to state the real invariant: the fallback yields a single flexible leaf variable; the raw id is irrelevant because `instantiate` mints one fresh var and there is no second occurrence to alias. Add: this path is unsound (accepts ill-typed uses) and should become a hard `Diagnostic` — Task #45. |
| `project.rs:385` | Low | **F15** (also parse-dont-validate §3.2) — TOML string-surgery. See above. |

---

## 4. Task cross-reference

- **#45** — constrain-exhaustiveness root fix for the exit-0 class. Closes F1,
  and thereby the entire §1 inventory (≈231 holes), F3 (registry asymmetry), and
  the F2/F14/F16 stringly-typed-kernel smells once the shared `KernelFn`
  resolver lands. **Open.** The single highest-leverage fix in this ledger.
- **#46** — wire plain HTML attributes (`Std.Html.Attributes`). Closes F11.
  Must land lower + `kernel_ty` scheme in one change.
- **#47** — `Std.Css`. No `Css` qualifier exists in canon; tracked for the CSS
  surface buildout. No exit-0 hole today (unreachable).
- **#31** — Phase-4 invalid-states sweep. Umbrella for F8 (category dispatch),
  F13 (classify_binop wildcard).
- **#43** — event Msg-propagation. Adjacent to the Event/Ui event-arm schemes
  and F16 (`onKeyPress` dead scheme / missing canon exposure).

New items to file (not yet tracked): F6 (integer `//` div-by-zero, **Critical**,
independent of #45 — file immediately), F7 (CSS `SafeCssValue` gate),
F12 (lockstep regression test), F17/F18/F19 (defense-in-depth panic gates),
F22 (tenant-isolation doc/port gap).

---

## 5. Verdict

- Compiler crates: **panic-clean**, error channels are typed
  (`Diagnostic`/`CliError`, no `String` error channels) — the first half of
  parse-don't-validate holds.
- Runtime session/render/coerce/tui: **sound**, with one CSS-integrity gap (F7)
  and three low defense-in-depth gaps (F18/F19 + the tenant doc gap F22).
- The **one structural hole** is the kernel tri-table with a fail-open type
  stage (F1/F2/F3) — it is the root of ≈231 exit-0 holes and every historical
  whack-a-mole patch. Fail-closed at type-check strictly dominates fail-at-cargo;
  the fix is to resolve kernel identity once into `KernelFn` and make
  `kernel_ty` exhaustive.
- **One independent Critical**: integer `//` (F6) panics from well-typed Sky and
  diverges from Go — fixable now without waiting on #45.
