(function () {
  var SEL =
    "a[href],button,input,textarea,select,[role=button],[role=link],[role=textbox],[contenteditable=true],h1,h2,h3,h4,h5,h6,[role=heading]";
  function skip(el) {
    return el.tagName === "INPUT" && String(el.type).toLowerCase() === "hidden";
  }
  function tag() {
    var els = document.querySelectorAll(SEL);
    var n = 0;
    for (var i = 0; i < els.length; i++) {
      if (skip(els[i])) continue;
      els[i].setAttribute("data-turbo-uid", String(++n));
    }
    return n;
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
    if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT") {
      var v = el.value;
      return v === undefined || v === null || v === "" ? null : String(v);
    }
    return null;
  }
  function elByUid(uid) {
    return document.querySelector('[data-turbo-uid="' + uid + '"]');
  }
  function lookup(uid) {
    var el = elByUid(uid);
    if (!el) return { ok: false, error: "unknown_uid" };
    return { ok: true, name: nameOf(el), role: roleOf(el) };
  }
  function click(uid) {
    var el = elByUid(uid);
    if (!el) return { ok: false, error: "unknown_uid" };
    el.click();
    return { ok: true };
  }
  function fill(uid, value) {
    var el = elByUid(uid);
    if (!el) return { ok: false, error: "unknown_uid" };
    el.focus();
    if ("value" in el) el.value = value;
    else if (el.isContentEditable) el.textContent = value;
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return { ok: true };
  }
  function collect(cap) {
    tag();
    cap = cap || 200;
    var els = document.querySelectorAll("[data-turbo-uid]");
    var out = [];
    var ae = document.activeElement;
    var n = Math.min(els.length, cap);
    for (var i = 0; i < n; i++) {
      var el = els[i];
      out.push({
        uid: el.getAttribute("data-turbo-uid"),
        role: roleOf(el),
        name: nameOf(el),
        value: valOf(el),
        focused: el === ae,
      });
    }
    return out;
  }
  window.__turboAx = { tag: tag, collect: collect, lookup: lookup, click: click, fill: fill };
  tag();
})();
