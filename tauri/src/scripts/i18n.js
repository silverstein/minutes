// Minutes frontend localization runtime.
//
// Design: the English source string is the translation key. This module does
// NOT require the (huge, framework-less, upstream-churning) index.html to be
// rewritten with message keys. Instead it walks the DOM and, for any text /
// placeholder / title / aria-label whose English value appears in the catalog,
// swaps in the translation. A MutationObserver re-applies to nodes the app
// creates dynamically (the ~hundreds of `el.textContent = '...'` sites).
//
// Safety: only strings present in the catalog are ever touched, so user data
// (meeting titles, transcript text, names) is never mistranslated. Switching
// language is lossless because each translated node remembers its English
// source. Catalogs live in separate files (locales/<tag>.js) loaded before this
// script; adding a translation is a data-only edit, and adding a language means
// a new catalog file plus its tag in SUPPORTED below.
//
// Exposes window.MinutesI18n = { setLocale, getLocale, t }.
(function () {
  'use strict';

  var LS_KEY = 'minutes.ui.language';
  var ATTRS = ['placeholder', 'title', 'aria-label'];
  var SKIP_TAGS = { SCRIPT: 1, STYLE: 1, NOSCRIPT: 1, TEXTAREA: 1, TEMPLATE: 1 };

  // Per-node bookkeeping so switching locales (including back to English) is
  // lossless. Each record remembers the English source AND the exact value we
  // last wrote (`rendered`) — the latter is how we tell "the app replaced this
  // text" apart from "we translated it", which must not depend on the current
  // locale. WeakMaps don't leak removed nodes.
  var nodeState = new WeakMap(); // textNode -> { en, lead, trail, rendered }
  var attrState = new WeakMap(); // element  -> { [attr]: { en, rendered } }

  // Every non-English locale that ships a catalog. 'en' is implicit (identity).
  var SUPPORTED = ['zh-CN', 'pt-BR'];
  // Locales whose script needs a CJK font fallback appended to the Latin stack.
  var CJK_LOCALES = { 'zh-CN': 1 };

  var currentLocale = 'en';
  var catalogs = {};             // tag -> { exact: Map, patterns: [{re, to}] }

  function isSupported(locale) { return locale === 'en' || SUPPORTED.indexOf(locale) !== -1; }

  // ── Catalog ────────────────────────────────────────────────────────────
  function buildCatalog() {
    var all = window.__MINUTES_I18N || {};
    SUPPORTED.forEach(function (tag) {
      var entry = all[tag] || {};
      var exact = new Map();
      var pats = [];
      var strings = entry.strings || {};
      Object.keys(strings).forEach(function (k) { exact.set(k, strings[k]); });
      (entry.patterns || []).forEach(function (p) {
        try { pats.push({ re: new RegExp(p.re), to: p.to }); } catch (_) { /* skip bad rule */ }
      });
      catalogs[tag] = { exact: exact, patterns: pats };
    });
  }

  // Does this English string have a translation in ANY shipped catalog?
  //
  // Capture is locale-independent on purpose: a node captured while the UI is
  // in English must still be translatable after the user switches language
  // without a reload, so the test has to be the union across catalogs.
  function hasTranslation(s) {
    for (var i = 0; i < SUPPORTED.length; i++) {
      var c = catalogs[SUPPORTED[i]];
      if (!c) continue;
      if (c.exact.has(s)) return true;
      for (var j = 0; j < c.patterns.length; j++) { if (c.patterns[j].re.test(s)) return true; }
    }
    return false;
  }

  // Render an English source string in the given locale (identity for 'en').
  function valueFor(locale, en) {
    var c = catalogs[locale];
    if (!c) return en;
    if (c.exact.has(en)) return c.exact.get(en);
    for (var i = 0; i < c.patterns.length; i++) {
      if (c.patterns[i].re.test(en)) return en.replace(c.patterns[i].re, c.patterns[i].to);
    }
    return en;
  }

  // ── Skip logic ───────────────────────────────────────────────────────────
  function isSkipped(el) {
    if (SKIP_TAGS[el.tagName]) return true;
    if (el.hasAttribute('data-i18n-skip')) return true;
    // xterm.js renders the embedded terminal — never touch its content.
    if (el.classList && el.classList.contains('xterm')) return true;
    return false;
  }

  // Collapse internal whitespace runs so multi-line HTML text (indented source)
  // matches a plain single-spaced catalog key. Lookups use this normalized form.
  function norm(s) { return s.replace(/\s+/g, ' '); }

  // ── Text nodes ────────────────────────────────────────────────────────────
  function captureText(node, raw) {
    var trimmed = raw.trim();
    if (!trimmed) return null;
    var en = norm(trimmed);
    if (!hasTranslation(en)) return null;
    var start = raw.indexOf(trimmed);
    var rec = {
      en: en,
      lead: raw.slice(0, start),
      trail: raw.slice(start + trimmed.length),
      rendered: null
    };
    nodeState.set(node, rec);
    return rec;
  }

  function renderText(node, rec) {
    var out = rec.lead + valueFor(currentLocale, rec.en) + rec.trail;
    if (node.nodeValue !== out) node.nodeValue = out;
    rec.rendered = out; // remember exactly what we wrote (locale-independent bookkeeping)
  }

  function translateTextNode(node) {
    var parent = node.parentNode;
    if (!parent || parent.nodeType !== 1 || SKIP_TAGS[parent.tagName]) return;
    var raw = node.nodeValue;
    if (!raw || !raw.trim()) return;

    var rec = nodeState.get(node);
    if (rec) {
      // Unchanged since our last write → just re-render for the current locale.
      // Otherwise the app replaced the text → recapture from the new source.
      if (raw !== rec.rendered) {
        rec = captureText(node, raw);
        if (!rec) { nodeState.delete(node); return; }
      }
    } else {
      rec = captureText(node, raw);
      if (!rec) return;
    }
    renderText(node, rec);
  }

  // ── Attributes ────────────────────────────────────────────────────────────
  function captureAttr(el, attr, cur) {
    var en = norm((cur || '').trim());
    if (!en || !hasTranslation(en)) return null;
    var map = attrState.get(el);
    if (!map) { map = {}; attrState.set(el, map); }
    var rec = { en: en, rendered: null };
    map[attr] = rec;
    return rec;
  }

  function translateAttrs(el) {
    for (var i = 0; i < ATTRS.length; i++) {
      var attr = ATTRS[i];
      if (!el.hasAttribute(attr)) continue;
      var cur = el.getAttribute(attr);
      var map = attrState.get(el);
      var rec = map ? map[attr] : undefined;

      if (rec) {
        if (cur !== rec.rendered) { // app changed it → recapture
          rec = captureAttr(el, attr, cur);
          if (!rec) { delete map[attr]; continue; }
        }
      } else {
        rec = captureAttr(el, attr, cur);
        if (!rec) continue;
      }

      var out = valueFor(currentLocale, rec.en);
      if (cur !== out) el.setAttribute(attr, out);
      rec.rendered = out;
    }
  }

  // ── Tree walk ─────────────────────────────────────────────────────────────
  function walk(root) {
    if (!root) return;
    if (root.nodeType === 3) { translateTextNode(root); return; }
    if (root.nodeType !== 1) return;
    if (isSkipped(root)) return;

    translateAttrs(root); // the root element itself
    var walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
      acceptNode: function (n) {
        if (n.nodeType === 1 && isSkipped(n)) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      }
    });
    var cur;
    while ((cur = walker.nextNode())) {
      if (cur.nodeType === 3) translateTextNode(cur);
      else translateAttrs(cur);
    }
  }

  // ── Apply / observe ───────────────────────────────────────────────────────
  var FONT_CLASS = 'i18n-cjk-font';
  var fontStyleEl = null;

  // Scoped CJK font fallback. Geist / Instrument Serif / Geist Mono have no CJK
  // glyphs, so Chinese text would otherwise render in a system default that may
  // clash with the design. This injects a rule that keeps the app's original
  // Latin font first and adds a system CJK stack after it — only when the UI is
  // in Chinese, so the Latin design is untouched in English.
  function ensureCjkFontRule() {
    if (fontStyleEl) return;
    fontStyleEl = document.createElement('style');
    fontStyleEl.setAttribute('data-i18n-font', '');
    fontStyleEl.textContent =
      '.' + FONT_CLASS + ' {\n' +
      '  --font-sans: \'Geist\', -apple-system, BlinkMacSystemFont, "PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif;\n' +
      '  --font-display: \'Instrument Serif\', ui-serif, "Iowan Old Style", Georgia, "Songti SC", "SimSun", serif;\n' +
      '  --font-mono: \'Geist Mono\', \'SF Mono\', Menlo, Consolas, monospace;\n' +
      '}\n';
    (document.head || document.documentElement).appendChild(fontStyleEl);
  }

  function updateFontClass() {
    if (document.body) {
      document.body.classList.toggle(FONT_CLASS, !!CJK_LOCALES[currentLocale]);
    }
  }

  function apply(locale) {
    currentLocale = locale;
    if (document.documentElement) {
      document.documentElement.lang = isSupported(locale) ? locale : 'en';
    }
    if (CJK_LOCALES[locale]) ensureCjkFontRule();
    updateFontClass();
    walk(document.documentElement);
  }

  var observer = new MutationObserver(function (mutations) {
    for (var i = 0; i < mutations.length; i++) {
      var m = mutations[i];
      if (m.type === 'childList') {
        for (var j = 0; j < m.addedNodes.length; j++) walk(m.addedNodes[j]);
      } else if (m.type === 'characterData') {
        translateTextNode(m.target);
      } else if (m.type === 'attributes' && m.target.nodeType === 1) {
        translateAttrs(m.target);
      }
    }
  });

  function startObserver() {
    observer.observe(document.documentElement, {
      subtree: true,
      childList: true,
      characterData: true,
      attributes: true,
      attributeFilter: ATTRS
    });
  }

  // ── Locale resolution / backend reconciliation ────────────────────────────
  function initialLocale() {
    try {
      var saved = localStorage.getItem(LS_KEY);
      if (isSupported(saved)) return saved;
    } catch (_) { /* ignore */ }
    var nav = (navigator.language || '').toLowerCase();
    if (nav.indexOf('zh') === 0) return 'zh-CN';
    if (nav.indexOf('pt') === 0) return 'pt-BR';
    return 'en';
  }

  function refreshFromBackend() {
    try {
      var core = window.__TAURI__ && window.__TAURI__.core;
      if (!core || !core.invoke) return;
      core.invoke('cmd_get_ui_language').then(function (loc) {
        if (!isSupported(loc)) return;
        try { localStorage.setItem(LS_KEY, loc); } catch (_) { /* ignore */ }
        if (loc !== currentLocale) apply(loc);
      }).catch(function () { /* command may not exist yet — English fallback */ });
    } catch (_) { /* ignore */ }
  }

  // ── Public API ────────────────────────────────────────────────────────────
  window.MinutesI18n = {
    // Switch language in-place (also persists a client-side cache; callers that
    // want it remembered across sessions should also persist via the backend
    // command cmd_set_ui_language).
    setLocale: function (locale) {
      if (!isSupported(locale)) return;
      try { localStorage.setItem(LS_KEY, locale); } catch (_) { /* ignore */ }
      apply(locale);
    },
    getLocale: function () { return currentLocale; },
    // Direct string translation, for any code that builds strings imperatively.
    t: function (s) { return valueFor(currentLocale, norm(String(s).trim())); }
  };

  // ── Boot ──────────────────────────────────────────────────────────────────
  buildCatalog();
  currentLocale = initialLocale();
  startObserver();               // translate nodes as the body is parsed
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', function () {
      updateFontClass();         // body exists now
      apply(currentLocale);      // full-tree safety net after parse
      refreshFromBackend();      // reconcile with persisted config
    });
  } else {
    updateFontClass();
    apply(currentLocale);
    refreshFromBackend();
  }
})();
