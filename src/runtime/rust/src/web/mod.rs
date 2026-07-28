//! Ipe.Web on the Rust backend — HTTP-first render + SSE patch loop.
//! Generic over the app's (Model, Msg); no `any`, static dispatch only.
// Re-exported from the target-neutral `dom` module (shared with the
// browser-WASM sink); module aliases keep `live::diff::Patch`-style paths valid.
pub use crate::dom::diff;
pub use crate::dom::dispatch;
pub use diff::*;
pub use dispatch::*;
pub mod sse;
pub use crate::dom::form;
pub use form::*;
pub use sse::*;
pub mod route;
pub use route::*;
pub mod console;
pub mod csrf;
pub mod style_inject;
// Pre-built console child + reverse-proxy — spawns the bundled console
// binary and proxies /_ipe/console/*; falls back to in-process `console` when the
// binary is absent.
pub mod console_proxy;
pub mod observability;
// Observability export pipelines: federation push to a parent ingest
// and remote-hub OTLP push. Both env-gated + inert by default.
pub mod hub_exporter;
pub mod push_exporter;
// Hub read-side kernels (the bundled console's data plane). Gated on `db` —
// they read the SQLite telemetry spill via sqlx, so a `live`-only program with
// no db never compiles them and stays sqlx-free.
#[cfg(feature = "db")]
pub mod hub;
#[cfg(feature = "db")]
pub use hub::*;
pub mod req;
pub use req::*;
pub mod store;
pub use store::*;
pub mod pubsub;
// Explicit re-export of ONLY the codegen-referenced kernel functions. A glob
// (`pub use pubsub::*`) leaked the broker's `Event<T>` into this namespace,
// colliding with the HTML `Event` enum re-exported below (`pub use …html::*`)
// and surfacing as `error: `Event` is ambiguous` in generated code that names
// `ipe_runtime::Event`. The broker internals (`Event`, `Broker`, `broker`,
// `subscribe`, `publish`) are `pub(crate)` in pubsub.rs — they never leave the
// crate, so they don't need re-exporting here.
pub use pubsub::{
    cmd_publish, cmd_publish_no_echo, pubsub_publish, pubsub_publish_no_echo, sub_subscribe_topic,
};

// Html ADTs + renderer now live in the standalone top-level `html` module;
// re-export them so live submodules (diff.rs, store.rs, …) that `use super::*`
// still see Html / Attribute / Event / render_html / html_render_.
pub use crate::html::*;

use super::*;

/// Body returned with a session-miss 404 from `/_ipe/event` and `/_ipe/sse`.
///
/// LOAD-BEARING CONTRACT — `client.js` `__ipeProbeSessionLost` only triggers
/// `window.location.reload()` (the SSE-reconnect recovery path) when a probe
/// POST gets a 404 + `X-Ipê-Live: 1` AND the body CONTAINS the substring
/// `"session not found"` (client.js l1481/l1530/l1536). Go's backend returns
/// the same string; diverging it (the old `"no session"` body) silently broke
/// recovery after a server restart — the browser shows "Reconnecting…" forever.
/// Guarded by `session_lost_body_tests`.
const SESSION_LOST_BODY: &str = "session not found";

// ─── Client assets ────────────────────────────────────────────────────────────

/// The browser-side Ipe.Web client, extracted verbatim from Go's
/// `liveJSWithCfgAndCsrfWithBase` template (runtime-go/rt/live.go:5853-7490).
/// The 12 header `%`-verb lines are replaced with static literals;
/// the two `%%` CSS escapes are un-escaped to `%`.
const CLIENT_JS: &str = include_str!("client.js");

/// Content-addressing for the client asset: computed ONCE at first access via
/// `OnceLock`. Holds `(hex16, base64full)` where:
///   - `hex16` — first 16 hex chars of SHA-256(CLIENT_JS) → used in the URL
///     (`/_ipe/client.<hex16>.js`) for cache-busting.
///   - `base64full` — standard base64 of the full 32-byte SHA-256 digest → the
///     `integrity="sha256-<base64full>"` SRI attribute value.
///
/// Both are derived from the same digest, computed once and interned.
/// The `sha2` crate is unconditionally available in every generated Web project
/// (`default` features always include `crypto` which gates `sha2`).
static CLIENT_JS_HASH: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

/// Return `(hex16, base64full)` for `CLIENT_JS`, computing once on first call.
fn client_js_hashes() -> &'static (String, String) {
    CLIENT_JS_HASH.get_or_init(|| {
        use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
        use sha2::{Digest, Sha256};
        let digest: [u8; 32] = Sha256::digest(CLIENT_JS.as_bytes()).into();
        let hex16: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        let base64full = B64.encode(digest);
        (hex16, base64full)
    })
}

/// The content-addressed URL path for the client JS asset, e.g.
/// `/_ipe/client.a1b2c3d4e5f6a7b8.js`. The path is stable for a given
/// `client.js` build and changes whenever the file changes — making
/// `Cache-Control: immutable` safe. Callers may prepend the sub-app `base`.
pub fn client_js_path() -> String {
    let (hex16, _) = client_js_hashes();
    format!("/_ipe/client.{}.js", hex16)
}

/// Minimal CSS reset injected into every Ipe.Web page.
/// Ported verbatim from Go's `liveBaseCSS` (runtime-go/rt/live.go:3847-3858).
const BASE_CSS: &str = concat!(
    "*,*::before,*::after{box-sizing:border-box}",
    "html,body{margin:0;padding:0;min-height:100%}",
    "body{min-height:100vh;display:flex;flex-direction:column;font-family:-apple-system,BlinkMacSystemFont,\"Segoe UI\",Roboto,\"Helvetica Neue\",Arial,sans-serif;line-height:1.4}",
    "#ipe-root{display:flex;flex-direction:column;flex:1 0 auto;min-height:0}",
    "h1,h2,h3,h4,h5,h6,p,ul,ol,li,figure,blockquote,pre,dl,dd{margin:0;padding:0;font-weight:inherit;font-size:inherit}",
    "button,input,select,textarea{font:inherit;color:inherit}",
    "button{background:none;border:0;padding:0;cursor:pointer;text-align:inherit}",
    "a{color:inherit;text-decoration:none}",
    "img,video,canvas,svg{display:block;max-width:100%}",
);

// ─── Page renders ─────────────────────────────────────────────────────────────

/// Render `view(model)` to a full HTML page and print it — the static
/// render path (the interactive server is `web_app`).
pub fn web_render_static<E, Model, Msg, FView>(view: FView, model: Model) -> IpeTask<E, ()>
where
    E: Send + 'static,
    Model: Send + 'static,
    Msg: Send + 'static,
    FView: Fn(Model) -> Html<Msg> + Send + 'static,
{
    Box::pin(async move {
        let mut tree = view(model);
        assign_ipe_ids(&mut tree, "r");
        style_inject::apply_style_injections(&mut tree);
        println!("{}", render_page(&render_html(&tree)));
        IpeResult::Ok(())
    })
}

/// Minimal page wrap. Kept byte-identical so example 27-live-static
/// continues to pass. The full client-bearing wrap is `render_page_full`.
pub fn render_page(body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body><div id=\"ipe-root\">{body}</div></body></html>"
    )
}

/// Escape a serde-serialised JSON string for safe embedding inside a
/// `<script>` element (HTML script-data context, not attribute context).
///
/// JSON alone is not sufficient: a string field containing `</script>` would
/// end the `<script>` element, breaking out of the data island into executable
/// script context and defeating the no-eval / no-`'unsafe-eval'` posture.
/// The five characters below are the only ones that matter in script-data
/// context; `serde_json`'s own output already encodes control characters, so
/// no other escaping is required.
///
/// Escapes applied (JSON numeric escapes — losslessly round-trippable by any
/// JSON parser, including `serde_json`):
/// - U+003C `<`    → `<`  (forecloses `</script`)
/// - U+003E `>`    → `>`  (defence-in-depth against `>` injection)
/// - U+0026 `&`    → `&`  (forecloses HTML entity injection)
/// - U+2028 LINE SEPARATOR   → ` `  (JSON-legal but HTML-hostile)
/// - U+2029 PARAGRAPH SEPARATOR → ` `
///
/// Identical escape class as the telemetry `json_escape` U+2028/2029 gap —
/// the island serialiser applies it here for consistency.
pub fn island_escape(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for ch in json.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// Page wrap for isomorphic SSR + WASM hydration (M7 mode 2).
///
/// Emits a standard HTML page with:
/// - The SSR body in `<div id="ipe-root">`.
/// - The WASM bundle boot scripts (external JS + `hydrate(island_json)` call).
/// - A **typed public-payload island** `<script type="application/ipe-model+json">`
///   carrying the XSS-escaped, serde-serialised `HydrationState` JSON.
///
/// The island body is read by the WASM client via
/// `document.querySelector('script[type="application/ipe-model+json"]').textContent`
/// and passed to the emitted `hydrate(model_json)` entry — parsed with `serde_json`,
/// never evaluated. The `island_escape` call forecloses all script-injection paths.
///
/// `body`        — SSR-rendered HTML (from `render_html` with ipe-ids assigned).
/// `island_json` — serde-serialised `HydrationState` (BEFORE island_escape;
///                 this function applies the escape internally).
/// `pkg_base`    — URL prefix for the WASM bundle assets, e.g. `/pkg` or `./pkg`.
pub fn render_page_hydrate(body: &str, island_json: &str, pkg_base: &str) -> String {
    let escaped = island_escape(island_json);
    format!(
        "<!DOCTYPE html>\
<html>\
<head><meta charset=\"utf-8\"></head>\
<body>\
<div id=\"ipe-root\">{body}</div>\
<script type=\"application/ipe-model+json\">{escaped}</script>\
<script type=\"module\">\
import init, {{ hydrate }} from '{pkg_base}/ipe_app.js';\
async function boot() {{\
  await init('{pkg_base}/ipe_app_bg.wasm');\
  const island = document.querySelector('script[type=\"application/ipe-model+json\"]');\
  hydrate(island ? island.textContent : '');\
}}\
boot();\
</script>\
</body>\
</html>"
    )
}

#[cfg(test)]
mod island_escape_tests {
    use super::island_escape;

    #[test]
    fn escapes_lt_gt_amp() {
        let input = r#"{"k":"<b>&amp;</b>"}"#;
        let out = island_escape(input);
        assert!(!out.contains('<'));
        assert!(!out.contains('>'));
        assert!(!out.contains('&'));
        assert!(out.contains("\\u003c"));
        assert!(out.contains("\\u003e"));
        assert!(out.contains("\\u0026"));
    }

    #[test]
    fn script_injection_foreclosed() {
        let payload = r#"{"x":"</script><script>evil()</script>"}"#;
        let out = island_escape(payload);
        assert!(!out.contains("</script>"), "script tag must not be present");
    }

    #[test]
    fn line_separator_escaped() {
        let payload = "\u{2028}\u{2029}";
        let out = island_escape(payload);
        assert!(!out.contains('\u{2028}'));
        assert!(!out.contains('\u{2029}'));
        assert!(out.contains("\\u2028"));
        assert!(out.contains("\\u2029"));
    }

    #[test]
    fn plain_json_is_lossless() {
        let payload = r#"{"count":42,"name":"hello"}"#;
        let out = island_escape(payload);
        assert_eq!(out, payload); // nothing to escape
    }
}

/// Full page wrap with the live client loaded as a cacheable external asset.
/// Mirrors Go's live page render (runtime-go/rt/live.go:3788).
///
/// `sid`  — session id (injected into the JS via `window.__IPE_SID`).
/// `base` — sub-app base path, e.g. "" for root-mounted apps.
/// `body` — pre-rendered HTML body (from `render_html`).
///
/// Two scripts are emitted in document order (no defer/async — execution order
/// is left-to-right by the HTML spec):
///   1. A tiny inline `<script>` setting the three per-session window globals
///      (`__IPE_SID`, `__IPE_BASE`, `__IPE_CSRF_TOKEN`). These MUST stay inline
///      because they are per-session values and must never be cached.
///   2. An external `<script src="…/_ipe/client.<hash>.js" integrity="sha256-…"
///      crossorigin="anonymous">` loading the invariant client body. The URL is
///      content-addressed (hash of the file) so it is safe to cache with
///      `immutable`. The SRI `integrity` attribute lets the browser verify the
///      file has not been tampered with before execution.
///
/// CSP note: the inline window-vars script still requires `script-src
/// 'unsafe-inline'` (unchanged from the fully-inlined baseline). The external
/// script requires no additional CSP directive beyond `script-src 'self'`
/// (already needed for same-origin resource loading). Adding a nonce to the
/// inline script to tighten CSP is deferred; it requires threading the nonce
/// through the response pipeline and is outside the scope of this change.
/// Server-side client-config templating (Go parity, live.go ~5993): read the
/// `IPE_LIVE_*` tuning env vars and emit the `window.__IPE_*` assignments the
/// client (`client.js`) reads with a hardcoded fallback. Without this the Rust
/// client ignored every `IPE_LIVE_RETRY_*` / `QUEUE_MAX` / `HELLO_TIMEOUT_MS` /
/// `HEARTBEAT_TTL_MS` / `BANNER` override. Totally parsed: a malformed value
/// falls back to Go's default; never panics.
fn live_client_config_js() -> String {
    fn num(var: &str, default: u64) -> u64 {
        crate::system::read_env_var(var)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(default)
    }
    // IPE_LIVE_BANNER: off/0/false → disabled (Go parity); anything else → on.
    let banner = !matches!(
        crate::system::read_env_var("IPE_LIVE_BANNER")
            .ok()
            .map(|s| s.trim().to_ascii_lowercase()),
        Some(ref v) if v == "off" || v == "0" || v == "false"
    );
    format!(
        "window.__IPE_BANNER_ENABLED={banner};\
         window.__IPE_RETRY_BASE_MS={};\
         window.__IPE_RETRY_MAX_MS={};\
         window.__IPE_RETRY_MAX_ATTEMPTS={};\
         window.__IPE_EVENT_QUEUE_MAX={};\
         window.__IPE_HELLO_TIMEOUT_MS={};\
         window.__IPE_HEARTBEAT_TTL_MS={};\
         window.__IPE_MSG_RECONNECTING=\"Reconnecting…\";\
         window.__IPE_MSG_OFFLINE=\"Connection lost — refresh to retry\";",
        num("IPE_LIVE_RETRY_BASE_MS", 500),
        num("IPE_LIVE_RETRY_MAX_MS", 16000),
        num("IPE_LIVE_RETRY_MAX_ATTEMPTS", 10),
        num("IPE_LIVE_QUEUE_MAX", 50),
        num("IPE_LIVE_HELLO_TIMEOUT_MS", 8000),
        num("IPE_LIVE_HEARTBEAT_TTL_MS", 35000),
    )
}

pub fn render_page_full(sid: &str, base: &str, body: &str, csrf_token: &str) -> String {
    // sid_js / base_js / csrf_js: Rust Debug ("{:?}") of a &str yields a
    // double-quoted, properly-escaped JS string literal for plain ASCII
    // session ids, base paths, and the hex CSRF token.
    let sid_js = format!("{sid:?}");
    let base_js = format!("{base:?}");
    let csrf_js = format!("{csrf_token:?}");
    let dev_banner = dev_console_banner(base);
    // Content-addressed client asset URL and SRI hash — computed once at first call.
    let (hex16, b64) = client_js_hashes();
    // Honour the sub-app base prefix so the external script request goes through
    // the parent proxy (same as /_ipe/sse, /_ipe/event, /_ipe/console).
    let client_src = format!("{base}/_ipe/client.{hex16}.js");
    let integrity = format!("sha256-{b64}");
    let config_js = live_client_config_js();
    format!(
        "<!DOCTYPE html><html><head>\
         <meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"ipe-base\" content=\"{base}\">\
         <style>{BASE_CSS}</style>\
         </head>\
         <body><div id=\"ipe-root\">{body}</div>{dev_banner}\
         <script>window.__IPE_SID={sid_js};window.__IPE_BASE={base_js};window.__IPE_CSRF_TOKEN={csrf_js};{config_js}</script>\
         <script src=\"{client_src}\" integrity=\"{integrity}\" crossorigin=\"anonymous\"></script>\
         </body></html>"
    )
}

/// Floating "🔍 Console" link injected into every dev-mode page. The
/// implementation lives in the always-compiled `telemetry` module so the
/// Ipe.Http.Server path (`server.rs`) shares the identical byte-exact banner;
/// this is a thin re-export for the Live page renderer.
fn dev_console_banner(base: &str) -> String {
    crate::telemetry::dev_console_banner(base)
}

// ─── web_app: axum mount + per-session TEA driver over SSE ─────────────────

use crate::tea::{IpeCmd, IpeSub};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::mpsc::{self, Receiver, Sender};

/// Per-session live state behind an `Arc<Mutex<…>>`. `index` / `last_view` are
/// re-derived on every commit; `sse_tx` is filled when the browser attaches the
/// SSE channel; `msg_tx` feeds the per-session driver loop.
pub struct SessionEntry<Model, Msg> {
    pub model: Model,
    pub last_view: Html<Msg>,
    pub index: HandlerIndex<Msg>,
    pub seq: u64,
    pub sse_tx: Option<SseTx>,
    pub msg_tx: Sender<Msg>,
}

/// SSE patches envelope. The browser client (`live/client.js`) consumes the
/// `event: patches` frame as `{globalSeq, patches}` and routes it through
/// `__ipeHandleResponse(undefined, _, _, globalSeq)` → `__ipeApplyPatches`.
/// We use `globalSeq` (the server-owned broadcast counter) rather than the
/// local `seq` so it never collides with the client's own POST-local seq gate.
#[derive(serde::Serialize)]
struct PatchEnvelope<'a> {
    #[serde(rename = "globalSeq")]
    global_seq: u64,
    patches: &'a [crate::dom::diff::Patch],
}

