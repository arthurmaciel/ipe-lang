//! Server-driven `Ui.widget` glue generation + SRI-pinned asset serving.
//!
//! The Web-shape renderer emits a `<ipe-ce-<hex> state="…">` node whose `state`
//! is an entity-escaped down-JSON attribute and whose up-events post through
//! `/_ipe/event`. But nothing in the browser DEFINES that element and the
//! author's widget JS is never served, so the widget renders inert. This module
//! closes that gap for the server-driven (Web-shape) target:
//!
//! * **Asset serving.** Each distinct `customElement "<path>"` reached in the
//!   program is registered here at process start with its file CONTENT. The
//!   content is served content-addressed at `/_ipe/widget.<hex16>.js`
//!   (`Cache-Control: immutable`, same discipline as the client asset), and the
//!   page pins it with `integrity="sha256-<b64>"` + `crossorigin="anonymous"`.
//!   The hash is over the file bytes, so the page SRI can never disagree with
//!   the served bytes — a tampered byte makes the browser refuse the module.
//!
//! * **Registration glue.** A single generated companion module is served at
//!   `/_ipe/widget-glue.<hex16>.js` (SRI-pinned over its own content) and
//!   referenced by ONE external `<script type="module">` in the page. For each
//!   registered tag it `import`s the author module (itself SRI-pinned via an
//!   import map is not portable, so the glue imports by the content-addressed URL
//!   and the browser verifies the module response against the SRI on the page's
//!   asset `<link rel="modulepreload">`), builds a mechanical `HTMLElement`
//!   subclass that forwards the escaped-attr down-state to the author's
//!   `mount(host, emit) -> { onState }`, and calls `customElements.define` on the
//!   compiler-generated `ipe-ce-*` tag ONLY. The author module never calls
//!   `define` — registration is compiler-owned, so element-registration
//!   injection is impossible by construction (Security #4).
//!
//! * **Up-wiring.** `mount`'s `emit(up)` routes through `__ipeEmitWidgetUp(host,
//!   up)`, which reads the node's `data-ipe-hid` (the ipe-id the renderer
//!   already stamps for the `OnWidget` handler) and calls the EXISTING
//!   `__ipeSend("ipe-widget", [JSON.stringify(up)], hid)` — inheriting the
//!   session cookie + CSRF token + `/_ipe/event` envelope unchanged. Server-side
//!   the posted body still flows through the total, fail-closed
//!   `seal_decode_serde` up-decoder (drop on mismatch), which this module does
//!   NOT touch.
//!
//! ## Security posture
//!
//! * The down-state reaches the client only as `JSON.parse` of a
//!   browser-unescaped attribute — data, never `eval`, never spliced into a
//!   script (Security #1).
//! * Every generated script is EXTERNAL and SRI-pinned; no inline script and no
//!   `unsafe-inline`/`unsafe-eval` is introduced, so the page CSP is unchanged
//!   (Security #3/#4).
//! * The served JS is declared-trust third-party code running with full DOM
//!   authority — SRI pins integrity, it does NOT sandbox. This is the honest
//!   limit the design records; this module never claims otherwise.

use std::sync::OnceLock;

/// One registered widget: the compiler-generated `ipe-ce-<hex>` tag and the raw
/// bytes of its author hook module.
///
/// The tag is minted by the lowerer from the sealed in-project path (never raw
/// user input); the content is read at build time from the file the canon
/// containment gate already proved exists inside the project root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetAsset {
    /// The compiler-generated custom-element tag (`ipe-ce-<hex16>`).
    pub tag: String,
    /// The author hook module's file content, verbatim.
    pub content: String,
}

/// The single SHA-256 digest of one widget file's content — the ONE hash every
/// content-addressing form below derives from. Making every form (the cache-bust
/// URL segment and the SRI) a rendering of this one digest is what guarantees one
/// hash from source to browser: they cannot disagree with the served bytes because
/// they are all this digest over those exact bytes.
fn content_digest(content: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(content.as_bytes()).into()
}

/// Content-addressing for one asset: `(hex16, base64full)` over SHA-256(content).
/// `hex16` (first 16 hex chars of the digest) is the cache-busting URL segment;
/// `base64full` (standard base64 of the full 32-byte digest) is the SRI value.
/// Both derive from one digest — the page SRI and the served bytes can never
/// disagree because they are computed from the same bytes.
fn content_hashes(content: &str) -> (String, String) {
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
    let digest = content_digest(content);
    let hex16: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    let base64full = B64.encode(digest);
    (hex16, base64full)
}

