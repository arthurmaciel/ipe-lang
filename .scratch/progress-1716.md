# 1716 FFI taxonomy rename — progress

Base a48a2c651. Branch feat-1716-ffi-rename.

## Renames
1. Ffi.kernel "X"  -> Ipe.Ffi.Kernel.kernel "X"  (stdlib: import Ipe.Ffi.Kernel as Kernel; Kernel.kernel "X")
2. Ipe.Js -> Ipe.Ffi.Js  (file move Ipe/Js.ipe -> Ipe/Ffi/Js.ipe; COMPILED_STD_MODULES dotted; all imports)
3. Ipe.Ui.widget -> Ipe.Ffi.Js.CustomElement.node ; customElement "p" -> Ipe.Ffi.Js.CustomElement.fromFile "p"

## Recognition mechanism (spelling-on-ALIAS + origin gate)
- detect_kernel_alias  resolve.rs:6590  matches qualifier "Ffi" + member "kernel". Origin gate: only EmbeddedStdlib/FfiInterface may mint (IPE-N0042).
- RESERVED_FFI_QUALIFIER_PATH env.rs:214 = ["Ipe","Ffi"]  (import boundary accept) -> ["Ipe","Ffi","Kernel"]
- customElement: CUSTOM_ELEMENT_CTOR resolve.rs:71 = "customElement" (bare local ref, annotated CustomElement). detect_custom_element_constructor resolve.rs:6397.
- Ffi.binding / Ffi.asserted = Rust FFI (Rust.Ffi module, ASSERTED_MODULE). OUT OF SCOPE. Untouched.
- builtins.rs:189 ("Ffi",2,0) = ErrorKind::Ffi ctor. UNTOUCHED.
- target_gate.rs:93 deny("Ffi","binding") = Rust FFI wasm gate. UNTOUCHED.

## Coverage matrices — FINDING: need NO change for this rename
- ForeignSurface (foreign_surface.rs) reads Capability::ALL wire names + source markers (seal_boundary_check, run_in_bwrap_jail, must_refuse) + capabilities.md. NOT the module namespace strings. Wire names unchanged -> no change.
- QUALIFIER_MODULE_OVERRIDES (surface.rs:270) keys = kernel-string module halves (Attr/Key/Mac/UiCells/EmailAddress) from "Module_function" split. Kernel STRINGS ("Ui_node") unchanged -> no change.
- Allowlist sizes MUST stay same. Verify via tests.

## Surface counts (before)
- Ffi.kernel in stdlib .ipe: 738 sites across 53 files
- import Ipe.Ffi as Ffi: 52 files
- Ipe.Js: stdlib Js.ipe + COMPILED_STD_MODULES + examples(6) + docs + tests(negative_suite, capabilities, widget_e2e) + runtime label strings
- widget/customElement: Ui.ipe (widget kernel Ui_widget), negative_suite tests

## Runtime string labels "Ipe.Js" — decide: user-facing module path vs internal kernel id
- js_port.rs, web/mod.rs, seal_codec.rs etc. — CHECK each: emitted module path (rename) vs runtime-internal (leave). Kernel STRING "Js_send" is NOT renamed (it is the kernel id, module-half "Js").

## Steps done: (none yet)

## DONE (Steps 1-3, unit-green)
- env.rs RESERVED_FFI_QUALIFIER_PATH -> ["Ipe","Ffi","Kernel"]; doc updated.
- resolve.rs detect_kernel_alias: match qualifier "Kernel"+member "kernel" (was "Ffi"). Docs + in-body comments updated.
- diagnostics: render.rs msg, code.rs IPE-N0028/N0042 titles+docs, diagnostic.rs docs -> Kernel.kernel.
- stdlib .ipe sweep: 738 Ffi.kernel->Kernel.kernel; 52 import Ipe.Ffi as Ffi -> import Ipe.Ffi.Kernel as Kernel. Prose swept.
- Js.ipe git-mv -> Ipe/Ffi/Js.ipe; module Ipe.Js->Ipe.Ffi.Js; lib.rs include_str + dotted "Ipe.Ffi.Js"; doc prose.
- All Ipe.Js importers (.ipe stdlib Browser + example js-ports) -> Ipe.Ffi.Js. Runtime prose+2 strings (js_port_glue SRI comment, wasm warn) -> Ipe.Ffi.Js.
- target_gate.rs test tuples "Ipe.Js"->"Ipe.Ffi.Js". compiler comments swept.
- Rust behavioral fixtures: canon/lib.rs, db_unsafe_row_read_marker, negative_suite, g_misc/golden_ffi_kernel_alias_seal, stdlib_docs, doc.rs, stdlib/lib.rs scanner (interned "Kernel" not "Ffi") -> Kernel.kernel + import Ipe.Ffi.Kernel.
- render_goldens.rs fixture span 40->64 + source Kernel.kernel. Golden name_kernel_alias re-blessed (clean path-change).
- ipe_canon 220/221->green, ipe_stdlib 221/221 green.
- BLESSING emit goldens (IPE_BLESS) in bg id bcpnpa0z1.

## SCOPE DECISION: // Ffi.kernel polyfill epilogue LEFT AS-IS
- Internal emit-machinery anchor (const ANCHOR "// Ffi.kernel polyfill"), NOT user-facing spelling. Renaming churns ENTIRE golden corpus for an internal codegen comment. Left unchanged; NOT the user surface. (Revisit if guardian wants it.)

## Coverage matrices: NO CHANGE NEEDED (verified)
- ForeignSurface reads Capability::ALL wire names + source markers, NOT module namespace. Unchanged.
- QUALIFIER_MODULE_OVERRIDES keys = kernel-string module-halves, unchanged.

## TODO
- Step 4: Ipe.Ui.widget -> Ipe.Ffi.Js.CustomElement.node (new compiled module exposing node=Kernel.kernel "Ui_widget"; retire widget from Ipe.Ui); customElement "p" -> CustomElement.fromFile "p" (recognition VarLocal->VarQual). Emit goldens for widget.
- Step 5: gen-stdlib-docs regen; divergence ledgers; taxonomy ADR; docs/guide/js.md; docs/reference/stdlib*.
- Full gate: clippy, fmt, full -p ipe nextest, examples build.
