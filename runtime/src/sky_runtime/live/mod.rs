//! Sky.Live on the Rust backend — HTTP-first render + SSE patch loop.
//! Generic over the app's (Model, Msg); no `any`, static dispatch only.
pub mod diff;
pub use diff::*;
pub mod dispatch;
pub use dispatch::*;
pub mod sse;
pub use sse::*;
pub mod form;
pub use form::*;
pub mod route;
pub use route::*;
pub mod console;
pub mod csrf;
pub mod style_inject;
// Pre-built console child + reverse-proxy — spawns the bundled console
// binary and proxies /_sky/console/*; falls back to in-process `console` when the
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
// `sky_runtime::Event`. The broker internals (`Event`, `Broker`, `broker`,
// `subscribe`, `publish`) are `pub(crate)` in pubsub.rs — they never leave the
// crate, so they don't need re-exporting here.
pub use pubsub::{
    cmd_publish, cmd_publish_no_echo, pubsub_publish, pubsub_publish_no_echo, sub_subscribe_topic,
};

// Html ADTs + renderer now live in the standalone top-level `html` module;
// re-export them so live submodules (diff.rs, store.rs, …) that `use super::*`
// still see Html / Attribute / Event / render_html / html_render_.
pub use crate::sky_runtime::html::*;

use super::*;

/// Body returned with a session-miss 404 from `/_sky/event` and `/_sky/sse`.
///
/// LOAD-BEARING CONTRACT — `client.js` `__skyProbeSessionLost` only triggers
/// `window.location.reload()` (the SSE-reconnect recovery path) when a probe
/// POST gets a 404 + `X-Sky-Live: 1` AND the body CONTAINS the substring
/// `"session not found"` (client.js l1481/l1530/l1536). Go's backend returns
/// the same string; diverging it (the old `"no session"` body) silently broke
/// recovery after a server restart — the browser shows "Reconnecting…" forever.
/// Guarded by `session_lost_body_tests`.
const SESSION_LOST_BODY: &str = "session not found";

// ─── Client assets ────────────────────────────────────────────────────────────

/// The browser-side Sky.Live client, extracted verbatim from Go's
/// `liveJSWithCfgAndCsrfWithBase` template (runtime-go/rt/live.go:5853-7490).
/// The 12 header `%`-verb lines are replaced with static literals;
/// the two `%%` CSS escapes are un-escaped to `%`.
const CLIENT_JS: &str = include_str!("client.js");

/// Content-addressing for the client asset: computed ONCE at first access via
/// `OnceLock`. Holds `(hex16, base64full)` where:
///   - `hex16` — first 16 hex chars of SHA-256(CLIENT_JS) → used in the URL
///     (`/_sky/client.<hex16>.js`) for cache-busting.
///   - `base64full` — standard base64 of the full 32-byte SHA-256 digest → the
///     `integrity="sha256-<base64full>"` SRI attribute value.
///
/// Both are derived from the same digest, computed once and interned.
/// The `sha2` crate is unconditionally available in every generated Live project
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
/// `/_sky/client.a1b2c3d4e5f6a7b8.js`. The path is stable for a given
/// `client.js` build and changes whenever the file changes — making
/// `Cache-Control: immutable` safe. Callers may prepend the sub-app `base`.
pub fn client_js_path() -> String {
    let (hex16, _) = client_js_hashes();
    format!("/_sky/client.{}.js", hex16)
}

/// Minimal CSS reset injected into every Sky.Live page.
/// Ported verbatim from Go's `liveBaseCSS` (runtime-go/rt/live.go:3847-3858).
const BASE_CSS: &str = concat!(
    "*,*::before,*::after{box-sizing:border-box}",
    "html,body{margin:0;padding:0;min-height:100%}",
    "body{min-height:100vh;display:flex;flex-direction:column;font-family:-apple-system,BlinkMacSystemFont,\"Segoe UI\",Roboto,\"Helvetica Neue\",Arial,sans-serif;line-height:1.4}",
    "#sky-root{display:flex;flex-direction:column;flex:1 0 auto;min-height:0}",
    "h1,h2,h3,h4,h5,h6,p,ul,ol,li,figure,blockquote,pre,dl,dd{margin:0;padding:0;font-weight:inherit;font-size:inherit}",
    "button,input,select,textarea{font:inherit;color:inherit}",
    "button{background:none;border:0;padding:0;cursor:pointer;text-align:inherit}",
    "a{color:inherit;text-decoration:none}",
    "img,video,canvas,svg{display:block;max-width:100%}",
);

// ─── Page renders ─────────────────────────────────────────────────────────────

/// Render `view(model)` to a full HTML page and print it — the static
/// render path (the interactive server is `live_app`).
pub fn live_render_static<E, Model, Msg, FView>(view: FView, model: Model) -> SkyTask<E, ()>
where
    E: Send + 'static,
    Model: Send + 'static,
    Msg: Send + 'static,
    FView: Fn(Model) -> Html<Msg> + Send + 'static,
{
    Box::pin(async move {
        let mut tree = view(model);
        assign_sky_ids(&mut tree, "r");
        style_inject::apply_style_injections(&mut tree);
        println!("{}", render_page(&render_html(&tree)));
        SkyResult::Ok(())
    })
}

/// Minimal page wrap. Kept byte-identical so example 27-live-static
/// continues to pass. The full client-bearing wrap is `render_page_full`.
pub fn render_page(body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body><div id=\"sky-root\">{body}</div></body></html>"
    )
}