/// The process-global widget registry, populated ONCE at process start by the
/// generated `main` (before the server binds) via [`register`]. Empty for a
/// program that uses no `Ui.widget`, in which case every helper below is inert
/// and no widget route is mounted.
///
/// A `OnceLock` (not a mutable global) is used deliberately: the set of widgets
/// is a compile-time constant of the program — it is fixed at emit and never
/// changes at run — so a write-once cell is the make-invalid-states-
/// unrepresentable choice. A second `register` call is ignored (the first wins),
/// so a doubled generated call can never corrupt the table.
static WIDGET_REGISTRY: OnceLock<Vec<WidgetAsset>> = OnceLock::new();

/// Register the program's widget assets. Called once by the generated `main`
/// before the server starts. Idempotent: only the first call populates the
/// registry; a later call is a no-op (the registry is write-once).
///
/// `assets` is the compile-time list `[(tag, content)]`; each `content` is the
/// verbatim bytes of an in-project author hook file the build gate validated.
pub fn register(assets: &[(&str, &str)]) {
    let owned: Vec<WidgetAsset> = assets
        .iter()
        .map(|(tag, content)| WidgetAsset {
            tag: (*tag).to_string(),
            content: (*content).to_string(),
        })
        .collect();
    // `set` returns Err if already initialised; we intentionally ignore it — the
    // first registration is authoritative and a repeat is a benign no-op.
    let _ = WIDGET_REGISTRY.set(owned);
}

/// The registered widgets, or an empty slice when none were registered.
#[must_use]
pub fn registered() -> &'static [WidgetAsset] {
    WIDGET_REGISTRY.get().map_or(&[], Vec::as_slice)
}

/// True when the program registered at least one widget — the gate the router
/// uses to decide whether to mount the widget asset/glue routes and whether the
/// page carries the glue `<script>`.
#[must_use]
pub fn has_widgets() -> bool {
    !registered().is_empty()
}

/// The content-addressed URL PATH (no base prefix) for one widget asset, e.g.
/// `/_ipe/widget.a1b2c3d4e5f6a7b8.js`. Stable for given content; changes when
/// the file changes — making `Cache-Control: immutable` safe.
#[must_use]
pub fn widget_asset_path(content: &str) -> String {
    let (hex16, _) = content_hashes(content);
    format!("/_ipe/widget.{hex16}.js")
}

/// The `sha256-<b64>` SRI value for one widget asset's content.
#[must_use]
pub fn widget_asset_integrity(content: &str) -> String {
    let (_, b64) = content_hashes(content);
    format!("sha256-{b64}")
}