/// Wire shape POSTed by the browser client to `/_ipe/event`
/// (`live/client.js` __ipeSend): `{sessionId, seq, msg, args, handlerId}`.
/// `handlerId` is the element's `data-ipe-hid` (== its ipe-id); `msg` is the
/// `ipe-<event>` marker. We resolve handlers server-side by ipe-id + event,
/// so `handlerId` is the authoritative locator; `event` is derived below.
#[derive(serde::Deserialize)]
struct EventBody {
    #[serde(default)]
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    #[serde(rename = "handlerId")]
    handler_id: String,
    /// Some senders use `id` instead of `handlerId`; accept both.
    #[serde(default)]
    id: String,
    /// Event name. The client posts the `ipe-<event>` marker value as `msg`;
    /// `render_html` makes that value the event name (click / input / submit / …),
    /// so `msg` is the authoritative event. `event` is an explicit-override slot
    /// for future senders. Resolution: `event` ?: `msg` ?: "click".
    #[serde(default)]
    event: String,
    #[serde(default)]
    msg: String,
    /// Event args. For click/input/keydown `args[0]` is a string; for `submit`
    /// `args[0]` is the form-data object `{name: value, …}`. Parsed as JSON
    /// values so both shapes decode.
    #[serde(default)]
    args: Vec<serde_json::Value>,
}

/// Coerce a wire arg `Value` to the string the click/input/keydown path expects.
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Boxed route resolver: a freshly-`init`'d model + GET path → the model whose
/// `page` field reflects the matched route.
type RouteResolver<Model> = Arc<dyn Fn(Model, &str) -> Model + Send + Sync>;
/// Boxed param resolver: a GET path → the matched route's `:name`→value params.
type ParamResolver = Arc<dyn Fn(&str) -> crate::dict::IpeDict<String> + Send + Sync>;
/// Boxed route predicate: does a GET path match a declared route? (Go
/// `matchAnyRoute` parity.) Gates the page handler's browser-noise 404 and
/// the unrouted-GET-against-a-live-session 404 — see `page`.
type RouteMatched = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Shared axum state: the session store + Arc'd TEA callbacks.
struct WebState<Model, Msg, FInit, FUpdate, FView, FSubs> {
    store: Arc<dyn store::SessionStore<Model, Msg>>,
    init: Arc<FInit>,
    update: Arc<FUpdate>,
    view: Arc<FView>,
    subs: Arc<FSubs>,
    /// Maps the freshly-`init`'d model + GET path to the model whose `page`
    /// field reflects the matched route. `web_app` passes identity (no
    /// routing); `web_app_routed` captures the route table + page-setter.
    /// `Page`/`set_page` are erased into this boxed closure, so `WebState`
    /// keeps its original 6 type params.
    route_resolver: RouteResolver<Model>,
    /// Maps a GET path to the matched route's `:name`→value params (for
    /// `req.params`). Model-independent so the page handler can build `req`
    /// BEFORE calling `init`. `web_app` returns empty; `web_app_routed`
    /// captures the route table.
    param_resolver: ParamResolver,
    /// Does a GET path match a declared route? (Go `matchAnyRoute` parity —
    /// `web_app` treats only `/` as routed; `web_app_routed` captures the
    /// route table.) An unrouted GET must never re-route a live session's
    /// model or rebuild its handler index: that wipes the handlers of the
    /// page the browser is showing, silently killing every subsequent event
    /// (form submits included).
    route_matched: RouteMatched,
    /// Live driver count for admission control. Each spawned `drive_session`
    /// holds a `SessionSlot` that decrements this on exit; a cookieless GET that
    /// would push it past `max_sessions()` is rejected (503) instead of minting
    /// an unbounded number of sessions. Decremented ONLY via `SessionSlot::drop`,
    /// so the leak fix (mortal driver) and this cap share one mechanism.
    session_count: Arc<AtomicUsize>,
}

// Manual Clone — derive would demand Clone on the closures (they're behind Arc).
impl<Model, Msg, FInit, FUpdate, FView, FSubs> Clone
    for WebState<Model, Msg, FInit, FUpdate, FView, FSubs>
{
    fn clone(&self) -> Self {
        WebState {
            store: self.store.clone(),
            init: self.init.clone(),
            update: self.update.clone(),
            view: self.view.clone(),
            subs: self.subs.clone(),
            route_resolver: self.route_resolver.clone(),
            param_resolver: self.param_resolver.clone(),
            route_matched: self.route_matched.clone(),
            session_count: self.session_count.clone(),
        }
    }
}

/// Max concurrent live-session drivers (admission control). 0 = unlimited
/// (opt-out). Default 50_000 — far above any single-instance real load, low
/// enough to bound memory under a session-creation flood. Env IPE_LIVE_MAX_SESSIONS.
fn max_sessions() -> usize {
    crate::system::read_env_var("IPE_LIVE_MAX_SESSIONS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50_000)
}

/// RAII admission slot: decrements `WebState::session_count` exactly once when
/// the owning `drive_session` task exits (any path). Paired 1:1 with the
/// `fetch_add` reservation at the session-create site — the ONLY decrement.
struct SessionSlot {
    count: Arc<AtomicUsize>,
}
impl Drop for SessionSlot {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Fire a `Cmd`: None/Batch recurse; Perform spawns the composed task→Msg thunk
/// and pushes the result back into the per-session loop.
fn run_cmd<Msg: Send + 'static>(cmd: IpeCmd<Msg>, tx: &Sender<Msg>, sid: &str) {
    match cmd {
        IpeCmd::None => {}
        IpeCmd::Batch(items) => {
            for c in items {
                run_cmd(c, tx, sid);
            }
        }
        IpeCmd::Perform(thunk) => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let m = thunk().await;
                // Bounded send: drop the Msg and warn if the session queue is
                // full (a stalled driver or a burst of fast Perform tasks).
                if tx.send(m).await.is_err() {
                    eprintln!(
                        "[ipe.live] run_cmd: session msg channel closed; dropping Perform result"
                    );
                }
            });
        }
        IpeCmd::Publish(thunk) => {
            // Inject this session's sid as the broadcast origin (Go parity:
            // liveApp.Publish sets Origin = session.sid). Fire-and-forget.
            let _ = thunk(sid);
        }
    }
}

/// (Re-)spawn subscription tasks. Aborts the previous handles first (one model,
/// re-evaluated each commit — Go tea_subs.go parity). When `subscriptions` is
/// `Sub.none`, this is exercised mainly by the None arm.
fn spawn_subs<Msg: Clone + Send + 'static>(
    sub: IpeSub<Msg>,
    tx: &Sender<Msg>,
    handles: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    for h in handles.drain(..) {
        h.abort();
    }
    fn go<Msg: Clone + Send + 'static>(
        sub: IpeSub<Msg>,
        tx: &Sender<Msg>,
        handles: &mut Vec<tokio::task::JoinHandle<()>>,
    ) {
        match sub {
            IpeSub::None => {}
            IpeSub::Batch(items) => {
                for s in items {
                    go(s, tx, handles);
                }
            }
            IpeSub::Every { ms, msg } => {
                if ms <= 0 {
                    return;
                }
                let tx = tx.clone();
                let dur = std::time::Duration::from_millis(ms as u64);
                let h = tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(dur).await;
                        // Bounded send: break when the session queue is full
                        // or the receiver is gone (driver exited).
                        if tx.send(msg.clone()).await.is_err() {
                            break;
                        }
                    }
                });
                handles.push(h);
            }
            IpeSub::Source(spawn) => {
                let tx = tx.clone();
                let emit: Arc<dyn Fn(Msg) + Send + Sync> = Arc::new(move |m| {
                    let _ = tx.try_send(m);
                });
                handles.push(spawn(emit));
            }
        }
    }
    go(sub, tx, handles);
}