/// Full page wrap with the live client loaded as a cacheable external asset.
/// Mirrors Go's live page render (runtime-go/rt/live.go:3788).
///
/// `sid`  — session id (injected into the JS via `window.__SKY_SID`).
/// `base` — sub-app base path, e.g. "" for root-mounted apps.
/// `body` — pre-rendered HTML body (from `render_html`).
///
/// Two scripts are emitted in document order (no defer/async — execution order
/// is left-to-right by the HTML spec):
///   1. A tiny inline `<script>` setting the three per-session window globals
///      (`__SKY_SID`, `__SKY_BASE`, `__SKY_CSRF_TOKEN`). These MUST stay inline
///      because they are per-session values and must never be cached.
///   2. An external `<script src="…/_sky/client.<hash>.js" integrity="sha256-…"
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
/// `SKY_LIVE_*` tuning env vars and emit the `window.__SKY_*` assignments the
/// client (`client.js`) reads with a hardcoded fallback. Without this the Rust
/// client ignored every `SKY_LIVE_RETRY_*` / `QUEUE_MAX` / `HELLO_TIMEOUT_MS` /
/// `HEARTBEAT_TTL_MS` / `BANNER` override. Totally parsed: a malformed value
/// falls back to Go's default; never panics.
fn live_client_config_js() -> String {
    fn num(var: &str, default: u64) -> u64 {
        crate::sky_runtime::system::read_env_var(var)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(default)
    }
    // SKY_LIVE_BANNER: off/0/false → disabled (Go parity); anything else → on.
    let banner = !matches!(
        crate::sky_runtime::system::read_env_var("SKY_LIVE_BANNER")
            .ok()
            .map(|s| s.trim().to_ascii_lowercase()),
        Some(ref v) if v == "off" || v == "0" || v == "false"
    );
    format!(
        "window.__SKY_BANNER_ENABLED={banner};\
         window.__SKY_RETRY_BASE_MS={};\
         window.__SKY_RETRY_MAX_MS={};\
         window.__SKY_RETRY_MAX_ATTEMPTS={};\
         window.__SKY_EVENT_QUEUE_MAX={};\
         window.__SKY_HELLO_TIMEOUT_MS={};\
         window.__SKY_HEARTBEAT_TTL_MS={};\
         window.__SKY_MSG_RECONNECTING=\"Reconnecting…\";\
         window.__SKY_MSG_OFFLINE=\"Connection lost — refresh to retry\";",
        num("SKY_LIVE_RETRY_BASE_MS", 500),
        num("SKY_LIVE_RETRY_MAX_MS", 16000),
        num("SKY_LIVE_RETRY_MAX_ATTEMPTS", 10),
        num("SKY_LIVE_QUEUE_MAX", 50),
        num("SKY_LIVE_HELLO_TIMEOUT_MS", 8000),
        num("SKY_LIVE_HEARTBEAT_TTL_MS", 35000),
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
    // the parent proxy (same as /_sky/sse, /_sky/event, /_sky/console).
    let client_src = format!("{base}/_sky/client.{hex16}.js");
    let integrity = format!("sha256-{b64}");
    let config_js = live_client_config_js();
    format!(
        "<!DOCTYPE html><html><head>\
         <meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"sky-base\" content=\"{base}\">\
         <style>{BASE_CSS}</style>\
         </head>\
         <body><div id=\"sky-root\">{body}</div>{dev_banner}\
         <script>window.__SKY_SID={sid_js};window.__SKY_BASE={base_js};window.__SKY_CSRF_TOKEN={csrf_js};{config_js}</script>\
         <script src=\"{client_src}\" integrity=\"{integrity}\" crossorigin=\"anonymous\"></script>\
         </body></html>"
    )
}

/// Floating "🔍 Console" link injected into every dev-mode page (Go parity:
/// `devBannerHTML`). Suppressed for a sub-app (`base` non-empty — e.g. the
/// bundled console child itself; a console link inside the console is
/// recursive), in production (`ENV`/`SKY_ENV` non-dev), and when the console
/// surface is disabled (`SKY_CONSOLE_EMBED=off` / `SKY_CONSOLE_AUTH=off`).
/// Rendered as a sibling of `#sky-root` so a body patch never blows it away;
/// `position:fixed` pins it bottom-right and `pointer-events` stays default so
/// the link is clickable.
fn dev_console_banner(base: &str) -> String {
    if !base.is_empty() || crate::sky_runtime::telemetry::production_from_env() {
        return String::new();
    }
    if matches!(
        crate::sky_runtime::system::read_env_var("SKY_CONSOLE_EMBED").as_deref(),
        Ok("off") | Ok("0") | Ok("false")
    ) || crate::sky_runtime::system::read_env_var("SKY_CONSOLE_AUTH")
        .map(|v| v == "off")
        .unwrap_or(false)
    {
        return String::new();
    }
    // Byte-match Go's `devBannerHTML` (dev_banner.go): same id, target/rel/title,
    // monospace blue styling, and the `&#128269;` entity (NOT a literal emoji) so
    // both backends emit identical bytes. href honours SKY_CONSOLE_URL (default
    // /_sky/console), attribute-escaped against a hostile env value.
    let url = crate::sky_runtime::system::read_env_var("SKY_CONSOLE_URL")
        .map(|v| v.trim().to_string())
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/_sky/console".to_string());
    let esc = url
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;");
    format!(
        "<a id=\"__sky-dev-console\" href=\"{esc}\" target=\"_blank\" rel=\"noopener\" \
         title=\"Sky Console (dev only)\" \
         style=\"position:fixed;right:12px;bottom:12px;z-index:2147483646;\
         font:12px/1.4 ui-monospace,Menlo,monospace;\
         background:#1c2027;color:#7eb6ff;\
         border:1px solid #353b46;border-radius:6px;\
         padding:6px 10px;text-decoration:none;\
         box-shadow:0 2px 8px rgba(0,0,0,0.4);\">\
         &#128269; Console</a>"
    )
}

// ─── live_app: axum mount + per-session TEA driver over SSE ─────────────────

use crate::sky_runtime::tea::{SkyCmd, SkySub};
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
/// `__skyHandleResponse(undefined, _, _, globalSeq)` → `__skyApplyPatches`.
/// We use `globalSeq` (the server-owned broadcast counter) rather than the
/// local `seq` so it never collides with the client's own POST-local seq gate.
#[derive(serde::Serialize)]
struct PatchEnvelope<'a> {
    #[serde(rename = "globalSeq")]
    global_seq: u64,
    patches: &'a [crate::sky_runtime::live::diff::Patch],
}

/// Wire shape POSTed by the browser client to `/_sky/event`
/// (`live/client.js` __skySend): `{sessionId, seq, msg, args, handlerId}`.
/// `handlerId` is the element's `data-sky-hid` (== its sky-id); `msg` is the
/// `sky-<event>` marker. We resolve handlers server-side by sky-id + event,
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
    /// Event name. The client posts the `sky-<event>` marker value as `msg`;
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
type ParamResolver = Arc<dyn Fn(&str) -> crate::sky_runtime::dict::SkyDict<String> + Send + Sync>;

