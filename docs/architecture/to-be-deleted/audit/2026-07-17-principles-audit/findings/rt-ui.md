# RT-UI findings

5 findings: 0 critical, 0 high, 2 medium, 3 low.

Audited: `src/runtime/rust/src/html.rs`, `css.rs`, `css_safety.rs`,
`ui/{element,render,helpers,input,keyed,lazy,mod}.rs`,
`dom/{diff,dispatch,form,req,mod}.rs`, `wasm/mod.rs` (+ `live/client.js`
read-only for sink-parity comparison).

Prior-audit status (runtime-audit-verdict.md items in this partition):
the CRITICAL diff-path XSS (diff_attrs/diff_events bypassing the render
gate) is FIXED — `dom/diff.rs` routes every patch attribute through
`insert_safe_attr` → `html::safe_patch_attr` (same `SafeAttrName` +
URL-scheme policy as first paint, with regression tests). The HIGH latent
`AttrAttribute` XSS is FIXED behaviourally (every HTML sink routes through
`render_into_ctx`'s `SafeAttrName`/`sanitise_url_attr`; `data-ipe-eval`'s
`__ipeRunEvals` sink is deleted — client.js now only references the
`__ipeRunPaths` replacement). Residual type-level gaps are RT-UI-005.

## RT-UI-001 · Unbounded native recursion in Ui render + DOM diff (stack-overflow abort)
- severity: medium
- axis: soundness
- principle: P3 soundness — "a well-typed Ipê program can never trigger a runtime failure"; fix-the-structure (the class is already recognised and closed in two sibling walkers, but not these)
- location: `src/runtime/rust/src/ui/render.rs:467-508` (`render_element` / `render_node_as` / `render_nearby_overlays`), `src/runtime/rust/src/dom/diff.rs:76-143` (`diff_node`)
- reachability: any Live/Webview/wasm app whose view tree depth scales with attacker-influenced Model data. Deep trees are constructible with O(1) app stack — e.g. `List.foldl (\_ acc -> Ui.el [] acc) base xs` over an attacker-length list wraps the accumulator once per element — so the runtime walker is the first (and only) overflow point. `render_element` runs on every commit (Live render pipeline, wasm `flush`), `diff_node` on every update.
- problem: `html.rs` caps its descent at `MAX_HTML_DEPTH = 1024` with an explicit comment that uncapped recursion over attacker-influenced nesting "would overflow the thread stack and ABORT the whole process — a panic the no-runtime-error thesis forbids", and `dom/dispatch.rs::walk` was made iterative for the same stated reason. The two remaining tree walkers in the same data path — the `Element -> Html` conversion and the structural diff — recurse natively once per nesting level with no cap. A deep tree therefore aborts the process (uncatchable stack exhaustion) before the html-render cap is ever reached. (Secondary members of the same class: the derived `Clone`/`Drop`/`PartialEq` glue on the recursive `Element`/`Html` enums.)
- fix direction: depth-cap (matching `MAX_HTML_DEPTH`, drop deeper nodes) or explicit work-stack iteration (matching `dispatch.rs::walk`) in both walkers.
- prior: new (verdict's ui section flagged other items; this asymmetry is unlisted)

## RT-UI-002 · `Ipe.Ui.Keyed` discards keys — identity loss on reorder + phantom divergence-ledger citation
- severity: medium
- axis: completeness
- principle: "Match the reference — diverge ONLY where … recorded in docs/divergences-from-sky.md"; P2 correctness (claimed capability silently no-ops)
- location: `src/runtime/rust/src/ui/keyed.rs:17-32` (+ `html.rs:714-733` `ipe_id_key`, the machinery it fails to use)
- reachability: any app using `Keyed.column`/`Keyed.row` (advertised in the authoring reference as "Keyed (ipe-key for diff identity)") with reorderable lists containing uncontrolled inputs or focus.
- problem: `keyed_column_`/`keyed_row_` drop the key string entirely instead of attaching the `ipe-key` attribute that `assign_ipe_ids` already consumes (the `:{key}` ipe-id segment exists precisely so keyed items keep identity across reorder — see html.rs test `keyed_items_keep_id_across_reorder`). The module doc claims this is "semantically correct (keys are a performance hint, not a behavioural contract)" — false for this runtime: without keys, reordering shifts positional ipe-ids, so the diff patches the wrong elements and uncontrolled-input state / focus attaches to the wrong row. The doc also cites "docs/divergences-from-sky.md §B-Keyed" — no such section exists (only §B-Lazy); the divergence is unrecorded, violating the ledger requirement.
- fix direction: attach `AttrAttribute("ipe-key", key)` to each keyed child (one line each; the downstream machinery is already built and tested), or record the divergence honestly and correct the comment.
- prior: new

## RT-UI-003 · wasm patch sink omits `selected` from DOM-property sync (client.js divergence)
- severity: low
- axis: correctness
- principle: P2 correctness — "one diff algorithm, two consumers … behaviour stays byte-parity" (the module's own stated contract)
- location: `src/runtime/rust/src/wasm/mod.rs:456-480` (`sync_dom_property`)
- reachability: `--target wasm` app patching `selected` on an `<option>` after the user has interacted with the `<select>`.
- problem: the doc comment says "mirror `client.js`'s value/checked/selected/disabled sync", and client.js does sync `el.selected` (client.js:928), but the Rust `match` has arms only for `value`/`checked`/`disabled`. `setAttribute("selected", …)` alone changes `defaultSelected`, not the live selection once the select is dirtied, so the visible selection goes stale on the wasm target where the SSE client would have updated it.
- fix direction: add a `"selected"` arm (checked cast to `HtmlOptionElement`, `set_selected(truthy)`).
- prior: new

## RT-UI-004 · Attribute-removal sentinel never resets `checked`/`value` DOM properties (both sinks)
- severity: low
- axis: correctness
- principle: P2 correctness (present-state stale after a legitimate state transition)
- location: `src/runtime/rust/src/wasm/mod.rs:443-447`; same shape in `src/runtime/rust/src/live/client.js:920` (removal branch)
- reachability: an `Ipe.Html` checkbox using `BoolAttr("checked", …)` toggling true→false: `diff_attrs` emits the empty-string removal sentinel; both patch appliers call only `removeAttribute`, never resetting the property.
- problem: the `checked` (and `value`) IDL properties do not reflect attribute removal once the control is dirty (user has interacted — the common case, since the toggle round-trip was user-initiated), so the checkbox stays visually checked after the server unchecks it. The `Ipe.Ui.Input` path is unaffected (it encodes checked as a string attr `"true"`/`"false"`, which takes the set-branch and syncs). Both our sinks share the shape, so this is likely inherited from the Go reference client — verify before treating as sanctioned parity; if Go has the same bug, the divergence policy ("diverge where strictly better") favours fixing both.
- fix direction: on empty-value removal of `checked`/`value`/`selected`/`disabled`, also reset the DOM property (false / "").
- prior: new

## RT-UI-005 · XSS gate is policy-centralised but comment-enforced, not type-enforced (prior fix partially landed)
- severity: low
- axis: completeness
- principle: make invalid states unrepresentable — "an invariant asserted in comments but not enforced by types"
- location: `src/runtime/rust/src/dom/diff.rs:8-20` (`Patch` with `pub attrs: HashMap<String, String>`), `src/runtime/rust/src/ui/element.rs:134-143` (`AttrAttribute(String, String)` + "do not write one" contract comment), `src/runtime/rust/src/ui/element.rs:130` (`AttrStyle` still multiplexes the `__col`/`__row`/`__grid` internal markers with user CSS)
- reachability: none today — every current writer routes through `insert_safe_attr`/`render_into_ctx` (verified: `insert_safe_attr` is the sole `Patch.attrs` inserter), so this is a smell, not a live hole.
- problem: the prior audit's fix direction was "make `Patch.attrs` carry `SafeAttrName`/`SafeAttrValue` tokens" and "split `AttrStyle` into `pub(crate) AttrInternalMarker` vs pub `AttrCssStyle`". What landed is behavioural: the policy fns are shared and every sink calls them, with tests — but `Patch.attrs` stays a raw public `HashMap<String,String>` any future code can insert into, the `AttrAttribute` no-bespoke-renderer invariant lives in a comment, and the `AttrStyle` marker split was not done (a user `Ui.style "__col" "true"` silently flips the internal layout-marker path — benign styling effect only).
- fix direction: newtype the patch-attr pair (constructor = `safe_patch_attr`) and split the `AttrStyle` internal markers into a `pub(crate)` variant.
- prior: runtime-audit-verdict.md html-render item 1 + ui item (Phase list §4) — partially fixed; residue only.
