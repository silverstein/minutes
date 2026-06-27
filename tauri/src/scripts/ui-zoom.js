(function () {
  var ZOOM_KEY = 'minutes.uiZoom';
  var TEXT_BASE = 16;
  var TEXT_MAX = 26;
  var channel = new BroadcastChannel(ZOOM_KEY);

  function applyZoom(next) {
    document.documentElement.style.setProperty('--text-scale-basis', next);
    document.dispatchEvent(new CustomEvent('uizoomchange', { detail: { value: next } }));
  }

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
})();