/// Shared axum state: the session store + Arc'd TEA callbacks.
struct LiveState<Model, Msg, FInit, FUpdate, FView, FSubs> {
    store: Arc<dyn store::SessionStore<Model, Msg>>,
    init: Arc<FInit>,
    update: Arc<FUpdate>,
    view: Arc<FView>,
    subs: Arc<FSubs>,
    /// Maps the freshly-`init`'d model + GET path to the model whose `page`
    /// field reflects the matched route. `live_app` passes identity (no
    /// routing); `live_app_routed` captures the route table + page-setter.
    /// `Page`/`set_page` are erased into this boxed closure, so `LiveState`
    /// keeps its original 6 type params.
    route_resolver: RouteResolver<Model>,
    /// Maps a GET path to the matched route's `:name`→value params (for
    /// `req.params`). Model-independent so the page handler can build `req`
    /// BEFORE calling `init`. `live_app` returns empty; `live_app_routed`
    /// captures the route table.
    param_resolver: ParamResolver,
    /// Live driver count for admission control. Each spawned `drive_session`
    /// holds a `SessionSlot` that decrements this on exit; a cookieless GET that
    /// would push it past `max_sessions()` is rejected (503) instead of minting
    /// an unbounded number of sessions. Decremented ONLY via `SessionSlot::drop`,
    /// so the leak fix (mortal driver) and this cap share one mechanism.
    session_count: Arc<AtomicUsize>,
}

// Manual Clone — derive would demand Clone on the closures (they're behind Arc).
impl<Model, Msg, FInit, FUpdate, FView, FSubs> Clone
    for LiveState<Model, Msg, FInit, FUpdate, FView, FSubs>
{
    fn clone(&self) -> Self {
        LiveState {
            store: self.store.clone(),
            init: self.init.clone(),
            update: self.update.clone(),
            view: self.view.clone(),
            subs: self.subs.clone(),
            route_resolver: self.route_resolver.clone(),
            param_resolver: self.param_resolver.clone(),
            session_count: self.session_count.clone(),
        }
    }
}

