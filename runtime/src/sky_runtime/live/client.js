var __skySid = window.__SKY_SID;
var __skyBase = window.__SKY_BASE || "";
var __skyCsrfToken = window.__SKY_CSRF_TOKEN || "";
// Server-templated config (mod.rs render_page_full → window.__SKY_* ; Go parity
// live.go ~5993). Each reads the injected window global when present, else the
// hardcoded default — so SKY_LIVE_RETRY_* / QUEUE_MAX / HELLO_TIMEOUT_MS /
// HEARTBEAT_TTL_MS / BANNER overrides reach the client. CSP-safe (no eval).
var __skyBannerEnabled = (window.__SKY_BANNER_ENABLED != null) ? window.__SKY_BANNER_ENABLED : true;
var __skyRetryBaseMs = (window.__SKY_RETRY_BASE_MS != null) ? window.__SKY_RETRY_BASE_MS : 500;
var __skyRetryMaxMs = (window.__SKY_RETRY_MAX_MS != null) ? window.__SKY_RETRY_MAX_MS : 16000;
var __skyRetryMaxAttempts = (window.__SKY_RETRY_MAX_ATTEMPTS != null) ? window.__SKY_RETRY_MAX_ATTEMPTS : 10;
var __skyEventQueueMax = (window.__SKY_EVENT_QUEUE_MAX != null) ? window.__SKY_EVENT_QUEUE_MAX : 50;
var __skyMsgReconnecting = (window.__SKY_MSG_RECONNECTING != null) ? window.__SKY_MSG_RECONNECTING : "Reconnecting…";
var __skyMsgOffline = (window.__SKY_MSG_OFFLINE != null) ? window.__SKY_MSG_OFFLINE : "Connection lost — refresh to retry";
var __skyHelloTimeoutMs = (window.__SKY_HELLO_TIMEOUT_MS != null) ? window.__SKY_HELLO_TIMEOUT_MS : 8000;
var __skyHeartbeatTtlMs = (window.__SKY_HEARTBEAT_TTL_MS != null) ? window.__SKY_HEARTBEAT_TTL_MS : 35000;

// ── Input authority protocol state ───────────────────────────
// See docs/skylive/input-authority-protocol.md §Client state.
// Step 2 populates these counters + per-input table on every send
// and response; Step 3 activates the patch filter that reads them;
// Step 4 activates the stale-drop test against __skyLastAppliedSeq.
//
// Cycle 3 P47 (pub/sub global+local seq split — see
// docs/skylive/pubsub-design.md §3.2): __skyLastGlobalSeq is the
// app-wide broadcast counter. The server stamps it onto every
// broadcast-derived SSE frame (event:patches OR event:patch); the
// client dedupes against the largest value already applied so a
// replayed broadcast (e.g. SSE reconnect that re-delivers buffered
// frames) drops at the boundary without mutating state twice. Frames
// from per-session dispatch (the common case) carry globalSeq=0 OR
// omit the field; the guard treats 0 / missing as "no broadcast
// ordering constraint" and never blocks.
var __skyClientSeq = 0;       // monotonic, client-owned; bumped on every __skySend
var __skyLastAppliedSeq = 0;  // server-owned; largest local seq already applied
var __skyLastGlobalSeq = 0;   // server-owned; largest broadcast globalSeq already applied (P47)
var __skyInputs = {};         // sky-id → InputEntry (populated by __skyBindOne)

function __skyInputEntry(sid) {
  var e = __skyInputs[sid];
  if (!e) {
    e = __skyInputs[sid] = {
      liveValue: "", lastSentSeq: 0, lastAckedSeq: 0,
      pendingDebounceId: null, pendingSend: null
    };
  }
  return e;
}

// __skyInputsSnapshot — dirty-input projection bundled into every
// outgoing event. Only entries whose user-typed value is newer than
// the server's latest ack are included, so the wire stays compact
// when the client and server agree.
function __skyInputsSnapshot() {
  var out = null;
  var ids = Object.keys(__skyInputs);
  for (var i = 0; i < ids.length; i++) {
    var e = __skyInputs[ids[i]];
    if (e.lastSentSeq <= e.lastAckedSeq) continue;
    if (!out) out = {};
    out[ids[i]] = {value: e.liveValue, seq: e.lastSentSeq};
  }
  return out;
}

// __skyIngestSeq — fold a response or SSE frame's {seq, ackInputs}
// into client state. seq advances __skyLastAppliedSeq monotonically;
// ackInputs retires per-input dirty flags so the next snapshot omits
// caught-up fields.
// __skyIsDirty — a typable form field (input / textarea / select)
// whose DOM state is authoritative over the server's view. The check
// is scoped to those tags ONLY: buttons, anchors, divs and other
// focused-but-non-typable elements have no keystrokes to preserve,
// so treating them as dirty would wrongly block patches that wipe
// their containing subtree (e.g. navigating from a "new game"
// screen into a board view, where the focused button legitimately
// disappears). Scope signals: focus, pending debounce keyed by
// data-sky-hid, or an unacked typed value at the input's sky-id.
function __skyIsDirty(el) {
  if (!el || el.nodeType !== 1) return false;
  var tag = el.tagName;
  if (tag !== "INPUT" && tag !== "TEXTAREA" && tag !== "SELECT") return false;
  if (el === document.activeElement) return true;
  var hid = el.getAttribute && el.getAttribute("data-sky-hid");
  if (hid && __skyInputPending[hid]) return true;
  var sid = el.getAttribute && el.getAttribute("sky-id");
  if (sid) {
    var e = __skyInputs[sid];
    if (e && e.lastSentSeq > e.lastAckedSeq) return true;
  }
  return false;
}

function __skyIngestSeq(seq, ackInputs, globalSeq) {
  if (typeof seq === "number" && seq > __skyLastAppliedSeq) {
    __skyLastAppliedSeq = seq;
  }
  // Cycle 3 P47: monotonic-applied semantics on the broadcast counter,
  // mirroring the local-seq path. Missing / zero / non-numeric globalSeq
  // is treated as "no broadcast ordering constraint" and ignored.
  if (typeof globalSeq === "number" && globalSeq > __skyLastGlobalSeq) {
    __skyLastGlobalSeq = globalSeq;
  }
  if (ackInputs) {
    var ids = Object.keys(ackInputs);
    for (var i = 0; i < ids.length; i++) {
      var e = __skyInputs[ids[i]];
      if (!e) continue;
      var n = ackInputs[ids[i]];
      if (n > e.lastAckedSeq) e.lastAckedSeq = n;
    }
  }
}

// __skyHandleResponse — gate DOM-mutating work behind the monotonic
// seq check (Step 4 / I2). An out-of-order or replayed frame with
// seq ≤ __skyLastAppliedSeq is dropped entirely: a newer frame has
// already landed with a later view, and applying the stale payload
// would regress the DOM. Legacy frames that omit seq (or report 0)
// always apply — pre-upgrade servers keep working.
//
// Cycle 3 P47 (pub/sub global+local seq split — see
// docs/skylive/pubsub-design.md §3.2): broadcast-derived frames also
// carry an OPTIONAL globalSeq. If supplied AND already applied (i.e.
// globalSeq > 0 && globalSeq <= __skyLastGlobalSeq) the frame is
// dropped — a replayed broadcast (e.g. an SSE reconnect re-delivering
// buffered frames) would otherwise mutate state twice. Both guards
// fire independently: a frame is dropped if EITHER counter has already
// passed it; the localSeq guard alone suffices for the legacy
// non-broadcast case (globalSeq omitted / 0 → broadcast guard always
// passes).
function __skyHandleResponse(seq, ackInputs, applyFn, globalSeq) {
  if (typeof seq === "number" && seq > 0 && seq <= __skyLastAppliedSeq) {
    return; // stale — a newer local-seq frame already landed
  }
  if (typeof globalSeq === "number" && globalSeq > 0 && globalSeq <= __skyLastGlobalSeq) {
    return; // stale — a newer broadcast frame already landed
  }
  __skyIngestSeq(seq, ackInputs, globalSeq);
  applyFn();
}

// ── Focus preservation via node identity ────────────────────
// Sky.Live renders subtrees via innerHTML replacement (both on JSON
// patches that carry p.html and on full-HTML navigations). Plain
// innerHTML DESTROYS the focused input element — even though JS is
// single-threaded, the browser's internal input-method editor (IME),
// autofill popover, undo stack, composition state, pointer-cursor
// blink, password manager affordances, and native caret are all
// tied to the live DOM NODE. Destroying it and recreating a clone
// with the same .value loses every one of those.
//
// The correct fix is to preserve node identity through the swap:
// before the replacement, locate the focused INPUT / TEXTAREA /
// SELECT, find its placeholder in the new HTML (by sky-id → name),
// then SPLICE the live node into the new tree in place of the
// placeholder. Server-side attrs (class, type, placeholder, ...)
// get copied onto the live node, EXCEPT value/checked/selected —
// those stay under user authority.
//
// The live node never gets "destroyed" — it only moves between
// parents. .value, .selectionStart, IME state, composition buffer,
// autofill state all survive. Keystrokes in flight land on the
// same node regardless of where the browser has currently attached
// it in the DOM tree.
//
// Re-focus at the end because replaceChild on a focused element
// temporarily blurs it (focus isn't a property of the node, it's
// a property of the document). Selection is lost and must be
// restored too.

