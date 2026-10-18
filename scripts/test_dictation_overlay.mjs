#!/usr/bin/env node
// Execute the shipped HUD script against deterministic DOM/audio/Tauri doubles.
// These checks prove presentation behavior, not native capture or insertion.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { test } from 'node:test';

const html = readFileSync(new URL('../tauri/src/dictation-overlay.html', import.meta.url), 'utf8');
const settingsHtml = readFileSync(new URL('../tauri/src/index.html', import.meta.url), 'utf8');
const script = [...html.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)]
  .map((match) => match[1]).find((source) => source.includes('renderOverlaySnapshot'));
assert.ok(script, 'the actual overlay controller must be executed');

function harness({ muted = false } = {}) {
  const elements = new Map();
  class Element {
    constructor() {
      this.className = ''; this.style = {}; this.textContent = '';
      this.listeners = new Map(); this.disabled = false;
      this.classList = {
        contains: (name) => this.className.split(/\s+/).includes(name),
        add: (name) => { if (!this.classList.contains(name)) this.className += ` ${name}`; },
        remove: (name) => { this.className = this.className.split(/\s+/).filter((v) => v !== name).join(' '); },
        toggle: (name, force) => {
          const on = force ?? !this.classList.contains(name);
          this.classList[on ? 'add' : 'remove'](name); return on;
        },
      };
    }
    addEventListener(name, callback) { this.listeners.set(name, callback); }
    insertBefore(element) { elements.set(element.id, element); }
    remove() { elements.delete(this.id); }
    querySelectorAll() { return bars; }
  }
  for (const match of html.matchAll(/id="([^"]+)"/g)) {
    const element = new Element(); element.id = match[1]; elements.set(element.id, element);
  }
  const bars = Array.from({ length: 7 }, () => new Element());
  const events = new Map(); const windowEvents = new Map(); const played = []; const playing = new Set();
  const commands = []; const timers = new Map(); let timerId = 0;
  class Audio {
    constructor(path) { this.name = path.match(/cue-(\w+)\.wav/)[1]; }
    pause() { playing.delete(this.name); }
    play() { played.push(this.name); playing.add(this.name); return Promise.resolve(); }
  }
  const context = vm.createContext({
    console, Audio, Date, localStorage: { getItem: () => muted ? 'false' : 'true' },
    document: {
      getElementById: (id) => elements.get(id) || null,
      createElement: () => new Element(), addEventListener: () => {},
    },
    window: {
      addEventListener: (name, callback) => windowEvents.set(name, callback),
      __TAURI__: {
        event: { listen: (name, callback) => { events.set(name, callback); return Promise.resolve(); } },
        core: { invoke: (command) => { commands.push(command); return Promise.resolve(null); } },
        window: { getCurrentWindow: () => ({ startDragging: async () => {}, close: async () => {} }) },
      },
    },
    setTimeout: (fn, ms) => { timers.set(++timerId, { fn, ms }); return timerId; },
    clearTimeout: (id) => timers.delete(id), setInterval: () => ++timerId, clearInterval: () => {},
    requestAnimationFrame: (fn) => fn(),
  });
  vm.runInContext(script, context, { filename: fileURLToPath(new URL('../tauri/src/dictation-overlay.html', import.meta.url)) });
  let revision = 0;
  return {
    elements, bars, played, playing, commands, timers,
    emit: (name, payload) => events.get(name)({ payload }),
    state: (state, captureStyle = 'locked', customRevision) => events.get('dictation:overlay')({
      payload: { state, captureStyle, revision: customRevision ?? ++revision },
    }),
    click: (id) => elements.get(id).listeners.get('click')({ stopPropagation() {} }),
    mute: () => windowEvents.get('storage')({ key: 'minutes.playCaptureCues', newValue: 'false' }),
  };
}

test('phrase success cannot flash delivery or play completion while recording', () => {
  const h = harness(); h.state('listening'); h.state('success');
  assert.deepEqual(h.played, []);
  assert.equal(h.elements.get('label').textContent, '');
  assert.equal(h.elements.get('capture-controls').classList.contains('hidden'), false);
});