/// The per-session driver: folds each Msg through `update`, diffs the new view
/// against the last, pushes patches over SSE (if attached), runs the resulting
/// Cmd, and re-evaluates subscriptions.
// Eight distinct per-session runtime handles (entry, both channel ends, the three
// Arc'd TEA callbacks, the store, the sid) — bundling them into a struct purely to
// satisfy the 7-arg heuristic would add indirection without clarifying anything.
#[allow(clippy::too_many_arguments)]
async fn drive_session<Model, Msg, FUpdate, FView, FSubs>(
    // WEAK ref: the driver must NOT keep the session alive. The strong holders are
    // the store map (until TTL evict) and any open SSE connection (pins the entry
    // for the connection lifetime — see sse_handler). When BOTH release, the entry
    // drops and the driver exits (tick `upgrade()` → None), closing the leak where
    // a strong-Arc + own-msg_tx made `recv()` never return None → immortal task.
    entry: Weak<Mutex<SessionEntry<Model, Msg>>>,
    mut msg_rx: Receiver<Msg>,
    msg_tx: Sender<Msg>,
    update: Arc<FUpdate>,
    view: Arc<FView>,
    subs: Arc<FSubs>,
    store: Arc<dyn store::SessionStore<Model, Msg>>,
    sid: String,
    // Admission-control slot: decrements WebState::session_count on driver exit.
    _slot: SessionSlot,
) where
    // PartialEq: the `noop` signal compares old vs new Model by structural
    // equality. Generated Model structs always derive PartialEq.
    Model: Clone + PartialEq + Send + 'static,
    // `Debug` is required to derive the BOUNDED Msg variant-name label for the
    // `ipe_live_msg_seconds` histogram (telemetry::variant_name). Generated Msg
    // enums always derive Debug, so this internal bound is always satisfiable.
    Msg: Clone + Send + std::fmt::Debug + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
    FView: Fn(Model) -> Html<Msg> + Send + Sync + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + Send + Sync + 'static,
{
    let mut sub_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    // Initial subscriptions — Go parity (setupSubscriptions runs at session
    // creation, before the first event; live.go:3729). Without this a
    // watch-only session never subscribes until it dispatches its own Msg, so a
    // pub/sub broadcast (or a Sub.every ticker) would never reach a freshly
    // loaded session. Wrapped in the session-sid scope so SkipOrigin filtering
    // binds the right owner.
    {
        // Upgrade transiently; if the session is already gone there is nothing to drive.
        let Some(strong) = entry.upgrade() else {
            return;
        };
        let model0 = {
            strong
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .model
                .clone()
        };
        pubsub::with_session_sid(sid.clone(), || {
            spawn_subs(subs(model0), &msg_tx, &mut sub_handles)
        });
    }
    // Periodic liveness check: the driver holds only a Weak ref, but it also holds
    // its own `msg_tx` clone, so `recv()` alone never returns None. The tick
    // upgrades the Weak — once the store has evicted the session AND no SSE
    // connection pins it, `upgrade()` returns None and the driver exits (freeing
    // the entry, the channel, and the admission slot). 30 s bounds a dead driver's
    // lifetime to one interval past eviction.
    let mut liveness = tokio::time::interval(std::time::Duration::from_secs(30));
    liveness.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let msg = tokio::select! {
            maybe = msg_rx.recv() => match maybe {
                Some(m) => m,
                None => break,
            },
            _ = liveness.tick() => {
                if entry.upgrade().is_none() {
                    break;
                }
                continue;
            }
        };
        // Upgrade for THIS iteration only; drop `strong` before the next select!
        // (holding it across the park would re-pin the entry and re-introduce the
        // leak). None ⇒ the session was evicted between messages ⇒ stop.
        let Some(strong) = entry.upgrade() else {
            break;
        };
        // Clone the model under a short lock, release before update.
        let model = {
            strong
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .model
                .clone()
        };
        // Msg-handling latency histogram (Go parity: ipe_live_msg_seconds{name},
        // msg_logging.go). The `name` label is the BOUNDED Msg variant name
        // (finite cardinality), never a payload — see telemetry::variant_name.
        // Extracted BEFORE `update` consumes `msg`.
        let msg_name = crate::telemetry::variant_name(&msg);
        let msg_started = std::time::Instant::now();
        let (next, cmd) = update(msg, model);
        crate::telemetry::metric_observe(
            "ipe_live_msg_seconds",
            &[("name", &msg_name)],
            msg_started.elapsed().as_secs_f64(),
        );
        // Borrow (not move) cmd to detect a no-command update; cmd is moved into
        // run_cmd later. Part of the `noop` signal below.
        let cmd_is_none = matches!(cmd, IpeCmd::None);

        let mut tree = view(next.clone());
        assign_ipe_ids(&mut tree, "r");
        style_inject::apply_style_injections(&mut tree);

        let (patches, seq, sse, noop) = {
            let mut e = strong.lock().unwrap_or_else(|e| e.into_inner());
            let patches = diff(&e.last_view, &tree);
            // noop (Go parity: oldHash==newHash && cmdIsNone && err==nil). Here
            // `e.model` STILL holds the OLD model (top-of-loop cloned it OUT; the
            // store isn't updated until the assignment below), so `e.model ==
            // next` is old==new — a STRUCTURAL equality (no hash-collision false
            // noop, unlike Go's hash), computed with NO extra clone. The Rust
            // dispatch has no error channel, so the `err==nil` conjunct is always
            // true and is dropped.
            let noop = cmd_is_none && e.model == next;
            e.last_view = tree.clone();
            e.index = build_index(&tree);
            e.model = next.clone();
            e.seq += 1;
            (patches, e.seq, e.sse_tx.clone(), noop)
        };
        // Msg counter (Go parity: ipe_live_msg_total{name,outcome,noop}). All
        // labels bounded: name = finite variant set, outcome = "ok" (this path
        // has no error channel), noop ∈ {true,false}. Emitted OUTSIDE the entry
        // lock (no registry-lock-under-entry-lock nesting).
        crate::telemetry::metric_inc(
            "ipe_live_msg_total",
            &[
                ("name", &msg_name),
                ("outcome", "ok"),
                ("noop", if noop { "true" } else { "false" }),
            ],
            1,
        );

        if !patches.is_empty()
            && let Some(sse) = sse
        {
            let env = PatchEnvelope {
                global_seq: seq,
                patches: &patches,
            };
            if let Ok(json) = serde_json::to_string(&env) {
                let _ = sse.send(SsePatch(sse::frame("patches", &json))).await;
            }
        }

        // Write-through: checkpoint the committed model to the store (a touch
        // for memory; a re-serialize for persistent backends — Go store.Set on
        // every commit). Re-inserting an evicted-but-active session with a fresh
        // last-seen is intended (Go parity): a session that processes a Msg is
        // alive. `strong` is dropped at the end of this iteration (block scope),
        // before the next select! park — never held across the await loop.
        store.set(&sid, strong.clone()).await;

        run_cmd(cmd, &msg_tx, &sid);
        pubsub::with_session_sid(sid.clone(), || {
            spawn_subs(subs(next.clone()), &msg_tx, &mut sub_handles)
        });
    }
    for h in sub_handles.drain(..) {
        h.abort();
    }
}

