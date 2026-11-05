// Applies the accent from customize.toml. Rust resolves and validates the theme
// and stays the single authority: this file only paints what it is given. The
// appearance itself (light or dark) is forced natively, so prefers-color-scheme
// is already correct by the time this runs.
(function () {
  const STYLE_ID = 'minutes-customize-theme';

  function apply(theme) {
    window.__MINUTES_THEME__ = theme || null;
    let style = document.getElementById(STYLE_ID);
    if (theme && theme.css) {
      if (!style) {
        style = document.createElement('style');
        style.id = STYLE_ID;
        (document.head || document.documentElement).appendChild(style);
      }
      style.textContent = theme.css;
    } else if (style) {
      style.remove();
    }
    document.dispatchEvent(new CustomEvent('minutes-theme-changed'));
  }

  // The value set at launch paints first, then the file is re-read so a hand
  // edit shows up as soon as a window opens.
  apply(window.__MINUTES_THEME__);
  const tauri = window.__TAURI__;
  if (!tauri || !tauri.core || !tauri.event) return;
  tauri.core.invoke('cmd_get_customization').then(apply).catch(() => {});
  tauri.event.listen('customize-theme-changed', (event) => apply(event.payload)).catch(() => {});
})();