test('finishing acknowledges once; verified delivery stays compact and does not overlap cues', () => {
  const h = harness(); h.state('listening'); h.state('processing'); h.state('processing');
  h.state('inserting');
  assert.equal(h.elements.get('pill').classList.contains('expanded'), false);
  assert.equal(h.elements.get('label').title, '');
  h.state('typed'); h.state('typed');
  assert.deepEqual(h.played, ['complete']);
  assert.deepEqual([...h.playing], ['complete']);
  assert.equal(h.elements.get('pill').classList.contains('expanded'), false);
  assert.equal(h.elements.get('label').textContent, '');
});

test('copy fallback exposes recovery and uses a failure cue rather than claiming insertion', () => {
  const h = harness(); h.state('listening');
  h.emit('dictation:insertion', { outcome: 'copied', method: 'clipboard_only', message: 'Could not type into the active app. Copied dictation instead.' });
  h.state('copied');
  assert.deepEqual(h.played, ['error']);
  assert.equal(h.elements.get('pill').classList.contains('expanded'), true);
  assert.equal(h.elements.get('permission-button').classList.contains('hidden'), false);
});

test('blocked delivery and recoverable audio never sound like successful delivery', () => {
  for (const state of ['blocked', 'recoverable']) {
    const h = harness(); h.state('listening'); h.state(state);
    assert.deepEqual(h.played, ['error']);
  }
});

test('cancel is quiet and stops any cue still playing', () => {
  const h = harness(); h.state('listening'); h.state('cancelled');
  assert.deepEqual(h.played, []); assert.equal(h.playing.size, 0);
});

test('muted preference prevents all cues, including live preference changes', () => {
  const muted = harness({ muted: true }); muted.state('listening'); muted.state('processing'); muted.state('typed');
  assert.deepEqual(muted.played, []);
  const live = harness(); live.state('listening'); live.mute(); live.state('processing'); live.state('typed');
  assert.deepEqual(live.played, []);
  assert.equal(live.playing.size, 0);
});

test('an explicit new session resets cue latches without replaying on capture-style changes', () => {
  const h = harness(); h.state('starting'); h.state('listening'); h.state('accumulating', 'held');
  h.state('accumulating', 'locked'); h.state('typed'); h.state('starting'); h.state('listening');
  assert.deepEqual(h.played, ['complete']);
});

test('stale snapshots cannot replay cues or regress the current state', () => {
  const h = harness(); h.state('listening', 'locked', 10); h.state('typed', 'locked', 11);
  h.state('listening', 'locked', 10); h.state('blocked', 'locked', 11);
  assert.deepEqual(h.played, ['complete']);
  assert.equal(h.elements.get('label').textContent, '');
});

test('waveform has a 16px basis, measured dynamic range, finite bounds and no fake idle motion', () => {
  assert.equal(/class="bar"\s+style="height:4px"/.test(html), false);
  assert.match(html, /\.waveform \.bar\s*\{[^}]*height:\s*16px/);
  const h = harness(); h.state('listening'); h.emit('dictation:level', 0);
  assert.ok(h.bars.every((bar) => bar.style.transform === 'scaleY(0.25)'));
  h.emit('dictation:level', 100); const loud = h.bars.map((bar) => bar.style.transform);
  assert.equal(h.bars[3].style.transform, 'scaleY(1)');
  h.emit('dictation:level', Infinity); assert.deepEqual(h.bars.map((bar) => bar.style.transform), loud);
  h.emit('dictation:level', 500); assert.deepEqual(h.bars.map((bar) => bar.style.transform), loud);
  h.state('processing'); h.emit('dictation:level', 0);
  assert.deepEqual(h.bars.map((bar) => bar.style.transform), loud);
});

test('dictation sound control writes the shared preference and synchronizes both settings views', () => {
  const elements = new Map(['settings-cues', 'dictation-cues'].map((id) => [id, {
    style: {}, textContent: '', attributes: {},
    setAttribute(name, value) { this.attributes[name] = value; },
  }]));
  const saved = new Map();
  const globals = {
    playCaptureCues: true, cueToggle: null, console,
    localStorage: { setItem: (key, value) => saved.set(key, value) },
    document: { getElementById: (id) => elements.get(id) },
  };
  const context = vm.createContext(globals);
  for (const name of ['setSettingsToggle', 'setCueToggle']) {
    const source = settingsHtml.match(new RegExp(`    function ${name}\\([^]*?^    }`, 'm'))?.[0];
    assert.ok(source); vm.runInContext(source, context);
  }
  vm.runInContext('setCueToggle(false)', context);
  assert.equal(saved.get('minutes.playCaptureCues'), 'false');
  assert.equal(globals.playCaptureCues, false);
  for (const element of elements.values()) assert.equal(element.textContent, 'Off');
  assert.equal(elements.get('dictation-cues').attributes['aria-pressed'], 'false');
  vm.runInContext('setCueToggle(true)', context);
  for (const element of elements.values()) assert.equal(element.textContent, 'On');
});