/// Generate a 128-bit random session id as 32 hex chars. Self-contained
/// (no uuid crate / uuid_kernel module dependency, since the generated
/// A fresh session id: **128 bits from the OS CSPRNG**, hex-encoded.
///
/// SECURITY: the sid is the SOLE bearer credential for a Ipe.Web session
/// (`sid_from_cookie` + `store.get` authorise every event off it). It MUST be
/// unpredictable. The prior scheme — `clock_nanos XOR counter` through
/// splitmix64 — was an invertible bijection over low-entropy, partly-known
/// inputs (the counter starts at 0; the clock is estimable), so sids were
/// guessable → session hijacking. OsRng is the OS CSPRNG (the same one
/// `crypto.rs` / `csrf.rs` use; no new dependency). Never panics.
fn new_sid() -> String {
    use aes_gcm::aead::{OsRng, rand_core::RngCore};
    let mut buf = [0u8; 16];
    OsRng.fill_bytes(&mut buf);
    let mut s = String::with_capacity(32);
    for b in buf {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Normalise a raw `IPE_LIVE_BASE_PATH` value: trim, drop a trailing slash,
/// ensure a single leading slash. `""` / `"/"` collapse to `""` (root-mounted —
/// no prefix). Mirrors Go's `normaliseBasePath` (runtime-go/rt/live.go:5901).
fn normalise_base_path(raw: &str) -> String {
    let t = raw.trim().trim_end_matches('/');
    if t.is_empty() {
        String::new()
    } else if t.starts_with('/') {
        t.to_string()
    } else {
        format!("/{t}")
    }
}

/// The session cookie name for a given (normalised) base path. At the root,
/// `__Host-ipe_sid` in secure mode (production / frame-ancestors) else `ipe_sid`;
/// for a sub-app a base-derived DISTINCT name so this child's session cookie can
/// never clobber the PARENT app's `ipe_sid` (both would otherwise be `Path=/` and
/// share the browser's cookie jar on the proxied paths). Go gives each sub-app a
/// distinct `cookieName` for the same reason (live.go:2769).
///
/// SECURITY (root, secure mode): the session cookie is the SOLE bearer credential
/// (`sid_from_cookie` + `store.get` authorise every `/_ipe/event` + `/_ipe/sse`),
/// so it gets the `__Host-` prefix — the browser then refuses any `Set-Cookie`
/// carrying a `Domain=` attribute, closing the sibling-subdomain cookie-tossing →
/// session-fixation vector (an attacker on `evil.example.com` with a valid cert
/// could otherwise plant `ipe_sid` for `example.com`). `__Host-` MANDATES
/// Secure + Path=/ + no-Domain — `page_response` satisfies all three in secure
/// mode (Secure flag set, root `cookie_path()` is `/`, no Domain attribute).
/// Mirrors `csrf::csrf_cookie_name()`. Plain-HTTP dev keeps the bare `ipe_sid`
/// (`__Host-` requires Secure, which a browser drops over `http://`). A sub-app
/// (Path != `/`) can never use `__Host-`, so it keeps the base-scoped name.
fn cookie_name_for(base: &str) -> String {
    if base.is_empty() {
        if csrf::cookies_secure() {
            "__Host-ipe_sid".to_string()
        } else {
            "ipe_sid".to_string()
        }
    } else {
        let suffix: String = base
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("ipe_sid{suffix}")
    }
}

/// Cookie `Path` for a given (normalised) base path: the base for a sub-app
/// (scopes the cookie to `/<base>/*` so it is never sent to the parent's own
/// routes — protecting the parent session), else `/`.
fn cookie_path_for(base: &str) -> String {
    if base.is_empty() {
        "/".to_string()
    } else {
        base.to_string()
    }
}

/// Normalised sub-app base path, read from `IPE_LIVE_BASE_PATH`. Empty when
/// unset (root-mounted app → byte-identical to a standalone Web server). When
/// set (this app runs as a reverse-proxied sub-app — e.g. the bundled console
/// mounted at `/_ipe/console`), the value is threaded into `render_page_full`
/// so the client JS prefixes `/_ipe/event` + `/_ipe/sse` with it. The browser
/// reaches this child only through the parent proxy, which strips the prefix
/// before forwarding — so the child's own router stays root-relative.
fn live_base_path() -> String {
    normalise_base_path(&crate::system::read_env_var("IPE_LIVE_BASE_PATH").unwrap_or_default())
}

/// The active session cookie name (read AND write must agree, so both
/// `page_response` and `sid_from_cookie` route through this).
fn session_cookie_name() -> String {
    cookie_name_for(&live_base_path())
}

/// The active session cookie `Path`.
fn cookie_path() -> String {
    cookie_path_for(&live_base_path())
}

/// Whether to trust `X-Forwarded-Proto` for TLS-termination detection. Mirrors
/// `server.rs`'s `IPE_TRUSTED_PROXY` gate (same env var, same rationale: a
/// client-supplied header must never be trusted by default — an operator opts
/// in only when a real reverse proxy sits in front of this process).
///
/// Snapshotted once (env is stable at process start; same rationale as
/// `csrf::cookies_secure()` — avoids a per-request global env-lock read).
fn trust_proxy_headers() -> bool {
    use std::sync::OnceLock;
    static TRUST: OnceLock<bool> = OnceLock::new();
    *TRUST.get_or_init(|| {
        crate::system::read_env_var("IPE_TRUSTED_PROXY")
            .map(|v| !v.is_empty() && v != "0" && v != "false")
            .unwrap_or(false)
    })
}

/// Request-scoped HTTPS detection, parameterised on the trust decision so it's
/// unit-testable without mutating the real (OnceLock-cached) process env —
/// `trust_proxy_headers()` snapshots once per process, so a test that mutates
/// `IPE_TRUSTED_PROXY` and expects `request_is_https` to observe the change
/// would be flaky/order-dependent. Only consulted (via `request_is_https`)
/// when `trust` is true — otherwise a client could forge `X-Forwarded-Proto`
/// to fool the Secure-cookie decision (the same footgun `server.rs` already
/// closed for `X-Forwarded-For`).
fn request_is_https_with_trust(headers: &axum::http::HeaderMap, trust: bool) -> bool {
    if !trust {
        return false;
    }
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

/// Request-scoped HTTPS detection: true when THIS request arrived over TLS at
/// the trusted proxy (`X-Forwarded-Proto: https`). See
/// `request_is_https_with_trust` for the testable core.
fn request_is_https(headers: &axum::http::HeaderMap) -> bool {
    request_is_https_with_trust(headers, trust_proxy_headers())
}

/// Build the full-page HTTP response for a GET (initial render or reuse): the
/// client-bearing HTML wrap + the session cookie (name/path base-path-aware).
fn page_response(
    sid: &str,
    body: &str,
    csrf_token: &str,
    headers: &axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let html = render_page_full(sid, &live_base_path(), body, csrf_token);
    // Session cookie carries `Secure` in production / frame-ancestors mode, OR
    // when this specific request arrived over TLS at a trusted proxy
    // (`request_is_https`, opt-in via `IPE_TRUSTED_PROXY` — closes the gap where
    // `csrf::cookies_secure()` snapshots `production_from_env() ||
    // frame_ancestors().is_some()` ONCE at process start and never inspects this
    // request's TLS / `X-Forwarded-Proto`, so a dev process fronted by a TLS
    // proxy would otherwise emit a non-Secure session cookie even though the
    // browser connection was HTTPS). The untrusted-proxy case (operator hasn't
    // set `IPE_TRUSTED_PROXY`) keeps ENV-only behaviour — still SOUND, just not
    // maximally precise, because it never marks a cookie Secure incorrectly,
    // only potentially fails to mark one Secure that could safely have been.
    //
    // NOTE: this does NOT change the `__Host-` cookie-NAME decision
    // (`csrf::cookies_secure()`, still process-global) — the cookie's identity
    // must stay stable across a browser session, or the double-submit compare
    // would spuriously fail whenever proxy-scheme detection flips between
    // requests. Only the SESSION cookie's `Secure` ATTRIBUTE becomes
    // request-scoped.
    //
    // SameSite=Lax stays so top-level navigations keep the session.
    let secure = if csrf::cookies_secure() || request_is_https(headers) {
        "; Secure"
    } else {
        ""
    };
    // SameSite (Go parity, live.go ~5653): a deploy opted into cross-origin
    // embedding via IPE_LIVE_FRAME_ANCESTORS needs `SameSite=None; Secure` so the
    // session cookie survives inside a third-party iframe; otherwise `Lax`
    // (top-level navigations keep the session). `cookies_secure()` is already true
    // in frame-ancestors mode, so `None` always pairs with `Secure`.
    let same_site = if csrf::frame_ancestors().is_some() {
        "None"
    } else {
        "Lax"
    };
    // Max-Age (Go parity, live.go ~5641): persist the cookie for the store TTL so a
    // tab-close doesn't drop a still-live server session. Without it the cookie is
    // session-scoped and the user loses state on tab close.
    let max_age = live_ttl().as_secs();
    let session_cookie = format!(
        "{}={sid}; Path={}; HttpOnly; SameSite={same_site}{secure}; Max-Age={max_age}",
        session_cookie_name(),
        cookie_path()
    );
    let csrf_cookie = csrf::csrf_set_cookie(csrf_token);
    let mut resp = (
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/html; charset=utf-8".to_string(),
        )],
        html,
    )
        .into_response();
    let h = resp.headers_mut();
    // Two Set-Cookie headers — `append`, not `insert`, so both land.
    if let Ok(v) = axum::http::HeaderValue::from_str(&session_cookie) {
        h.append(axum::http::header::SET_COOKIE, v);
    }
    if let Ok(v) = axum::http::HeaderValue::from_str(&csrf_cookie) {
        h.append(axum::http::header::SET_COOKIE, v);
    }
    // Security response headers (Go parity + hardening) — page GET only.
    for (name, val) in csrf::security_headers() {
        if let Ok(v) = axum::http::HeaderValue::from_str(&val) {
            h.insert(axum::http::HeaderName::from_static(name), v);
        }
    }
    resp
}

/// Maximum request body bytes for `/_ipe/event`: `IPE_LIVE_MAX_BODY_BYTES`,
/// default 5 MiB (5 << 20 = 5 242 880). Mirrors Go's `handleEvent` body cap
/// (runtime-go/rt/live.go ~l3911). The default covers `Event.onFile` /
/// `Event.onImage` data-URL payloads; override for larger file uploads.
fn live_max_body_bytes() -> usize {
    crate::system::read_env_var("IPE_LIVE_MAX_BODY_BYTES")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(5 << 20)
}

#[cfg(test)]
mod live_max_body_bytes_tests {
    // IPE_LIVE_MAX_BODY_BYTES=0 must floor at the default, not disable the
    // body (matching server::max_body's `.filter(|&n| n > 0)`). Without the
    // floor a 0 value would 413 every /_ipe/event POST.
    //
    // This tests the parsing/filtering formula directly rather than mutating
    // the real env var: `std::env::set_var` is not thread-safe under a
    // parallel test harness, and `IPE_LIVE_MAX_BODY_BYTES` already has an
    // env-mutating test in server.rs (`max_body_env_override`) — a second
    // unsynchronized mutator of the same key would make both tests
    // intermittently flaky. Mirrors the established convention documented at
    // `server::tests::ws_send_buffer_default_is_256`.
    fn parse(raw: Option<&str>) -> usize {
        raw.and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(5 << 20)
    }

    #[test]
    fn live_max_body_bytes_floors_at_default_on_zero() {
        assert_eq!(parse(None), 5 << 20);
        assert_eq!(parse(Some("1024")), 1024);
        assert_eq!(parse(Some("0")), 5 << 20); // invalid → default, not "reject everything"
    }
}

/// Session idle-TTL: `IPE_LIVE_TTL` seconds, default 1800 (30 min) — matches the
/// Go `[live] ttl` default.
fn live_ttl() -> std::time::Duration {
    let secs = crate::system::read_env_var("IPE_LIVE_TTL")
        .ok()
        .and_then(|s| parse_duration_secs(&s))
        .unwrap_or(1800u64);
    std::time::Duration::from_secs(secs)
}

/// Parse a Go-style duration (Go parity for `IPE_LIVE_TTL` / `[live] ttl`): a bare
/// integer is seconds (legacy), otherwise one or more `<number><unit>` segments
/// with units `h` / `m` / `s` (e.g. `30m`, `1h`, `24h`, `90s`, `1h30m`). Total: any
/// malformed input returns `None` (caller falls back to the default) — never panics.
fn parse_duration_secs(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // Bare integer → seconds (legacy form).
    if let Ok(n) = s.parse::<u64>() {
        return Some(n);
    }
    let mut total: u64 = 0;
    let mut num: u64 = 0;
    let mut saw_unit = false;
    let mut saw_digit = false;
    for ch in s.chars() {
        if let Some(d) = ch.to_digit(10) {
            num = num.checked_mul(10)?.checked_add(d as u64)?;
            saw_digit = true;
        } else {
            let unit_secs = match ch {
                'h' => 3600,
                'm' => 60,
                's' => 1,
                _ => return None, // unknown unit / stray char → malformed
            };
            if !saw_digit {
                return None; // a unit with no preceding number
            }
            total = total.checked_add(num.checked_mul(unit_secs)?)?;
            num = 0;
            saw_digit = false;
            saw_unit = true;
        }
    }
    // A trailing number with no unit (e.g. `1h30`) is malformed.
    if saw_digit || !saw_unit {
        return None;
    }
    Some(total)
}

/// Graceful-drain grace window (Go parity for `srv.Close()`): how long the
/// pure axum graceful drain is allowed before we force a CLEAN exit-0. Go does
/// NOT drain — it calls `srv.Close()` which forcibly drops every connection
/// (including the never-idle SSE streams) and returns immediately. axum's
/// `with_graceful_shutdown` instead WAITS for every connection to finish, so an
/// open SSE `EventSource` (heartbeat every 15 s, otherwise idle — it never
/// completes) would hang the drain forever. This window lets ordinary in-flight
/// requests finish, then force-exits 0 so SSE clients are dropped exactly as Go
/// drops them (the browser banner flips to "Reconnecting…", same UX as a deploy).
/// Tunable via `IPE_LIVE_SHUTDOWN_GRACE_MS` (default 1500 ms; 0 = exit at once).
fn shutdown_grace() -> std::time::Duration {
    let ms = crate::system::read_env_var("IPE_LIVE_SHUTDOWN_GRACE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1500);
    std::time::Duration::from_millis(ms)
}

/// Best-effort bounded flush of all active telemetry exporters (push + hub).
/// Waits at most 500 ms in total. Never panics, never blocks shutdown beyond
/// the cap. This MUST be called before every `process::exit` because
/// `process::exit` skips Drop, so the mpsc Sender never drops and the
/// batchers' channel-close drain path never runs without this explicit flush.
async fn flush_exporters() {
    // 500 ms total cap (split across two exporters in sequence — each is capped
    // independently so a slow/unavailable first target doesn't eat all of the
    // second exporter's budget).
    const CAP_MS: u64 = 250;
    push_exporter::flush_now(CAP_MS).await;
    hub_exporter::flush_now(CAP_MS).await;
}

/// Push a bounded `event: reload` frame to every session THIS PROCESS is
/// currently serving over SSE, so a connected browser skips its own
/// reconnect-wait and refetches immediately instead of waiting out the
/// retry backoff ladder. Dev-mode only — see H23 ("dev-only reload channel
/// ABSENT, not disabled, in production"): the production gate lives at the
/// ONE call site chain ([`maybe_push_reload_to_live_sessions`], called from
/// `live_shutdown_signal`), never inside this helper — a caller that
/// reaches this function has already decided dev-mode applies. Delivery is
/// best-effort, at-most-once, never retried: a full/closed channel just
/// drops that one session's frame (the browser's own reconnect logic
/// already covers the restart-detection floor; this only shaves latency),
/// and a session that disconnects between the enumerate and the push
/// misses a frame it can't act on anyway.
async fn push_reload_to_live_sessions<Model, Msg>(store: &Arc<dyn store::SessionStore<Model, Msg>>)
where
    Model: Send + 'static,
    Msg: Send + 'static,
{
    for handle in store.live_sessions().await {
        let tx = handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sse_tx
            .clone();
        if let Some(tx) = tx {
            let _ = tx.send(SsePatch(sse::frame("reload", "{}"))).await;
        }
    }
}

/// The H23 production gate over [`push_reload_to_live_sessions`]: in
/// production (`ENV`/`IPE_ENV` set to a non-dev marker) the push path is
/// UNREACHABLE — same one-`if` shape every other production gate in this
/// module uses (dev-console mount, metrics auth). Split from
/// `live_shutdown_signal` so the gate itself is unit-testable without
/// delivering a real signal.
async fn maybe_push_reload_to_live_sessions<Model, Msg>(
    store: &Arc<dyn store::SessionStore<Model, Msg>>,
) where
    Model: Send + 'static,
    Msg: Send + 'static,
{
    if !crate::telemetry::production_from_env() {
        push_reload_to_live_sessions(store).await;
    }
}

/// Await the FIRST shutdown signal (SIGINT or SIGTERM), then run the graceful
/// teardown and return so axum's `with_graceful_shutdown` drains in-flight
/// connections and the serve future resolves `Ok(())` (→ the IpeTask is `Ok` →
/// the generated entry exits 0). Go parity: `live.go:3503` (the SIGINT/SIGTERM
/// handler that prints the line, flips readyz, drains, then returns naturally).
///
/// Two escapes guard against the drain hanging — both keep the no-panic thesis:
///  - A bounded grace timer that force-exits 0 (CLEAN) after `shutdown_grace()`,
///    so a never-idle SSE stream can't wedge the process (Go's `srv.Close()`
///    equivalent — it drops long-lived connections rather than waiting).
///  - A SECOND signal (Ctrl-C twice) that force-exits 130 immediately (Go's
///    nested `os.Exit(130)` escalation).
///
/// Robustness: a failed SIGTERM registration must NOT crash — it degrades to
/// SIGINT-only (`ctrl_c`). On non-unix only `ctrl_c` is available.
async fn live_shutdown_signal<Model, Msg>(store: Arc<dyn store::SessionStore<Model, Msg>>)
where
    Model: Send + 'static,
    Msg: Send + 'static,
{
    // First press: block until SIGINT or SIGTERM arrives.
    wait_for_term_or_int().await;

    // Print to stdout (Go uses `fmt.Println`, which is stdout). The leading
    // newline keeps the `^C` echo on its own line, matching Go.
    println!("\nIpe.Web shutting down…");

    // Flip readyz → draining so orchestrators stop routing new traffic while
    // in-flight requests finish (Go: `SetReady(false)`).
    observability::mark_draining();

    // Dev-only proactive `event: reload` push to every locally-live SSE
    // session, fired once the shutdown is committed and BEFORE the bounded
    // grace-timer drain begins — a connected browser refetches immediately
    // instead of waiting out its reconnect backoff. Production-gated (H23).
    maybe_push_reload_to_live_sessions(&store).await;

    // Tear down the console child (Go: `ShutdownSubApps`; here the pre-built
    // console child, if one was spawned). Idempotent no-op when none exists.
    // Load-bearing: the child is tracked in a `static` whose `Drop`
    // (`kill_on_drop`) never runs on `process::exit`, so this explicit
    // `start_kill` is what prevents an orphan console child after a clean exit.
    console_proxy::shutdown_console();

    // Telemetry export pipelines (push/hub exporters) flush every ~2 s on a
    // tick. The channel-close drain ONLY runs when the mpsc Sender is dropped,
    // which requires Drop — and `process::exit` skips Drop entirely. Without an
    // explicit pre-exit flush the grace-timer and watchdog paths below would
    // silently lose ≤1 batch-interval (~2 s default) of buffered telemetry.
    // `flush_exporters` sends a Flush sentinel to each active exporter and waits
    // a bounded 500 ms; it is best-effort (telemetry only, never user data) and
    // never hangs shutdown. Go's `ShutdownTracing`/`RunShutdownHooks` are the
    // equivalent path.

    // Grace timer: force a CLEAN exit-0 after the window so a never-idle SSE
    // connection can't hang the drain (Go's `srv.Close()` drops them outright).
    // Spawned (not awaited) so we still return immediately and let the axum drain
    // win the race when there are no long-lived connections (the common case →
    // sub-window exit). Exit 0 keeps the IpeTask-Ok / exit-0 contract.
    tokio::spawn(async {
        tokio::time::sleep(shutdown_grace()).await;
        // Defense-in-depth: kill the console child again in case it was spawned
        // after the first teardown call (shutdown_console is idempotent).
        console_proxy::shutdown_console();
        flush_exporters().await;
        std::process::exit(0);
    });

    // Second press: a watchdog that force-exits 130 if the user hits Ctrl-C
    // again while the drain is in progress (Go parity: the nested
    // `<-sigCh; os.Exit(130)`). Spawned (not awaited).
    tokio::spawn(async {
        wait_for_term_or_int().await;
        eprintln!("Ipe.Web: forcing exit (second signal)");
        console_proxy::shutdown_console();
        flush_exporters().await;
        std::process::exit(130); // 128 + SIGINT(2)
    });
    // Return → axum drains in-flight connections → serve future resolves Ok
    // (fast path when nothing long-lived is open; otherwise the grace timer
    // force-exits 0). The graceful return path also flushes exporters to cover
    // the no-open-connections fast exit where process tear-down follows quickly.
    flush_exporters().await;
}

/// Resolve when the next SIGINT or SIGTERM arrives. Total + robust: if SIGTERM
/// can't be registered (rare), fall back to SIGINT (`ctrl_c`) only rather than
/// panicking. On non-unix, only `ctrl_c` exists.
async fn wait_for_term_or_int() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            // SIGTERM registration failed — degrade to SIGINT only.
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// `Ipe.Web.app { init, update, view, subscriptions }` — serve via axum.
///
/// HTTP-first: a GET renders the full page with the embedded client, opens a
/// per-session TEA loop, and serves an SSE patch channel + a POST event
/// endpoint. The driver diffs view-over-view and pushes patches over SSE.
///
/// `init` receives a typed `req::WebReq` (path/query/method/params/headers/
/// cookies) built from the incoming request; the driver calls `init(req)` so a
/// req-reader can bootstrap session state on first render. A non-req init is
/// monomorphised to ignore the threaded `WebReq`.
#[allow(clippy::too_many_arguments)]
pub fn web_app<E, Model, Msg, FInit, FUpdate, FView, FSubs>(
    init: FInit,
    update: FUpdate,
    view: FView,
    subscriptions: FSubs,
    store_kind: String,
    store_path: String,
    schema_tag: [u8; 32],
) -> IpeTask<E, ()>
where
    E: From<String> + Send + 'static,
    Model:
        serde::Serialize + serde::de::DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
    // Debug: forwarded through serve_live → drive_session for the
    // ipe_live_msg_seconds{name} label. Generated Msg enums always derive Debug.
    Msg: Clone + Send + Sync + std::fmt::Debug + 'static,
    FInit: Fn(req::WebReq) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
    FView: Fn(Model) -> Html<Msg> + Send + Sync + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + Send + Sync + 'static,
{
    Box::pin(async move {
        let store =
            store::choose_store::<Model, Msg>(&store_kind, &store_path, live_ttl(), schema_tag)
                .await;
        let state = WebState {
            store,
            init: Arc::new(init),
            update: Arc::new(update),
            view: Arc::new(view),
            subs: Arc::new(subscriptions),
            // No routing: GET serves the freshly-init'd model unchanged; no params.
            route_resolver: Arc::new(|m, _path| m),
            param_resolver: Arc::new(|_path| crate::dict::dict_empty()),
            // Go `matchAnyRoute` parity: with no route table only `/` is a
            // page URL.
            route_matched: Arc::new(|path| path == "/"),
            session_count: Arc::new(AtomicUsize::new(0)),
        };
        serve_live(state).await
    })
}

/// `Ipe.Web.app { …, routes, notFound }` with URL routing — serve via axum.
///
/// Identical to `web_app` except a `route_resolver` is built from the route
/// table + page-setter: on each GET it matches the path to a `Page` value
/// (param strings applied via the route closures) and writes it into the
/// freshly-`init`'d model's `page` field via `set_page`. `Page`/`FSetPage`
/// are erased into the boxed resolver, so `serve_live`/`WebState` keep the
/// original 6 type params.
#[allow(clippy::too_many_arguments)]
pub fn web_app_routed<E, Model, Msg, Page, FInit, FUpdate, FView, FSubs, FSetPage>(
    init: FInit,
    update: FUpdate,
    view: FView,
    subscriptions: FSubs,
    routes: Vec<route::Route<Page>>,
    not_found: Page,
    set_page: FSetPage,
    store_kind: String,
    store_path: String,
    schema_tag: [u8; 32],
) -> IpeTask<E, ()>
where
    E: From<String> + Send + 'static,
    Model:
        serde::Serialize + serde::de::DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
    // Debug: forwarded through serve_live → drive_session for the
    // ipe_live_msg_seconds{name} label. Generated Msg enums always derive Debug.
    Msg: Clone + Send + Sync + std::fmt::Debug + 'static,
    Page: Clone + Send + Sync + 'static,
    FInit: Fn(req::WebReq) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
    FView: Fn(Model) -> Html<Msg> + Send + Sync + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + Send + Sync + 'static,
    FSetPage: Fn(Page, Model) -> Model + Send + Sync + 'static,
{
    Box::pin(async move {
        let routes = Arc::new(routes);
        let not_found = Arc::new(not_found);
        let set_page = Arc::new(set_page);
        let routes_for_params = routes.clone();
        let routes_for_match = routes.clone();
        let resolver: RouteResolver<Model> =
            Arc::new(move |m, path| (set_page)(route::match_routes(&routes, &not_found, path), m));
        let param_resolver: ParamResolver =
            Arc::new(move |path| route::match_params(&routes_for_params, path));
        let route_matched: RouteMatched =
            Arc::new(move |path| route::matches_any(&routes_for_match, path));
        let store =
            store::choose_store::<Model, Msg>(&store_kind, &store_path, live_ttl(), schema_tag)
                .await;
        let state = WebState {
            store,
            init: Arc::new(init),
            update: Arc::new(update),
            view: Arc::new(view),
            subs: Arc::new(subscriptions),
            route_resolver: resolver,
            param_resolver,
            route_matched,
            session_count: Arc::new(AtomicUsize::new(0)),
        };
        serve_live(state).await
    })
}

/// Go parity (live.go `isBrowserNoisePath`): a path a browser or crawler
/// requests automatically (favicon, service-worker probe, source-map fetch,
/// `.well-known` discovery, static asset by extension). When unrouted, these
/// must never touch session state: they'd otherwise race the real `GET /`
/// for session creation (double `init`) or — worse — re-route a LIVE
/// session's model and rebuild its handler index from the `notFound` view,
/// orphaning every handler on the page the browser is actually showing (all
/// subsequent events, form submits included, would silently resolve to
/// nothing).
fn is_browser_noise_path(p: &str) -> bool {
    if matches!(
        p,
        "/favicon.ico"
            | "/robots.txt"
            | "/sitemap.xml"
            | "/apple-touch-icon.png"
            | "/apple-touch-icon-precomposed.png"
            | "/service-worker.js"
            | "/sw.js"
            | "/manifest.json"
    ) || p.starts_with("/.well-known/")
    {
        return true;
    }
    // Requests for assets by well-known extension are browser noise — real
    // page routes never end in these suffixes.
    [
        ".ico", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".css", ".js", ".map", ".woff",
        ".woff2", ".ttf",
    ]
    .iter()
    .any(|ext| p.ends_with(ext))
}

/// Go parity (live.go `handleInitial`): serve an unrouted browser-noise file
/// from the static dir's ROOT when it exists there. Browsers always probe
/// `/favicon.ico` (and friends) at the origin root, never under `/static/`,
/// so without this shortcut an author with a configured static dir has no
/// way to suppress the 404. `None` → the caller 404s.
///
/// Security: the path is attacker-shaped. Any non-plain segment (empty, `.`,
/// `..`) is rejected BEFORE the join — stricter than Go's `filepath.Clean`,
/// no traversal can escape the dir. A directory (or unreadable file) reads
/// as `Err` → `None` → 404.
async fn serve_noise_from_static_root(path: &str) -> Option<axum::response::Response> {
    use axum::response::IntoResponse;
    let dir = crate::system::read_env_var("IPE_LIVE_STATIC_DIR")
        .ok()
        .filter(|d| !d.is_empty())?;
    let rel = path.trim_start_matches('/');
    if rel.is_empty()
        || rel
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return None;
    }
    let candidate = std::path::Path::new(&dir).join(rel);
    let bytes = tokio::fs::read(&candidate).await.ok()?;
    let mime = match rel.rsplit('.').next().unwrap_or("") {
        "ico" => "image/x-icon",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "map" | "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    };
    Some(
        (
            axum::http::StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, mime)],
            bytes,
        )
            .into_response(),
    )
}

