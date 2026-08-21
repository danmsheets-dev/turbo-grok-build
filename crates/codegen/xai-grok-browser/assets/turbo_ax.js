(function () {
  var SEL =
    "a[href],button,input,textarea,select,[contenteditable=true]," +
    "h1,h2,h3,h4,h5,h6,[role=heading]," +
    "[role=button],[role=link],[role=textbox],[role=tab],[role=menuitem]," +
    "[role=option],[role=listbox],[role=combobox],[role=switch],[role=checkbox]," +
    "[role=radio],[role=status],[role=alert],[role=dialog],[role=alertdialog]," +
    "dialog,[aria-modal=true]";
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
  function roleName(el) {
    return String(el.getAttribute("role") || "").toLowerCase();
  }
  function isOverlay(el) {
    var r = roleName(el);
    if (r === "dialog" || r === "alertdialog") return true;
    if (el.tagName === "DIALOG") return true;
    if (el.getAttribute("aria-modal") === "true") return true;
    return false;
  }
  function isStatus(el) {
    var r = roleName(el);
    return r === "status" || r === "alert";
  }
  function headingPriority(el) {
    if (!isHeading(el)) return false;
    var n = nameOf(el).toLowerCase();
    return (
      n === "experience" ||
      n === "about" ||
      n === "education" ||
      n === "featured" ||
      n.indexOf("experience") === 0 ||
      n.indexOf("about") === 0
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
  // Document order across the light DOM, open shadow roots, and same-origin
  // iframes. Closed shadows and cross-origin frames stay unreachable here.
  function candidates(root, out) {
    var all = root.querySelectorAll("*");
    for (var i = 0; i < all.length; i++) {
      var el = all[i];
      if (el.matches(SEL) && !skip(el)) out.push(el);
      if (el.shadowRoot) candidates(el.shadowRoot, out);
      if (el.tagName === "IFRAME" || el.tagName === "FRAME") {
        try {
          var doc = el.contentDocument;
          if (doc && doc.documentElement) candidates(doc, out);
        } catch (e) {
          /* cross-origin */
        }
      }
    }
    return out;
  }
  function inViewport(el) {
    var r = el.getBoundingClientRect();
    var vh = window.innerHeight || 0;
    var vw = window.innerWidth || 0;
    return r.bottom > 0 && r.right > 0 && r.top < vh && r.left < vw;
  }
  // Over cap, drop the least useful first. Overlay/status/Experience headings
  // stay even when Activity/feed nodes would otherwise consume the cap.
  function applyCap(els, cap) {
    if (els.length <= cap) return els;
    var tiers = [[], [], [], [], [], []];
    for (var i = 0; i < els.length; i++) {
      var el = els[i];
      var vis = inViewport(el);
      var overlay = isOverlay(el) || isCloseControl(el);
      var priority = headingPriority(el) || isStatus(el);
      var head = isHeading(el);
      var tier;
      if (overlay) tier = 0;
      else if (priority && vis) tier = 1;
      else if (!head && vis) tier = 2;
      else if (!head) tier = 3;
      else if (vis) tier = 4;
      else tier = 5;
      tiers[tier].push(el);
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
  function isCloseControl(el) {
    var n = nameOf(el).toLowerCase();
    if (n === "close" || n === "dismiss" || n.indexOf("close your") === 0) return true;
    if (n === "close your conversation" || n.indexOf("close this") === 0) return true;
    return false;
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
    if (el.tagName === "DIALOG") return "dialog";
    return "generic";
  }
  function labelledBy(el) {
    var ids = el.getAttribute("aria-labelledby");
    if (!ids) return "";
    var parts = ids.split(/\s+/);
    var out = [];
    var doc = el.ownerDocument || document;
    for (var i = 0; i < parts.length; i++) {
      if (!parts[i]) continue;
      var n = doc.getElementById(parts[i]);
      if (n) out.push(((n.innerText || n.textContent || "") + "").replace(/\s+/g, " ").trim());
    }
    return out.filter(Boolean).join(" ");
  }
  function labelFor(el) {
    if (el.labels && el.labels.length) {
      return ((el.labels[0].innerText || el.labels[0].textContent || "") + "")
        .replace(/\s+/g, " ")
        .trim();
    }
    if (el.id) {
      try {
        var lab = (el.ownerDocument || document).querySelector(
          'label[for="' + CSS.escape(el.id) + '"]'
        );
        if (lab) return ((lab.innerText || lab.textContent || "") + "").replace(/\s+/g, " ").trim();
      } catch (e) {
        /* CSS.escape unavailable */
      }
    }
    return "";
  }
  function nameOf(el) {
    var s =
      el.getAttribute("aria-label") ||
      labelledBy(el) ||
      el.getAttribute("alt") ||
      el.getAttribute("title") ||
      labelFor(el) ||
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
  function selectAll(el) {
    try {
      if (typeof el.select === "function") {
        el.select();
        return;
      }
    } catch (e) {
      /* ignore */
    }
    if (!el.isContentEditable) return;
    var doc = el.ownerDocument || document;
    var sel = (doc.defaultView || window).getSelection();
    if (!sel) return;
    var range = doc.createRange();
    range.selectNodeContents(el);
    sel.removeAllRanges();
    sel.addRange(range);
  }
  function fillContentEditable(el, value) {
    selectAll(el);
    var inserted = false;
    try {
      inserted = document.execCommand("insertText", false, value);
    } catch (e) {
      inserted = false;
    }
    if (!inserted) {
      try {
        var before = new InputEvent("beforeinput", {
          bubbles: true,
          cancelable: true,
          composed: true,
          inputType: "insertText",
          data: value,
        });
        el.dispatchEvent(before);
        if (!before.defaultPrevented) {
          el.textContent = value;
        }
      } catch (e2) {
        el.textContent = value;
      }
      try {
        el.dispatchEvent(
          new InputEvent("input", {
            bubbles: true,
            cancelable: false,
            composed: true,
            inputType: "insertText",
            data: value,
          })
        );
      } catch (e3) {
        el.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
      }
    }
    // Lexical/Ember often only enable Send after a paste-shaped clipboard event.
    try {
      var dt = new DataTransfer();
      dt.setData("text/plain", value);
      el.dispatchEvent(
        new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: dt })
      );
    } catch (e4) {
      /* ClipboardEvent may reject constructed clipboardData */
    }
  }
  function fillNativeValue(el, value) {
    // React/Vue install their own `value` setter and track the last value they
    // saw. A plain `el.value = v` leaves that tracker stale, so the framework
    // dedupes the synthetic `input` and onChange never fires.
    var proto = Object.getPrototypeOf(el);
    var desc = proto && Object.getOwnPropertyDescriptor(proto, "value");
    if (desc && desc.set) desc.set.call(el, value);
    else el.value = value;
    try {
      el.dispatchEvent(
        new InputEvent("input", {
          bubbles: true,
          cancelable: false,
          composed: true,
          inputType: "insertText",
          data: value,
        })
      );
    } catch (e) {
      el.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
    }
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }
  function fill(uid, value) {
    var r = resolve(uid);
    if (!r.ok) return r;
    var el = r.el;
    if (el.disabled || el.readOnly) return { ok: false, error: "element_not_editable" };
    if (el.scrollIntoView) el.scrollIntoView({ block: "center", inline: "nearest" });
    el.focus();
    if (el.isContentEditable) {
      fillContentEditable(el, value);
    } else if ("value" in el) {
      fillNativeValue(el, value);
    } else {
      return { ok: false, error: "element_not_editable" };
    }
    el.dispatchEvent(new FocusEvent("blur", { bubbles: false }));
    return { ok: true, epoch: epoch };
  }
  function hover(uid) {
    var r = resolve(uid);
    if (!r.ok) return r;
    var el = r.el;
    if (el.scrollIntoView) el.scrollIntoView({ block: "center", inline: "nearest" });
    var rect = el.getBoundingClientRect();
    var x = rect.left + rect.width / 2;
    var y = rect.top + rect.height / 2;
    var opts = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y };
    el.dispatchEvent(new MouseEvent("mouseover", opts));
    el.dispatchEvent(new MouseEvent("mouseenter", opts));
    el.dispatchEvent(new MouseEvent("mousemove", opts));
    return { ok: true, epoch: epoch };
  }
  function selectOption(uid, value) {
    var r = resolve(uid);
    if (!r.ok) return r;
    var el = r.el;
    if (el.tagName !== "SELECT") return { ok: false, error: "not_select" };
    var wanted = String(value);
    var found = false;
    for (var i = 0; i < el.options.length; i++) {
      var opt = el.options[i];
      if (opt.value === wanted || opt.text === wanted || opt.label === wanted) {
        el.selectedIndex = i;
        found = true;
        break;
      }
    }
    if (!found) return { ok: false, error: "unknown_option" };
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return { ok: true, epoch: epoch, value: el.value };
  }
  function scrollToUid(uid) {
    var r = resolve(uid);
    if (!r.ok) return r;
    if (r.el.scrollIntoView) r.el.scrollIntoView({ block: "center", inline: "nearest" });
    return { ok: true, epoch: epoch };
  }
  function scrollByDelta(dx, dy) {
    window.scrollBy(dx || 0, dy || 0);
    return { ok: true, x: window.scrollX, y: window.scrollY };
  }
  function pressKey(key, uid) {
    var target = document.activeElement || document.body;
    if (uid) {
      var r = resolve(uid);
      if (!r.ok) return r;
      target = r.el;
      if (target.focus) target.focus();
    }
    var opts = { key: key, code: key, bubbles: true, cancelable: true, composed: true };
    target.dispatchEvent(new KeyboardEvent("keydown", opts));
    target.dispatchEvent(new KeyboardEvent("keypress", opts));
    if (key === "Enter" && target.form && typeof target.form.requestSubmit === "function") {
      /* don't auto-submit; the page handler decides */
    }
    target.dispatchEvent(new KeyboardEvent("keyup", opts));
    return { ok: true, epoch: epoch };
  }
  function markFileInput(uid) {
    var r = resolve(uid);
    if (!r.ok) return r;
    var el = r.el;
    if (el.tagName !== "INPUT" || String(el.type).toLowerCase() !== "file") {
      return { ok: false, error: "not_file_input" };
    }
    el.setAttribute("data-turbo-file-target", "1");
    return { ok: true };
  }
  function pageText() {
    var main =
      document.querySelector("main, [role=main], article, [role=article]") || document.body;
    if (!main) return "";
    return ((main.innerText || main.textContent || "") + "").replace(/\s+/g, " ").trim().slice(0, 4000);
  }
  function pageContains(text) {
    if (!text) return true;
    var hay = ((document.body && (document.body.innerText || document.body.textContent)) || "")
      .toLowerCase();
    return hay.indexOf(String(text).toLowerCase()) >= 0;
  }
  function collect(cap) {
    cap = cap || 200;
    var els = tag(cap);
    var out = [];
    var ae = document.activeElement;
    var overlay = false;
    for (var i = 0; i < els.length; i++) {
      var el = els[i];
      if (isOverlay(el)) overlay = true;
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
    return { epoch: epoch, nodes: out, overlay: overlay };
  }
  var api = {
    tag: tag,
    collect: collect,
    lookup: lookup,
    click: click,
    fill: fill,
    hover: hover,
    select: selectOption,
    scrollTo: scrollToUid,
    scrollBy: scrollByDelta,
    pressKey: pressKey,
    markFileInput: markFileInput,
    pageText: pageText,
    pageContains: pageContains,
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
