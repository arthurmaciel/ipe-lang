# Phase E Task-0 — Html / Ui / Live `stdlib_scheme` derivation table

Read-only derivation for the 43 reachable kernels still on the
`Ty::Var(u32::MAX)` fallback (the Std.Html / Std.Ui / Sky.Live rendering
family). Each derived `stdlib_scheme` arm is verified against three
independent arity sources:

- **(a) decl arity** — `crates/sky_kernels/src/lib.rs` `decl()` 3rd field.
- **(b) lower `callee_arity`** — `crates/sky_lower/src/lower.rs` (the arity
  SSOT the lowerer peels for eta-expansion).
- **(c) runtime fn params** — the actual `pub fn` named in `decl()`'s 5th
  field, in `runtime/src/sky_runtime/{html.rs,ui/helpers.rs,live/mod.rs}`.

Preserving the upstream Go "Sky" naming. Principles order:
security > correctness > soundness > efficiency > completeness > readability.

## Helper names — all verified present (constrain.rs 1990-2148)

`int()` `float()` `string()` `bool_ty()` `var(n)` `fun(a,b)` `list(t)`
`attr(m)` `elem_t(m)` `html_t(m)` `length()` `color()` — all in
`stdlib_scheme`'s local scope. `var = Ty::Var` (2010); `fun` curries
(2011). msg-poly rule confirmed against schemed siblings: 0-arity
attribute = bare `attr(var(0))` (e.g. `UiCenterX`), N-arity =
`fun(.., attr(var(0)))` — `var(0)` reused as the single msg var per
kernel.

## Verdict summary

- **35 / 43** triple-agree cleanly → GO.
- **7 / 43** (`Html.div/span/a/button/p/input/img`) — **decl().arity is WRONG**
  (off by one). Runtime fn params AND lower `callee_arity` agree with each
  other on the Elm shape; only the registry disagrees. The derived scheme
  arrow count matches runtime+lower (the correct authorities) but violates
  the documented `arrow-count == decl().arity` invariant **because the
  registry is buggy**. → BLOCK until registry `decl().arity` is corrected.
- **1 / 43** (`Live.appRouted`) — **not a simple curried Ty** (closed config
  record) AND its lowering is `Feature::RoutedLiveApp` *unsupported*
  (lower.rs:2966). → FLAGGED; decision required.

---

## Table 1 — the 35 clean kernels (triple-agree)