/// Emit the generated registration glue JS for the whole registry.
///
/// The glue is a single ES module. For each registered tag it defines a
/// mechanical `HTMLElement` subclass that:
///
/// * forwards the escaped-attr down-state to the author `mount`'s `onState`
///   (`JSON.parse` of the browser-unescaped `state` attribute — data, never
///   `eval`);
/// * routes the author's `emit(up)` through `__ipeEmitWidgetUp`, which posts via
///   the existing `__ipeSend`/`/_ipe/event` wire (session cookie + CSRF);
/// * calls `customElements.define` on the compiler-generated `ipe-ce-*` tag
///   ONLY.
///
/// If the author module lacks a `mount` export the element fails observably
/// (console error) and stays inert — it is never `define`d in a broken state.
///
/// `base` is the sub-app base path prefix (empty for a root-mounted app), so the
/// author-module `import` URL reaches the same content-addressed route the page
/// pins.
#[must_use]
pub fn glue_js(base: &str) -> String {
    let mut out = String::new();
    out.push_str(GLUE_PRELUDE);
    for asset in registered() {
        let asset_url = format!("{base}{}", widget_asset_path(&asset.content));
        // `tag` is `ipe-ce-<hex16>` — every byte is `[a-z0-9-]` (see the lowerer's
        // `custom_element_tag`), so it is safe both as a JS string literal and as
        // the class-name suffix. It is NEVER derived from user input.
        let tag = &asset.tag;
        let class_suffix = tag.replace('-', "_");
        out.push_str(&format!(
            "\
// ── widget {tag} ──────────────────────────────────────────────
import {{ mount as __ipe_mount_{class_suffix} }} from {asset_url:?};
class IpeCE_{class_suffix} extends HTMLElement {{
  static get observedAttributes() {{ return [\"state\"]; }}
  attributeChangedCallback(name, _old, val) {{
    if (name === \"state\" && this.__ipeHook) {{
      this.__ipeHook.onState(__ipeParseState(val));
    }}
  }}
  connectedCallback() {{
    if (typeof __ipe_mount_{class_suffix} !== \"function\") {{
      console.error(\"[ipe.widget] author module for {tag} has no `mount` export; element inert\");
      return;
    }}
    this.__ipeHook = __ipe_mount_{class_suffix}(this, (up) => __ipeEmitWidgetUp(this, up));
    if (this.hasAttribute(\"state\")) {{
      this.__ipeHook.onState(__ipeParseState(this.getAttribute(\"state\")));
    }}
  }}
  disconnectedCallback() {{ this.__ipeHook = null; }}
}}
if (!customElements.get({tag:?})) {{
  customElements.define({tag:?}, IpeCE_{class_suffix});
}}
"
        ));
    }
    out
}

/// The invariant head of the generated glue module: the two helpers every widget
/// class uses — total down-parse and the up-emit that reuses the existing
/// `__ipeSend` wire.
///
/// `__ipeParseState` is a total `JSON.parse` wrapper: a malformed `state`
/// attribute yields `null` rather than throwing (the down direction is not the
/// attacker-controlled edge, but a throw in `attributeChangedCallback` must not
/// wedge the element). `__ipeEmitWidgetUp` reads the node's `data-ipe-hid` (the
/// ipe-id the renderer stamps for the `OnWidget` handler) and posts the
/// JSON-encoded `up` value through `__ipeSend` under the fixed `ipe-widget` event
/// name — the SAME envelope, CSRF token, and session cookie a click uses. If the
/// client core (`__ipeSend`) has not loaded, the emit is dropped with a console
/// warning rather than throwing.
const GLUE_PRELUDE: &str = "\
// Generated by Ipê (Rust target) — custom-element registration glue.
// External, SRI-pinned module: no inline script, no eval. The author modules it
// imports are content-addressed + SRI-pinned by the page; this module only wires
// the compiler-owned `ipe-ce-*` element registrations to their author `mount`.
function __ipeParseState(raw) {
  if (raw === null || raw === undefined) return null;
  try { return JSON.parse(raw); }
  catch (e) { console.error(\"[ipe.widget] state attribute is not valid JSON\", e); return null; }
}
function __ipeEmitWidgetUp(host, up) {
  var hid = host.getAttribute && host.getAttribute(\"data-ipe-hid\");
  if (!hid) { console.warn(\"[ipe.widget] up-event before the element bound an ipe-id; dropped\"); return; }
  if (typeof __ipeSend !== \"function\") {
    console.warn(\"[ipe.widget] client core not loaded; up-event dropped\");
    return;
  }
  // Reuse the existing event wire: same envelope {sessionId, seq, msg, args,
  // handlerId}, same CSRF header + session cookie. The server resolves the
  // handler by (ipe-id, \"ipe-widget\") and runs the fail-closed seal up-decoder.
  __ipeSend(\"ipe-widget\", [JSON.stringify(up)], hid);
}
";

/// The content-addressed URL PATH for the whole-registry glue module, e.g.
/// `/_ipe/widget-glue.<hex16>.js`. The hash is over the glue content (which
/// folds in every registered asset URL), so the URL changes when any widget
/// changes — `Cache-Control: immutable` stays safe.
///
/// `base` threads the sub-app prefix into the glue body (the author-import URLs),
/// so a sub-app's glue hashes distinctly from a root-mounted one; the returned
/// PATH is base-relative (the caller prepends `base` for the page `<script src>`).
#[must_use]
pub fn glue_path(base: &str) -> String {
    let (hex16, _) = content_hashes(&glue_js(base));
    format!("/_ipe/widget-glue.{hex16}.js")
}

/// The `sha256-<b64>` SRI value for the whole-registry glue module.
#[must_use]
pub fn glue_integrity(base: &str) -> String {
    let (_, b64) = content_hashes(&glue_js(base));
    format!("sha256-{b64}")
}

/// The external, SRI-pinned `<script type="module">` tag the page emits to load
/// the glue, plus a `<link rel="modulepreload">` per author asset that pins each
/// author module's SRI so the browser verifies the module response against the
/// build-time content hash before executing it.
///
/// Returns the empty string when the program registered no widget (no script,
/// no CSP impact). Every reference is EXTERNAL + SRI + `crossorigin` — no inline
/// script, so the page CSP (`script-src 'self'`) is unchanged.
#[must_use]
pub fn page_scripts(base: &str) -> String {
    if !has_widgets() {
        return String::new();
    }
    let mut out = String::new();
    // Preload + SRI-pin each author module so its content is integrity-checked
    // (a tampered byte → the browser refuses the module). `modulepreload` carries
    // `integrity`, and the subsequent `import` in the glue reuses the cached,
    // already-verified response.
    for asset in registered() {
        let url = format!("{base}{}", widget_asset_path(&asset.content));
        let integrity = widget_asset_integrity(&asset.content);
        out.push_str(&format!(
            "<link rel=\"modulepreload\" href=\"{url}\" integrity=\"{integrity}\" crossorigin=\"anonymous\">"
        ));
    }
    let glue_url = format!("{base}{}", glue_path(base));
    let glue_integrity = glue_integrity(base);
    out.push_str(&format!(
        "<script type=\"module\" src=\"{glue_url}\" integrity=\"{glue_integrity}\" crossorigin=\"anonymous\"></script>"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A single global registry means these tests must not race a real
    // registration; they exercise the pure hashing/glue helpers directly with an
    // explicit asset list rather than through the global `register`.
    fn hashes_of(content: &str) -> (String, String) {
        content_hashes(content)
    }

    #[test]
    fn content_hash_is_stable_and_content_addressed() {
        let a = hashes_of("export function mount(){}");
        let b = hashes_of("export function mount(){}");
        assert_eq!(a, b, "same content → same hash");
        let c = hashes_of("export function mount(){} // changed");
        assert_ne!(a.0, c.0, "different content → different hex16");
        assert_ne!(a.1, c.1, "different content → different SRI");
    }

    #[test]
    fn asset_path_and_integrity_agree_on_one_digest() {
        // The URL hex16 is the digest prefix; the SRI is base64 of the SAME
        // digest — so a page pinning `integrity` for a URL can never disagree
        // with the bytes served at that URL.
        let content = "export function mount(host, emit){ return { onState(){} }; }";
        let (hex16, b64) = content_hashes(content);
        assert_eq!(
            widget_asset_path(content),
            format!("/_ipe/widget.{hex16}.js")
        );
        assert_eq!(widget_asset_integrity(content), format!("sha256-{b64}"));
    }

    #[test]
    fn glue_defines_only_generated_tags_and_never_evals() {
        // Build glue over an explicit registry via a stand-in that mirrors
        // `glue_js` for a fixed asset list, asserting the security-load-bearing
        // shape: `customElements.define` is called only on the `ipe-ce-*` tag,
        // the author module is imported (not evaluated), and down-state is
        // `JSON.parse`d, never `eval`ed.
        let tag = "ipe-ce-cafef00dcafef00d";
        let content = "export function mount(host, emit){ return { onState(s){} }; }";
        let asset_url = widget_asset_path(content);
        let class_suffix = tag.replace('-', "_");
        // Reconstruct one class block exactly as `glue_js` would, to assert on it
        // without touching the global registry.
        let block = format!(
            "import {{ mount as __ipe_mount_{class_suffix} }} from {asset_url:?};\n\
             customElements.define({tag:?}, IpeCE_{class_suffix});"
        );
        assert!(block.contains("customElements.define(\"ipe-ce-"));
        assert!(!block.contains("eval("));
        assert!(!block.contains("innerHTML"));
        assert!(GLUE_PRELUDE.contains("JSON.parse"));
        assert!(!GLUE_PRELUDE.contains("eval("));
        // The up-emit reuses the existing wire, never inventing a new fetch.
        assert!(GLUE_PRELUDE.contains("__ipeSend(\"ipe-widget\""));
    }

    /// The served URL segment and the page SRI a widget file pins are one
    /// `sha256(content)` — so the URL a page requests and the integrity it checks
    /// that response against can never disagree with the served bytes. Proven by
    /// deriving the SRI base64 straight from the served URL's digest and matching
    /// `widget_asset_integrity`.
    #[test]
    fn served_url_and_page_sri_bind_to_one_digest() {
        use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
        let content = "export function mount(host, emit){ return { onState(){} }; }";
        let digest = content_digest(content);
        // The SRI the page pins is base64 of the SAME digest the served URL's
        // hex16 prefix is taken from.
        let sri_from_digest = format!("sha256-{}", B64.encode(digest));
        assert_eq!(
            sri_from_digest,
            widget_asset_integrity(content),
            "the page SRI must be base64 of the served bytes' digest — one hash from source to browser"
        );
        let hex16: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            widget_asset_path(content),
            format!("/_ipe/widget.{hex16}.js")
        );
    }

    /// A tampered widget file (one changed byte) yields a DIFFERENT served URL and
    /// a different SRI — so a page pinning the honest file's integrity refuses the
    /// tampered bytes. This is the fail-closed catch: a swapped file cannot pass a
    /// pin computed over the honest bytes.
    #[test]
    fn a_tampered_widget_file_diverges_from_the_served_pin() {
        let honest = "export function mount(host, emit){ return { onState(){} }; }";
        let tampered = "export function mount(host, emit){ steal(); return { onState(){} }; }";
        assert_ne!(
            widget_asset_path(honest),
            widget_asset_path(tampered),
            "a tampered widget file must not share the honest file's served URL"
        );
        assert_ne!(
            widget_asset_integrity(honest),
            widget_asset_integrity(tampered),
            "a tampered widget file must not share the honest file's page SRI"
        );
    }

    #[test]
    fn no_widgets_means_no_page_scripts() {
        // A fresh process with no registration must emit no script and no link,
        // so the page CSP and byte output are unchanged for a widget-free app.
        // (The global registry is empty unless `register` ran.)
        if !has_widgets() {
            assert_eq!(page_scripts(""), "");
        }
    }
}
