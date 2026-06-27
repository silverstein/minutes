(function () {
  var ZOOM_KEY = 'minutes.uiZoom';
  var TEXT_BASE = 16;
  var TEXT_MAX = 26;
  var channel = new BroadcastChannel(ZOOM_KEY);

  function applyZoom(next) {
    document.documentElement.style.setProperty('--text-scale-basis', next);
    document.dispatchEvent(new CustomEvent('uizoomchange', { detail: { value: next } }));
  }

  // Restore before CSS renders
  var saved = parseFloat(localStorage.getItem(ZOOM_KEY));
  if (saved && saved >= TEXT_BASE && saved <= TEXT_MAX) {
    document.documentElement.style.setProperty('--text-scale-basis', saved);
  }

  channel.onmessage = function (e) {
    applyZoom(e.data.value);
  };

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
    applyZoom(next);
    channel.postMessage({ value: next });
  });

  // On initial load: resize window proportionally to saved zoom.
  // Skips the main window — its dimensions are managed by applyRecallWindowLayout
  // (which reads the zoom ratio at call time) so zoom is applied automatically on
  // expand/collapse and on uizoomchange. Secondary windows are resized here.
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