| kernel | decl arity | lower callee_arity (cite) | runtime fn signature (cite) | derived `stdlib_scheme` Ty | agree? |
|---|---|---|---|---|---|
| `Ui.none` | 0 | 0 (lower.rs:3695) | `ui_none_<M>() -> Element<M>` (helpers.rs:17) | `elem_t(var(0))` | ✓✓✓ |
| `Ui.centerX` | 0 | 0 (3709) | `ui_center_x_<M>() -> Attribute<M>` (helpers.rs:111) | `attr(var(0))` | ✓✓✓ |
| `Ui.centerY` | 0 | 0 (3711) | `ui_center_y_<M>() -> Attribute<M>` (helpers.rs:116) | `attr(var(0))` | ✓✓✓ |
| `Ui.alignLeft` | 0 | 0 (3713) | `ui_align_left_<M>() -> Attribute<M>` (helpers.rs:121) | `attr(var(0))` | ✓✓✓ |
| `Ui.alignRight` | 0 | 0 (3715) | `ui_align_right_<M>() -> Attribute<M>` (helpers.rs:126) | `attr(var(0))` | ✓✓✓ |
| `Ui.alignTop` | 0 | 0 (3717) | `ui_align_top_<M>() -> Attribute<M>` (helpers.rs:131) | `attr(var(0))` | ✓✓✓ |
| `Ui.alignBottom` | 0 | 0 (3719) | `ui_align_bottom_<M>() -> Attribute<M>` (helpers.rs:136) | `attr(var(0))` | ✓✓✓ |
| `Ui.pointer` | 0 | 0 (3721) | `ui_pointer_<M>() -> Attribute<M>` (helpers.rs:141) | `attr(var(0))` | ✓✓✓ |
| `Ui.clip` | 0 | 0 (3723) | `ui_clip_<M>() -> Attribute<M>` (helpers.rs:146) | `attr(var(0))` | ✓✓✓ |
| `Ui.scrollbars` | 0 | 0 (3725) | `ui_scrollbars_<M>() -> Attribute<M>` (helpers.rs:151) | `attr(var(0))` | ✓✓✓ |
| `Font.bold` | 0 | 0 (3727) | `ui_font_bold_<M>() -> Attribute<M>` (helpers.rs:283) | `attr(var(0))` | ✓✓✓ |
| `Font.italic` | 0 | 0 (3729) | `ui_font_italic_<M>() -> Attribute<M>` (helpers.rs:288) | `attr(var(0))` | ✓✓✓ |
| `Html.render` | 1 | 1 (3734) | `html_render_<M>(node: Html<M>) -> String` (html.rs:753) | `fun(html_t(var(0)), string())` | ✓✓✓ |
| `Html.escapeHtml` | 1 | 1 (3736) | `html_escape_text_(s: String) -> String` (html.rs:760) | `fun(string(), string())` | ✓✓✓ |
| `Html.escapeAttr` | 1 | 1 (3738) | `html_escape_attr_(s: String) -> String` (html.rs:767) | `fun(string(), string())` | ✓✓✓ |
| `Html.attrToString` | 1 | 1 (3740) | `html_attr_to_string_<M>(attr: Attribute<M>) -> String` (html.rs:772) | `fun(attr(var(0)), string())` | ✓✓✓ |
| `Ui.text` | 1 | 1 (3743) | `ui_text_<M>(s: String) -> Element<M>` (helpers.rs:22) | `fun(string(), elem_t(var(0)))` | ✓✓✓ |
| `Ui.html` | 1 | 1 (3745) | `ui_html_<M: Clone>(h: Html<M>) -> Element<M>` (helpers.rs:27) | `fun(html_t(var(0)), elem_t(var(0)))` | ✓✓✓ |
| `Ui.spacing` | 1 | 1 (3748) | `ui_spacing_<M>(n: i64) -> Attribute<M>` (helpers.rs:83) | `fun(int(), attr(var(0)))` | ✓✓✓ |
| `Ui.padding` | 1 | 1 (3750) | `ui_padding_<M>(n: i64) -> Attribute<M>` (helpers.rs:88) | `fun(int(), attr(var(0)))` | ✓✓✓ |
| `Ui.width` | 1 | 1 (3752) | `ui_width_<M>(l: Length) -> Attribute<M>` (helpers.rs:101) | `fun(length(), attr(var(0)))` | ✓✓✓ |
| `Ui.height` | 1 | 1 (3754) | `ui_height_<M>(l: Length) -> Attribute<M>` (helpers.rs:106) | `fun(length(), attr(var(0)))` | ✓✓✓ |
| `Ui.gridColumns` | 1 | 1 (3756) | `ui_grid_columns_<M>(n: i64) -> Attribute<M>` (helpers.rs:156) | `fun(int(), attr(var(0)))` | ✓✓✓ |
| `Background.color` | 1 | 1 (3768) | `ui_background_color_<M>(c: Color) -> Attribute<M>` (helpers.rs:237) | `fun(color(), attr(var(0)))` | ✓✓✓ |
| `Background.image` | 1 | 1 (3770) | `ui_background_image_<M>(s: String) -> Attribute<M>` (helpers.rs:242) | `fun(string(), attr(var(0)))` | ✓✓✓ |
| `Border.width` | 1 | 1 (3773) | `ui_border_width_<M>(n: i64) -> Attribute<M>` (helpers.rs:249) | `fun(int(), attr(var(0)))` | ✓✓✓ |
| `Border.rounded` | 1 | 1 (3775) | `ui_border_rounded_<M>(n: i64) -> Attribute<M>` (helpers.rs:254) | `fun(int(), attr(var(0)))` | ✓✓✓ |
| `Border.color` | 1 | 1 (3777) | `ui_border_color_<M>(c: Color) -> Attribute<M>` (helpers.rs:259) | `fun(color(), attr(var(0)))` | ✓✓✓ |
| `Font.size` | 1 | 1 (3780) | `ui_font_size_<M>(n: i64) -> Attribute<M>` (helpers.rs:266) | `fun(int(), attr(var(0)))` | ✓✓✓ |
| `Font.color` | 1 | 1 (3782) | `ui_font_color_<M>(c: Color) -> Attribute<M>` (helpers.rs:271) | `fun(color(), attr(var(0)))` | ✓✓✓ |
| `Font.family` | 1 | 1 (3784) | `ui_font_family_<M>(families: Vec<String>) -> Attribute<M>` (helpers.rs:278) | `fun(list(string()), attr(var(0)))` | ✓✓✓ |
| `Html.text` | 1 | 1 (3787) | `html_text_node_<M>(s: String) -> Html<M>` (helpers.rs:298) | `fun(string(), html_t(var(0)))` | ✓✓✓ |
| `Html.raw` | 1 | 1 (3789) | `html_raw_node_<M>(s: String) -> Html<M>` (helpers.rs:303) | `fun(string(), html_t(var(0)))` | ✓✓✓ |
| `Ui.paddingXY` | 2 | 2 (3846) | `ui_padding_xy_<M>(x: i64, y: i64) -> Attribute<M>` (helpers.rs:96) | `fun(int(), fun(int(), attr(var(0))))` | ✓✓✓ |
| `Html.node` | 3 | 3 (3874) | `html_node_<M>(tag: String, attrs: Vec<Attribute<M>>, children: Vec<Html<M>>) -> Html<M>` (helpers.rs:329) | `fun(string(), fun(list(attr(var(0))), fun(list(html_t(var(0))), html_t(var(0)))))` | ✓✓✓ |