test('held capture keeps release guidance; buttons use the existing native finish and cancel commands', () => {
  const h = harness(); h.state('listening', 'held');
  assert.equal(h.elements.get('finish-button').classList.contains('hidden'), true);
  assert.equal(h.elements.get('label').textContent, '');
  assert.match(h.elements.get('pill').title, /Release to finish/);
  assert.equal(h.elements.get('pill').classList.contains('capturing'), true);
  h.state('accumulating', 'locked'); h.click('finish-button'); h.click('cancel-button');
  assert.ok(h.commands.includes('cmd_stop_dictation')); assert.ok(h.commands.includes('cmd_cancel_dictation'));
});


test('startup and insertion share a quiet compact shell without internal phase labels', () => {
  assert.match(html, /class="pill compact busy"/);
  assert.match(html, /id="label"><\/span>/);
  const h = harness();
  for (const state of ['starting', 'loading', 'processing', 'inserting']) {
    h.state(state);
    assert.equal(h.elements.get('pill').classList.contains('compact'), true);
    assert.equal(h.elements.get('pill').classList.contains('busy'), true);
    assert.equal(h.elements.get('label').textContent, '');
    assert.equal(h.elements.get('waveform').style.display, 'none');
    assert.equal(h.elements.get('timer').style.display, 'none');
  }
  assert.equal(h.elements.get('announcement').textContent, 'Dictation processing.');
});

test('clipboard-only outcome offers text on demand without claiming insertion', () => {
  const h = harness(); h.state('starting'); h.state('listening');
  h.emit('dictation:result', 'The blue bicycle arrives Thursday.');
  h.state('copied');
  assert.equal(h.elements.get('pill').classList.contains('clipboard'), true);
  assert.equal(h.elements.get('pill').classList.contains('compact'), true);
  assert.equal(h.elements.get('pill').classList.contains('expanded'), false);
  assert.equal(h.elements.get('announcement').textContent, 'Dictation copied to clipboard.');
  h.click('clipboard-button');
  assert.equal(h.elements.get('pill').classList.contains('expanded'), true);
  assert.equal(h.elements.get('pill').classList.contains('compact'), false);
  assert.equal(h.elements.get('text-content').textContent, 'The blue bicycle arrives Thursday.');
  assert.equal([...h.timers.values()].some((timer) => timer.ms === 2400), false);
});

test('verified insertion dismisses once; a fresh session cancels the pending close', () => {
  for (const outcome of ['typed', 'pasted']) {
    const h = harness(); h.state('listening'); h.state('processing'); h.state('inserting');
    h.state(outcome); h.state(outcome);
    assert.equal(h.elements.get('pill').classList.contains('dismissing'), true);
    assert.equal([...h.timers.values()].filter((timer) => timer.ms === 150).length, 1);
    h.state('starting');
    assert.equal(h.elements.get('pill').classList.contains('dismissing'), false);
    assert.equal([...h.timers.values()].filter((timer) => timer.ms === 150).length, 0);
  }
});


test('capture remains silent with sounds enabled, including rapid restart after delivery', () => {
  const h = harness();
  h.state('starting'); h.state('loading'); h.state('listening');
  h.state('accumulating', 'held'); h.state('accumulating', 'locked');
  h.state('success'); h.state('processing'); h.state('inserting');
  assert.deepEqual(h.played, []);
  h.state('pasted');
  assert.deepEqual([...h.playing], ['complete']);
  h.state('starting');
  assert.equal(h.playing.size, 0, 'a prior delivery tone cannot enter the next capture');
  h.state('listening');
  assert.deepEqual(h.played, ['complete']);
  // Older backends can resume directly with Listening rather than Starting.
  h.state('typed'); h.state('listening');
  assert.equal(h.playing.size, 0);
});
