(function () {
  var saved = parseFloat(localStorage.getItem('minutes.uiZoom'));
  if (saved && saved >= 16 && saved <= 26) {
    document.documentElement.style.setProperty('--text-scale-basis', saved);
  }
})();