### Puzzle resolutions (from Table 1)

- **Puzzle 3 — `Font.family`**: `List String`, NOT `String`. Runtime takes
  `families: Vec<String>` (helpers.rs:278). Scheme `fun(list(string()),
  attr(var(0)))`. Lower comment at 3783 agrees ("List String -> Attribute").
- **Puzzle 4 — `Ui.html`**: `Html msg -> Element msg`. Runtime
  `ui_html_<M: Clone>(h: Html<M>) -> Element<M>`. Scheme
  `fun(html_t(var(0)), elem_t(var(0)))`.
- **Puzzle 5 — `Ui.width`/`height`**: `Length -> Attribute msg`, confirmed —
  runtime takes `l: Length`. `length()` is the already-schemed nullary
  con.
- **Puzzle 6 — `Html.attrToString`**: `Attribute msg -> String`. Runtime
  `html_attr_to_string_<M>(attr: Attribute<M>) -> String`.
- Note `Html.escapeHtml`/`escapeAttr` are **non-polymorphic** (`String ->
  String`) — no `var(0)`, matching the runtime's non-generic `(s: String)`.

---

## Table 2 — the 7 BLOCKED node kernels (registry decl().arity is WRONG)

For these, the runtime fn and lower `callee_arity` **agree with each
other** on the canonical Elm shape; the registry `decl().arity` is the sole
outlier (off by one). The derived scheme below matches runtime + lower (the
correct authorities). The `Δ` column marks the registry bug.

| kernel | decl arity | lower callee_arity (cite) | runtime fn params (cite) | derived scheme (arrow count = callee = runtime) | Δ |
|---|---|---|---|---|---|
| `Html.div` | **3** | 2 (3855) | `html_div_<M>(attrs, children) -> Html<M>` = 2 (helpers.rs:338) | `fun(list(attr(var(0))), fun(list(html_t(var(0))), html_t(var(0))))` | decl 3 ≠ 2 |
| `Html.span` | **3** | 2 (3857) | `html_span_<M>(attrs, children) -> Html<M>` = 2 (helpers.rs:346) | `fun(list(attr(var(0))), fun(list(html_t(var(0))), html_t(var(0))))` | decl 3 ≠ 2 |
| `Html.a` | **3** | 2 (3859) | `html_a_<M>(attrs, children) -> Html<M>` = 2 (helpers.rs:354) | `fun(list(attr(var(0))), fun(list(html_t(var(0))), html_t(var(0))))` | decl 3 ≠ 2 |
| `Html.button` | **3** | 2 (3861) | `html_button_<M>(attrs, children) -> Html<M>` = 2 (helpers.rs:362) | `fun(list(attr(var(0))), fun(list(html_t(var(0))), html_t(var(0))))` | decl 3 ≠ 2 |
| `Html.p` | **3** | 2 (3863) | `html_p_<M>(attrs, children) -> Html<M>` = 2 (helpers.rs:376) | `fun(list(attr(var(0))), fun(list(html_t(var(0))), html_t(var(0))))` | decl 3 ≠ 2 |
| `Html.input` | **2** | 1 (3791) | `html_input_<M>(attrs) -> Html<M>` = 1 (helpers.rs:384) | `fun(list(attr(var(0))), html_t(var(0)))` | decl 2 ≠ 1 |
| `Html.img` | **2** | 1 (3793) | `html_img_<M>(attrs) -> Html<M>` = 1 (helpers.rs:389) | `fun(list(attr(var(0))), html_t(var(0)))` | decl 2 ≠ 1 |