/// Max concurrent live-session drivers (admission control). 0 = unlimited
/// (opt-out). Default 50_000 — far above any single-instance real load, low
/// enough to bound memory under a session-creation flood. Env SKY_LIVE_MAX_SESSIONS.
fn max_sessions() -> usize {
    crate::sky_runtime::system::read_env_var("SKY_LIVE_MAX_SESSIONS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50_000)
}

/// RAII admission slot: decrements `LiveState::session_count` exactly once when
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
fn run_cmd<Msg: Send + 'static>(cmd: SkyCmd<Msg>, tx: &Sender<Msg>, sid: &str) {
    match cmd {
        SkyCmd::None => {}
        SkyCmd::Batch(items) => {
            for c in items {
                run_cmd(c, tx, sid);
            }
        }
        SkyCmd::Perform(thunk) => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let m = thunk().await;
                // Bounded send: drop the Msg and warn if the session queue is
                // full (a stalled driver or a burst of fast Perform tasks).
                if tx.send(m).await.is_err() {
                    eprintln!(
                        "[sky.live] run_cmd: session msg channel closed; dropping Perform result"
                    );
                }
            });
        }
        SkyCmd::Publish(thunk) => {
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
    sub: SkySub<Msg>,
    tx: &Sender<Msg>,
    handles: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    for h in handles.drain(..) {
        h.abort();
    }
    fn go<Msg: Clone + Send + 'static>(
        sub: SkySub<Msg>,
        tx: &Sender<Msg>,
        handles: &mut Vec<tokio::task::JoinHandle<()>>,
    ) {
        match sub {
            SkySub::None => {}
            SkySub::Batch(items) => {
                for s in items {
                    go(s, tx, handles);
                }
            }
            SkySub::Every { ms, msg } => {
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
            SkySub::Source(spawn) => {
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
    // Admission-control slot: decrements LiveState::session_count on driver exit.
    _slot: SessionSlot,
) where
    // PartialEq: the `noop` signal compares old vs new Model by structural
    // equality. Generated Model structs always derive PartialEq.
    Model: Clone + PartialEq + Send + 'static,
    // `Debug` is required to derive the BOUNDED Msg variant-name label for the
    // `sky_live_msg_seconds` histogram (telemetry::variant_name). Generated Msg
    // enums always derive Debug, so this internal bound is always satisfiable.
    Msg: Clone + Send + std::fmt::Debug + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, SkyCmd<Msg>) + Send + Sync + 'static,
    FView: Fn(Model) -> Html<Msg> + Send + Sync + 'static,
    FSubs: Fn(Model) -> SkySub<Msg> + Send + Sync + 'static,
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
        // Msg-handling latency histogram (Go parity: sky_live_msg_seconds{name},
        // msg_logging.go). The `name` label is the BOUNDED Msg variant name
        // (finite cardinality), never a payload — see telemetry::variant_name.
        // Extracted BEFORE `update` consumes `msg`.
        let msg_name = crate::sky_runtime::telemetry::variant_name(&msg);
        let msg_started = std::time::Instant::now();
        let (next, cmd) = update(msg, model);
        crate::sky_runtime::telemetry::metric_observe(
            "sky_live_msg_seconds",
            &[("name", &msg_name)],
            msg_started.elapsed().as_secs_f64(),
        );
        // Borrow (not move) cmd to detect a no-command update; cmd is moved into
        // run_cmd later. Part of the `noop` signal below.
        let cmd_is_none = matches!(cmd, SkyCmd::None);

        let mut tree = view(next.clone());
        assign_sky_ids(&mut tree, "r");
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
        // Msg counter (Go parity: sky_live_msg_total{name,outcome,noop}). All
        // labels bounded: name = finite variant set, outcome = "ok" (this path
        // has no error channel), noop ∈ {true,false}. Emitted OUTSIDE the entry
        // lock (no registry-lock-under-entry-lock nesting).
        crate::sky_runtime::telemetry::metric_inc(
            "sky_live_msg_total",
            &[
                ("name", &msg_name),
                ("outcome", "ok"),
                ("noop", if noop { "true" } else { "false" }),
            ],
            1,
        );

        if !patches.is_empty() {
            if let Some(sse) = sse {
                let env = PatchEnvelope {
                    global_seq: seq,
                    patches: &patches,
                };
                if let Ok(json) = serde_json::to_string(&env) {
                    let _ = sse.send(SsePatch(sse::frame("patches", &json))).await;
                }
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
/// SECURITY: the sid is the SOLE bearer credential for a Sky.Live session
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

/// Normalise a raw `SKY_LIVE_BASE_PATH` value: trim, drop a trailing slash,
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
/// `__Host-sky_sid` in secure mode (production / frame-ancestors) else `sky_sid`;
/// for a sub-app a base-derived DISTINCT name so this child's session cookie can
/// never clobber the PARENT app's `sky_sid` (both would otherwise be `Path=/` and
/// share the browser's cookie jar on the proxied paths). Go gives each sub-app a
/// distinct `cookieName` for the same reason (live.go:2769).
///
/// SECURITY (root, secure mode): the session cookie is the SOLE bearer credential
/// (`sid_from_cookie` + `store.get` authorise every `/_sky/event` + `/_sky/sse`),
/// so it gets the `__Host-` prefix — the browser then refuses any `Set-Cookie`
/// carrying a `Domain=` attribute, closing the sibling-subdomain cookie-tossing →
/// session-fixation vector (an attacker on `evil.example.com` with a valid cert
/// could otherwise plant `sky_sid` for `example.com`). `__Host-` MANDATES
/// Secure + Path=/ + no-Domain — `page_response` satisfies all three in secure
/// mode (Secure flag set, root `cookie_path()` is `/`, no Domain attribute).
/// Mirrors `csrf::csrf_cookie_name()`. Plain-HTTP dev keeps the bare `sky_sid`
/// (`__Host-` requires Secure, which a browser drops over `http://`). A sub-app
/// (Path != `/`) can never use `__Host-`, so it keeps the base-scoped name.
fn cookie_name_for(base: &str) -> String {
    if base.is_empty() {
        if csrf::cookies_secure() {
            "__Host-sky_sid".to_string()
        } else {
            "sky_sid".to_string()
        }
    } else {
        let suffix: String = base
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("sky_sid{suffix}")
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

/// Normalised sub-app base path, read from `SKY_LIVE_BASE_PATH`. Empty when
/// unset (root-mounted app → byte-identical to a standalone Live server). When
/// set (this app runs as a reverse-proxied sub-app — e.g. the bundled console
/// mounted at `/_sky/console`), the value is threaded into `render_page_full`
/// so the client JS prefixes `/_sky/event` + `/_sky/sse` with it. The browser
/// reaches this child only through the parent proxy, which strips the prefix
/// before forwarding — so the child's own router stays root-relative.
fn live_base_path() -> String {
    normalise_base_path(
        &crate::sky_runtime::system::read_env_var("SKY_LIVE_BASE_PATH").unwrap_or_default(),
    )
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

/// Build the full-page HTTP response for a GET (initial render or reuse): the
/// client-bearing HTML wrap + the session cookie (name/path base-path-aware).
fn page_response(sid: &str, body: &str, csrf_token: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    let html = render_page_full(sid, &live_base_path(), body, csrf_token);
    // Session cookie carries `Secure` in production / frame-ancestors mode (was
    // unconditionally omitted — a downgrade hole). NOTE the decision is
    // ENV-gated, NOT request-scoped: `csrf::cookies_secure()` snapshots
    // `production_from_env() || frame_ancestors().is_some()` once at process
    // start; it does NOT inspect this request's TLS / `X-Forwarded-Proto`. A
    // dev process fronted by a TLS proxy therefore emits a non-Secure session
    // cookie. Request-scoped TLS detection needs trusted-proxy gating + header
    // plumbing across the cookie-name (`__Host-`) decision and is deferred.
    // SameSite=Lax stays so top-level navigations keep the session.
    let secure = if csrf::cookies_secure() {
        "; Secure"
    } else {
        ""
    };
    // SameSite (Go parity, live.go ~5653): a deploy opted into cross-origin
    // embedding via SKY_LIVE_FRAME_ANCESTORS needs `SameSite=None; Secure` so the
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

/// Maximum request body bytes for `/_sky/event`: `SKY_LIVE_MAX_BODY_BYTES`,
/// default 5 MiB (5 << 20 = 5 242 880). Mirrors Go's `handleEvent` body cap
/// (runtime-go/rt/live.go ~l3911). The default covers `Event.onFile` /
/// `Event.onImage` data-URL payloads; override for larger file uploads.
fn live_max_body_bytes() -> usize {
    crate::sky_runtime::system::read_env_var("SKY_LIVE_MAX_BODY_BYTES")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(5 << 20)
}

#[cfg(test)]
mod live_max_body_bytes_tests {
    use super::live_max_body_bytes;

    // SKY_LIVE_MAX_BODY_BYTES=0 must floor at the default, not disable the
    // body (matching server::max_body's already-correct `.filter(|&n| n > 0)`
    // — the missing floor previously 413'd every /_sky/event POST).
    #[test]
    fn live_max_body_bytes_floors_at_default_on_zero() {
        std::env::remove_var("SKY_LIVE_MAX_BODY_BYTES");
        assert_eq!(live_max_body_bytes(), 5 << 20);
        std::env::set_var("SKY_LIVE_MAX_BODY_BYTES", "1024");
        assert_eq!(live_max_body_bytes(), 1024);
        std::env::set_var("SKY_LIVE_MAX_BODY_BYTES", "0"); // invalid → default, not "reject everything"
        assert_eq!(live_max_body_bytes(), 5 << 20);
        std::env::remove_var("SKY_LIVE_MAX_BODY_BYTES");
    }
}

/// Session idle-TTL: `SKY_LIVE_TTL` seconds, default 1800 (30 min) — matches the
/// Go `[live] ttl` default.
fn live_ttl() -> std::time::Duration {
    let secs = crate::sky_runtime::system::read_env_var("SKY_LIVE_TTL")
        .ok()
        .and_then(|s| parse_duration_secs(&s))
        .unwrap_or(1800u64);
    std::time::Duration::from_secs(secs)
}

/// Parse a Go-style duration (Go parity for `SKY_LIVE_TTL` / `[live] ttl`): a bare
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
/// Tunable via `SKY_LIVE_SHUTDOWN_GRACE_MS` (default 1500 ms; 0 = exit at once).
fn shutdown_grace() -> std::time::Duration {
    let ms = crate::sky_runtime::system::read_env_var("SKY_LIVE_SHUTDOWN_GRACE_MS")
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

/// Await the FIRST shutdown signal (SIGINT or SIGTERM), then run the graceful
/// teardown and return so axum's `with_graceful_shutdown` drains in-flight
/// connections and the serve future resolves `Ok(())` (→ the SkyTask is `Ok` →
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
async fn live_shutdown_signal() {
    // First press: block until SIGINT or SIGTERM arrives.
    wait_for_term_or_int().await;

    // Print to stdout (Go uses `fmt.Println`, which is stdout). The leading
    // newline keeps the `^C` echo on its own line, matching Go.
    println!("\nSky.Live shutting down…");

    // Flip readyz → draining so orchestrators stop routing new traffic while
    // in-flight requests finish (Go: `SetReady(false)`).
    observability::mark_draining();

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
    // sub-window exit). Exit 0 keeps the SkyTask-Ok / exit-0 contract.
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
        eprintln!("Sky.Live: forcing exit (second signal)");
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

/// `Std.Live.app { init, update, view, subscriptions }` — serve via axum.
///
/// HTTP-first: a GET renders the full page with the embedded client, opens a
/// per-session TEA loop, and serves an SSE patch channel + a POST event
/// endpoint. The driver diffs view-over-view and pushes patches over SSE.
///
/// `init` receives a typed `req::LiveReq` (path/query/method/params/headers/
/// cookies) built from the incoming request; the driver calls `init(req)` so a
/// req-reader can bootstrap session state on first render. A non-req init is
/// monomorphised to ignore the threaded `LiveReq`.
#[allow(clippy::too_many_arguments)]
pub fn live_app<E, Model, Msg, FInit, FUpdate, FView, FSubs>(
    init: FInit,
    update: FUpdate,
    view: FView,
    subscriptions: FSubs,
    store_kind: String,
    store_path: String,
) -> SkyTask<E, ()>
where
    E: From<String> + Send + 'static,
    Model:
        serde::Serialize + serde::de::DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
    // Debug: forwarded through serve_live → drive_session for the
    // sky_live_msg_seconds{name} label. Generated Msg enums always derive Debug.
    Msg: Clone + Send + Sync + std::fmt::Debug + 'static,
    FInit: Fn(req::LiveReq) -> (Model, SkyCmd<Msg>) + Send + Sync + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, SkyCmd<Msg>) + Send + Sync + 'static,
    FView: Fn(Model) -> Html<Msg> + Send + Sync + 'static,
    FSubs: Fn(Model) -> SkySub<Msg> + Send + Sync + 'static,
{
    Box::pin(async move {
        let store = store::choose_store::<Model, Msg>(&store_kind, &store_path, live_ttl()).await;
        let state = LiveState {
            store,
            init: Arc::new(init),
            update: Arc::new(update),
            view: Arc::new(view),
            subs: Arc::new(subscriptions),
            // No routing: GET serves the freshly-init'd model unchanged; no params.
            route_resolver: Arc::new(|m, _path| m),
            param_resolver: Arc::new(|_path| crate::sky_runtime::dict::dict_empty()),
            session_count: Arc::new(AtomicUsize::new(0)),
        };
        serve_live(state).await
    })
}

/// `Std.Live.app { …, routes, notFound }` with URL routing — serve via axum.
///
/// Identical to `live_app` except a `route_resolver` is built from the route
/// table + page-setter: on each GET it matches the path to a `Page` value
/// (param strings applied via the route closures) and writes it into the
/// freshly-`init`'d model's `page` field via `set_page`. `Page`/`FSetPage`
/// are erased into the boxed resolver, so `serve_live`/`LiveState` keep the
/// original 6 type params.
#[allow(clippy::too_many_arguments)]
pub fn live_app_routed<E, Model, Msg, Page, FInit, FUpdate, FView, FSubs, FSetPage>(
    init: FInit,
    update: FUpdate,
    view: FView,
    subscriptions: FSubs,
    routes: Vec<route::Route<Page>>,
    not_found: Page,
    set_page: FSetPage,
    store_kind: String,
    store_path: String,
) -> SkyTask<E, ()>
where
    E: From<String> + Send + 'static,
    Model:
        serde::Serialize + serde::de::DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
    // Debug: forwarded through serve_live → drive_session for the
    // sky_live_msg_seconds{name} label. Generated Msg enums always derive Debug.
    Msg: Clone + Send + Sync + std::fmt::Debug + 'static,
    Page: Clone + Send + Sync + 'static,
    FInit: Fn(req::LiveReq) -> (Model, SkyCmd<Msg>) + Send + Sync + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, SkyCmd<Msg>) + Send + Sync + 'static,
    FView: Fn(Model) -> Html<Msg> + Send + Sync + 'static,
    FSubs: Fn(Model) -> SkySub<Msg> + Send + Sync + 'static,
    FSetPage: Fn(Page, Model) -> Model + Send + Sync + 'static,
{
    Box::pin(async move {
        let routes = Arc::new(routes);
        let not_found = Arc::new(not_found);
        let set_page = Arc::new(set_page);
        let routes_for_params = routes.clone();
        let resolver: RouteResolver<Model> =
            Arc::new(move |m, path| (set_page)(route::match_routes(&routes, &not_found, path), m));
        let param_resolver: ParamResolver =
            Arc::new(move |path| route::match_params(&routes_for_params, path));
        let store = store::choose_store::<Model, Msg>(&store_kind, &store_path, live_ttl()).await;
        let state = LiveState {
            store,
            init: Arc::new(init),
            update: Arc::new(update),
            view: Arc::new(view),
            subs: Arc::new(subscriptions),
            route_resolver: resolver,
            param_resolver,
            session_count: Arc::new(AtomicUsize::new(0)),
        };
        serve_live(state).await
    })
}

/// Shared server setup for `live_app` / `live_app_routed`: nested HTTP
/// handlers (`page` / `sse_handler` / `event_handler`), router + bind/serve.
/// The only per-entry difference (the `route_resolver`) lives on `state`.
async fn serve_live<E, Model, Msg, FInit, FUpdate, FView, FSubs>(
    state: LiveState<Model, Msg, FInit, FUpdate, FView, FSubs>,
) -> SkyResult<E, ()>
where
    E: From<String> + Send + 'static,
    Model: Clone + PartialEq + Send + 'static,
    // Debug: forwarded to drive_session for the sky_live_msg_seconds{name} label.
    Msg: Clone + Send + std::fmt::Debug + 'static,
    FInit: Fn(req::LiveReq) -> (Model, SkyCmd<Msg>) + Send + Sync + 'static,
    FUpdate: Fn(Msg, Model) -> (Model, SkyCmd<Msg>) + Send + Sync + 'static,
    FView: Fn(Model) -> Html<Msg> + Send + Sync + 'static,
    FSubs: Fn(Model) -> SkySub<Msg> + Send + Sync + 'static,
{
    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};

    {
        // ── GET page (root + any path) ────────────────────────────────────
        async fn page<Model, Msg, FInit, FUpdate, FView, FSubs>(
            State(st): State<LiveState<Model, Msg, FInit, FUpdate, FView, FSubs>>,
            method: axum::http::Method,
            uri: axum::http::Uri,
            headers: axum::http::HeaderMap,
        ) -> Response
        where
            Model: Clone + PartialEq + Send + 'static,
            // Debug: the GET handler creates a session and spawns drive_session,
            // which needs the bound for the sky_live_msg_seconds{name} label.
            Msg: Clone + Send + std::fmt::Debug + 'static,
            FInit: Fn(req::LiveReq) -> (Model, SkyCmd<Msg>) + Send + Sync + 'static,
            FUpdate: Fn(Msg, Model) -> (Model, SkyCmd<Msg>) + Send + Sync + 'static,
            FView: Fn(Model) -> Html<Msg> + Send + Sync + 'static,
            FSubs: Fn(Model) -> SkySub<Msg> + Send + Sync + 'static,
        {
            // Cookie-based session lifecycle (Go store.Get on every GET):
            //   * Live hit  → reuse the in-process session; re-apply routing for
            //                 this GET's path + re-render (no new driver).
            //   * Cold hit  → a persisted model (post-restart / different replica);
            //                 hydrate a fresh driver seeded with it (no init).
            //   * miss      → init a new session.
            let cookie_sid = sid_from_cookie(&headers);
            // CSRF double-submit token: reuse the browser's existing well-formed
            // `__sky_csrf` cookie (so a reload keeps the same token), else mint a
            // fresh one. page_response sets the cookie + injects the value into
            // the page JS; the client echoes it back in the X-Sky-Csrf header.
            let csrf_tok = csrf::cookie_value(&headers, csrf::csrf_cookie_name())
                .filter(|t| csrf::token_is_well_formed(t))
                .unwrap_or_else(csrf::gen_token);
            let hit = match cookie_sid.as_ref() {
                Some(s) => st.store.get(s).await.map(|h| (s.clone(), h)),
                None => None,
            };

            let (sid, model, cmd0) = match hit {
                Some((sid, store::StoreHit::Live(handle))) => {
                    // sid is carried from the cookie lookup; the "hit but no sid"
                    // state is unrepresentable.
                    let body = {
                        let mut e = handle.lock().unwrap_or_else(|e| e.into_inner());
                        e.model = (st.route_resolver)(e.model.clone(), uri.path());
                        let mut tree = (st.view)(e.model.clone());
                        assign_sky_ids(&mut tree, "r");
                        style_inject::apply_style_injections(&mut tree);
                        e.index = build_index(&tree);
                        e.last_view = tree.clone();
                        render_html(&tree)
                    };
                    st.store.set(&sid, handle).await; // touch last-seen
                    return page_response(&sid, &body, &csrf_tok);
                }
                Some((sid, store::StoreHit::Cold(m))) => {
                    // A returning user with a valid sid cookie → not new attack
                    // volume, so NOT rejected; but count its driver so the slot it
                    // gets below is paired (decremented on the driver's exit).
                    st.session_count.fetch_add(1, Ordering::SeqCst);
                    (sid, (st.route_resolver)(m, uri.path()), SkyCmd::None)
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
            assign_sky_ids(&mut tree, "r");
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

            page_response(&sid, &body, &csrf_tok)
        }

        // ── GET /_sky/sse ─────────────────────────────────────────────────
        async fn sse_handler<Model, Msg, FInit, FUpdate, FView, FSubs>(
            State(st): State<LiveState<Model, Msg, FInit, FUpdate, FView, FSubs>>,
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
                // X-Sky-Live: 1 lets the client distinguish a genuine session-lost
                // 404 (reload to recover) from a wedged proxy (client.js probes for
                // exactly this header — l1481/l1530).
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        [(axum::http::HeaderName::from_static("x-sky-live"), "1")],
                        SESSION_LOST_BODY,
                    )
                        .into_response();
                }
            };

            let (tx, rx) = sse::channel();
            {
                entry.lock().unwrap_or_else(|e| e.into_inner()).sse_tx = Some(tx.clone());
            }

            // Metrics (Go parity: sky_live_sse_connections_total /
            // sky_live_sessions_active). Count the connection and mark the session
            // active; the gauge is decremented when the response body stream is
            // dropped on disconnect (the SessionGauge guard below).
            crate::sky_runtime::telemetry::metric_inc("sky_live_sse_connections_total", &[], 1);
            crate::sky_runtime::telemetry::metric_add_gauge("sky_live_sessions_active", &[], 1);

            // Immediate hello + ~2KB proxy-buffer padding comment, then a 15s
            // heartbeat keepalive (Go parity: live.go SSE handshake).
            let _ = tx
                .send(SsePatch(format!(": {}\n\n", " ".repeat(2048))))
                .await;
            // Go-parity hello payload (live.go ~5486): `{"v":1,"sid":...,"ts":<ms>}`.
            // Reaching here means `entry` exists ⇒ the cookie sid was a live session,
            // so `sid` is Some; the impossible None degrades to an empty sid (the
            // client already holds its sid via window.__SKY_SID — the body is
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
            // restart / `sky watch` rebuild / redeploy paired with a persistent
            // store — has no live subscriptions from the previous process, so
            // nothing pushes until the next user Msg. Render the current view once
            // and ship it as a full-body `event: patch` frame; the client consumes
            // `{seq, body}` → __skyPatch full replace (client.js:1318). No globalSeq
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
                    crate::sky_runtime::telemetry::metric_add_gauge(
                        "sky_live_sessions_active",
                        &[],
                        -1,
                    );
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

        // ── POST /_sky/event ──────────────────────────────────────────────
        async fn event_handler<Model, Msg, FInit, FUpdate, FView, FSubs>(
            State(st): State<LiveState<Model, Msg, FInit, FUpdate, FView, FSubs>>,
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
                        [(axum::http::HeaderName::from_static("x-sky-live"), "1")],
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
                // X-Sky-Live: 1 lets the client distinguish a genuine session-lost
                // 404 (reload to recover) from a wedged proxy (client.js probes for
                // exactly this header — l1481/l1530).
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        [(axum::http::HeaderName::from_static("x-sky-live"), "1")],
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
                        "[sky.live] event_handler: session msg queue full or closed; dropping event ({})",
                        e
                    );
                    return (StatusCode::TOO_MANY_REQUESTS, "event queue full").into_response();
                }
            }
            // Real patches flow over SSE from the driver; ack with an empty list.
            // X-Sky-Live: 1 marks this as a genuine Sky.Live response (the client
            // treats a 200 WITHOUT it as a wedged-proxy signal).
            (
                StatusCode::OK,
                [
                    (axum::http::header::CONTENT_TYPE, "application/json"),
                    (axum::http::HeaderName::from_static("x-sky-live"), "1"),
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
        // SKY_CONSOLE_DB_PATH is set — the console child reads it via the
        // hub kernels. db-gated; a no-op for live-without-db apps. Enabled
        // BEFORE the console child spawns so early telemetry lands in the spill
        // the child will read.
        #[cfg(feature = "db")]
        crate::sky_runtime::telemetry_spill::enable_from_env().await;

        // Observability export pipelines: federation push to a parent ingest
        // (SKY_PARENT_URL) and remote-hub OTLP push (SKY_CONSOLE_HUB).
        // Both env-gated + inert by default.
        push_exporter::enable_from_env().await;
        hub_exporter::enable_from_env().await;

        // Console precedence: try the pre-built console child +
        // reverse-proxy; fall back to the in-process console when the binary is
        // absent / spawn fails / readiness times out / the gate is closed.
        // Decided HERE (before the router is built) so both the proxy routes and
        // the in-process console routes sit under the same `track` middleware,
        // and the two never collide on `/_sky/console`.
        let use_console_proxy = console_proxy::ensure_console_proxy().await;

        // Body-size cap on /_sky/event: mirrors Go's http.MaxBytesReader
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
        let client_js_route_path = client_js_path(); // e.g. "/_sky/client.a1b2c3d4e5f6a7b8.js"
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

        let mut router = Router::new()
            .route(
                "/_sky/sse",
                get(sse_handler::<Model, Msg, FInit, FUpdate, FView, FSubs>),
            )
            .route("/_sky/event", event_route)
            .route(&client_js_route_path, get(serve_client_js))
            // Observability surface (Go parity — observability.go).
            .route("/_sky/healthz", get(observability::healthz))
            .route("/_sky/readyz", get(observability::readyz))
            .route("/_sky/buildinfo", get(observability::buildinfo))
            .route("/_sky/metrics", get(observability::metrics))
            // Observability federation receiver stays on the parent regardless
            // of console mode (sub-apps push telemetry here). Body-capped (reuses
            // the /_sky/event limit) so an unbounded ingest POST can't exhaust
            // memory before the JSON parse.
            .route(
                "/_sky/observability/ingest",
                post(console::ingest)
                    .layer(axum::extract::DefaultBodyLimit::max(live_max_body_bytes())),
            );

        router = if use_console_proxy {
            // Real bundled Sky.Live console, spawned as a child + proxied. The
            // child process logs its OWN `session store: …` line + a
            // `reverse-proxy ready` line (console_proxy), so the parent does not
            // duplicate the inline-mount log here.
            console_proxy::proxy_routes(router)
        } else {
            // In-process console (plain-HTML shell + JSON APIs). Go mounts the
            // console as an in-process Sky.Live sub-app that inits its OWN session
            // store, so Go logs the memory-store line TWICE (root + console) and
            // then the inline-mount line (console.go:328). The Rust in-process
            // console has no separate store, so we emit the matching SECOND store
            // line + the mount line here — but ONLY when the console actually
            // mounts (gate open: not a sub-app, not `SKY_CONSOLE_AUTH=off`, not
            // production-without-admin-token), mirroring Go's mount skip.
            if console_proxy::gate_allows() {
                eprintln!("{}", store::memory_store_log_line(live_ttl()));
                eprintln!(
                    "[sky.console] inline console mounted as Sky.Live sub-app at /_sky/console mode={}",
                    console::console_auth_mode_label()
                );
            }
            router
                .route("/_sky/console", get(console::console_html))
                .route("/_sky/console/api/overview", get(console::api_overview))
                .route("/_sky/console/api/logs", get(console::api_logs))
                .route("/_sky/console/api/errors", get(console::api_errors))
                .route("/_sky/console/api/traces", get(console::api_traces))
                .route(
                    "/_sky/console/api/metrics-summary",
                    get(console::api_metrics_summary),
                )
        };

        // sky.toml `[live] static` (baked as SKY_LIVE_STATIC_DIR) → serve files at
        // /static/* via ServeDir (Go parity: live.go staticURL "/static"). MUST be
        // added before the `/*path` page catch-all so a /static/<file> request hits
        // ServeDir, not the page handler (which would return HTML). ServeDir blocks
        // `..` path traversal by construction (percent-decodes first, so `%2e%2e` is
        // caught too). NOTE: like Go's http.FileServer it FOLLOWS symlinks inside the
        // dir — the dir is author-controlled (sky.toml [live] static), so that is the
        // intended contract + Go-parity, NOT a confinement guarantee. Absent/empty →
        // no static mount.
        if let Some(dir) = crate::sky_runtime::system::read_env_var("SKY_LIVE_STATIC_DIR")
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
            // rt.go:3463 etc.). Symmetric with Sky.Http.Server (server.rs). The
            // Rust thesis is that well-typed Sky can't panic, so this is the
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
            // Symmetric with Sky.Http.Server (the body shape is shared in `core`;
            // the Live router can't reference `server.rs` — a Live-only generated
            // project doesn't include it).
            .layer(tower_http::catch_panic::CatchPanicLayer::custom(
                |err: Box<dyn std::any::Any + Send + 'static>| {
                    use axum::response::IntoResponse;
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        crate::sky_runtime::core::panic_500_body(&*err),
                    )
                        .into_response()
                },
            ))
            .layer(axum::middleware::from_fn(observability::track))
            .with_state(state);

        pubsub::mark_live_running();

        let port: i64 = crate::sky_runtime::system::read_env_var("SKY_LIVE_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8000);
        let addr = format!("0.0.0.0:{port}");
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => return SkyResult::Err(format!("Live.app: bind {addr}: {e}").into()),
        };
        // Bind-address line (stderr, Rust-specific — carries the 0.0.0.0 bind).
        eprintln!("[sky.live] listening on http://{addr}");
        // Go-parity user-facing line (stdout, `fmt.Printf("Sky.Live listening on
        // :%d\n", port)` — live.go:3546).
        println!("Sky.Live listening on :{port}");
        // Graceful shutdown (Go parity — live.go:3503): trap SIGINT/SIGTERM,
        // print the shutdown line, drain in-flight requests, and return cleanly so
        // the SkyTask resolves Ok → the generated entry exits 0 (NOT 130). A
        // SECOND signal force-exits 130 via the watchdog inside live_shutdown_signal.
        match axum::serve(listener, app)
            .with_graceful_shutdown(live_shutdown_signal())
            .await
        {
            Ok(()) => ok_res(()),
            Err(e) => SkyResult::Err(format!("Live.app: serve: {e}").into()),
        }
    }
}

/// Read the session cookie from request headers. Uses the base-path-aware
/// cookie name (`session_cookie_name`) so a sub-app reads its own scoped cookie,
/// never the parent's `sky_sid`.
fn sid_from_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    let name = session_cookie_name();
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for c in raw.split(';') {
        let c = c.trim();
        if let Some((k, v)) = c.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

// The Std.Html `Ffi.callPure "htmlXxx"` kernel wrappers (html_render_,
// html_escape_text_, html_escape_attr_, html_attr_to_string_) now live in the
// standalone top-level `sky_runtime::html` module (re-exported here via
// `use super::*`), so a non-Live Std.Html / Std.Ui render doesn't pull this
// server module in.

#[cfg(test)]
mod dev_banner_tests {
    use super::dev_console_banner;

    #[test]
    fn banner_byte_matches_go_dev_banner_markup() {
        // Go parity (dev_banner.go devBannerHTML): same id, target/rel/title,
        // monospace blue style, `&#128269;` ENTITY (not a literal emoji).
        let b = dev_console_banner("");
        let expected = "<a id=\"__sky-dev-console\" href=\"/_sky/console\" target=\"_blank\" \
            rel=\"noopener\" title=\"Sky Console (dev only)\" \
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
        assert_eq!(dev_console_banner("/_sky/console"), "");
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
        assert_eq!(parse_duration_secs("45m"), Some(2700)); // the e2e check (SKY_LIVE_TTL=45m)
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
        assert_eq!(normalise_base_path("/_sky/console"), "/_sky/console");
        assert_eq!(normalise_base_path("/_sky/console/"), "/_sky/console");
        assert_eq!(normalise_base_path("_sky/console"), "/_sky/console");
        assert_eq!(normalise_base_path("  /billing/  "), "/billing");
    }

    #[test]
    fn cookie_name_is_sky_sid_at_root_distinct_under_base() {
        assert_eq!(cookie_name_for(""), "sky_sid");
        // Distinct from the parent's `sky_sid` so the proxied child can't clobber it.
        assert_eq!(cookie_name_for("/_sky/console"), "sky_sid__sky_console");
        assert_ne!(cookie_name_for("/_sky/console"), "sky_sid");
    }

    #[test]
    fn cookie_path_scopes_to_base() {
        assert_eq!(cookie_path_for(""), "/");
        // Scoped → the cookie is never sent to the parent's own routes.
        assert_eq!(cookie_path_for("/_sky/console"), "/_sky/console");
    }

    #[test]
    fn render_page_threads_base_into_meta_and_window_global() {
        let root = render_page_full("sid1", "", "<b>x</b>", "deadbeef");
        assert!(root.contains("<meta name=\"sky-base\" content=\"\">"));
        assert!(root.contains("window.__SKY_BASE=\"\""));

        let sub = render_page_full("sid1", "/_sky/console", "<b>x</b>", "deadbeef");
        assert!(sub.contains("<meta name=\"sky-base\" content=\"/_sky/console\">"));
        assert!(sub.contains("window.__SKY_BASE=\"/_sky/console\""));
    }

    #[test]
    fn render_page_emits_external_client_script_with_sri() {
        let root = render_page_full("sid1", "", "<b>x</b>", "tok1");
        // Per-session values stay inline.
        assert!(root.contains("window.__SKY_SID=\"sid1\""));
        assert!(root.contains("window.__SKY_CSRF_TOKEN=\"tok1\""));
        // CLIENT_JS body must NOT be inlined.
        assert!(!root.contains("var __skySid = window.__SKY_SID"));
        // External script tag with content-addressed src.
        assert!(root.contains("<script src=\"/_sky/client."));
        assert!(root.contains(".js\" integrity=\"sha256-"));
        assert!(root.contains("crossorigin=\"anonymous\">"));
        // SRI attribute is present and non-empty.
        assert!(root.contains("integrity=\"sha256-"));
    }

    #[test]
    fn render_page_sub_app_prefixes_client_src() {
        let sub = render_page_full("sid1", "/_sky/console", "<b>x</b>", "tok1");
        // External script src must carry the base prefix.
        assert!(root_or_sub_has_prefixed_client_src(&sub, "/_sky/console"));
    }

    fn root_or_sub_has_prefixed_client_src(html: &str, base: &str) -> bool {
        // Find `<script src="` and check the src starts with `base/_sky/client.`
        let needle = format!("<script src=\"{}/_sky/client.", base);
        html.contains(&needle)
    }

    #[test]
    fn client_js_path_is_content_addressed_and_stable() {
        let p1 = client_js_path();
        let p2 = client_js_path();
        // Same result on repeated calls (OnceLock).
        assert_eq!(p1, p2);
        // Path format: /_sky/client.<16 hex chars>.js
        assert!(p1.starts_with("/_sky/client."));
        assert!(p1.ends_with(".js"));
        let hash_part = p1
            .trim_start_matches("/_sky/client.")
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
    //! `client.js` `__skyProbeSessionLost` reloads the page when its probe
    //! POST to `/_sky/event` gets a 404 + `X-Sky-Live: 1` whose body CONTAINS
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
            [(HeaderName::from_static("x-sky-live"), "1")],
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
        let app = Router::new().route("/_sky/event", post(no_session_event_handler));

        let req = Request::builder()
            .method("POST")
            .uri("/_sky/event")
            .body(Body::from("{}"))
            .expect("build probe request");

        let resp = app.oneshot(req).await.expect("router responds");

        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "must be a 404");
        assert_eq!(
            resp.headers()
                .get("x-sky-live")
                .expect("x-sky-live header present")
                .to_str()
                .expect("ascii header value"),
            "1",
            "X-Sky-Live marker distinguishes session-lost from a wedged proxy"
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
        std::env::remove_var("SKY_LIVE_MAX_SESSIONS");
        assert_eq!(max_sessions(), 50_000);
        std::env::set_var("SKY_LIVE_MAX_SESSIONS", "7");
        assert_eq!(max_sessions(), 7);
        std::env::set_var("SKY_LIVE_MAX_SESSIONS", "0");
        assert_eq!(max_sessions(), 0, "0 = unlimited opt-out");
        std::env::set_var("SKY_LIVE_MAX_SESSIONS", "garbage");
        assert_eq!(max_sessions(), 50_000, "unparseable falls back to default");
        std::env::remove_var("SKY_LIVE_MAX_SESSIONS");
    }
}
