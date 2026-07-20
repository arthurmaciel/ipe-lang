# Native Rust formatter (#315) — approved-approach design spec

Status: approach unanimously endorsed by the review panel; specific rustfmt layout-rule
details are settled empirically against the 483-golden byte-diff corpus during implementation.


## approach

Ipê-native, configurable, rustfmt-compatible Rust emitter (#315), built as a Wadler/Leijen-style Doc algebra constructed DURING the owned-IR walk in emit_expr_at (no re-lex, no second parse). The emitter's per-node builders return a Doc instead of a String; a single deterministic renderer lays the Doc out to fmt-clean bytes. WHY (ADR leads with this, in PRINCIPLES order): (1) SOUNDNESS/CORRECTNESS — structural token-preservation: every token the current emitter emits is carried as a Doc leaf, so the emitted-vs-rendered leaf sequence is a checkable invariant that structurally forbids the paren-drop / token-drift class of bug; (2) SPEED — removes the per-file `rustfmt` subprocess fork from the native emit path (project.rs run_rustfmt), the dominant emit cost; (3) STABILITY — output is fmt-clean BY CONSTRUCTION, so the CI `cargo fmt` gate can never red on emitted output (a real recurring failure eliminated at the root). Clean Rust in the wasm playground is incidental. VERIFIED against real rustfmt 1.9.0 with the EXACT golden-harness flags `rustfmt --edition 2024 --style-edition 2024` (confirmed from project.rs: EMITTED_EDITION=\"2024\", both --edition and --style-edition, stdin-piped) — I ran 13 probes and reproduced param_patterns:311-337 byte-for-byte before writing this. The binding acceptance gate is byte-golden (native(doc) == checked-in golden bytes) on all 73 main.rs goldens plus per-builder fixtures; the fixed-point rustfmt(native)==native is demoted to a necessary-not-sufficient smoke check; goldens are NEVER re-blessed.

## scoping

IN SCOPE: replace the String-returning emission path in emit_expr.rs (emit_expr_at, pub at 5914; emit_expr at 5783) and its callees with Doc-returning builders; a Doc IR module (doc.rs) + renderer (render.rs); config surface (RustFmtConfig) threaded through EmitCtx; cutover of project.rs to skip run_rustfmt when the native emitter is on. The census covers emit_expr.rs (520 format!/write! sites, 31 Expr variants) plus the framework emit_* modules that call emit_expr_at (emit_types 54, emit_live 21, emit_tui 10, emit_cli 3, emit_webview 4, emit_model_schema 3, static_build 4, rust_file 2, preamble 1). OUT OF SCOPE: changing WHAT tokens are emitted (the SEAL forbids it); changing the IR; the wasm playground UI. The renderer must reproduce rustfmt's layout for exactly the construct shapes the emitter produces — NOT be a general rustfmt.

## sealInvariant

Token-preservation is a STRUCTURAL leaf-sequence invariant, checked independently of layout. Oracle: the existing pub `emit_expr_at` (5914) returns the current pre-rustfmt String. Property: for every fixture expr, `whitespace_normalize(concat(leaves(build_doc(expr))))` == `whitespace_normalize(emit_expr_at(expr))`. whitespace_normalize collapses runs of spaces/newlines to a single space (token adjacency only). This means EVERY builder must carry EXACTLY the tokens the current arm emits: the BinOp arm emits `({l} {op} {r})` — the outer `(` and `)` are Text leaves, NEVER dropped (this is precisely the paren-drop that broke the SEAL in the rejected spec); Append emits `format!(\"{{}}{{}}\", {l}, {r})` and IntDiv emits `ipe_runtime::math::ipe_int_div({l}, {r})` — both Call-shaped, no infix parens; Let emits `({ let name = value; body })`; Destructure emits `({ stmts body })`; If emits `(if cond { then } else { else })`. A missing or extra leaf fails the property test at build time, not as a sweep panic. The property test is driven by an all-Expr-variant fixture matrix so an unhandled variant (missing builder) is a test failure, not a runtime `unreachable!`.

## binopChainMechanism

DERIVED from real rustfmt 1.9.0 bytes (--edition 2024 --style-edition 2024), reproduces param_patterns:311-337 byte-for-byte (verified via probe_paramlike). RESTATED PRECISELY to fix the panel's blocking issue — the post-boundary break is NOT remaining-width-driven:

BUILD: walk a left-assoc same-precedence run into a flat operand Vec `[op0=None, o0, +, o1, +, o2, ...]`. A higher-precedence sub-expr (`b*c` under a `+` chain) is ONE atomic operand (probe_mixed: `a + b*c + d` breaks only at `+`, `b*c` stays grouped). EVERY paren the emitter emits is carried as a Text leaf: the whole-chain wrapping `(`...`)` AND each nested operand's own `(`...`)`.

RENDER (chain-specific mode, keyed on a tagged Concat — see docIR):
(1) LINE-1 MAX-FIT: pack the maximal left-NESTED prefix that fits width(100) from the chain's start column, retaining ALL leading open-parens. If the entire flattened chain fits (probe_indent: 94 cols), emit inline, DONE — no operator breaks.
(2) ONCE BROKEN: EVERY subsequent operator breaks UNCONDITIONALLY, one-per-line, to a SINGLE shared, NON-ACCUMULATING indent = chain_base + 4, invariant to paren-nesting depth. This is NOT a remaining-width test. PROOF: probe_tinytail `(((((((a+b)+c)+d)+e)+f)+g)+h` with tiny e/f/g/h breaks EVERY tail operator one-per-line to col 8 though `+e +f +g +h` trivially fits remaining width; probe_final with 5 nested opens still uses ONE shared col-8 indent.
(3) SOLE INLINE EXCEPTION — LAST-LINE GLUE: the ONE operator immediately following a MULTILINE operand (block `})` OR forced-break call `)`-on-own-short-line — probe_callbreak/g1/g2/gluewidth confirm it is NOT block-specific) stays INLINE, attached at that operand's CLOSING-LINE column, IFF `closing_col + \" \" + op + \" \" + next_operand_first_line` fits width(100). When the closing line has no room (probe_callbreak: operand ends on a long arg line), the operator breaks to shared indent instead. Remaining-width governs ONLY the line-1 max-fit boundary (1) and this single glue decision (3) — NEVER the post-boundary operators (2).

Byte-gate: param_patterns:351-355 — `})) + crate::main_apply_m({...})` glues at col 8/10 (each `)` closing line), `}) + crate::main_ignore_arg(99)` glues (block close), then `+ main_sum_pair` / `+ main_get_y` / `+ main_first_of_alias` / `+ main_countdown` ALL break to col 12 (chain_base 8 + 4). Reproduced exactly.

## docIR

Frozen 7-variant enum (a 7th Chain variant is ADDED, per the panel — a generic Group cannot render 'first operator inline-glued, rest broken' within one chain, and P3 lanes depend on the frozen enum):

enum Doc {
  Text(Cow<'static, str>),              // a leaf token (incl. every paren)
  Concat(Vec<Doc>),                      // sequence, no break points
  Line,                                  // break candidate -> space when flat, newline+indent when broken
  Nest(usize, Box<Doc>),                 // indent inner by N (used non-accumulating: Nest(4, ...))
  Group(Box<Doc>),                       // all-flat-if-fits-else-all-break (blocks, call arg lists, if branches)
  Chain { base_indent, operands: Vec<ChainOperand> },  // the binop-chain node
  Softline,                              // zero-width break candidate (empty when flat)
}
struct ChainOperand { leading_op: Option<Text>, doc: Doc, is_multiline_boundary: bool }

The Chain node encodes the mechanism above: the renderer applies the named chain-rendering mode (line-1 max-fit; unconditional post-break to base+4; single last-line glue). is_multiline_boundary is computed by the renderer from whether `doc` rendered to >1 line, NOT stored statically — the glue decision is a render-time last-line-column measurement. Group is used for the operand-internal layout (call arg lists, blocks, if-branches) so a call operand forced-breaks its OWN args independently of the chain. FREEZE GATE: the Chain node's rendering of param_patterns + probe_tinytail + probe_callbreak is proven byte-exact in P0 BEFORE the enum is frozen; P1 lanes MUST use Chain, never re-implement a chain as a generic Group.

## emittedConstructs

- BinOp infix: `({l} {op} {r})` for Add/Sub/Mul/Div/Eq/Neq/Lt/Gt/Le/Ge/And/Or — parens ALWAYS carried as leaves (chain-flattened, see binopChainMechanism)
- BinOp::Append -> `format!("{}{}", l, r)` — Call-shaped, no infix parens
- BinOp::IntDiv -> `ipe_runtime::math::ipe_int_div(l, r)` — Call-shaped
- Let -> `({ let name = value; body })` (or inlined `({ inlined_body })` when non-clone multi-use)
- Destructure -> `({ <binding-stmts joined by space> body })`
- If -> `(if cond { then } else { else })` — BLOCK-ELSE-ALWAYS, never `else if`: the emitter recursively emits a nested If in else position as `(if ...)`, and the inner parens structurally forbid rustfmt from collapsing to `else if` (probe_emitter_if confirms rustfmt preserves `else { (if ...) }` with the added indent level)
- Ctor / Call / Apply / TailRecur — call-shaped arg lists (forced-break call layout)
- Match -> `match scrutinee { arms }`
- Tuple `(a, b)`, List, Cons, Record, Access, Update
- Lambda / SharedLambda -> the `{ let __ipe_fn: Box<dyn Fn..> = Box::new(move |..| -> .. { body }); __ipe_fn }` block
- TaskSeq / TaskSeqSync
- leaf literals: Int, Float, Str (`{s:?}.to_string()`), Char, Bool, Unit, Var, CloneVar (`x.clone()`), FuncValue
- TailLoop / TailRecur reachable only inside statement-position emit (7815/8303 arms), emitted as the loop prologue+`loop { match }` + `continue`

## siteCensus

Re-run against HEAD (emit_expr.rs = 8681 lines, NOT the stale ~6000/524 the rejected spec cited). emit_expr.rs: 520 format!/write! sites (505 format!, 13 write!, 2 writeln!) across 31 Expr variants. The central emission match is in emit_expr_at (5914); every arm mapped to a builder: Int/Float/Str/Char/Bool/Unit/Var/CloneVar/FuncValue (5931-5965, leaf builders); Ctor(5966); BinOp(5972 -> chain builder + Append/IntDiv Call-shaped); Let(6004 -> destructure_block); Destructure(6035 -> destructure_block); If(6053 -> if_expr); Call(6061); Tuple(6179); List(6189); Cons(6190); ListIndexClone(6196); ListLenCheck(6204); Record(6215); Access(6216 -> access builder, MUST be in the builder map — panel [1] flagged it omitted from prose); Update(6247); Lambda(6250)/SharedLambda(6253); Apply(6256); FuncValue(6257); Match(6258); TaskSeq(6278)/TaskSeqSync(6313); TailLoop/TailRecur(6325 -> Err in expr position; the REAL emission is in the statement-position arms at 7815/7857/7863/7882 and 8303/8313/8322/8330/8342, which emit `loop { match } / continue` prologue — these are a SEPARATE statement-emitter surface and are mapped to stmt-builders). Framework modules calling emit_expr_at: emit_types(54), emit_live(21), emit_tui(10), emit_webview(4), static_build(4), emit_cli(3), emit_model_schema(3), rust_file(2), preamble(1). DESIGN HOLES named now: (a) the statement-position emit surface (7815/8303 families) is a distinct String-builder path that must ALSO become Doc-returning or the block bodies won't be fmt-clean — it is IN scope, mapped to stmt-builders in P2; (b) emit_types' 54 sites emit type syntax (generics, Box<dyn Fn ..>) that has its OWN rustfmt layout (the `+ Send + Sync + 'static` bound wrapping) — a distinct builder family, probed and gated separately in P2.

## acceptanceGate

BINDING: native(build_doc(expr) |> render) bytes == the checked-in golden bytes, BYTE-exact, on ALL 73 main.rs goldens + every per-builder fixture — measured with the renderer, NOT via a rustfmt round-trip. Goldens are NEVER re-blessed; when the renderer disagrees with a golden, the RENDERER is fixed. SECONDARY (necessary, NOT sufficient, demoted): the fixed-point rustfmt(--edition 2024 --style-edition 2024, native_output) == native_output — proves fmt-idempotence only; passing it while failing the byte-golden gate is still a FAIL. The SEAL leaf-sequence property gates independently at build time. This resolves the prior spec's gate conflation: fixed-point can be green while byte-output is wrong (e.g. a greedy chain that is itself fmt-stable but doesn't match rustfmt's break pattern), so it can never be the acceptance gate.

## configModel

RustFmtConfig { native: bool (default true post-cutover), max_width: usize (100), style_edition: \"2024\" }, carried on EmitCtx. When native=false, the emitter still builds Docs but the renderer is bypassed and project.rs run_rustfmt runs (the legacy path, retained behind cfg during migration and as the wasm-off escape hatch). The renderer reads max_width/style_edition so a future style-edition bump is a config change, not a code change. On wasm32 native is forced true (rustfmt subprocess is unavailable there anyway).

## migration

Strangler cutover behind RustFmtConfig.native. During P1/P2 the emitter builds Docs but project.rs still runs rustfmt (native=false) so every intermediate commit stays green against the existing goldens (rustfmt(native_leaves)==golden already holds by the SEAL). P3 flips native=true and drops the subprocess; the legacy rustfmt path stays behind config for one release as an escape hatch and is deleted after a green sweep. No golden is re-blessed at any point.

## testStrategy

Three layers, in PRINCIPLES order. (1) SEAL leaf-sequence property test (correctness/soundness): for an all-31-variant fixture matrix, whitespace_normalize(leaves(build_doc)) == whitespace_normalize(emit_expr_at) — structurally forbids token drift and catches a missing builder as a test failure. (2) BINDING byte-golden gate (correctness): native(build_doc |> render) bytes == the checked-in golden bytes for all 73 main.rs goldens + per-builder fixtures (chain, if, destructure, mixed-precedence, block-glue, call-break-glue, tinytail). Goldens are NEVER re-blessed — the renderer is fixed to match them. (3) SMOKE fixed-point (necessary, not sufficient, DEMOTED): rustfmt(--edition 2024 --style-edition 2024, native_output) == native_output — proves fmt-idempotence but does NOT prove correctness, so it can never substitute for (2). P0 exit gate byte-diffs param_patterns + probe_tinytail + probe_callbreak THROUGH the renderer so a greedy remaining-width renderer red-gates at P0 (the panel's central defect). CI adds a fmt-clean assertion on emitted sweep output.

## phasedPlan

- P0 — de-risk make-or-break (single lane, blocks everything) — Build doc.rs (7-variant frozen enum incl. Chain) + render.rs implementing the EXACT verified mechanism: line-1 max-fit, unconditional post-break to base+4, single last-line glue. Write a standalone renderer prototype and byte-diff it against real rustfmt output for the P0 exit-gate probe set: param_patterns:311-337 (full chain), probe_tinytail (tiny post-boundary operands each break), probe_callbreak (forced-break call glue), probe_shorttail (block glue), probe_mixed (precedence atomicity), probe_stair/couple, if-expr (fits/breaks/nested-else), destructure-block (fits/breaks). ALL must byte-match BEFORE the Doc enum is frozen. If any fails, the mechanism is wrong — fix here, not in P1. Exit gate = every P0 probe byte-exact through the renderer + Chain node proven necessary (a generic Group demonstrably cannot render glue+break-in-one-chain).
- P1 — core builders (parallel lanes, on frozen enum) — Convert the emit_expr_at expr-position arms to Doc builders: chain builder (BinOp incl. Append/IntDiv Call-shaped), if_expr, destructure_block (Let+Destructure), Ctor/Call/Apply/TailRecur call-shaped, Tuple/List/Cons/Record/Access/Update, Lambda/SharedLambda block, Match, TaskSeq. Each lane adds its per-builder byte-goldens and passes the SEAL property for its variants. Lanes MUST use the Chain node, never a generic Group for chains.
- P2 — framework + statement surface — Convert the statement-position emit surface (7815/8303 families: If/Let/Destructure/Match/Call/TailRecur stmt arms -> loop/continue prologue) and the framework emit_* modules (emit_types 54 incl. the Box<dyn Fn + Send + Sync + 'static> bound-wrap builder, emit_live 21, emit_tui 10, emit_webview 4, static_build 4, emit_cli 3, emit_model_schema 3, rust_file 2, preamble 1). Probe + gate emit_types' bound-wrapping layout separately.
- P3 — cutover + config — Thread RustFmtConfig through EmitCtx; wire project.rs to skip run_rustfmt when native=true; force native on wasm32; keep legacy rustfmt path behind config for one release. Run the full 73-golden byte gate + the fixed-point smoke + a full examples sweep with fmt-clean assertion. Measure emit-time speedup (subprocess-fork removed).

## fileChanges

- src/compiler/backend/rust/src/doc.rs — NEW — frozen 7-variant Doc enum (Text/Concat/Line/Nest/Group/Chain/Softline) + ChainOperand; leaves() iterator for the SEAL oracle
- src/compiler/backend/rust/src/render.rs — NEW — deterministic renderer: fits()/mode per Group, and the chain-rendering mode (line-1 max-fit, unconditional post-break to base+4, single last-line-width glue). max_width/style_edition from RustFmtConfig
- src/compiler/backend/rust/src/emit_expr.rs — emit_expr_at (5914) and callees return Doc; BinOp arm (5972) -> chain builder carrying wrapping parens as leaves (fixes paren-drop); Let(6004)/Destructure(6035) -> destructure_block; If(6053) -> if_expr block-else-always; all 31 variants -> builders; statement-position arms (7815/8303) -> stmt Doc-builders
- src/compiler/backend/rust/src/emit_types.rs — 54 sites -> Doc builders incl. the Box<dyn Fn + Send + Sync + 'static> bound-wrap layout builder
- src/compiler/backend/rust/src/project.rs — skip run_rustfmt when RustFmtConfig.native (default true); retain legacy path behind config; force native on wasm32
- src/compiler/backend/rust/src/lib.rs — add RustFmtConfig to EmitCtx; expose doc/render modules
- src/ipe-cli/tests/golden_doc_render.rs — NEW — SEAL leaf-sequence property test (all-variant matrix) + per-builder byte-goldens (chain/if/destructure/mixed/block-glue/call-glue/tinytail) + demoted fixed-point smoke

## risks

- A P1 lane implements the chain as a generic Group (all-flat-or-all-break), which cannot render 'first op glued, rest broken' and byte-fails param_patterns's tail — The Chain node is a distinct frozen enum variant; P1 lanes MUST use it. The P0 exit gate byte-proves Chain on param_patterns+tinytail+callbreak before freeze, and the byte-golden gate reds any lane that regresses
- Greedy remaining-width renderer passes T1/param-only probes by coincidence but fails short-tail-after-boundary — probe_tinytail (tiny tails must each break) and probe_callbreak (forced-break call glue) are in the P0 EXIT gate, byte-diffed through the renderer — a greedy renderer red-gates at P0, not P1
- emit_types Box<dyn Fn + Send + Sync + 'static> bound-wrapping has its own rustfmt layout not covered by the chain mechanism — Probed and gated as a separate builder family in P2 with its own byte-goldens (static_bound golden already exercises it)
- Statement-position emit surface (7815/8303) left as String path -> block bodies not fmt-clean by construction — Named as design hole (a); converted to stmt Doc-builders in P2, gated by the full-golden byte diff
- style-edition bump changes layout, silently invalidating the renderer — style_edition is config; the byte-golden harness pins --style-edition 2024; a bump is a deliberate re-probe + golden regen, never silent

## openQuestions

- Whether the statement-position surface (7815/8303) should share the Chain/Group renderer or get a thin stmt-only wrapper — leaning shared (it emits the same expr Docs inside block bodies); resolve in P2 by probing a TailLoop golden.
- Whether to keep the legacy rustfmt path permanently as a `--fmt=external` opt-in or delete it after one green release — leaning delete once the sweep is green, since the SEAL + byte-golden gate make it redundant.

## effortEstimate

P0 ~3-4 days (the make-or-break renderer + freeze proof — highest risk, single lane). P1 ~1.5 weeks (31 builders, parallelizable 3-4 lanes). P2 ~1 week (statement surface + emit_types bound-wrap + framework modules). P3 ~3 days (cutover, config, sweep, speed measurement). Total ~3.5-4 weeks with the P0 gate binding before any P1 lane starts.

## missingBuilders

if_expr(cond, then_, else_): emits `(if {cond} {{ {then} }} else {{ {else} }})` with Group'd branches; BLOCK-ELSE-ALWAYS (never else-if) because the emitter parenthesizes a nested else-If as `(if ...)`, which structurally blocks rustfmt's else-if collapse (probe_emitter_if: rustfmt keeps `else { (if ...) }` at the extra indent). Byte-goldens: parenthesized-if-fits (inline), if-that-breaks (each branch on own line), nested-if-in-else (extra indent level). destructure_block(binders, body): emits `({ <binding stmts joined by space> body })` (shared by Let and Destructure arms); Group'd so short forms stay `({ let (a,b)=arg; (a+b) })` inline and long forms break each stmt to base+4 with `})` closing. Byte-goldens from param_patterns:322-326 (`({ let (a, b) = arg_6; (a + b) })` broken form) and :334-338 (record destructure). Both builders covered by checked-in byte-goldens so the prior emit-time-panic hole (unenumerated construct) is closed.
