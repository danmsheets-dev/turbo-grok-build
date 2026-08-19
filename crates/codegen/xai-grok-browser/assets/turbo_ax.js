(function () {
  var SEL =
    "a[href],button,input,textarea,select,[role=button],[role=link],[role=textbox],[contenteditable=true],h1,h2,h3,h4,h5,h6,[role=heading]";
  var prev = window.__turboAx;
  // Survive re-injection: keep the epoch monotonic so a uid minted before a
  // re-inject can never silently resolve to a different element after it.
  var epoch = prev && typeof prev.epoch === "number" ? prev.epoch : 0;
  // uid -> element, authoritative for click/fill resolution. Lives in the
  // isolated world so page script can neither read nor forge entries. Carried
  // across re-injection with `epoch` so uids minted before a reload stay stale.
  var registry = prev && prev.registry instanceof Map ? prev.registry : new Map();

  function isHeading(el) {
    var t = el.tagName;
    return (
      t === "H1" || t === "H2" || t === "H3" || t === "H4" || t === "H5" || t === "H6" ||
      el.getAttribute("role") === "heading"
    );
  }
  function skip(el) {
    if (el.tagName === "INPUT" && String(el.type).toLowerCase() === "hidden") return true;
    // Not rendered: no boxes at all (display:none, detached, empty inline).
    if (!el.getClientRects().length) return true;
    if (el.closest('[aria-hidden="true"]')) return true;
    var view = el.ownerDocument && el.ownerDocument.defaultView;
    var st = view && view.getComputedStyle(el);
    if (st && (st.visibility === "hidden" || st.visibility === "collapse")) return true;
    return false;
  }
  // Document order across the light DOM and every open shadow root. One pass:
  // a shadow host can itself be a candidate, so test and recurse together.
  function candidates(root, out) {
    var all = root.querySelectorAll("*");
    for (var i = 0; i < all.length; i++) {
      var el = all[i];
      if (el.matches(SEL) && !skip(el)) out.push(el);
      if (el.shadowRoot) candidates(el.shadowRoot, out);
    }
    return out;
  }
  function inViewport(el) {
    var r = el.getBoundingClientRect();
    var vh = window.innerHeight || 0;
    var vw = window.innerWidth || 0;
    return r.bottom > 0 && r.right > 0 && r.top < vh && r.left < vw;
  }
  // Over cap, drop the least useful first: offscreen headings, then headings,
  // then offscreen interactives. Document order is preserved within each tier.
  function applyCap(els, cap) {
    if (els.length <= cap) return els;
    var tiers = [[], [], [], []];
    for (var i = 0; i < els.length; i++) {
      var head = isHeading(els[i]);
      var vis = inViewport(els[i]);
      tiers[head ? (vis ? 2 : 3) : vis ? 0 : 1].push(els[i]);
    }
    var keep = [];
    for (var t = 0; t < tiers.length && keep.length < cap; t++) {
      for (var k = 0; k < tiers[t].length && keep.length < cap; k++) keep.push(tiers[t][k]);
    }
    var order = new Map();
    for (var m = 0; m < els.length; m++) order.set(els[m], m);
    keep.sort(function (a, b) {
      return order.get(a) - order.get(b);
    });
    return keep;
  }
  function tag(cap) {
    epoch += 1;
    registry.clear();
    var els = applyCap(candidates(document, []), cap || 200);
    for (var i = 0; i < els.length; i++) {
      var uid = epoch + "-" + (i + 1);
      // The attribute stays for snapshot debugging, but it is NOT the identity:
      // it lives in the page's DOM and the page can rewrite it.
      els[i].setAttribute("data-turbo-uid", uid);
      registry.set(uid, els[i]);
    }
    return els;
  }
  function roleOf(el) {
    var r = el.getAttribute("role");
    if (r) return r;
    var t = el.tagName;
    if (t === "A") return "link";
    if (t === "BUTTON") return "button";
    if (t === "SELECT") return "combobox";
    if (t === "TEXTAREA") return "textbox";
    if (t === "H1" || t === "H2" || t === "H3" || t === "H4" || t === "H5" || t === "H6")
      return "heading";
    if (t === "INPUT") {
      var ty = String(el.type || "text").toLowerCase();
      if (ty === "submit" || ty === "button" || ty === "reset" || ty === "image") return "button";
      if (ty === "checkbox") return "checkbox";
      if (ty === "radio") return "radio";
      if (ty === "password") return "password";
      return "textbox";
    }
    if (el.isContentEditable) return "textbox";
    return "generic";
  }
  function nameOf(el) {
    var s =
      el.getAttribute("aria-label") ||
      el.getAttribute("alt") ||
      el.getAttribute("title") ||
      el.getAttribute("placeholder") ||
      el.getAttribute("name") ||
      "";
    if (!s) s = ((el.innerText || el.textContent || "") + "").replace(/\s+/g, " ").trim();
    return String(s).slice(0, 200);
  }
  function valOf(el) {
    if (el.tagName === "INPUT" && String(el.type).toLowerCase() === "password") return null;
    if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT") {
      var v = el.value;
      return v === undefined || v === null || v === "" ? null : String(v);
    }
    return null;
  }
  // Fields the host must refuse to fill regardless of the value's shape.
  function secretOf(el) {
    var ty = String(el.type || "").toLowerCase();
    if (ty === "password") return "password";
    var ac = String(el.getAttribute("autocomplete") || "").toLowerCase();
    if (ac.indexOf("one-time-code") >= 0) return "one-time-code";
    if (ac.indexOf("current-password") >= 0 || ac.indexOf("new-password") >= 0) return "password";
    if (ac.indexOf("cc-number") >= 0 || ac.indexOf("cc-csc") >= 0) return "payment";
    return null;
  }
  // Resolve from the isolated-world registry, NEVER from a DOM query.
  //
  // `data-turbo-uid` is an ordinary attribute in the page's DOM, so a hostile
  // page can stamp a uid we just minted onto a control of its choosing;
  // `querySelector` returns the first match in document order, which lets the
  // page decide what a click lands on. This registry lives in the CDP isolated
  // world, which page script cannot reach. It also resolves elements at any
  // shadow-root depth, where the old document-level query only reached depth 1.
  function elByUid(uid) {
    var el = registry.get(String(uid));
    if (!el) return null;
    return el.isConnected ? el : null;
  }
  // A uid minted by an older snapshot must never resolve. Stale uids are the
  // difference between clicking "More information" and clicking "Delete".
  function resolve(uid) {
    var parts = String(uid).split("-");
    if (parts.length !== 2 || String(Number(parts[0])) !== parts[0]) {
      return { ok: false, error: "unknown_uid" };
    }
    if (Number(parts[0]) !== epoch) return { ok: false, error: "stale_uid" };
    var el = elByUid(uid);
    if (!el) return { ok: false, error: "unknown_uid" };
    return { ok: true, el: el };
  }
  function lookup(uid) {
    var r = resolve(uid);
    if (!r.ok) return r;
    return {
      ok: true,
      name: nameOf(r.el),
      role: roleOf(r.el),
      secret: secretOf(r.el),
      epoch: epoch,
    };
  }
  function click(uid) {
    var r = resolve(uid);
    if (!r.ok) return r;
    var el = r.el;
    if (el.disabled) return { ok: false, error: "element_disabled" };
    if (el.scrollIntoView) el.scrollIntoView({ block: "center", inline: "nearest" });
    // Real pointer sequence: custom menus and framework handlers listen for
    // pointerdown/mousedown, not the synthetic click() shortcut.
    var rect = el.getBoundingClientRect();
    var x = rect.left + rect.width / 2;
    var y = rect.top + rect.height / 2;
    var opts = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y };
    try {
      el.dispatchEvent(new PointerEvent("pointerdown", opts));
    } catch (e) {
      /* PointerEvent may be unavailable; mouse events below still fire. */
    }
    el.dispatchEvent(new MouseEvent("mousedown", opts));
    if (el.focus) el.focus();
    try {
      el.dispatchEvent(new PointerEvent("pointerup", opts));
    } catch (e) {
      /* ignore */
    }
    el.dispatchEvent(new MouseEvent("mouseup", opts));
    el.click();
    return { ok: true, epoch: epoch };
  }
  function fill(uid, value) {
    var r = resolve(uid);
    if (!r.ok) return r;
    var el = r.el;
    if (el.disabled || el.readOnly) return { ok: false, error: "element_not_editable" };
    if (el.scrollIntoView) el.scrollIntoView({ block: "center", inline: "nearest" });
    el.focus();
    if ("value" in el) {
      // React/Vue install their own `value` setter and track the last value they
      // saw. A plain `el.value = v` leaves that tracker stale, so the framework
      // dedupes the synthetic `input` and onChange never fires.
      var proto = Object.getPrototypeOf(el);
      var desc = proto && Object.getOwnPropertyDescriptor(proto, "value");
      if (desc && desc.set) desc.set.call(el, value);
      else el.value = value;
    } else if (el.isContentEditable) {
      el.textContent = value;
    } else {
      return { ok: false, error: "element_not_editable" };
    }
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    el.dispatchEvent(new FocusEvent("blur", { bubbles: false }));
    return { ok: true, epoch: epoch };
  }
  function collect(cap) {
    cap = cap || 200;
    var els = tag(cap);
    var out = [];
    var ae = document.activeElement;
    for (var i = 0; i < els.length; i++) {
      var el = els[i];
      out.push({
        // The uid we just minted, not a read-back of the page-writable
        // attribute, so a page cannot influence what the snapshot reports.
        uid: epoch + "-" + (i + 1),
        role: roleOf(el),
        name: nameOf(el),
        value: valOf(el),
        focused: el === ae,
      });
    }
    return { epoch: epoch, nodes: out };
  }
  var api = {
    tag: tag,
    collect: collect,
    lookup: lookup,
    click: click,
    fill: fill,
    // Handed to the next injection so uid identity survives re-inject. Only
    // reachable from the isolated world; `window` here is not the page's.
    registry: registry,
  };
  // Live getter: `epoch` advances inside tag(), and a re-injection reads it
  // back off the old object to stay monotonic.
  Object.defineProperty(api, "epoch", {
    enumerable: true,
    get: function () {
      return epoch;
    },
  });
  window.__turboAx = api;
})();
