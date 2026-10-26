import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import test from 'node:test';

const html = readFileSync(new URL('../tauri/src/index.html', import.meta.url), 'utf8');
const start = html.indexOf('    // ── Parakeet v3 install: the invitation banner');
const end = html.indexOf('    maybeShowParakeetInvite();', start);
assert(start > 0 && end > start);
const source = html.slice(start, end);

const MB = 1024 * 1024;
const idle = { offered: true, invite: true, switchesFromWhisper: true, downloadBytes: 639 * MB, phase: 'idle', done: 0, total: 0, error: null };

function element() {
  const classes = new Set();
  const listeners = {};
  return {
    hidden: false,
    textContent: '',
    style: {},
    classList: {
      toggle: (name, on) => (on ? classes.add(name) : classes.delete(name)),
      remove: (name) => classes.delete(name),
      contains: (name) => classes.has(name),
    },
    addEventListener: (type, fn) => { listeners[type] = fn; },
    click: () => listeners.click(),
  };
}

function harness(statuses, { needsSetup = false } = {}) {
  const elements = {};
  const calls = [];
  let reads = 0;
  const context = vm.createContext({
    document: { getElementById: (id) => (elements[id] ??= element()) },
    formatBytes: (bytes) => (bytes > 0 ? `${Math.round(bytes / MB)} MB` : ''),
    setTimeout: () => 0,
    clearTimeout: () => {},
    console,
    checkSetup: async () => calls.push('checkSetup'),
    loadSettings: async () => calls.push('loadSettings'),
    invoke: async (name) => {
      calls.push(name);
      if (name === 'cmd_parakeet_install_status') return statuses[Math.min(reads++, statuses.length - 1)];
      if (name === 'cmd_needs_setup') return { needsSetup };
      return null;
    },
  });
  vm.runInContext(source, context);
  return { context, elements, calls, run: (code) => vm.runInContext(code, context) };
}

test('the invitation names the download size and Not now dismisses it for this version', async () => {
  const { elements, calls, run } = harness([idle]);
  await run('maybeShowParakeetInvite()');
  assert(elements['parakeet-banner'].classList.contains('active'));
  assert.match(elements['parakeet-banner-sub'].textContent, /One-time download: 639 MB\.$/);
  assert.equal(elements['settings-parakeet-install'].hidden, false);
  assert.equal(elements['btn-settings-parakeet-install'].textContent, 'Install Parakeet v3 (639 MB)');
  assert.match(elements['settings-parakeet-install-status'].textContent, /from Whisper to Auto/);

  await elements['btn-parakeet-dismiss'].click();
  assert(calls.includes('cmd_dismiss_parakeet_invite'));
  assert(!elements['parakeet-banner'].classList.contains('active'));
});

test('nothing shows when the install is not offered', async () => {
  const { elements, run } = harness([{ ...idle, offered: false, invite: false }]);
  await run('maybeShowParakeetInvite()');
  assert(!elements['parakeet-banner'].classList.contains('active'));
  assert.equal(elements['settings-parakeet-install'].hidden, true);
});

test('installing shows progress, then reloads settings once and offers Done', async () => {
  const downloading = { ...idle, phase: 'downloading', done: 320 * MB, total: 639 * MB };
  const done = { ...idle, offered: false, invite: false, phase: 'done', done: 639 * MB, total: 639 * MB };
  const { elements, calls, run } = harness([idle, downloading, done, done]);
  await run('maybeShowParakeetInvite()');
  await elements['btn-parakeet-install'].click();
  assert(calls.includes('cmd_install_parakeet'));
  assert.equal(elements['parakeet-banner-title'].textContent, 'Downloading Parakeet v3');
  assert.equal(elements['parakeet-banner-meta'].textContent, '320 MB of 639 MB');
  assert.equal(elements['parakeet-banner-progress-fill'].style.width, `${(320 / 639) * 100}%`);
  assert.equal(elements['btn-parakeet-install'].hidden, true);
  assert.equal(elements['btn-parakeet-dismiss'].hidden, true);

  await run('refreshParakeetInstall()');
  assert.equal(elements['parakeet-banner-title'].textContent, 'Parakeet v3 is ready');
  assert.equal(elements['settings-parakeet-install-status'].textContent, 'Installed. Meetings and dictation now use Parakeet v3.');
  assert.deepEqual(calls.filter((c) => c === 'checkSetup' || c === 'loadSettings'), ['checkSetup', 'loadSettings']);

  // A settings reload after completion does not loop back into another reload.
  await run('refreshParakeetInstall()');
  assert.equal(calls.filter((c) => c === 'loadSettings').length, 1);

  assert.equal(elements['btn-parakeet-dismiss'].textContent, 'Done');
  await elements['btn-parakeet-dismiss'].click();
  assert(!calls.includes('cmd_dismiss_parakeet_invite'));
});

test('a failed install explains why and offers Retry', async () => {
  const failed = { ...idle, phase: 'error', error: 'Parakeet model file encoder.int8.onnx failed its checksum' };
  const { elements, run } = harness([failed]);
  await run('maybeShowParakeetInvite()');
  assert.equal(elements['parakeet-banner-title'].textContent, 'Parakeet v3 did not install');
  assert.match(elements['parakeet-banner-error'].textContent, /checksum/);
  assert.equal(elements['btn-parakeet-install'].textContent, 'Retry');
  assert.equal(elements['btn-settings-parakeet-install'].textContent, 'Retry install');
});

test('first-run setup hides the banner, since onboarding offers Parakeet itself', async () => {
  const { elements, run } = harness([idle], { needsSetup: true });
  await run('maybeShowParakeetInvite()');
  assert(!elements['parakeet-banner'].classList.contains('active'));
  // Settings still offers the install.
  assert.equal(elements['settings-parakeet-install'].hidden, false);
});