/// Shared server setup for `web_app` / `web_app_routed`: nested HTTP
/// handlers (`page` / `sse_handler` / `event_handler`), router + bind/serve.
/// The only per-entry difference (the `route_resolver`) lives on `state`.
async fn serve_live<E, Model, Msg, FInit, FUpdate, FView, FSubs>(
    state: WebState<Model, Msg, FInit, FUpdate, FView, FSubs>,
) -> IpeResult<E, ()>
where
    E: From<String> + Send + 'static,
    Model: Clone + PartialEq + Send + 'static,
    // Debug: forwarded to drive_session for the ipe_live_msg_seconds{name} label.
    Msg: Clone + Send + std::fmt::Debug + 'static,
    FInit: Fn(req::WebReq) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
    FView: Fn(Model) -> Html<Msg> + Send + Sync + 'static,
    FSubs: Fn(Model) -> IpeSub<Msg> + Send + Sync + 'static,
{
    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};

    {
        // ── GET page (root + any path) ────────────────────────────────────
        async fn page<Model, Msg, FInit, FUpdate, FView, FSubs>(
            State(st): State<WebState<Model, Msg, FInit, FUpdate, FView, FSubs>>,
            method: axum::http::Method,
            uri: axum::http::Uri,
            headers: axum::http::HeaderMap,
        ) -> Response
        where
            Model: Clone + PartialEq + Send + 'static,
            // Debug: the GET handler creates a session and spawns drive_session,
            // which needs the bound for the ipe_live_msg_seconds{name} label.
            Msg: Clone + Send + std::fmt::Debug + 'static,
            FInit: Fn(req::WebReq) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
            FUpdate: Fn(Msg, Model) -> (Model, IpeCmd<Msg>) + Send + Sync + 'static,
            FView: Fn(Model) -> Html<Msg> + Send + Sync + 'static,
            FSubs: Fn(Model) -> IpeSub<Msg> + Send + Sync + 'static,
        {
            // Cookie-based session lifecycle (Go store.Get on every GET):
            //   * Web hit  → reuse the in-process session; re-apply routing for
            //                 this GET's path + re-render (no new driver).
            //   * Cold hit  → a persisted model (post-restart / different replica);
            //                 hydrate a fresh driver seeded with it (no init).
            //   * miss      → init a new session.
            let cookie_sid = sid_from_cookie(&headers);
            // CSRF double-submit token: reuse the browser's existing well-formed
            // `__ipe_csrf` cookie (so a reload keeps the same token), else mint a
            // fresh one. page_response sets the cookie + injects the value into
            // the page JS; the client echoes it back in the X-Ipê-Csrf header.
            let csrf_tok = csrf::cookie_value(&headers, csrf::csrf_cookie_name())
                .filter(|t| csrf::token_is_well_formed(t))
                .unwrap_or_else(csrf::gen_token);

            // Go parity (handleInitial): unrouted browser-noise paths 404 (or
            // serve from the static root) BEFORE any session work — they must
            // never run `init` (double-init race against the real `GET /`) and
            // never touch an existing session (see the routed guards below).
            let routed = (st.route_matched)(uri.path());
            if !routed && is_browser_noise_path(uri.path()) {
                if let Some(resp) = serve_noise_from_static_root(uri.path()).await {
                    return resp;
                }
                return (StatusCode::NOT_FOUND, "404 page not found").into_response();
            }

            let hit = match cookie_sid.as_ref() {
                Some(s) => st.store.get(s).await.map(|h| (s.clone(), h)),
                None => None,
            };

            // Go parity (handleInitial): an unrouted GET against an EXISTING
            // session (live or persisted) 404s WITHOUT touching it. Re-routing
            // here would write the `notFound` page into the model and rebuild
            // the handler index from that view, orphaning every handler on the
            // page the browser is still showing — the next event POST (form
            // submit, click, input) would silently resolve to nothing.
            if !routed && hit.is_some() {
                return (StatusCode::NOT_FOUND, "404 page not found").into_response();
            }

            let (sid, model, cmd0) = match hit {
                Some((sid, store::StoreHit::Live(handle))) => {
                    // sid is carried from the cookie lookup; the "hit but no sid"
                    // state is unrepresentable.
                    let body = {
                        let mut e = handle.lock().unwrap_or_else(|e| e.into_inner());
                        e.model = (st.route_resolver)(e.model.clone(), uri.path());
                        let mut tree = (st.view)(e.model.clone());
                        assign_ipe_ids(&mut tree, "r");
                        style_inject::apply_style_injections(&mut tree);
                        e.index = build_index(&tree);
                        e.last_view = tree.clone();
                        render_html(&tree)
                    };
                    st.store.set(&sid, handle).await; // touch last-seen
                    return page_response(&sid, &body, &csrf_tok, &headers);
                }
                Some((sid, store::StoreHit::Cold(m))) => {
                    // A returning user with a valid sid cookie → not new attack
                    // volume, so NOT rejected; but count its driver so the slot it
                    // gets below is paired (decremented on the driver's exit).
                    st.session_count.fetch_add(1, Ordering::SeqCst);
                    (sid, (st.route_resolver)(m, uri.path()), IpeCmd::None)
                }
                None => {
                    // Admission control (cookieless = brand-new session = the
                    // attack surface). Reserve a slot atomically: fetch_add-then-test
                    // avoids the load-then-add TOCTOU where N concurrent GETs all
                    // pass at cap-1. ALWAYS reserve (so the slot built below is
                    // paired 1:1 with a decrement); only the rejection is gated on
                    // cap>0 (0 = unlimited opt-out). Over cap → roll back + 503.
                    let cap = max_sessions();
                    let reserved = st.session_count.fetch_add(1, Ordering::SeqCst);
                    if cap > 0 && reserved >= cap {
                        st.session_count.fetch_sub(1, Ordering::SeqCst);
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            [(axum::http::header::RETRY_AFTER, "2")],
                            "server at session capacity",
                        )
                            .into_response();
                    }
                    // Build the request context (params from routing — empty when
                    // unrouted) and init a fresh model. The param_resolver is
                    // model-independent, breaking the init↔routing cycle.
                    let params = (st.param_resolver)(uri.path());
                    let req = req::live_req(&method, &uri, &headers, params);
                    let (m, c) = (st.init)(req);
                    // Session fixation guard: a store MISS means this sid is NOT a
                    // known session, so NEVER adopt the client-supplied cookie value
                    // — always mint a fresh sid. (A HIT path keeps cookie_sid.)
                    let s = new_sid();
                    (s, (st.route_resolver)(m, uri.path()), c)
                }
            };

            let mut tree = (st.view)(model.clone());
            assign_ipe_ids(&mut tree, "r");
            style_inject::apply_style_injections(&mut tree);
            let index = build_index(&tree);
            let body = render_html(&tree);

            // Bounded per-session Msg queue: cap at 1024 to prevent a fast
            // client from growing the queue without bound (per-session memory
            // DoS). On overflow events are dropped with a warn (see
            // event_handler). Go serialises dispatch under sess.mu instead of
            // a channel — no Go bound to match; 1024 is far above any
            // legitimate burst of user-driven events.
            let (msg_tx, msg_rx) = mpsc::channel::<Msg>(1024);
            let entry = Arc::new(Mutex::new(SessionEntry {
                model,
                last_view: tree,
                index,
                seq: 0,
                sse_tx: None,
                msg_tx: msg_tx.clone(),
            }));
            st.store.set(&sid, entry.clone()).await;

            // The admission slot for this driver (reserved by the Cold/None arm
            // above); its Drop decrements session_count when the driver exits.
            let slot = SessionSlot {
                count: st.session_count.clone(),
            };
            // Spawn the per-session driver with a WEAK entry ref (the store +
            // any SSE connection are the strong holders) so the driver is mortal:
            // it exits once the session is evicted and unconnected, releasing the
            // slot. The local strong `entry` drops at this handler's return,
            // leaving the store (+ future SSE) as the only strong holders.
            tokio::spawn(drive_session(
                Arc::downgrade(&entry),
                msg_rx,
                msg_tx.clone(),
                st.update.clone(),
                st.view.clone(),
                st.subs.clone(),
                st.store.clone(),
                sid.clone(),
                slot,
            ));
            // Fire init's Cmd into the loop (None for a cold-restored session).
            run_cmd(cmd0, &msg_tx, &sid);

            page_response(&sid, &body, &csrf_tok, &headers)
        }

        // ── GET /_ipe/sse ─────────────────────────────────────────────────
        async fn sse_handler<Model, Msg, FInit, FUpdate, FView, FSubs>(
            State(st): State<WebState<Model, Msg, FInit, FUpdate, FView, FSubs>>,
            headers: axum::http::HeaderMap,
        ) -> Response
        where
            Model: Clone + Send + 'static,
            Msg: Clone + Send + 'static,
            FInit: Send + Sync + 'static,
            FUpdate: Send + Sync + 'static,
            FView: Send + Sync + 'static,
            FSubs: Send + Sync + 'static,
        {
            let sid = sid_from_cookie(&headers);
            let entry = match &sid {
                Some(s) => match st.store.get(s).await {
                    Some(store::StoreHit::Live(h)) => Some(h),
                    _ => None,
                },
                None => None,
            };
            let entry = match entry {
                Some(e) => e,
                // X-Ipê-Live: 1 lets the client distinguish a genuine session-lost
                // 404 (reload to recover) from a wedged proxy (client.js probes for
                // exactly this header — l1481/l1530).
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        [(axum::http::HeaderName::from_static("x-ipe-live"), "1")],
                        SESSION_LOST_BODY,
                    )
                        .into_response();
                }
            };

            let (tx, rx) = sse::channel();
            {
                entry.lock().unwrap_or_else(|e| e.into_inner()).sse_tx = Some(tx.clone());
            }

            // Metrics (Go parity: ipe_live_sse_connections_total /
            // ipe_live_sessions_active). Count the connection and mark the session
            // active; the gauge is decremented when the response body stream is
            // dropped on disconnect (the SessionGauge guard below).
            crate::telemetry::metric_inc("ipe_live_sse_connections_total", &[], 1);
            crate::telemetry::metric_add_gauge("ipe_live_sessions_active", &[], 1);

            // Immediate hello + ~2KB proxy-buffer padding comment, then a 15s
            // heartbeat keepalive (Go parity: live.go SSE handshake).
            let _ = tx
                .send(SsePatch(format!(": {}\n\n", " ".repeat(2048))))
                .await;
            // Go-parity hello payload (live.go ~5486): `{"v":1,"sid":...,"ts":<ms>}`.
            // Reaching here means `entry` exists ⇒ the cookie sid was a live session,
            // so `sid` is Some; the impossible None degrades to an empty sid (the
            // client already holds its sid via window.__IPE_SID — the body is
            // confirmatory). The sid is hex (new_sid) ⇒ JSON-safe without escaping.
            let hello_sid = sid.as_deref().unwrap_or("");
            let hello_ts = chrono::Utc::now().timestamp_millis();
            let _ = tx
                .send(SsePatch(sse::frame(
                    "hello",
                    &format!("{{\"v\":1,\"sid\":\"{hello_sid}\",\"ts\":{hello_ts}}}"),
                )))
                .await;

            // Reconnect-resync (Go parity: handleSSE full-body frame, live.go:5498).
            // A session restored from the store on a cold hit — or any process
            // restart / `ipe watch` rebuild / redeploy paired with a persistent
            // store — has no live subscriptions from the previous process, so
            // nothing pushes until the next user Msg. Render the current view once
            // and ship it as a full-body `event: patch` frame; the client consumes
            // `{seq, body}` → __ipePatch full replace (client.js:1318). No globalSeq
            // field → the client's broadcast-dedup guard (globalSeq>0) can never
            // drop this authoritative, idempotent frame. Bump seq under the same
            // lock the event path uses so it stays monotonic vs later patches; drop
            // the guard before the await (never hold a std Mutex across .await).
            let resync = {
                let mut g = entry.lock().unwrap_or_else(|e| e.into_inner());
                g.seq += 1;
                let html = render_html(&g.last_view);
                serde_json::json!({ "seq": g.seq, "body": html }).to_string()
            };
            let _ = tx.send(SsePatch(sse::frame("patch", &resync))).await;
            {
                let tx = tx.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                        if tx
                            .send(SsePatch(sse::frame("heartbeat", "{}")))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            }

            // Drop guard tied to the stream lifetime: when the client disconnects
            // (axum drops the response body) or the channel closes, the unfold
            // state — and this guard — drops, decrementing the active-sessions
            // gauge exactly once.
            struct SessionGauge;
            impl Drop for SessionGauge {
                fn drop(&mut self) {
                    crate::telemetry::metric_add_gauge("ipe_live_sessions_active", &[], -1);
                }
            }
            // Pin the STRONG entry Arc into the stream state for the connection's
            // whole life. This is load-bearing: the driver now holds only a Weak
            // ref, so without this an idle-but-SSE-connected (watch-only) session
            // — one receiving Cmd.publish / Sub.every broadcasts but sending no
            // user Msgs, hence never written-through to refresh store last-seen —
            // would be TTL-evicted, its last strong ref dropped, and its driver
            // would exit mid-stream. Holding the strong Arc here keeps it (and its
            // driver) alive exactly as long as the client stays connected; on
            // disconnect axum drops the body → this Arc releases.
            let body_stream = futures_util::stream::unfold(
                (rx, SessionGauge, entry),
                |(mut rx, guard, entry)| async move {
                    rx.recv().await.map(|SsePatch(s)| {
                        (
                            Ok::<_, std::io::Error>(axum::body::Bytes::from(s)),
                            (rx, guard, entry),
                        )
                    })
                },
            );
            match Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
                .header(axum::http::header::CACHE_CONTROL, "no-cache")
                .header("x-accel-buffering", "no")
                .body(axum::body::Body::from_stream(body_stream))
            {
                Ok(r) => r.into_response(),
                // Headers/status are all literals, so this never fails; total
                // fallback per the no-runtime-errors rule.
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }

        // ── POST /_ipe/event ──────────────────────────────────────────────
        async fn event_handler<Model, Msg, FInit, FUpdate, FView, FSubs>(
            State(st): State<WebState<Model, Msg, FInit, FUpdate, FView, FSubs>>,
            headers: axum::http::HeaderMap,
            body: axum::body::Bytes,
        ) -> Response
        where
            Model: Clone + Send + 'static,
            Msg: Clone + Send + 'static,
            FInit: Send + Sync + 'static,
            FUpdate: Send + Sync + 'static,
            FView: Send + Sync + 'static,
            FSubs: Send + Sync + 'static,
        {
            let parsed: EventBody = match serde_json::from_slice(&body) {
                Ok(b) => b,
                Err(_) => return (StatusCode::BAD_REQUEST, "bad body").into_response(),
            };
            // Authenticate the target session by the COOKIE sid ONLY — never the
            // body-supplied `sessionId`. Trusting a body id lets a caller act on
            // ANY session by naming it (an auth-bypass that, paired with a
            // guessable sid, was a hijack path). A legitimate browser always has
            // the HttpOnly session cookie by the time an event fires (the page
            // GET set it). No cookie → no session.
            let _ = &parsed.session_id; // body field retained for wire-compat; not trusted for auth
            let sid = match sid_from_cookie(&headers) {
                Some(s) => s,
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        [(axum::http::HeaderName::from_static("x-ipe-live"), "1")],
                        SESSION_LOST_BODY,
                    )
                        .into_response();
                }
            };
            let entry = match st.store.get(&sid).await {
                Some(store::StoreHit::Live(h)) => Some(h),
                _ => None,
            };
            let entry = match entry {
                Some(e) => e,
                // X-Ipê-Live: 1 lets the client distinguish a genuine session-lost
                // 404 (reload to recover) from a wedged proxy (client.js probes for
                // exactly this header — l1481/l1530).
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        [(axum::http::HeaderName::from_static("x-ipe-live"), "1")],
                        SESSION_LOST_BODY,
                    )
                        .into_response();
                }
            };

            let hid = if !parsed.handler_id.is_empty() {
                parsed.handler_id
            } else {
                parsed.id
            };
            // Event name: explicit `event` override, else the `msg` marker
            // (render_html sets it to the event name), else default to click.
            let event = if !parsed.event.is_empty() {
                parsed.event
            } else if !parsed.msg.is_empty() {
                parsed.msg
            } else {
                "click".to_string()
            };

            let (msg, seq) = {
                let e = entry.lock().unwrap_or_else(|e| e.into_inner());
                if event == "submit" {
                    // args[0] is the form-data object {name: value, …}.
                    let fd: FormData = parsed
                        .args
                        .first()
                        .and_then(|v| v.as_object())
                        .map(|o| {
                            o.iter()
                                .map(|(k, v)| (k.clone(), value_to_string(v)))
                                .collect()
                        })
                        .unwrap_or_default();
                    (e.index.resolve_form(&hid, &event, fd), e.seq)
                } else {
                    let args: Vec<String> = parsed.args.iter().map(value_to_string).collect();
                    (e.index.resolve(&hid, &event, &args), e.seq)
                }
            };
            if let Some(m) = msg {
                let tx = {
                    entry
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .msg_tx
                        .clone()
                };
                // try_send is non-blocking; on a full queue drop the event and
                // return 429 so the client can back off (Go parity: Go
                // serialises under sess.mu and drops the handler if the
                // session is gone; no client-side queue bound to match — we
                // choose 429 over silent drop so the browser retry loop fires).
                if let Err(e) = tx.try_send(m) {
                    eprintln!(
                        "[ipe.live] event_handler: session msg queue full or closed; dropping event ({})",
                        e
                    );
                    return (StatusCode::TOO_MANY_REQUESTS, "event queue full").into_response();
                }
            }
            // Real patches flow over SSE from the driver; ack with an empty list.
            // X-Ipê-Live: 1 marks this as a genuine Ipe.Web response (the client
            // treats a 200 WITHOUT it as a wedged-proxy signal).
            (
                StatusCode::OK,
                [
                    (axum::http::header::CONTENT_TYPE, "application/json"),
                    (axum::http::HeaderName::from_static("x-ipe-live"), "1"),
                ],
                format!("{{\"seq\":{seq},\"patches\":[]}}"),
            )
                .into_response()
        }

        // Background TTL eviction (Go memoryStore.cleanupLoop parity): sweep
        // idle-expired sessions every 60 s. Persistent backends also prune their
        // checkpoint table in `sweep`.
        {
            let store = state.store.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    tick.tick().await;
                    store.sweep().await;
                }
            });
        }

        // Enable the telemetry SQLite spill when
        // IPE_CONSOLE_DB_PATH is set — the console child reads it via the
        // hub kernels. db-gated; a no-op for live-without-db apps. Enabled
        // BEFORE the console child spawns so early telemetry lands in the spill
        // the child will read.
        #[cfg(feature = "db")]
        crate::telemetry_spill::enable_from_env().await;

        // Observability export pipelines: federation push to a parent ingest
        // (IPE_PARENT_URL) and remote-hub OTLP push (IPE_CONSOLE_HUB).
        // Both env-gated + inert by default.
        push_exporter::enable_from_env().await;
        hub_exporter::enable_from_env().await;

        // Console precedence: try the pre-built console child +
        // reverse-proxy; fall back to the in-process console when the binary is
        // absent / spawn fails / readiness times out / the gate is closed.
        // Decided HERE (before the router is built) so both the proxy routes and
        // the in-process console routes sit under the same `track` middleware,
        // and the two never collide on `/_ipe/console`.
        let use_console_proxy = console_proxy::ensure_console_proxy().await;

        // Body-size cap on /_ipe/event: mirrors Go's http.MaxBytesReader
        // (runtime-go/rt/live.go:3915). axum's DefaultBodyLimit applies
        // before the handler sees the bytes, so an over-sized payload is
        // rejected at the extract layer with 413 Payload Too Large.
        let event_route = post(event_handler::<Model, Msg, FInit, FUpdate, FView, FSubs>)
            .layer(axum::extract::DefaultBodyLimit::max(live_max_body_bytes()));

        // Content-addressed client JS asset route. The URL is computed once at
        // startup from SHA-256(CLIENT_JS) so the path changes when the file
        // changes, making `Cache-Control: immutable` safe. This route is CSRF-
        // exempt (GET; the CSRF middleware only checks mutating verbs) and open
        // to all (it's a static public asset). It is registered BEFORE the
        // catch-all `/*path` route so it is matched first.
        let client_js_route_path = client_js_path(); // e.g. "/_ipe/client.a1b2c3d4e5f6a7b8.js"
        async fn serve_client_js() -> impl axum::response::IntoResponse {
            use axum::http::header;
            (
                [
                    (
                        header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    ),
                    (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
                ],
                CLIENT_JS,
            )
        }

        // Cloned for the shutdown path's dev-only reload push — the router's
        // `.with_state(state)` takes ownership of `state` below.
        let shutdown_store = state.store.clone();

        let mut router = Router::new()
            .route(
                "/_ipe/sse",
                get(sse_handler::<Model, Msg, FInit, FUpdate, FView, FSubs>),
            )
            .route("/_ipe/event", event_route)
            .route(&client_js_route_path, get(serve_client_js))
            // Observability surface (Go parity — observability.go).
            .route("/_ipe/healthz", get(observability::healthz))
            .route("/_ipe/readyz", get(observability::readyz))
            .route("/_ipe/buildinfo", get(observability::buildinfo))
            .route("/_ipe/metrics", get(observability::metrics))
            // Observability federation receiver stays on the parent regardless
            // of console mode (sub-apps push telemetry here). Body-capped (reuses
            // the /_ipe/event limit) so an unbounded ingest POST can't exhaust
            // memory before the JSON parse.
            .route(
                "/_ipe/observability/ingest",
                post(console::ingest)
                    .layer(axum::extract::DefaultBodyLimit::max(live_max_body_bytes())),
            );

        router = if use_console_proxy {
            // Real bundled Ipe.Web console, spawned as a child + proxied. The
            // child process logs its OWN `session store: …` line + a
            // `reverse-proxy ready` line (console_proxy), so the parent does not
            // duplicate the inline-mount log here.
            console_proxy::proxy_routes(router)
        } else {
            // In-process console (plain-HTML shell + JSON APIs). Go mounts the
            // console as an in-process Ipe.Web sub-app that inits its OWN session
            // store, so Go logs the memory-store line TWICE (root + console) and
            // then the inline-mount line (console.go:328). The Rust in-process
            // console has no separate store, so we emit the matching SECOND store
            // line + the mount line here — but ONLY when the console actually
            // mounts (gate open: not a sub-app, not `IPE_CONSOLE_AUTH=off`, not
            // production-without-admin-token), mirroring Go's mount skip.
            if console_proxy::gate_allows() {
                eprintln!("{}", store::memory_store_log_line(live_ttl()));
                eprintln!(
                    "[ipe.console] inline console mounted as Ipe.Web sub-app at /_ipe/console mode={}",
                    console::console_auth_mode_label()
                );
            }
            router
                .route("/_ipe/console", get(console::console_html))
                .route("/_ipe/console/api/overview", get(console::api_overview))
                .route("/_ipe/console/api/logs", get(console::api_logs))
                .route("/_ipe/console/api/errors", get(console::api_errors))
                .route("/_ipe/console/api/traces", get(console::api_traces))
                .route(
                    "/_ipe/console/api/metrics-summary",
                    get(console::api_metrics_summary),
                )
        };

        // ipe.toml `[live] static` (baked as IPE_LIVE_STATIC_DIR) → serve files at
        // /static/* via ServeDir (Go parity: live.go staticURL "/static"). MUST be
        // added before the `/*path` page catch-all so a /static/<file> request hits
        // ServeDir, not the page handler (which would return HTML). ServeDir blocks
        // `..` path traversal by construction (percent-decodes first, so `%2e%2e` is
        // caught too). NOTE: like Go's http.FileServer it FOLLOWS symlinks inside the
        // dir — the dir is author-controlled (ipe.toml [live] static), so that is the
        // intended contract + Go-parity, NOT a confinement guarantee. Absent/empty →
        // no static mount.
        if let Some(dir) = crate::system::read_env_var("IPE_LIVE_STATIC_DIR")
            .ok()
            .filter(|d| !d.is_empty())
        {
            router = router.nest_service("/static", tower_http::services::ServeDir::new(dir));
        }

        let app: Router = router
            .route("/", get(page::<Model, Msg, FInit, FUpdate, FView, FSubs>))
            .route(
                "/*path",
                get(page::<Model, Msg, FInit, FUpdate, FView, FSubs>),
            )
            // Layer order (axum: last `.layer` = outermost): CSRF is INNER of
            // observability::track so a rejected CSRF POST still gets counted +
            // access-logged (Go parity — CSRF sits inside the observability mw).
            .layer(axum::middleware::from_fn(csrf::csrf_middleware))
            // Per-request panic recovery (Go parity — its handlers run under a
            // defer/recover that returns 500 instead of crashing the worker;
            // rt.go:3463 etc.). Symmetric with Ipe.Http.Server (server.rs). The
            // Rust thesis is that well-typed Ipê can't panic, so this is the
            // defense-in-depth FLOOR, not the foundation: a handler / csrf-mw
            // panic becomes a 500 instead of an unwound tokio task that drops the
            // connection with no response. Placed INNER of `track` (and OUTER of
            // csrf + the route handlers) so the converted 500 returns through
            // track's `next.run().await` normally — track still counts +
            // access-logs + histograms it as status 500, matching Go (whose
            // recover is innermost; the outer middleware observes the 500). If it
            // were outermost the panic would unwind through track, skipping its
            // post-`next.run` metering. The custom responder classifies + logs the
            // panic SERVER-SIDE (errId, via core::panic_500_body) and returns a 500
            // carrying ONLY the errId — never the panic message (no info leak).
            // Symmetric with Ipe.Http.Server (the body shape is shared in `core`;
            // the Web router can't reference `server.rs` — a Web-only generated
            // project doesn't include it).
            .layer(tower_http::catch_panic::CatchPanicLayer::custom(
                |err: Box<dyn std::any::Any + Send + 'static>| {
                    use axum::response::IntoResponse;
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        crate::core::panic_500_body(&*err),
                    )
                        .into_response()
                },
            ))
            .layer(axum::middleware::from_fn(observability::track))
            .with_state(state);

        pubsub::mark_live_running();

        let port: i64 = crate::system::read_env_var("IPE_LIVE_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8000);
        let addr = format!("0.0.0.0:{port}");
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => return IpeResult::Err(format!("Web.app: bind {addr}: {e}").into()),
        };
        // Bind-address line (stderr, Rust-specific — carries the 0.0.0.0 bind).
        eprintln!("[ipe.web] listening on http://{addr}");
        // Go-parity user-facing line (stdout, `fmt.Printf("Ipe.Web listening on
        // :%d\n", port)` — live.go:3546).
        println!("Ipe.Web listening on :{port}");
        // Graceful shutdown (Go parity — live.go:3503): trap SIGINT/SIGTERM,
        // print the shutdown line, drain in-flight requests, and return cleanly so
        // the IpeTask resolves Ok → the generated entry exits 0 (NOT 130). A
        // SECOND signal force-exits 130 via the watchdog inside live_shutdown_signal.
        match axum::serve(listener, app)
            .with_graceful_shutdown(live_shutdown_signal(shutdown_store))
            .await
        {
            Ok(()) => ok_res(()),
            Err(e) => IpeResult::Err(format!("Web.app: serve: {e}").into()),
        }
    }
}