### Adversarial proof that there is NO third argument

The task premise ("`div`/`span`/`a`/`button`/`p` are decl arity 3 — what is
the THIRD argument?") is a trap. Inspecting the real Rust signatures:

```rust
pub fn html_div_<M>(
    attrs: Vec<crate::sky_runtime::html::Attribute<M>>,   // arg 1
    children: Vec<Html<M>>,                               // arg 2
) -> Html<M> { Html::HElement("div".to_owned(), attrs, children) }
```

Exactly **two** parameters — the tag name (`"div"`) is a baked-in string
literal, **not** a parameter (contrast `html_node_`, which does take a `tag`
arg and is genuinely arity 3). Likewise `html_input_`/`html_img_` take one
`attrs` arg and pass `vec![]` for children (void elements). The lower
`callee_arity` SSOT independently records 2 and 1 (lower.rs:3855-3863,
3791-3793) with matching Elm-shape doc comments. So runtime and lower agree;
`decl().arity = 3`/`2` is a registry off-by-one bug.

**This is exactly the class already fixed** for AEAD and Jwt-encode, where
the registry `decl().arity` was "corrected 3→2 to match the Rust runtime"
(constrain.rs:2783, 2809). The identical one-line correction is required
here:

- `lib.rs:1119-1123` — `Html.div/span/a/button/p`: arity `3 → 2`.
- `lib.rs:1124-1125` — `Html.input/img`: arity `2 → 1`.

**Until the registry is corrected, arrow-count == decl().arity == callee_arity
does NOT hold for these 7 → BLOCK.** (The scheme values above are already
correct against the two authoritative sources; only the registry must move
to restore the triple invariant the `first_schemed_were_holes` /
build-fixture gates rely on.)

---

## Table 3 — `Live.appRouted` (FLAGGED — not a simple curried Ty)

| kernel | decl arity | lower callee_arity | runtime fn | lowering |
|---|---|---|---|---|
| `Live.appRouted` | 1 | 1 (lower.rs:3819) | `live_app_routed<E,Model,Msg,Page,...>(init, update, view, subscriptions, routes, not_found, set_page, store_kind, store_path)` = **9 params** (live/mod.rs:1115) | **`Err(unsupported(.., Feature::RoutedLiveApp))`** (lower.rs:2966) |

`Live.appRouted` is Sky-level **arity 1**: it takes ONE closed config record
that the lowerer is meant to destructure into the 9 positional runtime args
(the same shape as `Live.app`, whose scheme at constrain.rs:2658 is a closed
`Ty::Record`, and whose lowering intercepts a record literal at
lower.rs:2925). The 9 runtime params are NOT nine Sky arrows — `set_page`,
`store_kind`, `store_path` are synthesised by the lowerer, not user fields.

**Two problems that make this NOT a paste-in curried `Ty`:**

1. **Its type is a record, not a curried arrow.** It cannot be written with
   the `fun/int/string/...` helpers alone; it needs a dedicated
   `Ty::Record` arm mirroring `LiveApp`/`TuiApp`/`WebviewApp`. The required
   field symbols already exist in `builtins` precisely for this:
   `live_f_routes` (`"routes"`, constrain.rs:297) and `live_f_not_found`
   (`"notFound"`, 298), plus the shared `live_f_init/update/view/
   subscriptions`.

2. **Its lowering is unsupported.** `lower.rs:2966` hard-fails
   `Feature::RoutedLiveApp`. Scheming it makes the program **type-check then
   fail at lower** with a clean SKY error (fail-closed — never
   exit-0-then-cargo-fail), but it is inconsistent to advertise a type for a
   kernel the lowerer refuses.

### Proposed correct handling (pick one — DECISION REQUIRED)

- **Option A (recommended, gate-total):** add the dedicated closed-record
  arm below. It type-checks honestly; `Feature::RoutedLiveApp` remains the
  fail-closed floor until routed lowering lands. Var convention mirrors
  `LiveApp`: `var(0)=Model`, `var(1)=Msg`, add `var(2)=Page`.

  ```rust
  K::LiveAppRouted => {
      let init_ret = tuple2(var(0), cmd(var(1)));
      let cfg_rec = Ty::Record({
          let mut m = BTreeMap::new();
          m.insert(self.builtins.live_f_init, fun(live_req(), init_ret.clone()));
          m.insert(self.builtins.live_f_update, fun(var(1), fun(var(0), init_ret)));
          m.insert(self.builtins.live_f_view, fun(var(0), html_t(var(1))));
          m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
          m.insert(self.builtins.live_f_routes, list(live_route()));
          m.insert(self.builtins.live_f_not_found, var(2));
          m
      });
      fun(cfg_rec, task_unit())
  }
  ```
  (Helpers `tuple2`, `cmd`, `sub`, `live_req`, `live_route`, `task_unit` are
  all present in scope — constrain.rs:2057-2161. This arm is NOT counted in
  the "35 clean" GO set; it needs its own review + a routed-lowering plan.)

- **Option B (exclude-until-lowered):** since lowering is unsupported, add a
  new `REACHABLE_BUT_UNLOWERED` exclusion (distinct from `KNOWN_UNBACKED`,
  which requires *no runtime fn* — `Live.appRouted` HAS one) so
  `stdlib_scheme_total_over_reachable` skips it until `Feature::RoutedLiveApp`
  is implemented. Do NOT force it into `KNOWN_UNBACKED` (that would lie about
  the missing runtime fn).

---

## Ready-to-paste Rust block — the 35 CLEAN arms only

Drop into `stdlib_scheme` (constrain.rs) inside the FIRST-SCHEMED region
(before the `_ => return None` at 2939). Every helper verified present.
**Excludes the 7 blocked node kernels (Table 2 — need the registry arity fix
first) and `Live.appRouted` (Table 3 — record arm + decision).**

```rust
// ── Std.Html serialise / escape (arity 1) ──
K::HtmlRender => fun(html_t(var(0)), string()),
K::HtmlEscapeText | K::HtmlEscapeAttr => fun(string(), string()),
K::HtmlAttrToString => fun(attr(var(0)), string()),

// ── Std.Ui element builders (arity 0 / 1) ──
K::UiNone => elem_t(var(0)),
K::UiText => fun(string(), elem_t(var(0))),
K::UiHtml => fun(html_t(var(0)), elem_t(var(0))),

// ── Std.Ui attribute builders — nullary (arity 0) ──
K::UiCenterX
| K::UiCenterY
| K::UiAlignLeft
| K::UiAlignRight
| K::UiAlignTop
| K::UiAlignBottom
| K::UiPointer
| K::UiClip
| K::UiScrollbars
| K::FontBold
| K::FontItalic => attr(var(0)),

// ── Std.Ui / Background / Border / Font attribute builders — Int arg ──
K::UiSpacing
| K::UiPadding
| K::UiGridColumns
| K::BorderWidth
| K::BorderRounded
| K::FontSize => fun(int(), attr(var(0))),

// ── attribute builders — Length arg ──
K::UiWidth | K::UiHeight => fun(length(), attr(var(0))),

// ── attribute builders — Color arg ──
K::BackgroundColor | K::BorderColor | K::FontColor => fun(color(), attr(var(0))),

// ── attribute builders — String / List String arg ──
K::BackgroundImage => fun(string(), attr(var(0))),
K::FontFamily => fun(list(string()), attr(var(0))),

// ── Std.Ui — two Int args (arity 2) ──
K::UiPaddingXY => fun(int(), fun(int(), attr(var(0)))),

// ── Std.Html leaf nodes (arity 1) ──
K::HtmlTextNode | K::HtmlRawNode => fun(string(), html_t(var(0))),

// ── Std.Html generic node (arity 3 — tag, attrs, children) ──
K::HtmlNode => fun(
    string(),
    fun(list(attr(var(0))), fun(list(html_t(var(0))), html_t(var(0)))),
),
```

### Blocked arms (paste ONLY after registry `decl().arity` is corrected)

```rust
// REQUIRES lib.rs arity fix: div/span/a/button/p 3→2 ; input/img 2→1.
K::HtmlDiv | K::HtmlSpan | K::HtmlA | K::HtmlButton | K::HtmlP => fun(
    list(attr(var(0))),
    fun(list(html_t(var(0))), html_t(var(0))),
),
K::HtmlInput | K::HtmlImg => fun(list(attr(var(0))), html_t(var(0))),
```

---

## FIRST_SCHEMED classification — all 43 are genuine `Ty::Var(u32::MAX)` holes

Verified: the legacy `kernel_ty` string table (constrain.rs:2948-5160) has
**no** arm for any `"Html"` / `"Ui"` / `"Live"` / `"Background"` / `"Border"` /
`"Font"` qualifier — every one falls to `_ => Ty::Var(u32::MAX)` (line 5160).
Therefore none has a legacy scheme; `stdlib_scheme_matches_legacy` (the
RELOCATED parity tripwire) does not apply to any of them, and each satisfies
`first_schemed_were_holes` (each WAS a hole). **All 43 belong in
FIRST_SCHEMED, none in RELOCATED.**

FIRST_SCHEMED additions (add to the `FIRST_SCHEMED` set in constrain.rs):

```
HtmlRender, HtmlEscapeText, HtmlEscapeAttr, HtmlAttrToString,
UiNone, UiText, UiHtml, UiSpacing, UiPadding, UiPaddingXY, UiWidth, UiHeight,
UiCenterX, UiCenterY, UiAlignLeft, UiAlignRight, UiAlignTop, UiAlignBottom,
UiPointer, UiClip, UiScrollbars, UiGridColumns,
BackgroundColor, BackgroundImage, BorderWidth, BorderRounded, BorderColor,
FontSize, FontColor, FontFamily, FontBold, FontItalic,
HtmlTextNode, HtmlRawNode, HtmlNode,
HtmlDiv, HtmlSpan, HtmlA, HtmlButton, HtmlP, HtmlInput, HtmlImg,   // after arity fix
LiveAppRouted                                                      // if Option A chosen
```

(The last 8 enter the set only once their prerequisites — registry arity fix
for the 7 nodes; record-arm + decision for `LiveAppRouted` — are resolved.)

---

## Self-review — arrow-count == decl().arity == callee_arity

| group | count | triple-agree? |
|---|---|---|
| arity-0 (nullary) | 12 | ✓ (decl 0 = callee 0 = rt 0) |
| arity-1 | 21 | ✓ |
| arity-2 (`paddingXY`) | 1 | ✓ |
| arity-3 (`node`) | 1 | ✓ |
| **clean subtotal** | **35** | **✓ GO** |
| node kernels (div/span/a/button/p/input/img) | 7 | ✗ decl().arity off-by-one (runtime + lower agree at 2/2/2/2/2/1/1) → **BLOCK** |
| `Live.appRouted` | 1 | ✗ not curried (record) + lowering unsupported → **FLAG** |
| **total** | **43** | |

Every helper name used (`int, string, bool_ty, fun, list, attr, elem_t,
html_t, length, color, var`) is verified present in `stdlib_scheme`'s scope.

## VERDICT: **NO-GO** on the table as a whole

GO requires *all 43* to triple-agree. 7 do not (registry `decl().arity` bug),
and `Live.appRouted` cannot be a simple curried `Ty` and is unlowered.

- **35 clean arms → GO** (paste block above; helpers verified).
- **7 node arms → BLOCK** pending the one-line registry fix (lib.rs:1119-1125:
  `3→2` div/span/a/button/p, `2→1` input/img). The scheme values are already
  runtime-correct; only the registry must move.
- **`Live.appRouted` → FLAG**: adopt Option A (closed-record arm, gate-total)
  or Option B (`REACHABLE_BUT_UNLOWERED` exclusion). Not `KNOWN_UNBACKED`
  (it has a runtime fn).
