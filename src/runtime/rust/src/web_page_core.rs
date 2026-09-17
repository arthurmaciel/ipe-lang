//! The server-free HTML page scaffold shared by every render host.
//!
//! `page_shell` and its `BASE_CSS` reset are pure `format!` over compile-time
//! literals — no axum, no session, no server dependency — so they belong to the
//! render core, not the HTTP `web` server. The full `web` module re-exports
//! `page_shell` from here, and the lean render-core `web` shell (native
//! `webview` + browser-WASM `wasm-client`) does too, so the ONE definition backs
//! every host (single source of truth).

/// Minimal CSS reset injected into every rendered page.
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

/// Shared HTML page scaffold used by every render path.
///
/// Emits, in order:
///   1. Standard HTML5 boilerplate (`<!DOCTYPE html><html>`).
///   2. A `<head>` containing:
///      - `<meta charset="utf-8">` (character encoding, always first).
///      - `<meta name="viewport" …>` (full-bleed on mobile and native webview).
///      - `<style>{BASE_CSS}</style>` (the compile-time reset; no user data).
///      - `head_extra` — any additional per-backend head content (empty string
///        for backends that need none).
///   3. `<body>{body_inner}</body>` — the pre-rendered HTML body.
///   4. `tail_scripts` — `<script>` tags appended after `</body>` (empty string
///      for backends that carry no scripts).
///
/// `body_inner` must already be HTML-escaped (produced by `render_html`).
/// `head_extra` and `tail_scripts` are compile-time or session-derived
/// literals assembled by the caller; no user-controlled text reaches them.
#[must_use]
pub fn page_shell(head_extra: &str, body_inner: &str, tail_scripts: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head>\
         <meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <style>{BASE_CSS}</style>\
         {head_extra}\
         </head>\
         <body>{body_inner}</body>\
         {tail_scripts}\
         </html>"
    )
}
