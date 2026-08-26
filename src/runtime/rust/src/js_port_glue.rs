//! The browser `Ipe.Js` port surface, served content-addressed with SRI.
//!
//! One ES module exposes the developer-facing `window.ipe` port object — the
//! IDENTICAL surface on the server-driven and the browser-WASM target:
//!
//! * `ipe.send(value)` — hand a JSON value to the Ipê program's inbound port.
//!   The value is stringified once and delivered to the runtime's in-channel
//!   (`js_subscribe` decodes it fail-closed).
//! * `ipe.onReceive(fn)` — register the handler the runtime calls with each
//!   outbound port frame a `js_send` produced (the frame is the canonical seal
//!   wire string; the handler parses it).
//! * `ipe.onSync(fn)` — alias of `onReceive` for a page that reads the outbound
//!   frame as a state sync rather than a message; one registration slot, so a
//!   page never wires two conflicting receivers.
//!
//! The module is addressed by the hash of its own bytes (`/_ipe/port.<hex16>.js`)
//! and pinned with an `integrity="sha256-<b64>"` SRI attribute — the same
//! content-addressing `widget_assets` uses, so a tampered byte makes the browser
//! refuse the module. The bytes are a fixed asset (no user input is ever spliced
//! in), so the address and SRI are constants of the build.

use sha2::{Digest, Sha256};

/// The browser port glue. A fixed ES module — no user input is ever interpolated,
/// so its bytes (and therefore its address and SRI) are constant per build.
///
/// `window.ipeOnReceive` is the single slot the runtime's outbound delivery
/// calls; `window.ipe.send` funnels an inbound value to `window.__ipePortSend`,
/// the seam the host page/runtime installs. Values cross only as JSON strings,
/// parsed as data (`JSON.parse` / `JSON.stringify`), never `eval`.
const PORT_GLUE_JS: &str = r#"// Ipe.Js browser port surface. Values cross as JSON strings only.
(function () {
  var onReceive = null;
  function deliver(raw) {
    if (typeof onReceive === "function") {
      try { onReceive(JSON.parse(raw)); } catch (_e) { /* drop a malformed frame */ }
    }
  }
  // The runtime calls this with each outbound seal frame (a JSON string).
  window.ipeOnReceive = deliver;
  window.ipe = {
    send: function (value) {
      var raw;
      try { raw = JSON.stringify(value); } catch (_e) { return; }
      if (typeof window.__ipePortSend === "function") {
        window.__ipePortSend(raw);
      }
    },
    onReceive: function (fn) { onReceive = fn; },
    onSync: function (fn) { onReceive = fn; },
  };
})();
"#;

/// The full 32-byte SHA-256 digest of the glue bytes.
fn digest() -> [u8; 32] {
    Sha256::digest(PORT_GLUE_JS.as_bytes()).into()
}

/// The glue module's source bytes.
#[must_use]
pub fn port_glue_js() -> &'static str {
    PORT_GLUE_JS
}

/// The content-addressed URL PATH (no base prefix) for the port glue asset,
/// `/_ipe/port.<hex16>.js`. Stable for given bytes; changes when the bytes
/// change — making `Cache-Control: immutable` safe.
#[must_use]
pub fn port_glue_path() -> String {
    let d = digest();
    let hex16: String = d[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("/_ipe/port.{hex16}.js")
}

/// The `sha256-<b64>` SRI value for the port glue asset.
#[must_use]
pub fn port_glue_integrity() -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
    format!("sha256-{}", B64.encode(digest()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_is_content_addressed_and_stable() {
        let p = port_glue_path();
        assert!(p.starts_with("/_ipe/port."));
        assert!(p.ends_with(".js"));
        assert_eq!(p, port_glue_path()); // deterministic
    }

    #[test]
    fn integrity_is_a_sha256_sri() {
        let i = port_glue_integrity();
        assert!(i.starts_with("sha256-"));
        assert_eq!(i, port_glue_integrity()); // deterministic
    }

    #[test]
    fn surface_exposes_send_receive_sync() {
        let js = port_glue_js();
        assert!(js.contains("window.ipe"));
        assert!(js.contains("send:"));
        assert!(js.contains("onReceive:"));
        assert!(js.contains("onSync:"));
        assert!(js.contains("window.ipeOnReceive"));
        // Values cross as JSON only — never eval.
        assert!(js.contains("JSON.parse"));
        assert!(js.contains("JSON.stringify"));
        assert!(!js.contains("eval("));
    }
}
