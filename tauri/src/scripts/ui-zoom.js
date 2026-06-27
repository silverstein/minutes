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
  // Skips the main window — its width is managed by the recall panel after load.
  // Delegates to Rust (cmd_scale_window) — JS-side setSize is silently ignored
  // for these windows.
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
