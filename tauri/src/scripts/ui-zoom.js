(function () {
  var ZOOM_KEY = 'minutes.uiZoom';
  var TEXT_BASE = 16;
  var TEXT_MAX = 26;
  var ZOOM_EVENT = 'uizoom:changed';

  function applyZoom(next) {
    document.documentElement.style.setProperty('--text-scale-basis', next);
    document.dispatchEvent(new CustomEvent('uizoomchange', { detail: { value: next } }));
  }

  function resizeWindow(zoom) {
    if (!window.__TAURI__) return;
    var webviewWindow = window.__TAURI__.webviewWindow;
    if (!webviewWindow) return;
    var win = webviewWindow.getCurrentWebviewWindow();
    if (win.label === 'main') return;
    window.__TAURI__.core.invoke('cmd_scale_window', { label: win.label, zoom: zoom })
      .catch(function () {});
  }

  // Restore before CSS renders
  var saved = parseFloat(localStorage.getItem(ZOOM_KEY));
  if (saved && saved >= TEXT_BASE && saved <= TEXT_MAX) {
    document.documentElement.style.setProperty('--text-scale-basis', saved);
  }

  // Cross-window zoom sync via Tauri events.
  // BroadcastChannel is not guaranteed to propagate across separate WKWebView
  // processes on macOS; Tauri events are the reliable cross-window signal.
  // Skip if already at this value — Tauri emit echoes back to the sender, and
  // the sender already applied locally in the keydown handler.
  if (window.__TAURI__ && window.__TAURI__.event) {
    window.__TAURI__.event.listen(ZOOM_EVENT, function (e) {
      var value = e.payload.value;
      var current = parseFloat(
        getComputedStyle(document.documentElement).getPropertyValue('--text-scale-basis')
      ) || TEXT_BASE;
      if (Math.abs(current - value) < 0.1) return;
      applyZoom(value);
      resizeWindow(value);
    });
  }

  document.addEventListener('keydown', function (e) {
    if (!(e.metaKey || e.ctrlKey) || e.shiftKey || e.altKey) return;
    var cur, next;
    if (e.key === '=' || e.key === '+') {
      e.preventDefault();
      cur = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--text-scale-basis')) || TEXT_BASE;
      next = Math.min(TEXT_MAX, cur + 2);
      localStorage.setItem(ZOOM_KEY, next);
    } else if (e.key === '-') {
      e.preventDefault();
      cur = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--text-scale-basis')) || TEXT_BASE;
      next = Math.max(TEXT_BASE, cur - 2);
      localStorage.setItem(ZOOM_KEY, next);
    } else if (e.key === '0') {
      e.preventDefault();
      next = TEXT_BASE;
      localStorage.removeItem(ZOOM_KEY);
    } else {
      return;
    }
    // Apply locally immediately, then broadcast to other windows.
    applyZoom(next);
    resizeWindow(next);
    if (window.__TAURI__ && window.__TAURI__.event) {
      window.__TAURI__.event.emit(ZOOM_EVENT, { value: next })
        .catch(function () {});
    }
  });

  // On initial load: resize non-main window to persisted zoom.
  // Main window is handled elsewhere.
  
  // Rust owns each window's base size and computes base * (zoom/16), so resizing
  // never reads the window's current dimensions (which would compound on reopen).
  window.addEventListener('DOMContentLoaded', function () {
    if (!window.__TAURI__) return;
    var zoom = parseFloat(localStorage.getItem(ZOOM_KEY));
    if (!zoom || zoom === TEXT_BASE || zoom < TEXT_BASE || zoom > TEXT_MAX) return;
    var webviewWindow = window.__TAURI__.webviewWindow;
    if (!webviewWindow) return;
    var win = webviewWindow.getCurrentWebviewWindow();
    if (win.label === 'main') return;
    window.__TAURI__.core.invoke('cmd_scale_window', { label: win.label, zoom: zoom })
      .catch(function () {});
  });
})();
