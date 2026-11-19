#!/usr/bin/env node
// Settings swaps its global-shortcut pickers for compositor-binding notes on
// Wayland, where the compositor owns global shortcuts (#64). These checks run
// the real `applyWaylandShortcutUI` from index.html against a stub DOM and
// pin the markup/CSS contract the swap relies on.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const html = readFileSync(new URL('../tauri/src/index.html', import.meta.url), 'utf8');
const script = [...html.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)]
  .map((match) => match[1])
  .find((source) => source.includes('applyWaylandShortcutUI'));
assert.ok(script, 'the settings script must define applyWaylandShortcutUI');

function extractFunction(source, name) {
  const start = source.indexOf(`function ${name}(`);
  assert.ok(start >= 0, name);
  const body = source.indexOf('{', source.indexOf(')', start));
  let depth = 0;
  for (let index = body; index < source.length; index++) {
    if (source[index] === '{') depth++;
    if (source[index] === '}' && --depth === 0) return source.slice(start, index + 1);
  }
  throw new Error(`Unterminated ${name}`);
}

const applyWaylandShortcutUI = extractFunction(script, 'applyWaylandShortcutUI');

function row() {
  const note = { hidden: true };
  const classes = new Set();
  return {
    note,
    classes,
    element: {
      classList: { add: (name) => classes.add(name) },
      querySelector: (selector) => (selector === '.wayland-shortcut-note' ? note : null),
    },
  };
}

function run(waylandSession, rows) {
  const context = vm.createContext({
    desktopCapabilities: { waylandSession },
    document: {
      querySelectorAll: (selector) =>
        selector === '[data-global-shortcut], [data-wayland-hide]' ? rows.map((r) => r.element) : [],
    },
  });
  vm.runInContext(`${applyWaylandShortcutUI}\napplyWaylandShortcutUI();`, context);
}

test('pickers hide and notes show when waylandSession is true', () => {
  const rows = [row(), row(), row()];
  run(true, rows);
  for (const r of rows) {
    assert.ok(r.classes.has('on-wayland'), 'row must be marked on-wayland so its controls hide');
    assert.equal(r.note.hidden, false, 'compositor instructions must be revealed');
  }
});

test('pickers stay untouched when waylandSession is false', () => {
  const rows = [row()];
  run(false, rows);
  assert.equal(rows[0].classes.size, 0);
  assert.equal(rows[0].note.hidden, true);
});

test('every tagged picker row ships a wayland-shortcut-note', () => {
  const taggedRows = html.match(/data-global-shortcut>/g) || [];
  const notes = html.match(/class="[^"]*wayland-shortcut-note" hidden/g) || [];
  assert.equal(taggedRows.length, notes.length, 'each picker row needs an instruction note');
  assert.ok(taggedRows.length >= 6, 'quick thought, live, palette, dictation, and recovery rows');
});

test('css hides picker controls under on-wayland', () => {
  for (const needle of [
    '[data-global-shortcut].on-wayland .settings-section-inline',
    '[data-global-shortcut].on-wayland .shortcut-select',
    '[data-global-shortcut].on-wayland>.toggle-btn',
    '[data-wayland-hide].on-wayland',
  ]) {
    assert.ok(html.includes(needle), needle);
  }
});

test('notes bind the toggle CLI commands for each picker', () => {
  for (const needle of [
    'minutes toggle-recording --mode quick-thought',
    'minutes toggle-recording --mode live',
    'minutes toggle-dictation',
    'bind = SUPER, R, exec, minutes toggle-recording --mode quick-thought',
    'bind = SUPER, D, exec, minutes toggle-dictation',
  ]) {
    assert.ok(html.includes(needle), needle);
  }
});
