// Shared xterm.js theme for Minutes — used by both index.html (Recall panel)
// and terminal.html (standalone fallback).
function getMinutesTheme(isDark) {
  if (isDark) {
    return {
      background: '#0d0d0b',
      foreground: '#e8e4da',
      cursor: '#30d158',
      cursorAccent: '#0d0d0b',
      selectionBackground: 'rgba(48, 209, 88, 0.18)',
      selectionForeground: '#e8e4da',
      black: '#0d0d0b',
      red: '#ff453a',
      green: '#30d158',
      yellow: '#d4832a',
      blue: '#c96b4e',
      magenta: '#bf5af2',
      cyan: '#7fc7b7',
      white: '#e8e4da',
      brightBlack: '#6b6760',
      brightRed: '#ff6b62',
      brightGreen: '#4ade70',
      brightYellow: '#e2a24f',
      brightBlue: '#db8a6f',
      brightMagenta: '#d789ff',
      brightCyan: '#9ed6c9',
      brightWhite: '#f8f4ed',
    };
  }

  return {
    background: '#f8f4ed',
    foreground: '#1a1916',
    cursor: '#c96b4e',
    cursorAccent: '#f8f4ed',
    selectionBackground: 'rgba(201, 107, 78, 0.16)',
    selectionForeground: '#1a1916',
    black: '#1a1916',
    red: '#c0392b',
    green: '#2e7d46',
    yellow: '#b8731e',
    blue: '#c96b4e',
    magenta: '#8b5cf6',
    cyan: '#4f8f86',
    white: '#efebe2',
    brightBlack: '#8c8880',
    brightRed: '#d44f3f',
    brightGreen: '#3e8f58',
    brightYellow: '#c88932',
    brightBlue: '#d67c5f',
    brightMagenta: '#9b6cff',
    brightCyan: '#63a39a',
    brightWhite: '#ffffff',
  };
}

const minutesThemeMediaQuery = window.matchMedia('(prefers-color-scheme: dark)');

window.getMinutesTheme = getMinutesTheme;
window.MINUTES_XTERM_THEME_QUERY = minutesThemeMediaQuery;
window.MINUTES_XTERM_THEME = getMinutesTheme(minutesThemeMediaQuery.matches);