// __skyPlaceholderUncontrolled — true when the server-rendered
// element has no authority attribute set (no value/checked/selected,
// no textarea content, no option[selected]). For these the user-
// owned client state is canonical; we splice the live node across
// the swap so the user's typing isn't blanked. See
// docs/skylive/input-authority-protocol.md §I6 (full-body
// preservation).
function __skyPlaceholderUncontrolled(placeholder) {
  if (!placeholder) return false;
  if (placeholder.hasAttribute("value")) return false;
  if (placeholder.hasAttribute("checked")) return false;
  if (placeholder.hasAttribute("selected")) return false;
  var tag = placeholder.tagName;
  if (tag === "TEXTAREA") {
    return (placeholder.textContent || "").length === 0;
  }
  if (tag === "SELECT") {
    return placeholder.querySelectorAll("option[selected]").length === 0;
  }
  // type=file: browsers refuse programmatic value assignment, the
  // user's selection is the only truth — always treat as uncontrolled.
  if (tag === "INPUT" && placeholder.getAttribute("type") === "file") return true;
  return true;
}

// __skyFindPlaceholder — locate a live input's slot in the new tree.
// Prefer sky-id (structurally stable + uniquely keyed). Fall back to
// tag+name only when the live element has no sky-id AND the new tree
// has exactly one match — preventing wrong-input collisions when
// names recur (e.g. multiple address forms with name="line1").
function __skyFindPlaceholder(tmp, live) {
  var sid = live.getAttribute && live.getAttribute("sky-id");
  if (sid) {
    var bySid = tmp.querySelector('[sky-id="' + sid.replace(/"/g, '\\"') + '"]');
    if (bySid) return bySid;
  }
  var name = live.getAttribute && live.getAttribute("name");
  if (!name) return null;
  var tag = live.tagName.toLowerCase();
  var matches = tmp.querySelectorAll(tag + '[name="' + name.replace(/"/g, '\\"') + '"]');
  if (matches.length === 1) return matches[0];
  return null;
}

// __skyReplaceHTMLPreservingFocus — the authoritative swap.
// Drop-in for plain innerHTML assignment that keeps:
//   1. The currently-focused input (.value, IME state, composition
//      buffer, selection range, scroll position).
//   2. EVERY uncontrolled input/textarea/select in the subtree
//      (anything the server didn't render an authority attribute for).
//      Without this, an unfocused password field gets recreated by the
//      innerHTML swap and the user's typed secret is blanked — see
//      Bug 2 in docs/skylive/architecture.md §Input preservation.
// Used by both __skyPatch (full body) and __skyApplyPatches (p.html
// and large p.text patches).
function __skyReplaceHTMLPreservingFocus(container, newHTML) {
  var focused = document.activeElement;
  var focusedInside = focused && focused !== document.body &&
      container.contains(focused) &&
      (focused.tagName === "INPUT" ||
       focused.tagName === "TEXTAREA" ||
       focused.tagName === "SELECT");

  // Parse the new HTML into a detached element so we can splice
  // preserved live nodes into it before committing.
  //
  // Namespace correctness: when the container element is in a foreign-
  // content namespace (SVG or MathML), parsing the new HTML via a
  // plain document.createElement("div") + .innerHTML = ... uses the
  // HTML insertion mode, so element names like <g>, <rect>, <text>
  // (which the diff emits as direct children when it replaces the
  // children of an <svg> element) end up in the XHTML namespace
  // rather than SVG. The elements appear in the DOM but the browser
  // doesn't lay them out as SVG primitives — the canvas silently goes
  // blank after a shape add/remove with no JS error to point at.
  //
  // Range.createContextualFragment parses HTML using the namespace
  // context of the range's container, preserving SVG/MathML element
  // namespaces correctly. The downstream code accepts either an
  // Element or a DocumentFragment via the same .firstChild /
  // .querySelectorAll / .parentNode.replaceChild surface, so no
  // other changes are needed.
  //
  // Repro before this fix: any Sky.Live view that emits an HTML
  // patch at a sky-id pointing at an <svg> element (the diff does
  // this whenever the SVG's children-count changes, or a child
  // tag/kind mismatches between renders) leaves the SVG with HTML-
  // namespaced children. Drawing tools, charts, and apps that swap
  // inline-SVG icon <path> children are the common victims.
  var tmp;
  if (container.namespaceURI && container.namespaceURI !== "http://www.w3.org/1999/xhtml") {
    var range = document.createRange();
    range.selectNodeContents(container);
    tmp = range.createContextualFragment(newHTML);
  } else {
    tmp = document.createElement("div");
    tmp.innerHTML = newHTML;
  }

  // Snapshot focused-state BEFORE any DOM mutation. Selection read
  // throws on some input types, so catch.
  var selStart = null, selEnd = null, scrollTop = 0;
  if (focusedInside) {
    try {
      selStart = focused.selectionStart;
      selEnd   = focused.selectionEnd;
    } catch (_) {}
    scrollTop = focused.scrollTop;
  }

  // Walk the LIVE container's inputs/textareas/selects and decide
  // which ones to splice. The focused element is ALWAYS spliced
  // (active typing wins). Other elements are spliced only when the
  // server-side placeholder is uncontrolled (no value/checked/
  // selected) — i.e. user state is canonical.
  var preservedFocus = null;
  var liveNodes = container.querySelectorAll("input, textarea, select");
  for (var i = 0; i < liveNodes.length; i++) {
    var live = liveNodes[i];
    var placeholder = __skyFindPlaceholder(tmp, live);
    if (!placeholder) continue; // server unmounted: honour the server
    var isFocused = (live === focused);
    if (!isFocused && !__skyPlaceholderUncontrolled(placeholder)) {
      // Controlled field with a server-supplied value — let the
      // server win. Default innerHTML swap will recreate it from
      // placeholder.
      continue;
    }
    // Mirror placeholder attrs (class, type, placeholder, disabled,
    // aria-*, …) onto the live node — except the three authority
    // attrs the user drives. The user's .value / .checked /
    // .selected DOM property survives untouched.
    __skyCopyAttrsExceptAuthority(placeholder, live);
    // Splice: replace the placeholder in tmp with the live node.
    // After this, the live node lives in tmp at the placeholder's
    // slot; the container still references it too (until the swap
    // below). DOM trees are tolerant of this — the upcoming
    // removeChild + appendChild commit moves it cleanly.
    placeholder.parentNode.replaceChild(live, placeholder);
    if (isFocused) preservedFocus = live;
  }

  // Commit: throw away container's current children (those we didn't
  // splice are stale; spliced ones already moved into tmp), then
  // attach tmp's children. Done.
  while (container.firstChild) container.removeChild(container.firstChild);
  while (tmp.firstChild) container.appendChild(tmp.firstChild);

  // Focus restoration on the SAME node — so .value, IME state,
  // composition buffer survive untouched. removeChild + appendChild
  // drop focus, so we re-set it now.
  if (preservedFocus) {
    try { preservedFocus.focus({preventScroll: true}); } catch (_) {
      try { preservedFocus.focus(); } catch (_) {}
    }
    if (typeof preservedFocus.setSelectionRange === "function" &&
        selStart !== null && selEnd !== null) {
      try { preservedFocus.setSelectionRange(selStart, selEnd); } catch (_) {}
    }
    if (scrollTop) preservedFocus.scrollTop = scrollTop;
  }
}

// __skyCopyAttrsExceptAuthority — mirror attrs from src onto dst,
// skipping the three the user drives directly. Removes attrs on
// dst that aren't in src (same "skip" rule). Used when splicing a
// live focused input into a server-rendered placeholder.
function __skyCopyAttrsExceptAuthority(src, dst) {
  if (!src || !dst || !src.attributes || !dst.attributes) return;
  var isAuthority = function(n) {
    return n === "value" || n === "checked" || n === "selected";
  };
  // Drop attrs that aren't present in src.
  var toRemove = [];
  for (var i = 0; i < dst.attributes.length; i++) {
    var n = dst.attributes[i].name;
    if (isAuthority(n)) continue;
    if (!src.hasAttribute(n)) toRemove.push(n);
  }
  for (var r = 0; r < toRemove.length; r++) dst.removeAttribute(toRemove[r]);
  // Add / update attrs from src.
  for (var j = 0; j < src.attributes.length; j++) {
    var a = src.attributes[j];
    if (isAuthority(a.name)) continue;
    if (dst.getAttribute(a.name) !== a.value) dst.setAttribute(a.name, a.value);
  }
}

// __skyPatch: full-body replacement for sky-nav clicks, popstate,
// and the server's full-HTML fallback path. Routes through the
// node-preservation splicer so keystrokes never land on a destroyed
// DOM node.
function __skyPatch(t) {
  var root = document.getElementById("sky-root");
  if (!root) return;
  // Strip the full-document envelope when present (sky-nav fetches
  // return <!doctype><html>...</html>). The regex captures exactly
  // the rendered body, same as before.
  var m = t.match(/<div id="sky-root">([\s\S]*?)<\/div><script>/);
  if (m) t = m[1];
  var scrollX = window.scrollX, scrollY = window.scrollY;
  __skyReplaceHTMLPreservingFocus(root, t);
  window.scrollTo(scrollX, scrollY);
  __skyBindEvents(document);
  __skyRunEvals(root);
  __skyRunPaths(root);
  __skyReviveScripts(root);
}

// __skyReviveScripts: browsers DO NOT execute <script> tags inserted
// via innerHTML (or any HTML-string assignment). When Sky.Live
// swaps the body via __skyReplaceHTMLPreservingFocus (sky-nav, full-
// body patches) or applies an attribute/HTML patch via
// __skyApplyPatches, any <script src=...> or inline <script>
// element in the new content is added to the DOM but never
// executed. This breaks any app-level JS bundle injected via the
// Sky-side Ui.html (Html.node "script" [...]) pattern (notably
// sky-editor's Editor.scriptTag).
//
// The fix: walk the new subtree for <script> elements, replace
// each with a freshly-created one carrying a STRICT ALLOWLIST of
// attributes. Freshly-created script nodes execute on insertion.
//
// Security (Cycle 3 audit gap C9 / cycle 2 plan P31):
//   - Attribute copy is filtered through __skyScriptAttrAllowlist.
//     Event-handler attrs (onerror, onload, onclick, …) are NEVER
//     re-emitted — the original unfiltered loop allowed an attacker
//     who controlled WYSIWYG content rendered back into Ui.html to
//     ship <script onerror=alert(1)> and watch the handler fire on
//     the next patch.
//   - Inline script bodies (textContent) are DROPPED unless the
//     element also carries a src= attribute (a same-origin opt-in:
//     Sky-bundled scripts like sky-editor's Editor.scriptTag set
//     src=; user-supplied inline bodies are silently rejected with
//     a console.warn so the misuse is visible during dev).
//   - Rejected scripts STILL get the data-sky-script-revived
//     marker so a subsequent revival pass doesn't reprocess them
//     (i.e. silent-drop is idempotent — no infinite warning storm).
//
// Idempotency: each revived <script> gets a data-sky-script-revived
// attribute; subsequent calls skip it. This prevents the bundle
// from re-loading on every patch (which would re-run any
// DOMContentLoaded handlers and re-fire setInterval-driven
// bootstraps multiple times).
//
// Safety: only matches <script> nodes inside root (the sky-root
// container). Top-level page <script> tags (in <head> or outside
// sky-root) are left alone — they ran on initial load and need
// no revival.
var __skyScriptAttrAllowlist = {
  "src": 1,
  "type": 1,
  "async": 1,
  "defer": 1,
  "integrity": 1,
  "crossorigin": 1,
  "nomodule": 1,
  "referrerpolicy": 1,
  "data-sky-script-revived": 1
};
function __skyReviveScripts(root) {
  if (!root) return;
  var scripts = root.querySelectorAll("script:not([data-sky-script-revived])");
  for (var i = 0; i < scripts.length; i++) {
    var old = scripts[i];
    // Mark the source element revived FIRST so a rejection branch
    // (no-src + inline body) doesn't re-trip on the next pass.
    try { old.setAttribute("data-sky-script-revived", "1"); } catch (_) {}
    var hasSrc = old.hasAttribute("src");
    var hasInline = !!(old.textContent && old.textContent.length > 0);
    // Reject inline-only scripts (no src) — same-origin opt-in via
    // src= is the contract. Console.warn so the misuse is visible
    // during dev; never throws (one bad node mustn't kill the loop).
    if (!hasSrc && hasInline) {
      try {
        if (typeof console !== "undefined" && console.warn) {
          console.warn("[sky.live] script revival rejected an inline <script> without src= (XSS hardening, gap C9). Bundle via src= for Sky-side scripts.");
        }
      } catch (_) {}
      continue;
    }
    var fresh = document.createElement("script");
    // Copy ONLY allowlisted attributes. Event-handler attrs (anything
    // starting with "on…") and any non-allowlisted attribute are
    // silently dropped — see __skyScriptAttrAllowlist.
    var droppedAttrs = null;
    for (var j = 0; j < old.attributes.length; j++) {
      var a = old.attributes[j];
      var n = a.name.toLowerCase();
      if (__skyScriptAttrAllowlist[n] === 1) {
        try { fresh.setAttribute(a.name, a.value); } catch (_) {}
      } else {
        // Capture for a single dev-time warn at the end (a single
        // <script onerror=…> shouldn't fire one warn per attr).
        if (!droppedAttrs) droppedAttrs = [];
        droppedAttrs.push(a.name);
      }
    }
    if (droppedAttrs) {
      try {
        if (typeof console !== "undefined" && console.warn) {
          console.warn("[sky.live] script revival dropped non-allowlisted attrs (XSS hardening, gap C9):", droppedAttrs.join(", "));
        }
      } catch (_) {}
    }
    // Inline body is now ONLY admitted when src= is also present.
    // This stays compatible with <script src=...>// optional inline
    // bootstrapping comment <\/script> patterns; the body is included
    // verbatim, the src= drives the actual execution.
    // (The escaped </ above prevents the literal closing-script tag
    // from terminating the inline JS wrapper at the HTML parser.)
    if (hasSrc && hasInline) {
      fresh.textContent = old.textContent;
    }
    fresh.setAttribute("data-sky-script-revived", "1");
    // Replacing the old node with the fresh one triggers script
    // execution (for src= it fetches + runs; for inline it runs
    // the body).
    old.parentNode.replaceChild(fresh, old);
  }
}

// ── Loading indicator ────────────────────────────────────────
// Call __skyLoaderStart() before network, __skyLoaderEnd() after. An element
// with id="sky-loader" gets the sky-loading class added/removed. Small
// 80ms delay so fast responses don't flash the indicator.
var __skyLoaderEl = null;
var __skyLoaderTimer = null;
function __skyLoaderStart() {
  __skyLoaderEl = __skyLoaderEl || document.getElementById("sky-loader");
  if (!__skyLoaderEl) return;
  clearTimeout(__skyLoaderTimer);
  __skyLoaderTimer = setTimeout(function() {
    __skyLoaderEl.classList.add("sky-loading");
  }, 80);
}
function __skyLoaderEnd() {
  clearTimeout(__skyLoaderTimer);
  if (__skyLoaderEl) __skyLoaderEl.classList.remove("sky-loading");
}

// ── Debounce ─────────────────────────────────────────────────
var __skyInputTimers = {};
var __skyInputPending = {};
function __skyDebouncedSend(msgName, args, hid, delay) {
  var key = hid || msgName;
  clearTimeout(__skyInputTimers[key]);
  __skyInputPending[key] = { msgName: msgName, args: args, hid: hid };
  __skyInputTimers[key] = setTimeout(function() {
    delete __skyInputPending[key];
    __skySend(msgName, args, hid, { noLoader: true });
  }, delay);
}
// Flush pending debounced input on blur (tab away / click elsewhere).
// Without this, typing fast then tabbing loses the last keystrokes
// because the debounce hasn't fired yet.
document.addEventListener("focusout", function(ev) {
  var t = ev.target;
  if (!t) return;
  var hid = t.getAttribute("data-sky-hid");
  var key = hid || t.getAttribute("sky-input");
  if (key && __skyInputPending[key]) {
    clearTimeout(__skyInputTimers[key]);
    var p = __skyInputPending[key];
    delete __skyInputPending[key];
    __skySend(p.msgName, p.args, p.hid, { noLoader: true });
  }
}, true);

// ── I3: flush on unmount ─────────────────────────────────────
// Any pending debounce that hasn't fired by the time the user
// navigates or closes the tab would normally be discarded — the
// setTimeout is torn down with the page. These handlers flush
// synchronously so the final keystroke always reaches the server.
// See docs/skylive/input-authority-protocol.md §I3.

// __skyCollectPendingBatch — snapshot every pending-debounce entry
// into a batch array, bumping __skyClientSeq per entry so each gets
// its own order in the batch processed server-side. Clears the
// pending map as a side effect so the regular debounce callback
// can't double-fire after a beacon.
function __skyCollectPendingBatch() {
  var keys = Object.keys(__skyInputPending);
  if (keys.length === 0) return null;
  var batch = [];
  for (var i = 0; i < keys.length; i++) {
    var k = keys[i];
    clearTimeout(__skyInputTimers[k]);
    var p = __skyInputPending[k];
    delete __skyInputPending[k];
    __skyClientSeq++;
    batch.push({
      seq: __skyClientSeq,
      msg: p.msgName || "",
      args: p.args || [],
      handlerId: p.hid || ""
    });
  }
  return batch;
}

// __skyFlushPendingBeacon — POST pending debounces via sendBeacon so
// the request survives page unload. Single beacon carries the whole
// batch + the latest inputState snapshot so the server ingests the
// final DOM values before dispatching. Silent no-op when there's
// nothing pending or the browser lacks sendBeacon support.
function __skyFlushPendingBeacon() {
  if (!navigator || typeof navigator.sendBeacon !== "function") return;
  var batch = __skyCollectPendingBatch();
  var snapshot = __skyInputsSnapshot();
  if (!batch && !snapshot) return;
  var body = { sessionId: __skySid };
  if (batch)    body.batch = batch;
  if (snapshot) body.inputState = snapshot;
  try {
    var blob = new Blob([JSON.stringify(body)], {type: "application/json"});
    navigator.sendBeacon(__skyBase + "/_sky/event", blob);
  } catch (_) {}
}

// __skyFlushPendingSync — synchronous variant for same-page
// transitions where sendBeacon is overkill. Calls __skySend for
// each pending entry; the fetch requests are fire-and-forget and
// the browser keeps them alive across same-origin navigation.
function __skyFlushPendingSync() {
  var batch = __skyCollectPendingBatch();
  if (!batch) return;
  for (var i = 0; i < batch.length; i++) {
    var b = batch[i];
    __skySend(b.msg, b.args, b.handlerId, {noLoader: true});
  }
}

// Capture-phase click listener inside sky-root: before a link click
// leaves the current page, drain any pending debounce so the final
// typed value reaches the server in the same origin as the
// outgoing navigation. Beacon path handles cross-page; sync path
// handles SPA-style internal routing.
document.addEventListener("click", function(ev) {
  var a = ev.target && ev.target.closest && ev.target.closest("a[href]");
  if (!a) return;
  var root = document.getElementById("sky-root");
  if (!root || !root.contains(a)) return;
  var href = a.getAttribute("href") || "";
  // External or cross-origin → beacon (browser will tear down the
  // page, fetch would be cancelled). Same-origin navigation inside
  // SPA-style routing → sync flush (fetch survives).
  var isExternal = /^(https?:)?\/\//.test(href) && a.host !== location.host;
  if (isExternal || href === "") {
    __skyFlushPendingBeacon();
  } else {
    __skyFlushPendingSync();
  }
}, true);

// Tab close / navigate away: sendBeacon is the only path that
// survives the teardown. Listen on both events because iOS Safari
// + bfcache fire pagehide instead of beforeunload.
window.addEventListener("beforeunload", __skyFlushPendingBeacon);
window.addEventListener("pagehide", __skyFlushPendingBeacon);

// ── Core send ────────────────────────────────────────────────
// Wire format (see docs/skylive/input-authority-protocol.md §Request):
//   {sessionId, seq, msg, args, handlerId, inputState?}
//   * seq is client-monotonic — server uses it to match responses to
//     the inputState snapshot that produced them.
//   * inputState carries the user's current DOM values for every
//     dirty input so the server's diff can align against reality
//     before emitting patches.
function __skySend(msgName, args, handlerId, opts) {
  opts = opts || {};
  if (!opts.noLoader) __skyLoaderStart();
  __skyClientSeq++;
  var mySeq = __skyClientSeq;
  // Stamp every currently-dirty input with this seq. The server's
  // ack (for a future response) will clear them back to parity.
  var dirtyIds = Object.keys(__skyInputs);
  for (var di = 0; di < dirtyIds.length; di++) {
    var de = __skyInputs[dirtyIds[di]];
    if (de.liveValue !== "" || de.pendingDebounceId !== null) {
      de.lastSentSeq = mySeq;
    }
  }
  var snapshot = __skyInputsSnapshot();
  var body = {
    sessionId: __skySid,
    seq: mySeq,
    msg: msgName || "",
    args: args || [],
    handlerId: handlerId || ""
  };
  if (snapshot) body.inputState = snapshot;
  __skyPostEvent(body);
}

// ── POST retry queue ─────────────────────────────────────────
// Wire-protocol POSTs are cheap (small JSON, idempotent on the
// server's seq-ordered state machine), so a transient network blip
// shouldn't lose the click. Failures push the body onto __skyEventQueue;
// retries fire on exponential backoff (500ms, 1s, 2s, … cap 16s);
// the SSE 'open' handler drains the queue eagerly when the server
// comes back. Cap at 50 entries — beyond that the user has been
// offline so long that replay isn't useful, drop oldest with a
// console warn so the page doesn't accumulate megabytes of state.
var __skyEventQueue = [];
var __skyRetryTimer = null;
var __skyRetryAttempts = 0;
// __skyRetryBaseMs / __skyRetryMaxMs / __skyRetryMaxAttempts /
// __skyEventQueueMax are templated at the top of this script from
// the SKY_LIVE_RETRY_* / SKY_LIVE_QUEUE_MAX env vars (see
// loadLiveBannerConfig).
function __skyPostEvent(body) {
  // Phase 1.2 — attach the per-session CSRF token. The server-side
  // middleware (runtime-go/rt/csrf_middleware.go) rejects POSTs
  // without a matching X-Sky-Csrf / __sky_csrf cookie pair. Empty
  // token means CSRF is disabled at the runtime level (sky.toml
  // [security] csrf = false) — header omitted, middleware skipped.
  var headers = {"Content-Type":"application/json"};
  if (__skyCsrfToken) headers["X-Sky-Csrf"] = __skyCsrfToken;
  fetch(__skyBase + "/_sky/event", {
    method: "POST",
    headers: headers,
    body: JSON.stringify(body),
    credentials: "same-origin"
  }).then(function(r){
    if (!r.ok && r.status >= 500) {
      // Server is up but rejecting (502/503/504 from a deploying LB,
      // or 500 from a panic that survived the recover guard). Treat
      // as transient — same retry path as a network failure.
      throw new Error("server " + r.status);
    }
    // Reverse-proxy wedge detection: a real Sky.Live response always
    // carries X-Sky-Live: 1. Without it, we're looking at a proxy-
    // rewritten response (e.g. some edges turn upstream 502 into 200
    // OK with an HTML error page). Applying that as a "patch" would
    // replace the user's DOM with the proxy's error page, so we refuse
    // it and route through the failure path instead.
    //
    // For JSON content-type we keep a backwards-compat shim during
    // rolling deploys: a pre-marker server still returns valid JSON
    // with seq + patches, structurally indistinguishable from the
    // marked form, so accept it. HTML / text responses without the
    // marker are always rejected — those are the proxy-wedge shape.
    var skyMark = r.headers.get("X-Sky-Live");
    var ct = r.headers.get("Content-Type") || "";
    var isJson = ct.indexOf("application/json") >= 0;
    if (skyMark !== "1" && !isJson) {
      throw new Error("non-sky response " + r.status);
    }
    if (isJson) {
      return r.json().then(function(data) {
        // Even JSON is rejected if it lacks the protocol shape (no
        // seq field): some proxies (Cloudflare access denied, fly.io
        // edge errors) return JSON error envelopes with 200 OK.
        if (skyMark !== "1" && (!data || typeof data.seq === "undefined")) {
          throw new Error("non-sky json response");
        }
        __skyLoaderEnd();
        __skyOnPostSuccess();
        if (!data) return;
        __skyHandleResponse(data.seq, data.ackInputs, function() {
          if (data.patches) __skyApplyPatches(data.patches);
        }, data.globalSeq);
      });
    }
    return r.text().then(function(t) {
      __skyLoaderEnd();
      __skyOnPostSuccess();
      var seqStr = r.headers.get("X-Sky-Seq");
      var seq = seqStr ? parseInt(seqStr, 10) : 0;
      var ackRaw = r.headers.get("X-Sky-Ack-Inputs");
      var ack = null;
      if (ackRaw) { try { ack = JSON.parse(ackRaw); } catch(_) {} }
      __skyHandleResponse(seq, ack, function() { __skyPatch(t); });
    });
  }).catch(function() {
    __skyLoaderEnd();
    __skyOnPostFailure(body);
  });
}
function __skyOnPostSuccess() {
  // A successful POST proves the server reachable — clear any
  // backoff state and drain queued events behind this one. If the
  // SSE was the trigger that drained the queue, this is a no-op.
  __skyRetryAttempts = 0;
  if (__skyRetryTimer !== null) {
    clearTimeout(__skyRetryTimer);
    __skyRetryTimer = null;
  }
  if (__skyStatus !== "connected") {
    __skySetStatus("connected", "");
  }
  // SSE recovery: if the watchdog tore down the EventSource (offline
  // terminal state), a successful POST proves the network is back, so
  // reopen the stream too — otherwise subscriptions and Cmd.perform
  // results would silently not arrive even though clicks work. Cancel
  // any pending reopen-with-backoff and bring it forward.
  if (__skySSE === null) {
    if (__skySseReopenTimer !== null) {
      clearTimeout(__skySseReopenTimer);
      __skySseReopenTimer = null;
    }
    __skyOpenSSE();
  }
  __skyDrainQueue();
}
function __skyOnPostFailure(body) {
  // FIFO drop when the queue is at the cap — bail on the oldest
  // pending event rather than the new one, so the user's most
  // recent intent is preserved.
  if (__skyEventQueue.length >= __skyEventQueueMax) {
    var dropped = __skyEventQueue.shift();
    if (window.console && console.warn) {
      console.warn("[sky.live] event queue at cap; dropped oldest", dropped);
    }
  }
  __skyEventQueue.push(body);
  __skyShowReconnecting();
  __skyScheduleRetry();
}
function __skyShowReconnecting() {
  if (__skyStatus === "offline") return;
  if (__skyStatus === "connected") {
    __skySetStatus("reconnecting", __skyMsgReconnecting);
  }
}
function __skyScheduleRetry() {
  if (__skyRetryTimer !== null) return;  // already pending
  if (__skyRetryAttempts >= __skyRetryMaxAttempts) {
    __skySetStatus("offline", __skyMsgOffline);
    return;
  }
  __skyRetryAttempts++;
  // 500, 1000, 2000, 4000, 8000, 16000, 16000, … (capped)
  var delay = Math.min(__skyRetryBaseMs * Math.pow(2, __skyRetryAttempts - 1), __skyRetryMaxMs);
  __skyRetryTimer = setTimeout(function() {
    __skyRetryTimer = null;
    __skyDrainQueue();
  }, delay);
}
function __skyDrainQueue() {
  if (__skyEventQueue.length === 0) return;
  // Send the head of the queue. If it succeeds, __skyOnPostSuccess
  // recurses into __skyDrainQueue to send the next one. If it
  // fails, the body re-enters the queue and the retry loop kicks
  // back in. Order is preserved (FIFO) — the server's seq matching
  // tolerates late deliveries via __skyHandleResponse.
  var head = __skyEventQueue.shift();
  __skyPostEvent(head);
}

// Apply a list of sky-id addressed patches with input authority (I1):
// value/checked/selected attrs on dirty inputs are dropped so the
// user's DOM wins; innerHTML patches route through
// __skyReplaceHTMLPreservingFocus which splices the live focused
// input (same DOM node, same .value, same IME/composition state)
// through the new HTML so it's never destroyed. Per-attr and
// textContent updates are fine as-is — they don't regenerate nodes.
function __skyApplyPatches(patches) {
  if (!patches || patches.length === 0) return;
  // Open <select> defence: native dropdowns close on ANY DOM mutation
  // inside the open select OR any ancestor that would re-mount it.
  // There's no JS API for "is the dropdown open", so use focus as the
  // conservative proxy: if a SELECT is the active element, treat its
  // subtree (and ancestors that would re-mount it) as off-limits for
  // this patch cycle. The next user interaction (option click, blur)
  // triggers a fresh response and reconciliation. Sibling subtrees
  // and unrelated parts of the DOM apply normally — the dropdown is
  // unaffected. See Bug 3 in docs/skylive/architecture.md.
  var openSel = (document.activeElement && document.activeElement.tagName === "SELECT")
      ? document.activeElement : null;
  for (var i = 0; i < patches.length; i++) {
    var p = patches[i];
    var el = document.querySelector('[sky-id="' + p.id.replace(/"/g, '\\"') + '"]');
    if (!el) continue;
    if (openSel && (el === openSel || el.contains(openSel) || openSel.contains(el))) {
      // Skip: any mutation here would close the dropdown mid-pick.
      continue;
    }
    if (p.text !== undefined && p.text !== null) {
      // textContent on a container that contains the focused input
      // would also wipe the input (replaces all children with one
      // text node). Guard the same way as innerHTML.
      if (__skyContainsFocusedInput(el)) {
        __skyReplaceHTMLPreservingFocus(el, __skyEscapeHTML(p.text));
      } else {
        el.textContent = p.text;
      }
    }
    if (p.html !== undefined && p.html !== null) {
      __skyReplaceHTMLPreservingFocus(el, p.html);
    }
    if (p.attrs) {
      var dirty = __skyIsDirty(el);
      var keys = Object.keys(p.attrs);
      // Cursor preservation: when applying a "value" attr to a
      // focused INPUT or TEXTAREA, snapshot the selection range
      // BEFORE setting .value (which otherwise resets the cursor
      // to the end of the new string). Common case: user clicked
      // into a textarea, paused so their dirty flag cleared, and
      // the server pushes a fresh value via SSE. Without this,
      // the cursor jumps to the end mid-edit. Clamping handles
      // shorter new values (selectionStart > newLen -> newLen).
      var isInputLike = el.tagName === "INPUT" || el.tagName === "TEXTAREA";
      var hadFocus = isInputLike && el === document.activeElement;
      var savedSelStart = null, savedSelEnd = null, savedScrollTop = 0;
      if (hadFocus) {
        try {
          savedSelStart = el.selectionStart;
          savedSelEnd = el.selectionEnd;
        } catch (_) {}
        savedScrollTop = el.scrollTop;
      }
      var valueChanged = false;
      for (var j = 0; j < keys.length; j++) {
        var k = keys[j], v = p.attrs[k];
        // Authority filter: the user is currently editing this
        // field, so the server's proposed value/checked/selected
        // would stomp in-flight keystrokes. Drop them and let the
        // next event round-trip settle the state.
        if (dirty && (k === "value" || k === "checked" || k === "selected")) {
          continue;
        }
        if (v === "") { el.removeAttribute(k); }
        else {
          el.setAttribute(k, v);
          // Sync DOM properties that don't reflect from attrs.
          if (k === "value" && ("value" in el)) {
            el.value = v;
            valueChanged = true;
          }
          if (k === "checked") el.checked = v !== "" && v !== "false";
          if (k === "selected") el.selected = v !== "" && v !== "false";
          if (k === "disabled") el.disabled = v !== "" && v !== "false";
        }
      }
      // Restore selection on focused input/textarea after a value
      // update. Clamp to the new value length so a shorter server
      // value does not throw RangeError. Scroll restore matters
      // mostly for multi-line textarea where the user may have
      // scrolled below the visible area.
      if (hadFocus && valueChanged && savedSelStart !== null &&
          typeof el.setSelectionRange === "function") {
        var newLen = (el.value || "").length;
        var s = Math.min(savedSelStart, newLen);
        var e = Math.min(savedSelEnd === null ? s : savedSelEnd, newLen);
        try { el.setSelectionRange(s, e); } catch (_) {}
        if (savedScrollTop) el.scrollTop = savedScrollTop;
      }
    }
    if (p.remove) el.remove();
  }
  // Any new sky-* attribute in the patched DOM needs a listener.
  __skyBindEvents(document);
  // After SSE-driven patches the URL also needs reconciling — without
  // this, programmatic Navigate Msgs would only update the in-memory
  // model and leave the address bar pointing at the previous page.
  __skyRunPaths(document);
  // Any <script> in newly-patched HTML wouldn't execute via innerHTML
  // — revive them so JS bundles (e.g. sky-editor) bootstrap correctly
  // when their host element first appears via a patch (not the initial
  // SSR).  See __skyReviveScripts above for the full rationale.
  var skyRootForPatches = document.getElementById("sky-root");
  if (skyRootForPatches) __skyReviveScripts(skyRootForPatches);
}

function __skyContainsFocusedInput(el) {
  var a = document.activeElement;
  if (!a || a === document.body) return false;
  var tag = a.tagName;
  if (tag !== "INPUT" && tag !== "TEXTAREA" && tag !== "SELECT") return false;
  return el === a || el.contains(a);
}

function __skyEscapeHTML(s) {
  var d = document.createElement("div");
  d.textContent = s == null ? "" : String(s);
  return d.innerHTML;
}

// ── TEA event binding ────────────────────────────────────────
// Walks the DOM for sky-<event> attributes and binds a native listener
// that extracts args and dispatches through the TEA update cycle.
// Re-run after every DOM patch because new sky-* attrs may have appeared.
function __skyBindEvents(root) {
  root = root || document;
  var events = ["click", "dblclick", "input", "change", "submit", "focus", "blur",
                "keydown", "keyup", "keypress", "mouseover", "mouseout",
                "mousedown", "mouseup"];
  for (var i = 0; i < events.length; i++) {
    __skyBindOne(root, events[i]);
  }
}

function __skyRunEvals(root) {
  var el = (root || document).querySelector("[data-sky-eval]");
  if (el) { try { (new Function(el.getAttribute("data-sky-eval")))(); } catch(e) {} el.remove(); }
}

// __skyRunPaths: safer, CSP-friendly alternative to data-sky-eval for
// the specific case of "update the address bar after a render." Looks
// for [data-sky-path] elements and pushes / replaces history if the
// value differs from location. No new Function(), no eval; the only
// DOM APIs touched are getAttribute and history.pushState /
// replaceState. Works under strict CSP (no 'unsafe-eval') and has no
// XSS surface (the value is a URL path, never executed).
//
// The element is intentionally NOT removed after running — Sky.Live's
// patches identify elements by sky-id and look them up via
// querySelector; removing the data-sky-path element would orphan its
// sky-id, and the next attribute patch (when the path changes) would
// silently skip. The path-check makes the call idempotent, so leaving
// the element in place is cheap — at most one comparison per patch.
function __skyRunPaths(root) {
  var els = (root || document).querySelectorAll("[data-sky-path]");
  for (var i = 0; i < els.length; i++) {
    var p = els[i].getAttribute("data-sky-path");
    if (!p) continue;
    if (location.pathname !== p) {
      try { history.pushState({}, "", p); } catch (_) {}
    } else if (location.search) {
      try { history.replaceState({}, "", p); } catch (_) {}
    }
  }
}

function __skyBindOne(root, eventName) {
  var selector = "[sky-" + eventName + "]";
  var nodes = root.querySelectorAll(selector);
  for (var i = 0; i < nodes.length; i++) {
    var el = nodes[i];
    if (el["__sky_" + eventName]) continue;
    el["__sky_" + eventName] = true;
    el.addEventListener(eventName, function(ev) {
      var target = ev.currentTarget;
      var msgName = target.getAttribute("sky-" + ev.type);
      var hid     = target.getAttribute("data-sky-hid");
      if (!msgName && !hid) return;
      // Some events want preventDefault (submit, form-link navigation);
      // click doesn't (we only intercept when the attribute is set).
      if (ev.type === "submit") ev.preventDefault();
      var args = __skyExtractArgs(ev);
      if (ev.type === "input") {
        // Track live value against sky-id so the snapshot bundled in
        // the next __skySend reflects the user's actual DOM state,
        // and so Step 3's patch filter can recognise dirty inputs.
        var sid = target.getAttribute("sky-id");
        if (sid) {
          var e = __skyInputEntry(sid);
          e.liveValue = args && args.length > 0 ? String(args[0]) : "";
        }
        __skyDebouncedSend(msgName, args, hid, 150);
        return;
      }
      __skySend(msgName, args, hid);
    });
  }
}

// Extract the args array for a DOM event following the legacy Sky.Live
// convention:
//   * click / focus / blur / mouse*    → []         (just the msg)
//   * input / change                   → [value]    (typed input value)
//   * submit                           → [formData] (plain object of [name]=value)
//   * keydown / keyup / keypress       → [key]      (event.key string)
function __skyExtractArgs(ev) {
  var t = ev.target;
  switch (ev.type) {
    case "input":
    case "change":
      if (!t) return [""];
      if (t.type === "checkbox" || t.type === "radio") return [t.checked];
      if (t.type === "number" || t.type === "range") return [t.valueAsNumber || 0];
      return [t.value == null ? "" : String(t.value)];
    case "submit":
      // Form-data assembly. Two non-obvious rules:
      //
      // 1. SUBMITTER FILTER. <button type="submit"> and
      //    <input type="submit"> entries appear in form.elements.
      //    Spec: only the SUBMITTER (the button that actually
      //    triggered the submit) contributes its name/value to
      //    the payload — peer submit buttons MUST NOT. Editors
      //    routinely use multiple submit buttons sharing one
      //    name="action" (Save / Format / Check); the naive
      //    "iterate everything" loop lets later buttons clobber
      //    earlier ones, so the LAST button name=action wins
      //    regardless of which the user clicked. Honour
      //    ev.submitter (modern browsers; falls back to
      //    document.activeElement for old Safari).
      //
      // 2. Disabled fields are excluded by the spec — skip them
      //    too so a disabled-but-submittable field doesn't leak
      //    a stale value.
      var data = {};
      var submitter = ev.submitter ||
          (document.activeElement && t && t.contains(document.activeElement)
              ? document.activeElement : null);
      if (t && t.elements) {
        for (var i = 0; i < t.elements.length; i++) {
          var el = t.elements[i];
          if (!el.name || el.disabled) continue;
          if (el.type === "submit" || el.type === "button" ||
              el.type === "image" || el.type === "reset") {
            // Only the submitter button contributes its name/value.
            if (el === submitter) data[el.name] = el.value;
            continue;
          }
          if (el.type === "checkbox" || el.type === "radio") {
            if (el.checked) data[el.name] = el.value;
          } else if (el.type === "file") {
            // File handling via sky-file / sky-image drivers (below).
          } else {
            data[el.name] = el.value;
          }
        }
      }
      return [data];
    case "keydown":
    case "keyup":
    case "keypress":
      return [ev.key || ""];
    default:
      return [];
  }
}

// ── File / Image drivers ─────────────────────────────────────
// onFile / onImage register via data-sky-ev-sky-file / -sky-image
// attributes. The client reads the chosen file, optionally resizes
// (for images), and sends a base64 data URL as the event value.
document.addEventListener("change", function(ev) {
  var el = ev.target;
  if (!el || el.tagName !== "INPUT" || el.type !== "file") return;
  var fileId  = el.getAttribute("data-sky-ev-sky-file");
  var imageId = el.getAttribute("data-sky-ev-sky-image");
  var f = el.files && el.files[0];
  if (!f) return;
  // Client-side size guard via fileMaxSize. Saves the round-trip when
  // the user picks a 100MB file: drop with a console.warn rather than
  // streaming the bytes server-side just to reject them. Server-side
  // validation should still happen — this is a UX nicety, not a
  // security boundary.
  var maxSize = parseInt(el.getAttribute("data-sky-ev-sky-file-max-size") || "0");
  if (maxSize > 0 && f.size > maxSize) {
    if (window.console && console.warn) {
      console.warn(
        "[sky.live] file " + f.name + " (" + f.size +
        " bytes) exceeds fileMaxSize " + maxSize + "; dispatch dropped"
      );
    }
    el.value = "";  // clear the input so the user can pick another
    return;
  }
  if (fileId) {
    var r = new FileReader();
    // __skySend's args param is List a on the wire (server expects
    // []json.RawMessage); a bare string would unmarshal-fail. Wrap
    // the data URL in a single-element array — the Sky-side Msg
    // constructor declared as 'String -> Msg' reads args[0].
    r.onload = function(e) { __skySend(fileId, [e.target.result]); };
    r.readAsDataURL(f);
  }
  if (imageId) {
    var maxW = parseInt(el.getAttribute("data-sky-ev-sky-file-max-width")  || "1200");
    var maxH = parseInt(el.getAttribute("data-sky-ev-sky-file-max-height") || "1200");
    __skyResizeImage(f, maxW, maxH, function(dataUrl) {
      // Same wire-format reason as the onFile branch — wrap in array.
      __skySend(imageId, [dataUrl]);
    });
  }
});

function __skyResizeImage(file, maxW, maxH, cb) {
  var img = new Image();
  var url = URL.createObjectURL(file);
  img.onload = function() {
    URL.revokeObjectURL(url);
    var w = img.width, h = img.height;
    if (w > maxW) { h = Math.round(h * maxW / w); w = maxW; }
    if (h > maxH) { w = Math.round(w * maxH / h); h = maxH; }
    var canvas = document.createElement("canvas");
    canvas.width = w; canvas.height = h;
    canvas.getContext("2d").drawImage(img, 0, 0, w, h);
    cb(canvas.toDataURL("image/jpeg", 0.85));
  };
  img.src = url;
}

// Expose programmatic dispatch for custom JS integrations (e.g. Firebase
// auth callbacks that need to send a Msg after the SDK resolves).
window.__sky_send = function(id, value, opts) { __skySend(id, value, opts); };
// sky-nav: intercept clicks on <a sky-nav ...> links so navigation is a
// client-side fetch + innerHTML swap instead of a full page reload.
// Falls back to normal navigation on modifier keys (cmd/ctrl/shift/alt),
// middle-click, and non-GET targets.
document.addEventListener("click", function(ev) {
  if (ev.defaultPrevented) return;
  if (ev.button !== 0) return;
  if (ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.altKey) return;
  var el = ev.target;
  while (el && el.tagName !== "A") el = el.parentElement;
  if (!el) return;
  if (!el.hasAttribute("sky-nav")) return;
  var href = el.getAttribute("href");
  if (!href || href.charAt(0) === "#") return;
  // External links are left to the browser.
  try {
    var u = new URL(href, window.location.href);
    if (u.origin !== window.location.origin) return;
  } catch (e) { return; }
  ev.preventDefault();
  fetch(href, { headers: { "X-Sky-Nav": "1" }, credentials: "same-origin" })
    .then(function(r) { return r.text(); })
    .then(function(t) {
      __skyPatch(t);
      window.history.pushState({}, "", href);
    })
    .catch(function() { window.location.href = href; });
});
window.addEventListener("popstate", function() {
  fetch(window.location.href, { headers: { "X-Sky-Nav": "1" }, credentials: "same-origin" })
    .then(function(r) { return r.text(); })
    .then(__skyPatch);
});
// ── Status banner (connection state) ─────────────────────────
// Single bottom-pinned element rendered by the runtime (NOT by the
// user's view) showing connection health. State machine:
//   "connected"     → invisible
//   "reconnecting"  → amber bar, "Reconnecting…" + attempt counter
//   "offline"       → red bar, "Connection lost — refresh to retry"
// State transitions land in commits 2 + 3; this commit just wires
// the DOM + setter so the rest of the JS can flip states without
// touching the HTML directly. Hidden via display:none until a real
// reconnect attempt fires (no flicker on initial page load).
var __skyStatus = "connected";          // current state
var __skyStatusEl = null;               // banner root, set on DOMContentLoaded
var __skyStatusMsgEl = null;            // text node child
var __skyStatusGraceTimer = null;       // 500ms anti-flicker timer
function __skySetStatus(state, msg) {
  __skyStatus = state;
  if (!__skyStatusEl) return;           // banner not yet injected
  // Strip the previous state class, add the current one.
  var classes = __skyStatusEl.className.split(" ").filter(function(c) {
    return c.indexOf("sky-status--") !== 0;
  });
  classes.push("sky-status--" + state);
  __skyStatusEl.className = classes.join(" ");
  if (__skyStatusMsgEl && msg !== undefined) {
    __skyStatusMsgEl.textContent = msg;
  }
}
function __skyInjectStatusBanner() {
  if (__skyStatusEl) return;            // idempotent
  if (!__skyBannerEnabled) return;      // SKY_LIVE_BANNER=off
  var el = document.createElement("div");
  el.id = "__sky-status";
  el.className = "sky-status sky-status--connected";
  el.setAttribute("role", "status");
  el.setAttribute("aria-live", "polite");
  // Inline styles — no global stylesheet leak. Max z-index puts the
  // banner above any user fixed-position element. Fixed position
  // bottom-center; transitions for fade in/out feel less jarring.
  el.style.cssText = [
    "position:fixed",
    "left:50%",
    "bottom:16px",
    "transform:translateX(-50%)",
    "padding:8px 16px",
    "border-radius:6px",
    "font:13px/1.4 -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif",
    "color:#fff",
    "box-shadow:0 2px 8px rgba(0,0,0,0.25)",
    "z-index:2147483647",
    "pointer-events:none",            // never intercept clicks
    "transition:opacity 200ms",
    "opacity:1"
  ].join(";");
  // State-specific styles applied via inline style overrides on
  // each setStatus call would be cleaner, but overriding via class
  // on a <style> tag keeps the inline cssText readable. Append a
  // tiny <style> with the variant rules.
  var style = document.createElement("style");
  style.textContent = "" +
    "#__sky-status.sky-status--connected{display:none}" +
    "#__sky-status.sky-status--reconnecting{background:#b45309}" +
    "#__sky-status.sky-status--offline{background:#b91c1c}";
  document.head.appendChild(style);
  var msgEl = document.createElement("span");
  msgEl.className = "sky-status__msg";
  el.appendChild(msgEl);
  document.body.appendChild(el);
  __skyStatusEl = el;
  __skyStatusMsgEl = msgEl;
  // Replay current state in case it changed before DOM was ready.
  __skySetStatus(__skyStatus, "");
}

// ── Server-Sent Events ───────────────────────────────────────
// Frame envelope since v0.9.3+: {seq, body, ackInputs?}. Falls back to
// treating e.data as a raw HTML body when JSON parsing fails, so a
// mixed-version rollout doesn't break the open-SSE connection.
//
// Reverse-proxy hardening: the browser's EventSource has no
// application-level liveness check — if a misbehaving proxy holds the
// socket open with no body or rewrites an upstream 502 to 200 with a
// non-SSE HTML payload, EventSource will fire 'open' and never fire
// 'error', leaving the client silently wedged. The server now sends
// an immediate 'hello' event and a periodic 'heartbeat'; the client
// watchdog (below) treats absence of either as a wedge and force-
// reconnects with backoff. See docs/skylive/architecture.md
// §SSE wedge detection.
var __skySSE = null;
var __skyOpenAt = 0;          // ms timestamp of last EventSource.open
var __skyLastSseAt = 0;       // ms timestamp of any SSE event
var __skyHelloOk = false;     // server sent its handshake this connection
var __skyWatchdogTimer = null;
var __skySseReopenTimer = null;
var __skyForcedClose = false; // true while we're tearing down to reopen
function __skyOpenSSE() {
  __skyForcedClose = false;
  __skyHelloOk = false;
  __skyOpenAt = 0;
  __skySSE = new EventSource(__skyBase + "/_sky/sse");
  __skySSE.addEventListener("hello", function(e) {
    // Handshake received — we know we hit a real Sky.Live v2 server,
    // not a proxy that intercepted with a generic 200. Anything
    // before hello is suspect, so the connected-state flip happens
    // HERE, not on EventSource.open. Remember that THIS page's
    // server speaks v2 so future watchdog cycles can tighten the
    // wedge-detection threshold to the fast 8s hello timeout.
    __skyServerSpeaksV2 = true;
    __skyHelloOk = true;
    __skyLastSseAt = Date.now();
    if (__skyStatusGraceTimer !== null) {
      clearTimeout(__skyStatusGraceTimer);
      __skyStatusGraceTimer = null;
    }
    if (__skyStatus !== "connected") {
      __skySetStatus("connected", "");
    }
    __skyRetryAttempts = 0;
    if (__skyRetryTimer !== null) {
      clearTimeout(__skyRetryTimer);
      __skyRetryTimer = null;
    }
    if (__skyEventQueue.length > 0) __skyDrainQueue();
  });
  __skySSE.addEventListener("heartbeat", function(e) {
    __skyLastSseAt = Date.now();
  });
  __skySSE.addEventListener("patch", function(e) {
    __skyLastSseAt = Date.now();
    // Old servers (pre-handshake) only ever send "patch" events.
    // A real patch is itself proof we're talking to a Sky.Live server,
    // not a proxy-rewritten 200-OK, so treat first-patch-without-hello
    // as an implicit handshake. This keeps a new client from trapping
    // itself when a rolling deploy puts it in front of an old server.
    if (!__skyHelloOk) {
      __skyHelloOk = true;
      if (__skyStatusGraceTimer !== null) {
        clearTimeout(__skyStatusGraceTimer);
        __skyStatusGraceTimer = null;
      }
      if (__skyStatus !== "connected") {
        __skySetStatus("connected", "");
      }
      __skyRetryAttempts = 0;
      if (__skyRetryTimer !== null) {
        clearTimeout(__skyRetryTimer);
        __skyRetryTimer = null;
      }
    }
    var frame;
    try { frame = JSON.parse(e.data); } catch (_) {
      // Legacy frame (pre-v0.9.3 server) — raw HTML, no seq to gate on.
      // Open-<select> defence (Bug 3): same-cycle as the patches path.
      // SSE-pushed full-body re-renders during an open dropdown would
      // collapse it; skip the body, the next user interaction triggers
      // reconciliation. Active user paths (sky-nav, popstate, POST
      // text fallback) are NOT defended — those are user-initiated and
      // dropping them would be worse UX than the dropdown collapsing.
      if (document.activeElement && document.activeElement.tagName === "SELECT") return;
      return __skyPatch(e.data.replace(/\\n/g, "\n"));
    }
    if (frame && typeof frame === "object") {
      __skyHandleResponse(frame.seq, frame.ackInputs, function() {
        if (document.activeElement && document.activeElement.tagName === "SELECT") return;
        if (frame.body) __skyPatch(frame.body.replace(/\\n/g, "\n"));
      }, frame.globalSeq);
    }
  });
  // Cycle 3 P50b / Gap C11 — structural-patches SSE event.
  //
  // The producer (Cycle 3 P50a) now ships event:patches for any
  // render whose diff against the previous tree fits in a small
  // patch list (the typical 1-3 attribute/text node change at
  // ~200-1000 B, vs the ~14 KB full body). The legacy event:patch
  // handler above stays for first-renders, reconnect-resync,
  // full-replace fallbacks, and any pre-P50a server.
  //
  // Shape parity with the HTTP /_sky/event reply: frame is
  // {seq, ackInputs, patches} — identical to writeEventJSON's
  // envelope, so __skyApplyPatches consumes both routes without
  // divergence. seq-gating via __skyHandleResponse means out-of-
  // order frames (a stale patches frame arriving after a fresher
  // patch frame, e.g. across a brief network blip) are dropped at
  // the same monotonic guard the HTTP path uses.
  //
  // No open-<select> defence at this outer level — __skyApplyPatches
  // already has its own per-patch focus-restore + open-select skip
  // (live.go:4386+); applying it twice would surface as a no-op
  // either way, but the inner check is the canonical defence.
  // Focus / input-authority / dirty-input filtering all flow through
  // the same code path as the HTTP-side patches application, so
  // in-flight typing is preserved without server-side clientState
  // alignment (the SSE producer passes nil clientState to diffTrees;
  // the client's __skyIsDirty filter takes over).
  __skySSE.addEventListener("patches", function(e) {
    __skyLastSseAt = Date.now();
    // Same implicit-handshake defence as the legacy patch listener:
    // a real patches frame proves we're talking to a Sky.Live server,
    // so unstick the hello check even if the dedicated 'hello' event
    // got eaten by a misbehaving proxy.
    if (!__skyHelloOk) {
      __skyHelloOk = true;
      if (__skyStatusGraceTimer !== null) {
        clearTimeout(__skyStatusGraceTimer);
        __skyStatusGraceTimer = null;
      }
      if (__skyStatus !== "connected") {
        __skySetStatus("connected", "");
      }
      __skyRetryAttempts = 0;
      if (__skyRetryTimer !== null) {
        clearTimeout(__skyRetryTimer);
        __skyRetryTimer = null;
      }
    }
    var frame;
    try { frame = JSON.parse(e.data); }
    catch (_) {
      // Producer guarantees JSON for event:patches; a non-JSON
      // payload is impossible from a P50a+ server. Drop silently
      // rather than running __skyPatch on garbage.
      return;
    }
    if (!frame || typeof frame !== "object" || !frame.patches) return;
    __skyHandleResponse(frame.seq, frame.ackInputs, function() {
      __skyApplyPatches(frame.patches);
    }, frame.globalSeq);
  });
  __skySSE.addEventListener("open", function() {
    // EventSource fired open — but we don't trust this alone, since a
    // proxy can rewrite a non-SSE 200 OK into something that fires
    // open without ever delivering a frame. Wait for 'hello' to flip
    // to connected. Just record the open timestamp so the watchdog
    // can measure "how long have we been open without a hello".
    __skyOpenAt = Date.now();
    __skyLastSseAt = Date.now();
  });
  __skySSE.addEventListener("error", function() {
    // Suppress the banner when we triggered the close ourselves
    // (force-reopen path) — those errors are an artefact of our own
    // teardown, not a real outage signal.
    if (__skyForcedClose) return;
    // CLOSED (2) means the browser failed the connection permanently.
    // Per the EventSource spec, this happens for any non-200 HTTP
    // response (Caddy/Nginx 502 when upstream is down, 504 timeout,
    // 503 service unavailable) AND for the wrong Content-Type. The
    // browser will NOT retry on its own — we have to drive the
    // reconnect ourselves. Without this branch the whole reconnect
    // story collapses behind a reverse proxy that returns proper
    // 5xx codes during outages.
    if (__skySSE && __skySSE.readyState === 2) {
      __skyForceReopenSSE();
      return;
    }
    // CONNECTING (0): browser is auto-retrying (network blip, no HTTP
    // response received yet). Show the banner only if the situation
    // persists past the grace window — a quick error+reopen burst
    // shouldn't paint chrome.
    if (__skyStatus !== "connected") return;
    if (__skyStatusGraceTimer !== null) return;
    __skyStatusGraceTimer = setTimeout(function() {
      __skyStatusGraceTimer = null;
      if (__skySSE && __skySSE.readyState === 1 && __skyHelloOk) return;
      __skySetStatus("reconnecting", __skyMsgReconnecting);
    }, 500);
  });
}

// __skyForceReopenSSE — close the current EventSource and queue a
// fresh open with backoff. Each call bumps the retry counter; once
// it exceeds __skyRetryMaxAttempts the banner flips to "offline" but
// reconnect attempts CONTINUE in the background at the max delay so
// a healed proxy is picked up automatically (otherwise the user is
// permanently stuck unless they click something or refresh, which is
// surprising on push-driven UIs like dashboards or chat). Backoff
// matches the POST retry schedule so the user doesn't see two
// independent timers.
function __skyForceReopenSSE() {
  __skyForcedClose = true;
  try { if (__skySSE) __skySSE.close(); } catch (_) {}
  __skySSE = null;
  if (__skyStatus === "connected") {
    __skySetStatus("reconnecting", __skyMsgReconnecting);
  }
  __skyRetryAttempts++;
  // Session-loss probe: when the SSE is wedged (typically a server
  // restart with the memory store, or a sky.toml [live] store change
  // wiping the persistent session), no amount of reopen retries can
  // recover the lost session — the only path forward is a full page
  // reload, which fires handleInitial and creates a fresh session.
  // We probe with a fake POST: a 404 + X-Sky-Live: 1 + body
  // containing "session not found" is the unambiguous signal that the
  // server is up but doesn't know our cookie. Anything else (network
  // error, 5xx, healthy 200) keeps the normal retry path engaged so
  // we don't reload on a transient blip — full reload destroys
  // uncontrolled-input state that v0.11.7's preservation rules can't
  // bring back.
  __skyProbeSessionLost();
  if (__skyRetryAttempts >= __skyRetryMaxAttempts && __skyStatus !== "offline") {
    __skySetStatus("offline", __skyMsgOffline);
  }
  if (__skySseReopenTimer !== null) {
    clearTimeout(__skySseReopenTimer);
  }
  var delay = Math.min(__skyRetryBaseMs * Math.pow(2, __skyRetryAttempts - 1), __skyRetryMaxMs);
  __skySseReopenTimer = setTimeout(function() {
    __skySseReopenTimer = null;
    __skyOpenSSE();
  }, delay);
}

// __skyProbeSessionLost — fire-and-forget POST whose only purpose is
// to read the server's reaction to our existing sky_sid cookie. If
// the server is up AND has lost our session (memory-store restart,
// store-kind change, session TTL expiry), we get a 404 with the
// X-Sky-Live marker and a "session not found" body. That's the cue
// to hard-reload — every reopen attempt would otherwise loop on the
// same 404 forever.
//
// Must NOT trigger any user-visible side effects on the server. We
// send a Msg name that no real app registers and supply no
// handlerId, so handleEvent's code path goes:
//   session not found → 404 (the case we're probing for)
//   session found, handler not found → 404 with a different body
//   (we explicitly check the body string to avoid false positives).
var __skyProbedReload = false;  // one-shot guard so we don't trigger
                                // multiple reloads from a burst of
                                // failed reopen attempts.
function __skyProbeSessionLost() {
  if (__skyProbedReload) return;
  var headers = {"Content-Type": "application/json"};
  if (__skyCsrfToken) headers["X-Sky-Csrf"] = __skyCsrfToken;
  fetch(__skyBase + "/_sky/event", {
    method: "POST",
    headers: headers,
    body: JSON.stringify({sessionId: __skySid, msg: "__skySessionPing", args: []}),
    credentials: "same-origin"
  }).then(function(r) {
    if (r.status !== 404) return;
    if (r.headers.get("X-Sky-Live") !== "1") return;
    return r.text().then(function(body) {
      // Specifically "session not found" — distinguishes from
      // "handler not found" (which means the session is fine, just
      // our probe Msg name doesn't exist; that's expected and
      // doesn't warrant a reload).
      if (body.indexOf("session not found") < 0) return;
      __skyProbedReload = true;
      if (window.console && console.warn) {
        console.warn("[sky.live] server lost our session — reloading page to recover");
      }
      window.location.reload();
    });
  }).catch(function() {
    // Network error / server down. Keep retrying via normal path.
  });
}

// __skyWatchdog — runs every 5s. Two wedge detectors layered:
//   1. Connection has been quiet for longer than __skyHeartbeatTtlMs
//      (35s default). Catches every wedge shape — a proxy holding
//      the socket open with no body, an upstream 502 rewritten to
//      200 + HTML, mid-stream TCP stalls. The 35s threshold is
//      tuned to be just over 2× the server's 15s heartbeat; if the
//      server is new we miss at most one heartbeat before reacting.
//   2. Faster handshake check: once this PAGE has confirmed the
//      server speaks the v2 protocol (any session received a hello),
//      tighten the threshold to __skyHelloTimeoutMs (8s) on every
//      subsequent connection. Pre-v2 servers stay on the slower
//      heartbeat-ttl path so a rolling deploy doesn't wedge new
//      clients hitting old pods. The page-scoped flag survives SSE
//      teardowns + reopens within the same tab.
// Both paths increment the retry counter via __skyForceReopenSSE,
// so a wedge that persists reaches "offline" instead of looping
// forever — but reopen attempts continue at the max delay so a
// healed proxy reconnects automatically without a refresh.
var __skyServerSpeaksV2 = false;
function __skyWatchdog() {
  // If we have no live EventSource AND no reopen scheduled, the
  // 'error' handler must have missed (rare race) or some path tore
  // it down without re-arming. Drive the reopen here so the page
  // never gets permanently disconnected.
  if (!__skySSE && __skySseReopenTimer === null) {
    __skyForceReopenSSE();
    return;
  }
  if (!__skySSE) return;
  // CLOSED (2): browser failed the connection (non-200, wrong CT)
  // and won't retry. The 'error' handler should have caught this,
  // but cover the case where it didn't fire (e.g. error during
  // initial handshake before listeners attached, or a browser
  // implementation quirk). Single source of truth — both paths end
  // in __skyForceReopenSSE.
  if (__skySSE.readyState === 2) {
    if (!__skyForcedClose) {
      __skyForceReopenSSE();
    }
    return;
  }
  if (__skySSE.readyState !== 1) return;  // CONNECTING (0): browser is retrying, leave it
  var now = Date.now();
  // Effective threshold:
  //   - Brand-new SSE on a v2-confirmed server → fast hello timeout
  //     (8s) since we expect a hello promptly.
  //   - Otherwise → conservative heartbeat ttl (35s) so old servers
  //     and idle dashboards don't false-positive.
  var quietMs = now - __skyLastSseAt;
  var threshold = __skyHeartbeatTtlMs;
  if (__skyServerSpeaksV2 && !__skyHelloOk) {
    threshold = __skyHelloTimeoutMs;
  }
  if (quietMs > threshold) {
    if (window.console && console.warn) {
      console.warn("[sky.live] SSE quiet for " + quietMs +
        "ms (threshold " + threshold + "ms) — reopening");
    }
    __skyForceReopenSSE();
  }
}

// Kick off the SSE connection + watchdog. Watchdog interval is short
// enough (5s) that a wedge is detected within 5s + helloTimeout / ttl
// of the actual fault, and long enough to not be a measurable CPU cost.
__skyOpenSSE();
__skyWatchdogTimer = setInterval(__skyWatchdog, 5000);

// On tab visibility change, re-evaluate immediately — when a tab
// resumes from background the OS may have torn down the underlying
// TCP, but EventSource sometimes lags in detecting it. Eager check
// avoids the user staring at a stale UI for the full watchdog cycle.
document.addEventListener("visibilitychange", function() {
  if (document.visibilityState === "visible") {
    __skyWatchdog();
  }
});

// ── Init ─────────────────────────────────────────────────────
// Bind initial DOM event listeners + inject the status banner once
// the HTML is parsed. Banner needs document.body to exist, so it
// goes through the same gate as event binding.
function __skyInit() {
  __skyBindEvents(document);
  __skyInjectStatusBanner();
}
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", __skyInit);
} else {
  __skyInit();
}
