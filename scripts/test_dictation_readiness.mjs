#!/usr/bin/env node
// Execute the shipped controllers. DOM/IPC doubles qualify UI behavior only.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { test } from 'node:test';
const source = name => readFileSync(new URL('../tauri/src/scripts/' + name, import.meta.url), 'utf8');
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const settle = async () => { for (let n = 0; n < 15; n++) await Promise.resolve(); };
function dom() {
  const elements = new Map(), observers = [], timers = new Map();
  let timerId = 0;
  class Target {
    listeners = new Map();
    addEventListener(name, run) { if (!this.listeners.has(name)) this.listeners.set(name, new Set()); this.listeners.get(name).add(run); }
    removeEventListener(name, run) { this.listeners.get(name)?.delete(run); }
    async emit(name, event = {}) { event.preventDefault ||= () => {}; event.stopImmediatePropagation ||= () => {}; for (const run of [...this.listeners.get(name) || []]) await run(event); }
  }
  class Element extends Target {
    className = ''; textContent = ''; children = []; dataset = {}; attributes = {}; disabled = false; value = ''; style = {};
    constructor(id) {
      super(); this.id = id;
      this.classList = { contains: name => this.className.split(' ').includes(name), add: name => { if (!this.classList.contains(name)) this.className += ' ' + name; }, remove: name => { this.className = this.className.split(' ').filter(x => x !== name).join(' '); } };
    }
    querySelectorAll(selector) { return this.children.flatMap(child => [...(selector === 'button' && child.type === 'button' ? [child] : []), ...child.querySelectorAll(selector)]); }
    append(...nodes) { this.children.push(...nodes); }
    replaceChildren(...nodes) { this.children = nodes; }
    setAttribute(name, value) { this.attributes[name] = value; }
    focus() { document.activeElement = this; }
    scrollIntoView() { this.scrolled = true; }
    contains(target) { return target === this || this.children.some(child => child.contains?.(target)); }
    closest() { return get('panel-dictation'); }
  }
  const document = new Target(), window = new Target();
  const get = id => { if (!elements.has(id)) elements.set(id, new Element(id)); return elements.get(id); };
  document.getElementById = get; document.createElement = () => new Element('');
  get('settings-overlay').classList.add('active'); get('panel-dictation').classList.add('is-active');
  const context = vm.createContext({ window, document, console, Error, Promise, MutationObserver: class { constructor(run) { observers.push(run); } observe() {} }, setTimeout: (run, ms) => { const id = ++timerId; timers.set(id, { run, ms }); return id; }, clearTimeout: id => timers.delete(id) });
  return { window, document, get, context, timers, mutate: async () => { observers.forEach(run => run()); await settle(); } };
}
function readiness(overrides = {}) {
  const h = dom(), calls = [], events = new Map();
  const values = {
    cmd_dictation_preferences: { model: { name: 'base', ready: true }, experience: { microphone_mode: 'system' } },
    cmd_shortcut_status: { enabled: true, pending: false, shortcut: 'CmdOrCtrl+Shift+Space', keycode: -1 },
    cmd_dictation_devices: { entries: [{ name: 'USB Microphone' }], candidates: [null], lidClosed: false },
    cmd_macos_permission_rows: [{ kind: 'microphone', runtimeUsable: true, status: 'granted' }, { kind: 'automation', runtimeUsable: false }],
    ...overrides,
  };
  h.get('shortcut-recorder-dictation').textContent = 'Command Shift Space'; h.get('settings-dictation-destination').value = 'insert';
  h.window.__TAURI__ = { core: { invoke: async (command, args) => { calls.push({ command, args: structuredClone(args) }); const value = values[command]; if (typeof value === 'function') return value(args); if (value instanceof Error) throw value; return structuredClone(value); } }, event: { listen: async (name, run) => events.set(name, run) } };
  let micTests = 0, help = 0;
  h.window.MinutesDictation = { show: () => {}, testMic: () => { micTests++; } };
  h.window.openDictationPermissionHelp = () => { help++; };
  vm.runInContext(source('dictation-readiness.js'), h.context);
  const row = label => h.get('dictation-ready-rows').children.find(root => root.children[0].children[0].textContent === label);
  return { ...h, calls, events, values, row, api: h.window.MinutesDictationReadiness, micTests: () => micTests, help: () => help };
}
test('readiness checks do not capture audio, download a model or write preferences', async () => {
  const h = readiness(); await h.api.refresh();
  assert.deepEqual(h.calls.map(x => x.command).sort(), ['cmd_dictation_devices','cmd_dictation_preferences','cmd_macos_permission_rows','cmd_shortcut_status'].sort());
  assert.equal(h.get('dictation-ready-title').textContent, 'Ready to dictate');
  assert.equal(h.get('dictation-ready-practice').disabled, false);
  assert.match(h.row('Text delivery').children[0].children[1].textContent, /may ask.*clipboard/);
});
test('missing model blocks practice and installs only the explicit selected model', async () => {
  const h = readiness({ cmd_dictation_preferences: { model: { name: 'small', ready: false }, experience: { microphone_mode: 'system' } } });
  await h.api.refresh(); assert.equal(h.get('dictation-ready-practice').disabled, true);
  h.values.cmd_download_model = () => { h.values.cmd_dictation_preferences.model.ready = true; return 'installed'; };
  await h.row('Speech model').children[1].children[0].emit('click');
  assert.deepEqual(h.calls.filter(x => x.command === 'cmd_download_model').map(x => x.args), [{ model: 'small' }]);
  assert.equal(h.get('dictation-ready-practice').disabled, false);
});
test('missing or closed-lid inputs block practice without claiming microphone success', async () => {
  const h = readiness({ cmd_dictation_devices: { entries: [{ name: 'MacBook Pro Microphone' }], candidates: [], lidClosed: true } });
  await h.api.refresh(); assert.equal(h.get('dictation-ready-practice').disabled, true);
  assert.match(h.row('Microphone').children[0].children[1].textContent, /No usable microphone/);
  await h.row('Microphone').children[1].children[0].emit('click'); assert.equal(h.micTests(), 1);
});
test('ungranted microphone access has an explicit action and never reports ready', async () => {
  const h = readiness({ cmd_macos_permission_rows: [{ kind: 'microphone', runtimeUsable: false, status: 'denied' }] });
  await h.api.refresh(); assert.equal(h.get('dictation-ready-practice').disabled, true);
  await h.row('Microphone').children[1].children[1].emit('click'); assert.equal(h.help(), 1);
});
test('disabled, starting and permission-blocked shortcuts each block practice', async () => {
  for (const state of [{ enabled: false }, { enabled: true, pending: true }, { enabled: true, needsPermission: true }]) {
    const h = readiness({ cmd_shortcut_status: state }); await h.api.refresh();
    assert.equal(h.get('dictation-ready-practice').disabled, true);
    assert.equal(h.row('Shortcut').dataset.state, 'attention');
  }
});
test('clipboard mode does not require permission to insert into an external app', async () => {
  const h = readiness(); h.get('settings-dictation-destination').value = 'clipboard'; await h.api.refresh();
  assert.equal(h.get('dictation-ready-practice').disabled, false);
  assert.match(h.row('Text delivery').children[0].children[1].textContent, /Paste them.*yourself/);
  assert.equal(h.row('Text delivery').children[1].children.length, 0);
});
test('failed inventory is actionable and successful recheck clears the error', async () => {
  const h = readiness({ cmd_macos_permission_rows: new Error('inventory unavailable') }); await h.api.refresh();
  assert.equal(h.get('dictation-ready-practice').disabled, true);
  assert.equal(h.get('dictation-ready-summary').dataset.error, 'true');
  h.values.cmd_macos_permission_rows = [{ kind: 'microphone', runtimeUsable: true }]; await h.api.refresh();
  assert.equal(h.get('dictation-ready-practice').disabled, false);
  assert.equal(h.get('dictation-ready-summary').dataset.error, 'false');
});
test('late stale inventory cannot replace a newer snapshot', async () => {
  const old = deferred(); const h = readiness({ cmd_dictation_preferences: () => old.promise });
  const first = h.api.refresh(); await settle();
  h.values.cmd_dictation_preferences = { model: { name: 'medium', ready: true }, experience: { microphone_mode: 'system' } };
  await h.api.refresh(); old.resolve({ model: { name: 'tiny', ready: false }, experience: { microphone_mode: 'system' } }); await first;
  assert.match(h.row('Speech model').children[0].children[1].textContent, /^medium is installed/);
  assert.equal(h.get('dictation-ready-practice').disabled, false);
});
test('slow inventory expires without freezing the recheck control', async () => {
  const blocked = deferred(); const h = readiness({ cmd_dictation_preferences: () => blocked.promise });
  const checking = h.api.refresh(); await settle();
  for (const timer of [...h.timers.values()]) if (timer.ms === 6000) timer.run();
  await checking; assert.equal(h.get('dictation-ready-refresh').disabled, false);
  assert.equal(h.get('dictation-ready-practice').disabled, true);
  blocked.resolve({ model: { name: 'base', ready: true } }); await settle();
  assert.equal(h.get('dictation-ready-practice').disabled, true);
});
test('practice reveals and focuses the actual text field without changing history', async () => {
  const h = readiness(); await h.api.refresh(); await h.get('dictation-ready-practice').emit('click');
  assert.equal(h.get('dictation-practice').open, true); assert.equal(h.document.activeElement.id, 'dictation-practice-text');
  assert.equal(h.calls.some(x => /save|set_setting|start/.test(x.command)), false);
});
test('download failure stays visible and preserves the selected model', async () => {
  const h = readiness({ cmd_dictation_preferences: { model: { name: 'small', ready: false }, experience: {} }, cmd_download_model: new Error('network unavailable') });
  await h.api.refresh(); await h.row('Speech model').children[1].children[0].emit('click');
  assert.match(h.get('dictation-ready-summary').textContent, /Could not install small.*network unavailable/);
  assert.equal(h.get('dictation-ready-practice').disabled, true); assert.equal(h.values.cmd_dictation_preferences.model.name, 'small');
});
test('healthy checks can be tucked away, while a new blocker expands them', async () => {
  const h = readiness(); await h.api.refresh();
  await h.get('dictation-ready-toggle').emit('click');
  assert.equal(h.get('dictation-ready-body').hidden, true);
  assert.equal(h.get('dictation-ready-toggle').attributes['aria-expanded'], 'false');
  assert.equal(h.get('dictation-ready-practice').disabled, false);
  h.values.cmd_dictation_preferences.model.ready = false; await h.api.refresh();
  assert.equal(h.get('dictation-ready-body').hidden, false);
  assert.equal(h.get('dictation-ready-toggle').hidden, true);
  assert.equal(h.get('dictation-ready-practice').disabled, true);
});
function recorder(behavior = {}) {
  const h = dom(), calls = [], updates = [], messages = [];
  let current = { enabled: true, shortcut: 'CmdOrCtrl+Alt+V', keycode: -1 };
  h.window.__TAURI__ = {};
  vm.runInContext(source('shortcut-recorder.js'), h.context);
  const invoke = async (command, args) => { calls.push({ command, args: structuredClone(args) }); if (behavior[command]) return behavior[command](args); if (command === 'cmd_probe_shortcut') return { supported: true, needs_permission: false }; if (command === 'cmd_set_shortcut') return { ...args }; };
  function attach(id, slot) {
    const button = h.get(id);
    h.window.MinutesShortcutRecorder.attach({ button, slot, standardOnly: true, read: () => current, build: event => event.choice || null,
      label: text => text, invoke, update: state => { current = state; updates.push(state); button.textContent = state.shortcut; }, status: text => messages.push(text) });
    return button;
  }
  const button = attach('recorder', 'dictation_paste_last');
  return { ...h, button, attach, calls, updates, messages, api: h.window.MinutesShortcutRecorder, current: () => current };
}
const key = (h, choice) => h.document.emit('keydown', { choice, key: 'V', code: 'KeyV' });
test('Escape restores an enabled recovery shortcut exactly once', async () => {
  const h = recorder(); await h.button.emit('click'); await h.document.emit('keydown', { key: 'Escape' });
  assert.equal(h.calls.filter(x => x.command === 'cmd_set_shortcut').length, 1);
  assert.equal(h.current().shortcut, 'CmdOrCtrl+Alt+V'); assert.equal(h.button.attributes['aria-pressed'], 'false');
});
test('a new recovery combination is saved once using the shared command', async () => {
  const h = recorder(); await h.button.emit('click'); await key(h, { shortcut: 'CmdOrCtrl+Shift+V', keycode: 9 });
  assert.equal(h.calls.filter(x => x.command === 'cmd_set_shortcut').length, 1);
  assert.equal(h.current().shortcut, 'CmdOrCtrl+Shift+V'); assert.equal(h.current().enabled, true);
});
test('failed shortcut save restores old keys and reports the conflict', async () => {
  const h = recorder({ cmd_set_shortcut: args => { if (args.shortcut !== 'CmdOrCtrl+Alt+V') throw new Error('already used'); return args; } });
  await h.button.emit('click'); await key(h, { shortcut: 'CmdOrCtrl+Shift+V', keycode: 9 });
  assert.equal(h.current().shortcut, 'CmdOrCtrl+Alt+V'); assert.match(h.messages.at(-1), /not changed.*already used/);
});
test('failed suspension preserves the old binding and never starts capturing keys', async () => {
  const h = recorder({ cmd_suspend_shortcut: () => { throw new Error('busy'); } }); await h.button.emit('click');
  assert.equal(h.current().enabled, true); assert.equal(h.calls.some(x => x.command === 'cmd_set_shortcut'), false);
  assert.equal(h.document.listeners.get('keydown')?.size || 0, 0); assert.match(h.messages.at(-1), /Could not prepare/);
});
test('focus loss and closing settings each restore the previous binding', async () => {
  for (const exit of ['blur', 'close']) {
    const h = recorder(); await h.button.emit('click');
    if (exit === 'blur') await h.window.emit('blur');
    else { h.get('settings-overlay').classList.remove('active'); await h.mutate(); }
    assert.equal(h.calls.filter(x => x.command === 'cmd_set_shortcut').length, 1); assert.equal(h.current().enabled, true);
  }
});
test('switching recorders restores the first before suspending the next', async () => {
  const h = recorder(); const second = h.attach('second', 'dictation_history'); await h.button.emit('click'); await second.emit('click');
  assert.deepEqual(h.calls.map(x => x.command), ['cmd_suspend_shortcut','cmd_set_shortcut','cmd_suspend_shortcut']);
  await h.api.cancel();
});
test('recovery recorder rejects standalone Caps Lock and keeps waiting', async () => {
  const h = recorder(); await h.button.emit('click'); await key(h, { shortcut: 'CapsLock', keycode: 57 });
  assert.equal(h.calls.some(x => x.command === 'cmd_set_shortcut'), false); assert.match(h.messages.at(-1), /key combination/);
  assert.equal(h.button.attributes['aria-pressed'], 'true'); await h.api.cancel();
});
test('cancelling while suspension is pending restores after it settles', async () => {
  const wait = deferred(); const h = recorder({ cmd_suspend_shortcut: () => wait.promise });
  const start = h.button.emit('click'); await settle(); const cancelled = h.api.cancel(); await settle(); wait.resolve(); await start; await cancelled;
  assert.equal(h.calls.filter(x => x.command === 'cmd_set_shortcut').length, 1); assert.equal(h.button.disabled, false);
  assert.equal(h.document.listeners.get('keydown')?.size || 0, 0);
});
test('a late probe cannot save new keys after cancellation', async () => {
  const wait = deferred(); const h = recorder({ cmd_probe_shortcut: () => wait.promise }); await h.button.emit('click');
  const probe = key(h, { shortcut: 'CmdOrCtrl+Shift+V', keycode: 9 }); await settle(); await h.api.cancel(); wait.resolve({ supported: true }); await probe;
  const writes = h.calls.filter(x => x.command === 'cmd_set_shortcut'); assert.equal(writes.length, 1); assert.equal(writes[0].args.shortcut, 'CmdOrCtrl+Alt+V');
});
test('failed restoration is reported without pretending the shortcut is enabled', async () => {
  const h = recorder({ cmd_set_shortcut: () => { throw new Error('registration failed'); } }); await h.button.emit('click');
  assert.equal(await h.api.cancel(), false); assert.equal(h.current().enabled, false); assert.match(h.messages.at(-1), /Could not restore/); assert.equal(h.button.disabled, false);
});