/// Read the session cookie from request headers. Uses the base-path-aware
/// cookie name (`session_cookie_name`) so a sub-app reads its own scoped cookie,
/// never the parent's `ipe_sid`.
fn sid_from_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    let name = session_cookie_name();
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for c in raw.split(';') {
        let c = c.trim();
        if let Some((k, v)) = c.split_once('=')
            && k.trim() == name
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

// The Ipe.Html `Ffi.callPure "htmlXxx"` kernel wrappers (html_render_,
// html_escape_text_, html_escape_attr_, html_attr_to_string_) now live in the
// standalone top-level `ipe_runtime::html` module (re-exported here via
// `use super::*`), so a non-Web Ipe.Html / Ipe.Ui render doesn't pull this
// server module in.

#[cfg(test)]
mod reload_push_tests {
    use super::*;
    use crate::web::store::{MemoryStore, SessionHandle, SessionStore};
    use std::time::Duration;
    use tokio::sync::mpsc::channel;

    fn handle_with(sse_tx: Option<SseTx>) -> SessionHandle<(), ()> {
        let (tx, _rx) = channel::<()>(1);
        let tree: Html<()> = Html::HText(String::new());
        let index = build_index(&tree);
        Arc::new(Mutex::new(SessionEntry {
            model: (),
            last_view: tree,
            index,
            seq: 0,
            sse_tx,
            msg_tx: tx,
        }))
    }

    /// Every SSE-attached live session receives exactly ONE `event: reload`
    /// frame; a session with no SSE connection is skipped without panicking.
    #[tokio::test]
    async fn push_reload_to_live_sessions_sends_one_frame_per_live_session() {
        let store_impl: MemoryStore<(), ()> = MemoryStore::new(Duration::from_secs(60));
        let (sse_tx, mut sse_rx) = sse::channel();
        store_impl.set("with_sse", handle_with(Some(sse_tx))).await;
        store_impl.set("without_sse", handle_with(None)).await;
        let store: Arc<dyn SessionStore<(), ()>> = Arc::new(store_impl);

        push_reload_to_live_sessions(&store).await;

        let frame = sse_rx
            .try_recv()
            .expect("the SSE-attached session must receive a reload frame");
        assert_eq!(frame.0, sse::frame("reload", "{}"));
        assert!(
            sse_rx.try_recv().is_err(),
            "exactly one frame per live session, never more"
        );
    }

    /// H23: with `ENV=production` the reload push is UNREACHABLE — the
    /// gated path pushes nothing; in dev it pushes. (The gate is tested via
    /// `maybe_push_reload_to_live_sessions`, the exact call
    /// `live_shutdown_signal` makes right after `mark_draining` — split out
    /// so no real OS signal is needed here.)
    #[tokio::test]
    async fn live_shutdown_signal_skips_the_reload_push_in_production() {
        use crate::system::{locked_remove_var, locked_set_var};
        let prior_env = std::env::var("ENV").ok();
        let prior_ipe_env = std::env::var("IPE_ENV").ok();

        let store_impl: MemoryStore<(), ()> = MemoryStore::new(Duration::from_secs(60));
        let (sse_tx, mut sse_rx) = sse::channel();
        store_impl.set("s", handle_with(Some(sse_tx))).await;
        let store: Arc<dyn SessionStore<(), ()>> = Arc::new(store_impl);

        locked_set_var("ENV", "production");
        maybe_push_reload_to_live_sessions(&store).await;
        assert!(
            sse_rx.try_recv().is_err(),
            "production must have NO reachable path that pushes the reload frame"
        );

        locked_set_var("ENV", "dev");
        maybe_push_reload_to_live_sessions(&store).await;
        assert!(
            sse_rx.try_recv().is_ok(),
            "dev mode must push the reload frame"
        );

        match prior_env {
            Some(v) => locked_set_var("ENV", &v),
            None => locked_remove_var("ENV"),
        }
        match prior_ipe_env {
            Some(v) => locked_set_var("IPE_ENV", &v),
            None => locked_remove_var("IPE_ENV"),
        }
    }
}

#[cfg(test)]
mod dev_banner_tests {
    use super::dev_console_banner;

    #[test]
    fn banner_byte_matches_go_dev_banner_markup() {
        // Go parity (dev_banner.go devBannerHTML): same id, target/rel/title,
        // monospace blue style, `&#128269;` ENTITY (not a literal emoji).
        let b = dev_console_banner("");
        let expected = "<a id=\"__ipe-dev-console\" href=\"/_ipe/console\" target=\"_blank\" \
            rel=\"noopener\" title=\"Ipe Console (dev only)\" \
            style=\"position:fixed;right:12px;bottom:12px;z-index:2147483646;\
            font:12px/1.4 ui-monospace,Menlo,monospace;\
            background:#1c2027;color:#7eb6ff;\
            border:1px solid #353b46;border-radius:6px;\
            padding:6px 10px;text-decoration:none;\
            box-shadow:0 2px 8px rgba(0,0,0,0.4);\">\
            &#128269; Console</a>";
        assert_eq!(b, expected, "dev console banner must byte-match Go");
        assert!(
            !b.contains("🔍"),
            "must use the &#128269; entity, not a literal emoji"
        );
    }

    #[test]
    fn banner_suppressed_for_subapp() {
        // A non-empty base = sub-app (e.g. the console child) → no recursive link.
        assert_eq!(dev_console_banner("/_ipe/console"), "");
    }
}

#[cfg(test)]
mod duration_parse_tests {
    use super::parse_duration_secs;

    #[test]
    fn go_style_durations_and_bare_seconds() {
        assert_eq!(parse_duration_secs("1800"), Some(1800)); // bare seconds (legacy)
        assert_eq!(parse_duration_secs("30m"), Some(1800));
        assert_eq!(parse_duration_secs("1h"), Some(3600));
        assert_eq!(parse_duration_secs("24h"), Some(86400));
        assert_eq!(parse_duration_secs("90s"), Some(90));
        assert_eq!(parse_duration_secs("1h30m"), Some(5400));
        assert_eq!(parse_duration_secs("45m"), Some(2700)); // the e2e check (IPE_LIVE_TTL=45m)
        assert_eq!(parse_duration_secs("  1h  "), Some(3600));
    }

    #[test]
    fn malformed_is_none_never_panics() {
        assert_eq!(parse_duration_secs(""), None);
        assert_eq!(parse_duration_secs("abc"), None);
        assert_eq!(parse_duration_secs("1d"), None); // unsupported unit
        assert_eq!(parse_duration_secs("1h30"), None); // trailing unit-less number
        assert_eq!(parse_duration_secs("m"), None); // unit with no number
        assert_eq!(parse_duration_secs("-5m"), None);
    }
}

#[cfg(test)]
mod request_is_https_tests {
    use super::request_is_https_with_trust;

    #[test]
    fn ignored_without_trust_opt_in() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(
            !request_is_https_with_trust(&h, false),
            "must ignore X-Forwarded-Proto without IPE_TRUSTED_PROXY opt-in"
        );
    }

    #[test]
    fn honoured_when_trusted() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(request_is_https_with_trust(&h, true));

        let mut h2 = axum::http::HeaderMap::new();
        h2.insert("x-forwarded-proto", "http".parse().unwrap());
        assert!(!request_is_https_with_trust(&h2, true));
    }

    #[test]
    fn missing_header_is_not_https() {
        let h = axum::http::HeaderMap::new();
        assert!(!request_is_https_with_trust(&h, true));
    }
}

