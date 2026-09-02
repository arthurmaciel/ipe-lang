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
  // Return an inbound typed frame to the Ipê program: a decoded intent, never a
  // thrown error, so a host permission denial is an ordinary case the program's
  // subscription decodes (parse-don't-validate at the trust boundary). Each frame
  // carries a `tag` naming its first-party sink so a module's inbound decoder
  // selects only its own frames; unknown tags simply fail that decoder closed.
  function reply(frame) {
    if (typeof window.__ipePortSend === "function") {
      try { window.__ipePortSend(JSON.stringify(frame)); }
      catch (_e) { /* best-effort inbound reply */ }
    }
  }
  // First-party Ipe.Browser.* sinks. Each recognises its own closed outbound
  // command shape and reaches exactly one Web API, trapping any host denial /
  // unavailability / timeout to a typed inbound frame (never a panic, never a
  // throw). The bytes are stdlib's and SRI-pinned, so a dependency cannot
  // substitute them.
  function clipboardSink(value) {
    // Ipe.Browser.Clipboard: `WriteText text` -> navigator.clipboard.writeText.
    if (value && typeof value === "object" && typeof value.WriteText === "string") {
      var text = value.WriteText;
      if (!navigator || !navigator.clipboard || typeof navigator.clipboard.writeText !== "function") {
        reply({ tag: "clipboard", event: "write", ok: false, error: "unavailable" });
        return true;
      }
      navigator.clipboard.writeText(text).then(
        function () { reply({ tag: "clipboard", event: "write", ok: true }); },
        function () { reply({ tag: "clipboard", event: "write", ok: false, error: "denied" }); }
      );
      return true;
    }
    // Ipe.Browser.Clipboard: `ReadText` -> navigator.clipboard.readText. The
    // nullary variant is externally tagged as the bare string "ReadText".
    if (value === "ReadText") {
      if (!navigator || !navigator.clipboard || typeof navigator.clipboard.readText !== "function") {
        reply({ tag: "clipboard", event: "read", ok: false, error: "unavailable" });
        return true;
      }
      navigator.clipboard.readText().then(
        function (text) { reply({ tag: "clipboard", event: "read", ok: true, text: String(text) }); },
        function () { reply({ tag: "clipboard", event: "read", ok: false, error: "denied" }); }
      );
      return true;
    }
    return false;
  }
  // Ipe.Browser.Geolocation: `Current opts` / `Watch opts` / `ClearWatch id`
  // -> navigator.geolocation.getCurrentPosition / watchPosition / clearWatch. A
  // position resolves to a typed `Coords` frame; a host denial, position
  // unavailability, or timeout traps to the matching typed error frame — the
  // three inbound error variants the module enumerates exhaustively.
  var geoWatchId = null;
  function geoOptions(opts) {
    // The Ipê `Options` record crosses as `{ enableHighAccuracy, timeout,
    // maximumAge }` (milliseconds). A non-positive `timeout` means "no deadline"
    // -> Infinity; a negative `maximumAge` is clamped to a fresh fix (0).
    var o = (opts && typeof opts === "object") ? opts : {};
    return {
      enableHighAccuracy: o.enableHighAccuracy === true,
      timeout: (typeof o.timeout === "number" && o.timeout > 0) ? o.timeout : Infinity,
      maximumAge: (typeof o.maximumAge === "number" && o.maximumAge > 0) ? o.maximumAge : 0,
    };
  }
  function geoOnPosition(pos) {
    var c = pos && pos.coords ? pos.coords : {};
    reply({
      tag: "geolocation", ok: true,
      lat: Number(c.latitude), lng: Number(c.longitude), accuracy: Number(c.accuracy),
    });
  }
  function geoOnError(err) {
    // PERMISSION_DENIED=1, POSITION_UNAVAILABLE=2, TIMEOUT=3 — mapped to the
    // module's closed inbound error vocabulary; anything else is unavailable.
    var code = err && typeof err.code === "number" ? err.code : 2;
    var kind = code === 1 ? "denied" : (code === 3 ? "timeout" : "unavailable");
    reply({ tag: "geolocation", ok: false, error: kind });
  }
  function geolocationSink(value) {
    // An outbound `JsCmd` is externally tagged: a payload-carrying variant is
    // `{ Current: opts }` / `{ Watch: opts }`; the nullary `ClearWatch` is the
    // bare string `"ClearWatch"`.
    var isClear = value === "ClearWatch" ||
      (value && typeof value === "object" && value.ClearWatch !== undefined);
    var isCurrent = value && typeof value === "object" && value.Current !== undefined;
    var isWatch = value && typeof value === "object" && value.Watch !== undefined;
    if (!isCurrent && !isWatch && !isClear) return false;
    if (isClear) {
      if (geoWatchId !== null && navigator && navigator.geolocation &&
          typeof navigator.geolocation.clearWatch === "function") {
        navigator.geolocation.clearWatch(geoWatchId);
      }
      geoWatchId = null;
      return true;
    }
    if (!navigator || !navigator.geolocation) {
      reply({ tag: "geolocation", ok: false, error: "unavailable" });
      return true;
    }
    var opts = geoOptions(isCurrent ? value.Current : value.Watch);
    if (isCurrent) {
      if (typeof navigator.geolocation.getCurrentPosition !== "function") {
        reply({ tag: "geolocation", ok: false, error: "unavailable" });
        return true;
      }
      navigator.geolocation.getCurrentPosition(geoOnPosition, geoOnError, opts);
    } else {
      if (typeof navigator.geolocation.watchPosition !== "function") {
        reply({ tag: "geolocation", ok: false, error: "unavailable" });
        return true;
      }
      geoWatchId = navigator.geolocation.watchPosition(geoOnPosition, geoOnError, opts);
    }
    return true;
  }
  function builtinSink(value) {
    if (clipboardSink(value)) return true;
    if (geolocationSink(value)) return true;
    return false;
  }
  function deliver(raw) {
    var value;
    try { value = JSON.parse(raw); } catch (_e) { return; /* drop a malformed frame */ }
    // A first-party browser-capability command is handled by its built-in sink;
    // anything else is a developer port frame routed to the registered receiver.
    if (builtinSink(value)) return;
    if (typeof onReceive === "function") { onReceive(value); }
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

    #[test]
    fn clipboard_sink_reaches_the_web_api_and_traps_denial_to_a_typed_result() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.Clipboard sink reaches its two Web APIs…
        assert!(js.contains("navigator.clipboard.writeText"));
        assert!(js.contains("navigator.clipboard.readText"));
        assert!(js.contains("WriteText"));
        assert!(js.contains("ReadText"));
        // …and traps a host denial / absence to a typed inbound frame, never a
        // throw: both the denied and the unavailable branches reply, no `eval`.
        assert!(js.contains("\"denied\""));
        assert!(js.contains("\"unavailable\""));
        assert!(js.contains("tag: \"clipboard\""));
        assert!(js.contains("reply("));
    }

    #[test]
    fn geolocation_sink_reaches_the_web_api_and_enumerates_the_three_error_variants() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.Geolocation sink reaches its Web APIs…
        assert!(js.contains("navigator.geolocation.getCurrentPosition"));
        assert!(js.contains("navigator.geolocation.watchPosition"));
        assert!(js.contains("navigator.geolocation.clearWatch"));
        assert!(js.contains("Current"));
        assert!(js.contains("Watch"));
        // …resolves a position to a typed Coords frame…
        assert!(js.contains("tag: \"geolocation\""));
        assert!(js.contains("lat:"));
        assert!(js.contains("lng:"));
        assert!(js.contains("accuracy:"));
        // …and enumerates all three inbound error variants, never a throw / eval.
        assert!(js.contains("\"denied\""));
        assert!(js.contains("\"unavailable\""));
        assert!(js.contains("\"timeout\""));
        assert!(!js.contains("eval("));
    }
}
