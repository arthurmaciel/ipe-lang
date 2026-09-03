//! The browser `Ipe.Ffi.Js` port surface, served content-addressed with SRI.
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
const PORT_GLUE_JS: &str = r#"// Ipe.Ffi.Js browser port surface. Values cross as JSON strings only.
(function () {
  var onReceive = null;
  // Return an inbound typed frame to the Ipê program: a decoded intent, never a
  // thrown error, so a host permission denial is an ordinary case the program's
  // subscription decodes (parse-don't-validate at the trust boundary). Each frame
  // carries a `tag` naming its first-party sink so a module's inbound decoder
  // selects only its own frames; unknown tags simply fail that decoder closed.
  // When `corId` is non-null, the runtime-private `__ipe_id` field is added so
  // the runtime routes this reply to the matching one-shot waiter rather than
  // broadcasting it to `js_subscribe` subscribers.
  function reply(frame, corId) {
    if (corId !== null && corId !== undefined) {
      frame.__ipe_id = corId;
    }
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
  function clipboardSink(value, corId) {
    // Ipe.Browser.Clipboard: `WriteText text` -> navigator.clipboard.writeText.
    if (value && typeof value === "object" && typeof value.WriteText === "string") {
      var text = value.WriteText;
      if (!navigator || !navigator.clipboard || typeof navigator.clipboard.writeText !== "function") {
        reply({ tag: "clipboard", event: "write", ok: false, error: "unavailable" }, corId);
        return true;
      }
      navigator.clipboard.writeText(text).then(
        function () { reply({ tag: "clipboard", event: "write", ok: true }, corId); },
        function () { reply({ tag: "clipboard", event: "write", ok: false, error: "denied" }, corId); }
      );
      return true;
    }
    // Ipe.Browser.Clipboard: `ReadText` -> navigator.clipboard.readText. The
    // nullary variant is externally tagged as the bare string "ReadText".
    if (value === "ReadText") {
      if (!navigator || !navigator.clipboard || typeof navigator.clipboard.readText !== "function") {
        reply({ tag: "clipboard", event: "read", ok: false, error: "unavailable" }, corId);
        return true;
      }
      navigator.clipboard.readText().then(
        function (text) { reply({ tag: "clipboard", event: "read", ok: true, text: String(text) }, corId); },
        function () { reply({ tag: "clipboard", event: "read", ok: false, error: "denied" }, corId); }
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
  var geoWatchIds = [];
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
  function makeGeoCallbacks(corId) {
    return {
      onPosition: function(pos) {
        var c = pos && pos.coords ? pos.coords : {};
        reply({
          tag: "geolocation", ok: true,
          lat: Number(c.latitude), lng: Number(c.longitude), accuracy: Number(c.accuracy),
        }, corId);
      },
      onError: function(err) {
        // PERMISSION_DENIED=1, POSITION_UNAVAILABLE=2, TIMEOUT=3 — mapped to the
        // module's closed inbound error vocabulary; anything else is unavailable.
        var code = err && typeof err.code === "number" ? err.code : 2;
        var kind = code === 1 ? "denied" : (code === 3 ? "timeout" : "unavailable");
        reply({ tag: "geolocation", ok: false, error: kind }, corId);
      },
    };
  }
  function geolocationSink(value, corId) {
    // An outbound `JsCmd` is externally tagged: a payload-carrying variant is
    // `{ Current: opts }` / `{ Watch: opts }`; the nullary `ClearWatch` is the
    // bare string `"ClearWatch"`.
    var isClear = value === "ClearWatch" ||
      (value && typeof value === "object" && value.ClearWatch !== undefined);
    var isCurrent = value && typeof value === "object" && value.Current !== undefined;
    var isWatch = value && typeof value === "object" && value.Watch !== undefined;
    if (!isCurrent && !isWatch && !isClear) return false;
    if (isClear) {
      if (geoWatchIds.length > 0 && navigator && navigator.geolocation &&
          typeof navigator.geolocation.clearWatch === "function") {
        for (var i = 0; i < geoWatchIds.length; i++) {
          navigator.geolocation.clearWatch(geoWatchIds[i]);
        }
      }
      geoWatchIds = [];
      return true;
    }
    if (!navigator || !navigator.geolocation) {
      reply({ tag: "geolocation", ok: false, error: "unavailable" }, corId);
      return true;
    }
    var opts = geoOptions(isCurrent ? value.Current : value.Watch);
    var cbs = makeGeoCallbacks(corId);
    if (isCurrent) {
      if (typeof navigator.geolocation.getCurrentPosition !== "function") {
        reply({ tag: "geolocation", ok: false, error: "unavailable" }, corId);
        return true;
      }
      navigator.geolocation.getCurrentPosition(cbs.onPosition, cbs.onError, opts);
    } else {
      if (typeof navigator.geolocation.watchPosition !== "function") {
        reply({ tag: "geolocation", ok: false, error: "unavailable" }, corId);
        return true;
      }
      geoWatchIds.push(navigator.geolocation.watchPosition(cbs.onPosition, cbs.onError, opts));
    }
    return true;
  }
  // Ipe.Browser.Notification: `RequestPermission` / `Show { title, body, tag }`
  // -> Notification.requestPermission / new Notification. A grant resolves to a
  // typed `granted` frame; a denial or an absent API traps to the matching typed
  // error frame — never a throw.
  function notificationSink(value, corId) {
    // `RequestPermission` is the bare string; `Show` is `{ Show: options }`.
    var isRequest = value === "RequestPermission" ||
      (value && typeof value === "object" && value.RequestPermission !== undefined);
    var isShow = value && typeof value === "object" && value.Show !== undefined;
    if (!isRequest && !isShow) return false;
    if (typeof Notification === "undefined") {
      reply({ tag: "notification", ok: false, error: "unavailable" }, corId);
      return true;
    }
    if (isRequest) {
      if (typeof Notification.requestPermission !== "function") {
        reply({ tag: "notification", ok: false, error: "unavailable" }, corId);
        return true;
      }
      Promise.resolve(Notification.requestPermission()).then(
        function (perm) {
          if (perm === "granted") {
            reply({ tag: "notification", ok: true, event: "granted" }, corId);
          } else {
            reply({ tag: "notification", ok: false, error: "denied" }, corId);
          }
        },
        function () { reply({ tag: "notification", ok: false, error: "denied" }, corId); }
      );
      return true;
    }
    // isShow: only display when permission is already granted; otherwise a typed
    // denial, never a silent no-op and never an unsolicited permission prompt.
    var opts = (value.Show && typeof value.Show === "object") ? value.Show : {};
    if (Notification.permission !== "granted") {
      reply({ tag: "notification", ok: false, error: "denied" }, corId);
      return true;
    }
    try {
      var body = typeof opts.body === "string" ? opts.body : "";
      var tag = typeof opts.tag === "string" ? opts.tag : "";
      new Notification(String(opts.title || ""), { body: body, tag: tag });
      reply({ tag: "notification", ok: true, event: "shown" }, corId);
    } catch (_e) {
      reply({ tag: "notification", ok: false, error: "unavailable" }, corId);
    }
    return true;
  }
  // Ipe.Browser.Storage: `Get key` / `Set { key, value }` / `Remove key` / `Clear`
  // -> localStorage.getItem / setItem / removeItem / clear. A read resolves to a
  // typed value frame (value omitted for an absent key); a private-mode quota throw
  // or an absent store traps to `unavailable` — never a throw.
  function storageSink(value, corId) {
    var isGet = value && typeof value === "object" && value.Get !== undefined;
    var isSet = value && typeof value === "object" && value.Set !== undefined;
    var isRemove = value && typeof value === "object" && value.Remove !== undefined;
    var isClear = value === "Clear" ||
      (value && typeof value === "object" && value.Clear !== undefined);
    if (!isGet && !isSet && !isRemove && !isClear) return false;
    var store = null;
    try { store = window.localStorage; } catch (_e) { store = null; }
    if (!store) {
      reply({ tag: "storage", ok: false, error: "unavailable" }, corId);
      return true;
    }
    try {
      if (isGet) {
        var got = store.getItem(String(value.Get));
        if (got === null || got === undefined) {
          reply({ tag: "storage", ok: true, event: "get" }, corId);
        } else {
          reply({ tag: "storage", ok: true, event: "get", value: String(got) }, corId);
        }
      } else if (isSet) {
        var entry = (value.Set && typeof value.Set === "object") ? value.Set : {};
        store.setItem(String(entry.key || ""), String(entry.value || ""));
        reply({ tag: "storage", ok: true, event: "set" }, corId);
      } else if (isRemove) {
        store.removeItem(String(value.Remove));
        reply({ tag: "storage", ok: true, event: "remove" }, corId);
      } else {
        store.clear();
        reply({ tag: "storage", ok: true, event: "clear" }, corId);
      }
    } catch (_e2) {
      reply({ tag: "storage", ok: false, error: "unavailable" }, corId);
    }
    return true;
  }
  // Ipe.Browser.Vibration: `Vibrate ms` / `Pattern [..]` / `Cancel`
  // -> navigator.vibrate(ms) / vibrate([..]) / vibrate(0). A supported actuator
  // replies `vibrated`; an absent API or a rejected request traps to `unavailable`.
  function vibrationSink(value, corId) {
    var isVibrate = value && typeof value === "object" && value.Vibrate !== undefined;
    var isPattern = value && typeof value === "object" && value.Pattern !== undefined;
    var isCancel = value === "Cancel" ||
      (value && typeof value === "object" && value.Cancel !== undefined);
    if (!isVibrate && !isPattern && !isCancel) return false;
    if (!navigator || typeof navigator.vibrate !== "function") {
      reply({ tag: "vibration", ok: false, error: "unavailable" }, corId);
      return true;
    }
    var arg;
    if (isVibrate) {
      arg = Number(value.Vibrate) || 0;
    } else if (isPattern) {
      arg = Array.isArray(value.Pattern) ? value.Pattern.map(function (n) { return Number(n) || 0; }) : [];
    } else {
      arg = 0;
    }
    var accepted = false;
    try { accepted = navigator.vibrate(arg); } catch (_e) { accepted = false; }
    if (accepted) {
      reply({ tag: "vibration", ok: true }, corId);
    } else {
      reply({ tag: "vibration", ok: false, error: "unavailable" }, corId);
    }
    return true;
  }
  // Ipe.Browser.Share: `Share { title, text, url }` -> navigator.share. A
  // completion replies `shared`; a user dismissal (AbortError) traps to
  // `cancelled`, an absent API to `unavailable` — never a throw. Empty payload
  // fields are omitted so the platform sheet only shows populated fields.
  function shareSink(value, corId) {
    if (!(value && typeof value === "object" && value.Share !== undefined)) return false;
    if (!navigator || typeof navigator.share !== "function") {
      reply({ tag: "share", ok: false, error: "unavailable" }, corId);
      return true;
    }
    var p = (value.Share && typeof value.Share === "object") ? value.Share : {};
    var data = {};
    if (typeof p.title === "string" && p.title !== "") data.title = p.title;
    if (typeof p.text === "string" && p.text !== "") data.text = p.text;
    if (typeof p.url === "string" && p.url !== "") data.url = p.url;
    try {
      navigator.share(data).then(
        function () { reply({ tag: "share", ok: true }, corId); },
        function (err) {
          var name = err && err.name ? String(err.name) : "";
          var kind = name === "AbortError" ? "cancelled" : "unavailable";
          reply({ tag: "share", ok: false, error: kind }, corId);
        }
      );
    } catch (_e) {
      reply({ tag: "share", ok: false, error: "unavailable" }, corId);
    }
    return true;
  }
  // Ipe.Browser.Battery: `Query` / `Watch` -> navigator.getBattery(). A reading
  // frame carries charging / level / chargingTime / dischargingTime; a
  // never-ending time (`Infinity`) is normalised to the -1 sentinel. An absent API
  // traps to `unavailable` — never a throw. `Watch` also attaches change listeners
  // that push fresh readings.
  function batteryReadingFrame(bat, corId) {
    function finite(x) {
      var n = Number(x);
      return (isFinite(n)) ? n : -1;
    }
    reply({
      tag: "battery", ok: true,
      charging: bat.charging === true,
      level: Number(bat.level),
      chargingTime: finite(bat.chargingTime),
      dischargingTime: finite(bat.dischargingTime),
    }, corId);
  }
  function batterySink(value, corId) {
    var isQuery = value === "Query" ||
      (value && typeof value === "object" && value.Query !== undefined);
    var isWatch = value === "Watch" ||
      (value && typeof value === "object" && value.Watch !== undefined);
    if (!isQuery && !isWatch) return false;
    if (!navigator || typeof navigator.getBattery !== "function") {
      reply({ tag: "battery", ok: false, error: "unavailable" }, corId);
      return true;
    }
    navigator.getBattery().then(
      function (bat) {
        batteryReadingFrame(bat, corId);
        if (isWatch) {
          // A watch subscription receives every subsequent change as a fresh
          // broadcast reading; the change events carry no correlation id.
          var push = function () { batteryReadingFrame(bat, null); };
          bat.addEventListener("chargingchange", push);
          bat.addEventListener("levelchange", push);
          bat.addEventListener("chargingtimechange", push);
          bat.addEventListener("dischargingtimechange", push);
        }
      },
      function () { reply({ tag: "battery", ok: false, error: "unavailable" }, corId); }
    );
    return true;
  }
  // Ipe.Browser.NetworkInfo: `Query` / `Watch` -> navigator.connection. A reading
  // frame carries effectiveType / downlink / rtt / saveData; an absent API traps to
  // `unavailable` — never a throw. `Watch` attaches a `change` listener that pushes
  // fresh readings.
  function networkInfoReadingFrame(conn, corId) {
    reply({
      tag: "network-info", ok: true,
      effectiveType: typeof conn.effectiveType === "string" ? conn.effectiveType : "",
      downlink: Number(conn.downlink) || 0,
      rtt: Number(conn.rtt) || 0,
      saveData: conn.saveData === true,
    }, corId);
  }
  function networkInfoSink(value, corId) {
    var isQuery = value === "Query" ||
      (value && typeof value === "object" && value.Query !== undefined);
    var isWatch = value === "Watch" ||
      (value && typeof value === "object" && value.Watch !== undefined);
    if (!isQuery && !isWatch) return false;
    var conn = navigator ? (navigator.connection || navigator.mozConnection || navigator.webkitConnection) : null;
    if (!conn) {
      reply({ tag: "network-info", ok: false, error: "unavailable" }, corId);
      return true;
    }
    networkInfoReadingFrame(conn, corId);
    if (isWatch && typeof conn.addEventListener === "function") {
      // A watch subscription receives every subsequent change as a fresh
      // broadcast reading; the change event carries no correlation id.
      conn.addEventListener("change", function () { networkInfoReadingFrame(conn, null); });
    }
    return true;
  }
  // Ipe.Browser.FilePicker: `PickFile` / `PickImage` -> <input type="file">.
  // A transient file-input element is created programmatically; after the user
  // selects a file, `FileReader.readAsDataURL` reads the contents and the full
  // data: URL is returned as the `dataUrl` field. A user cancellation resolves
  // to the typed `cancelled` frame; an absent File API or a read error to
  // `unavailable` — never a thrown error or a silent drop.
  function filePickerSink(value, corId) {
    var isPickFile = value === "PickFile" ||
      (value && typeof value === "object" && value.PickFile !== undefined);
    var isPickImage = value === "PickImage" ||
      (value && typeof value === "object" && value.PickImage !== undefined);
    if (!isPickFile && !isPickImage) return false;
    if (typeof File === "undefined" || typeof FileReader === "undefined") {
      reply({ tag: "file-picker", ok: false, error: "unavailable" }, corId);
      return true;
    }
    var input = document.createElement("input");
    input.type = "file";
    if (isPickImage) {
      input.accept = "image/*";
    }
    var settled = false;
    input.addEventListener("change", function () {
      if (settled) return;
      settled = true;
      var file = input.files && input.files[0];
      if (!file) {
        reply({ tag: "file-picker", ok: false, error: "cancelled" }, corId);
        return;
      }
      var reader = new FileReader();
      reader.onload = function (ev) {
        reply({
          tag: "file-picker", ok: true,
          name: String(file.name),
          mime: String(file.type || "application/octet-stream"),
          dataUrl: String(ev.target.result),
        }, corId);
      };
      reader.onerror = function () {
        reply({ tag: "file-picker", ok: false, error: "unavailable" }, corId);
      };
      reader.readAsDataURL(file);
    });
    // A focus event after the dialog closes without a selection signals cancel.
    window.addEventListener("focus", function onFocus() {
      window.removeEventListener("focus", onFocus);
      setTimeout(function () {
        if (!settled) {
          settled = true;
          reply({ tag: "file-picker", ok: false, error: "cancelled" }, corId);
        }
      }, 400);
    }, { once: true });
    input.click();
    return true;
  }
  // Ipe.Browser.Camera: `CapturePhoto` -> <input type="file" capture="environment"
  // accept="image/*">. On mobile this directs the OS to open the camera; on
  // desktop it falls back to a plain image file picker. The result shape is
  // identical: a Captured frame carrying name / mime / dataUrl. A user
  // cancellation resolves to `cancelled`; an absent File API to `unavailable`.
  function cameraSink(value, corId) {
    var isCapture = value === "CapturePhoto" ||
      (value && typeof value === "object" && value.CapturePhoto !== undefined);
    if (!isCapture) return false;
    if (typeof File === "undefined" || typeof FileReader === "undefined") {
      reply({ tag: "camera", ok: false, error: "unavailable" }, corId);
      return true;
    }
    var input = document.createElement("input");
    input.type = "file";
    input.accept = "image/*";
    input.capture = "environment";
    var settled = false;
    input.addEventListener("change", function () {
      if (settled) return;
      settled = true;
      var file = input.files && input.files[0];
      if (!file) {
        reply({ tag: "camera", ok: false, error: "cancelled" }, corId);
        return;
      }
      var reader = new FileReader();
      reader.onload = function (ev) {
        reply({
          tag: "camera", ok: true,
          name: String(file.name),
          mime: String(file.type || "image/jpeg"),
          dataUrl: String(ev.target.result),
        }, corId);
      };
      reader.onerror = function () {
        reply({ tag: "camera", ok: false, error: "unavailable" }, corId);
      };
      reader.readAsDataURL(file);
    });
    window.addEventListener("focus", function onFocus() {
      window.removeEventListener("focus", onFocus);
      setTimeout(function () {
        if (!settled) {
          settled = true;
          reply({ tag: "camera", ok: false, error: "cancelled" }, corId);
        }
      }, 400);
    }, { once: true });
    input.click();
    return true;
  }
  // Ipe.Browser.Microphone: `CaptureAudio { maxDurationMs, mimeType }` →
  // getUserMedia({ audio: true }) → MediaRecorder. The host records for
  // `maxDurationMs` milliseconds (capped at 8 000 ms to fit inside the
  // Js.request deadline), collects ondataavailable chunks, assembles them into
  // one Blob, reads it via FileReader.readAsDataURL, and replies ONCE with the
  // base-64 audio data URL. Every host trap (getUserMedia rejection, absent
  // MediaRecorder, permission denied) → a typed Denied or Unavailable frame,
  // never a throw. No eval, no innerHTML.
  function microphoneSink(value, corId) {
    var opts = value && typeof value === "object" && value.CaptureAudio;
    if (!opts || typeof opts !== "object") return false;
    // Deny-by-default: require corId so the result is only routed to the
    // correct one-shot waiter, never broadcast to js_subscribe subscribers.
    if (corId === null || corId === undefined) {
      return true; // recognised but uncorrelated — swallow, never broadcast
    }
    // Cap maxDurationMs at 8 000 ms: the Js.request deadline is 10 s; the
    // sink needs the remainder for Blob assembly and FileReader.readAsDataURL.
    var maxMs = (typeof opts.maxDurationMs === "number" && opts.maxDurationMs > 0)
      ? Math.min(opts.maxDurationMs, 8000) : 3000;
    var mimeType = (typeof opts.mimeType === "string" && opts.mimeType !== "")
      ? opts.mimeType : "";
    if (!navigator || typeof navigator.mediaDevices === "undefined" ||
        typeof navigator.mediaDevices.getUserMedia !== "function") {
      reply({ tag: "microphone", ok: false, error: "unavailable" }, corId);
      return true;
    }
    if (typeof MediaRecorder === "undefined") {
      reply({ tag: "microphone", ok: false, error: "unavailable" }, corId);
      return true;
    }
    navigator.mediaDevices.getUserMedia({ audio: true }).then(
      function (stream) {
        var recOpts = {};
        // Only set the mimeType when the browser reports it as supported;
        // passing an unsupported type throws a NotSupportedError.
        if (mimeType !== "" && MediaRecorder.isTypeSupported(mimeType)) {
          recOpts.mimeType = mimeType;
        }
        var recorder;
        try {
          recorder = new MediaRecorder(stream, recOpts);
        } catch (_e) {
          stream.getTracks().forEach(function (t) { t.stop(); });
          reply({ tag: "microphone", ok: false, error: "unavailable" }, corId);
          return;
        }
        var chunks = [];
        var actualMime = recorder.mimeType || "audio/webm";
        var startTime = Date.now();
        var settled = false;
        function finish() {
          if (settled) return;
          settled = true;
          stream.getTracks().forEach(function (t) { t.stop(); });
          var blob = new Blob(chunks, { type: actualMime });
          var durationMs = Math.min(Date.now() - startTime, maxMs);
          var reader = new FileReader();
          reader.onload = function (ev) {
            reply({
              tag: "microphone", ok: true,
              data: String(ev.target.result),
              mime: actualMime,
              durationMs: durationMs,
            }, corId);
          };
          reader.onerror = function () {
            reply({ tag: "microphone", ok: false, error: "unavailable" }, corId);
          };
          reader.readAsDataURL(blob);
        }
        recorder.ondataavailable = function (ev) {
          if (ev.data && ev.data.size > 0) {
            chunks.push(ev.data);
          }
        };
        recorder.onstop = finish;
        recorder.onerror = function () {
          if (!settled) {
            settled = true;
            stream.getTracks().forEach(function (t) { t.stop(); });
            reply({ tag: "microphone", ok: false, error: "unavailable" }, corId);
          }
        };
        try {
          recorder.start();
        } catch (_e) {
          stream.getTracks().forEach(function (t) { t.stop(); });
          reply({ tag: "microphone", ok: false, error: "unavailable" }, corId);
          return;
        }
        setTimeout(function () {
          if (recorder.state !== "inactive") {
            try { recorder.stop(); } catch (_e) { finish(); }
          }
        }, maxMs);
      },
      function (err) {
        // getUserMedia rejection — map to typed denial or unavailability.
        var name = err && err.name ? String(err.name) : "";
        var kind = name === "NotAllowedError" || name === "PermissionDeniedError"
          ? "denied" : "unavailable";
        reply({ tag: "microphone", ok: false, error: kind }, corId);
      }
    );
    return true;
  }
  function builtinSink(value, corId) {
    if (clipboardSink(value, corId)) return true;
    if (geolocationSink(value, corId)) return true;
    if (notificationSink(value, corId)) return true;
    if (storageSink(value, corId)) return true;
    if (vibrationSink(value, corId)) return true;
    if (shareSink(value, corId)) return true;
    if (batterySink(value, corId)) return true;
    if (networkInfoSink(value, corId)) return true;
    if (filePickerSink(value, corId)) return true;
    if (cameraSink(value, corId)) return true;
    if (microphoneSink(value, corId)) return true;
    return false;
  }
  function deliver(raw) {
    var value;
    try { value = JSON.parse(raw); } catch (_e) { return; /* drop a malformed frame */ }
    // If this frame carries a correlation id, it is a `js_request` envelope:
    // extract the id and the inner payload command, and pass the id through to
    // the reply so the runtime can route it to the one-shot waiter.
    var corId = null;
    if (value && typeof value === "object" && "__ipe_id" in value) {
      corId = value.__ipe_id;
      value = value.payload !== undefined ? value.payload : value;
    }
    // A first-party browser-capability command is handled by its built-in sink;
    // anything else is a developer port frame routed to the registered receiver.
    if (builtinSink(value, corId)) return;
    // For non-first-party frames, if there is a correlation id the developer JS
    // handler must echo it back; we pass it as a second argument to onReceive.
    if (typeof onReceive === "function") { onReceive(value, corId); }
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

    #[test]
    fn notification_sink_reaches_the_web_api_and_traps_denial_to_a_typed_result() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.Notification sink reaches its Web APIs…
        assert!(js.contains("Notification.requestPermission"));
        assert!(js.contains("new Notification("));
        assert!(js.contains("RequestPermission"));
        assert!(js.contains("value.Show"));
        // …resolves a grant / display to a typed frame…
        assert!(js.contains("tag: \"notification\""));
        assert!(js.contains("\"granted\""));
        assert!(js.contains("\"shown\""));
        // …and traps a denial / absence to a typed inbound frame, never a throw.
        assert!(js.contains("\"denied\""));
        assert!(js.contains("\"unavailable\""));
        assert!(!js.contains("eval("));
    }

    #[test]
    fn storage_sink_reaches_web_storage_and_traps_unavailability_to_a_typed_result() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.Storage sink reaches localStorage…
        assert!(js.contains("localStorage"));
        assert!(js.contains("store.getItem("));
        assert!(js.contains("store.setItem("));
        assert!(js.contains("store.removeItem("));
        assert!(js.contains("store.clear()"));
        assert!(js.contains("value.Get"));
        assert!(js.contains("value.Set"));
        // …resolves reads / writes to typed events…
        assert!(js.contains("tag: \"storage\""));
        assert!(js.contains("event: \"get\""));
        assert!(js.contains("event: \"set\""));
        // …and traps a private-mode / absent store to a typed frame, never a throw.
        assert!(js.contains("\"unavailable\""));
        assert!(!js.contains("eval("));
    }

    #[test]
    fn vibration_sink_reaches_the_web_api_and_traps_absence_to_a_typed_result() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.Vibration sink reaches navigator.vibrate…
        assert!(js.contains("navigator.vibrate"));
        assert!(js.contains("value.Vibrate"));
        assert!(js.contains("value.Pattern"));
        // …acknowledges a supported actuator…
        assert!(js.contains("tag: \"vibration\""));
        // …and traps an absent actuator to a typed frame, never a throw.
        assert!(js.contains("\"unavailable\""));
        assert!(!js.contains("eval("));
    }

    #[test]
    fn share_sink_reaches_the_web_api_and_traps_cancel_and_absence_to_typed_results() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.Share sink reaches navigator.share…
        assert!(js.contains("navigator.share"));
        assert!(js.contains("value.Share"));
        // …acknowledges a completion…
        assert!(js.contains("tag: \"share\""));
        // …and enumerates the cancel + unavailable outcomes, never a throw.
        assert!(js.contains("AbortError"));
        assert!(js.contains("\"cancelled\""));
        assert!(js.contains("\"unavailable\""));
        assert!(!js.contains("eval("));
    }

    #[test]
    fn battery_sink_reaches_the_web_api_and_traps_absence_to_a_typed_result() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.Battery sink reaches navigator.getBattery…
        assert!(js.contains("navigator.getBattery"));
        // …emits a typed reading with all four status fields…
        assert!(js.contains("tag: \"battery\""));
        assert!(js.contains("charging:"));
        assert!(js.contains("level:"));
        assert!(js.contains("chargingTime:"));
        assert!(js.contains("dischargingTime:"));
        // …and traps an absent API to a typed frame, never a throw.
        assert!(js.contains("\"unavailable\""));
        assert!(!js.contains("eval("));
    }

    #[test]
    fn network_info_sink_reaches_the_web_api_and_traps_absence_to_a_typed_result() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.NetworkInfo sink reaches navigator.connection…
        assert!(js.contains("navigator.connection"));
        // …emits a typed reading with all four hint fields…
        assert!(js.contains("tag: \"network-info\""));
        assert!(js.contains("effectiveType:"));
        assert!(js.contains("downlink:"));
        assert!(js.contains("rtt:"));
        assert!(js.contains("saveData:"));
        // …and traps an absent API to a typed frame, never a throw.
        assert!(js.contains("\"unavailable\""));
        assert!(!js.contains("eval("));
    }

    #[test]
    fn file_picker_sink_opens_input_element_and_traps_cancel_and_absence_to_typed_results() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.FilePicker sink creates a file input element…
        assert!(js.contains("filePickerSink"));
        assert!(js.contains("PickFile"));
        assert!(js.contains("PickImage"));
        assert!(js.contains("input.type = \"file\""));
        assert!(js.contains("FileReader"));
        assert!(js.contains("readAsDataURL"));
        // …resolves a selection to a typed frame with name / mime / dataUrl fields…
        assert!(js.contains("tag: \"file-picker\""));
        assert!(js.contains("name:"));
        assert!(js.contains("mime:"));
        assert!(js.contains("dataUrl:"));
        // …and enumerates the cancelled + unavailable outcomes, never a throw.
        assert!(js.contains("\"cancelled\""));
        assert!(js.contains("\"unavailable\""));
        assert!(!js.contains("eval("));
    }

    #[test]
    fn camera_sink_uses_capture_attribute_and_traps_cancel_and_absence_to_typed_results() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.Camera sink uses the capture attribute…
        assert!(js.contains("cameraSink"));
        assert!(js.contains("CapturePhoto"));
        assert!(js.contains("input.capture = \"environment\""));
        assert!(js.contains("input.accept = \"image/*\""));
        // …reads via FileReader…
        assert!(js.contains("FileReader"));
        assert!(js.contains("readAsDataURL"));
        // …resolves a capture to a typed frame with name / mime / dataUrl fields…
        assert!(js.contains("tag: \"camera\""));
        // …and enumerates the cancelled + unavailable outcomes, never a throw.
        assert!(js.contains("\"cancelled\""));
        assert!(js.contains("\"unavailable\""));
        assert!(!js.contains("eval("));
    }

    #[test]
    fn microphone_sink_reaches_get_user_media_and_traps_denial_and_absence_to_typed_results() {
        let js = port_glue_js();
        // The first-party Ipe.Browser.Microphone sink reaches getUserMedia…
        assert!(js.contains("microphoneSink"));
        assert!(js.contains("CaptureAudio"));
        assert!(js.contains("navigator.mediaDevices.getUserMedia"));
        assert!(js.contains("MediaRecorder"));
        // …caps maxDurationMs to 8 000 ms…
        assert!(js.contains("Math.min(opts.maxDurationMs, 8000)"));
        // …assembles chunks via FileReader.readAsDataURL…
        assert!(js.contains("ondataavailable"));
        assert!(js.contains("FileReader"));
        assert!(js.contains("readAsDataURL"));
        // …resolves a recording to a typed frame with data / mime / durationMs fields…
        assert!(js.contains("tag: \"microphone\""));
        assert!(js.contains("data:"));
        assert!(js.contains("durationMs:"));
        // …and enumerates the denied + unavailable outcomes, never a throw / eval.
        assert!(js.contains("\"denied\""));
        assert!(js.contains("\"unavailable\""));
        assert!(js.contains("NotAllowedError"));
        assert!(!js.contains("eval("));
    }

    #[test]
    fn geolocation_watch_ids_tracked_as_set_so_double_watch_is_fully_clearable() {
        let js = port_glue_js();
        // Ids stored in an array, not a single scalar — a second Watch pushes
        // rather than overwrites, so ClearWatch drains every id in the set.
        assert!(js.contains("geoWatchIds"));
        assert!(js.contains("geoWatchIds.push("));
        assert!(js.contains("geoWatchIds.length"));
        assert!(js.contains("geoWatchIds = []"));
        // No single-id scalar: the old geoWatchId variable must not exist.
        assert!(!js.contains("geoWatchId ="));
        assert!(!js.contains("geoWatchId !=="));
    }
}