#[cfg(test)]
mod base_path_tests {
    use super::{
        client_js_path, cookie_name_for, cookie_path_for, normalise_base_path, render_page_full,
    };

    #[test]
    fn normalise_root_and_empty_collapse() {
        assert_eq!(normalise_base_path(""), "");
        assert_eq!(normalise_base_path("/"), "");
        assert_eq!(normalise_base_path("   "), "");
    }

    #[test]
    fn normalise_adds_leading_drops_trailing() {
        assert_eq!(normalise_base_path("/_ipe/console"), "/_ipe/console");
        assert_eq!(normalise_base_path("/_ipe/console/"), "/_ipe/console");
        assert_eq!(normalise_base_path("_ipe/console"), "/_ipe/console");
        assert_eq!(normalise_base_path("  /billing/  "), "/billing");
    }

    #[test]
    fn cookie_name_is_ipe_sid_at_root_distinct_under_base() {
        assert_eq!(cookie_name_for(""), "ipe_sid");
        // Distinct from the parent's `ipe_sid` so the proxied child can't clobber it.
        assert_eq!(cookie_name_for("/_ipe/console"), "ipe_sid__ipe_console");
        assert_ne!(cookie_name_for("/_ipe/console"), "ipe_sid");
    }

    #[test]
    fn cookie_path_scopes_to_base() {
        assert_eq!(cookie_path_for(""), "/");
        // Scoped → the cookie is never sent to the parent's own routes.
        assert_eq!(cookie_path_for("/_ipe/console"), "/_ipe/console");
    }

    #[test]
    fn render_page_threads_base_into_meta_and_window_global() {
        let root = render_page_full("sid1", "", "<b>x</b>", "deadbeef");
        assert!(root.contains("<meta name=\"ipe-base\" content=\"\">"));
        assert!(root.contains("window.__IPE_BASE=\"\""));

        let sub = render_page_full("sid1", "/_ipe/console", "<b>x</b>", "deadbeef");
        assert!(sub.contains("<meta name=\"ipe-base\" content=\"/_ipe/console\">"));
        assert!(sub.contains("window.__IPE_BASE=\"/_ipe/console\""));
    }

    #[test]
    fn render_page_emits_external_client_script_with_sri() {
        let root = render_page_full("sid1", "", "<b>x</b>", "tok1");
        // Per-session values stay inline.
        assert!(root.contains("window.__IPE_SID=\"sid1\""));
        assert!(root.contains("window.__IPE_CSRF_TOKEN=\"tok1\""));
        // CLIENT_JS body must NOT be inlined.
        assert!(!root.contains("var __ipeSid = window.__IPE_SID"));
        // External script tag with content-addressed src.
        assert!(root.contains("<script src=\"/_ipe/client."));
        assert!(root.contains(".js\" integrity=\"sha256-"));
        assert!(root.contains("crossorigin=\"anonymous\">"));
        // SRI attribute is present and non-empty.
        assert!(root.contains("integrity=\"sha256-"));
    }

    #[test]
    fn render_page_sub_app_prefixes_client_src() {
        let sub = render_page_full("sid1", "/_ipe/console", "<b>x</b>", "tok1");
        // External script src must carry the base prefix.
        assert!(root_or_sub_has_prefixed_client_src(&sub, "/_ipe/console"));
    }

    fn root_or_sub_has_prefixed_client_src(html: &str, base: &str) -> bool {
        // Find `<script src="` and check the src starts with `base/_ipe/client.`
        let needle = format!("<script src=\"{}/_ipe/client.", base);
        html.contains(&needle)
    }

    #[test]
    fn client_js_path_is_content_addressed_and_stable() {
        let p1 = client_js_path();
        let p2 = client_js_path();
        // Same result on repeated calls (OnceLock).
        assert_eq!(p1, p2);
        // Path format: /_ipe/client.<16 hex chars>.js
        assert!(p1.starts_with("/_ipe/client."));
        assert!(p1.ends_with(".js"));
        let hash_part = p1
            .trim_start_matches("/_ipe/client.")
            .trim_end_matches(".js");
        assert_eq!(hash_part.len(), 16, "URL hash should be 16 hex chars");
        assert!(
            hash_part.chars().all(|c| c.is_ascii_hexdigit()),
            "URL hash should be hex: {hash_part}"
        );
    }
}

#[cfg(test)]
mod session_lost_body_tests {
    //! Guards the LOAD-BEARING session-lost 404 wire contract.
    //!
    //! After a server restart, the browser recovers ONLY because
    //! `client.js` `__ipeProbeSessionLost` reloads the page when its probe
    //! POST to `/_ipe/event` gets a 404 + `X-Ipe-Live: 1` whose body CONTAINS
    //! the substring `"session not found"` (client.js l1481/l1530/l1536). A
    //! refactor that flips the body back to the old `"no session"` would
    //! silently strand every client on a permanent "Reconnecting…" banner —
    //! this module makes that regression a compile/test failure.
    use super::SESSION_LOST_BODY;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderName, Request, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use tower::ServiceExt; // for `oneshot`

    /// The exact response shape both real `event_handler` session-miss arms
    /// emit (no-cookie + store-miss), reproduced here over `SESSION_LOST_BODY`
    /// — the SAME constant the handlers reference — so the substring contract
    /// is mechanically checked against the real source of truth.
    async fn no_session_event_handler() -> Response {
        (
            StatusCode::NOT_FOUND,
            [(HeaderName::from_static("x-ipe-live"), "1")],
            SESSION_LOST_BODY,
        )
            .into_response()
    }

    #[test]
    fn const_body_satisfies_client_probe_substring() {
        // client.js: `if (body.indexOf("session not found") < 0) return;`
        assert!(
            SESSION_LOST_BODY.contains("session not found"),
            "session-lost 404 body must contain the client-probed substring \
             \"session not found\"; got {SESSION_LOST_BODY:?}"
        );
        // Belt-and-braces: the old broken body must never reappear.
        assert_ne!(
            SESSION_LOST_BODY, "no session",
            "session-lost body regressed to \"no session\" — client recovery breaks"
        );
    }

    #[tokio::test]
    async fn event_session_miss_returns_404_marker_and_contract_body() {
        let app = Router::new().route("/_ipe/event", post(no_session_event_handler));

        let req = Request::builder()
            .method("POST")
            .uri("/_ipe/event")
            .body(Body::from("{}"))
            .expect("build probe request");

        let resp = app.oneshot(req).await.expect("router responds");

        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "must be a 404");
        assert_eq!(
            resp.headers()
                .get("x-ipe-live")
                .expect("x-ipe-live header present")
                .to_str()
                .expect("ascii header value"),
            "1",
            "X-Ipe-Live marker distinguishes session-lost from a wedged proxy"
        );

        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .expect("collect body");
        let body = String::from_utf8(bytes.to_vec()).expect("utf8 body");
        assert!(
            body.contains("session not found"),
            "body must contain the client-probed recovery substring; got {body:?}"
        );
    }
}

#[cfg(test)]
mod admission_control_tests {
    use super::*;

    // Closes the leak/cap coupling: SessionSlot decrements EXACTLY once on drop,
    // paired 1:1 with the reservation fetch_add — no underflow, no double-count.
    #[test]
    fn session_slot_decrements_once_on_drop() {
        let count = Arc::new(AtomicUsize::new(0));
        // Simulate a reservation (what the Cold/None arm does).
        count.fetch_add(1, Ordering::SeqCst);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        {
            let _slot = SessionSlot {
                count: count.clone(),
            };
            assert_eq!(
                count.load(Ordering::SeqCst),
                1,
                "slot construction must not change the count"
            );
        } // _slot drops here → one decrement
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "slot drop must decrement exactly once"
        );
    }

    // Reserve/drop M >> N times returns to 0 (counter exactness, no leak/underflow).
    #[test]
    fn reserve_then_release_balances_to_zero() {
        let count = Arc::new(AtomicUsize::new(0));
        for _ in 0..1000 {
            count.fetch_add(1, Ordering::SeqCst);
            let _slot = SessionSlot {
                count: count.clone(),
            };
        }
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    // max_sessions(): env override, default, and the 0=unlimited opt-out.
    #[test]
    fn max_sessions_parsing() {
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_LIVE_MAX_SESSIONS") };
        assert_eq!(max_sessions(), 50_000);
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_LIVE_MAX_SESSIONS", "7") };
        assert_eq!(max_sessions(), 7);
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_LIVE_MAX_SESSIONS", "0") };
        assert_eq!(max_sessions(), 0, "0 = unlimited opt-out");
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::set_var("IPE_LIVE_MAX_SESSIONS", "garbage") };
        assert_eq!(max_sessions(), 50_000, "unparseable falls back to default");
        // SAFETY: test-only env mutation; `std::env::set_var`/`remove_var` are `unsafe` in Rust 2024 due to the reader/mutator `environ` race.
        unsafe { std::env::remove_var("IPE_LIVE_MAX_SESSIONS") };
    }
}
