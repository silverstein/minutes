import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import test from 'node:test';

const html = readFileSync(new URL('../tauri/src/index.html', import.meta.url), 'utf8');
function slice(from, to) {
  const start = html.indexOf(from);
  const end = html.indexOf(to, start);
  assert(start > 0 && end > start, from);
  return html.slice(start, end);
}
const source = slice('    async function installOnboardingModels(', '    function setupOnboarding(')
  + slice('    function finishOnboardingModelStep(', '    // Shown when the meeting list could not be read.');

const MB = 1024 * 1024;

function element(tag = 'div', props = {}) {
  const classes = new Set((props.className || '').split(' ').filter(Boolean));
  const el = {
    tag, children: [], dataset: { ...(props.dataset || {}) }, textContent: props.textContent || '',
    disabled: false, style: {}, listeners: {},
    get className() { return [...classes].join(' '); },
    set className(value) { classes.clear(); value.split(' ').filter(Boolean).forEach((c) => classes.add(c)); },
    classList: {
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
      contains: (c) => classes.has(c),
      toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)),
      replace: (a, b) => { if (classes.delete(a)) classes.add(b); },
    },
    addEventListener: (type, fn) => { el.listeners[type] = fn; },
    prepend: (child) => el.children.unshift(child),
    querySelectorAll: (selector) => el.children.filter((c) => c.matches(selector)),
    querySelector: (selector) => el.children.find((c) => c.matches(selector)) || null,
    matches: (selector) => {
      if (selector === '.model-btn') return classes.has('model-btn');
      const data = selector.match(/^\[data-model="(.+)"\]$/);
      return data ? el.dataset.model === data[1] : false;
    },
  };
  return el;
}

function harness({ offered = true, statuses = [], needsSetupAfter = false } = {}) {
  const options = element('div');
  options.children.push(
    element('button', { className: 'btn btn-primary btn-sm model-btn', dataset: { model: 'small' }, textContent: 'Small (466 MB, recommended)' }),
    element('button', { className: 'btn btn-secondary btn-sm model-btn', dataset: { model: 'base' }, textContent: 'Base (142 MB, lower resource use)' }),
  );
  const nodes = {
    '.model-options': options,
    '#step-model-body p': element('p', { textContent: 'Choose a meeting model.' }),
    '.download-progress': element('div'),
    '.progress-fill': element('div'),
    '.progress-text': element('span'),
    '#step-model': element('div'),
    '#step-model-status': element('span'),
    '#step-model-body': element('div'),
  };
  const root = {
    querySelector: (selector) => nodes[selector] || null,
    querySelectorAll: (selector) => options.querySelectorAll(selector),
  };
  const calls = [];
  let statusReads = 0;
  const idle = { offered, invite: offered, downloadBytes: 639 * MB, phase: 'idle', done: 0, total: 0, error: null };
  const context = vm.createContext({
    document: { createElement: (tag) => element(tag) },
    formatBytes: (bytes) => (bytes > 0 ? `${Math.round(bytes / MB)} MB` : ''),
    setTimeout: (fn) => fn(),
    console,
    currentSetup: null,
    activationState: {},
    applyActivationState: () => {},
    invoke: async (name, args) => {
      calls.push(args ? { name, args: JSON.parse(JSON.stringify(args)) } : { name });
      if (name === 'cmd_parakeet_install_status') {
        return statusReads++ === 0 ? idle : statuses[Math.min(statusReads - 2, statuses.length - 1)];
      }
      if (name === 'cmd_needs_setup') {
        const after = calls.some((c) => c.name === 'cmd_set_setting');
        return { needsSetup: after ? needsSetupAfter : true, modelSelections: { dictation: 'base', standaloneLive: '' } };
      }
      return 'ok';
    },
  });
  vm.runInContext(source, context);
  return { root, nodes, options, calls, run: (code) => vm.runInContext(code, context), context };
}

test('where Parakeet v3 is offered, onboarding recommends it and keeps Whisper as alternatives', async () => {
  const { context, root, options, nodes } = harness();
  await context.offerParakeetInOnboarding(root);
  const [first, small, base] = options.children;
  assert.equal(first.dataset.model, 'parakeet');
  assert.equal(first.textContent, 'Parakeet v3 (781 MB, recommended)');
  assert(first.classList.contains('btn-primary'));
  assert.equal(small.textContent, 'Whisper Small (466 MB)');
  assert(small.classList.contains('btn-secondary') && !small.classList.contains('btn-primary'));
  assert.equal(base.textContent, 'Whisper Base (142 MB, lower resource use)');
  assert.match(nodes['#step-model-body p'].textContent, /comes with Whisper Base/);

  // Rendering twice does not add a second Parakeet button.
  await context.offerParakeetInOnboarding(root);
  assert.equal(options.children.filter((c) => c.dataset.model === 'parakeet').length, 1);
});

test('where Parakeet v3 is not offered, onboarding is unchanged', async () => {
  const { context, root, options } = harness({ offered: false });
  await context.offerParakeetInOnboarding(root);
  assert.deepEqual(options.children.map((c) => c.dataset.model), ['small', 'base']);
  assert.equal(options.children[0].textContent, 'Small (466 MB, recommended)');
});

test('choosing Parakeet installs Whisper Base first, then Parakeet with real progress', async () => {
  const downloading = { phase: 'downloading', done: 320 * MB, total: 639 * MB };
  const done = { phase: 'done', done: 639 * MB, total: 639 * MB };
  const { context, root, nodes, calls } = harness({ statuses: [downloading, done] });
  await context.offerParakeetInOnboarding(root);
  await context.installOnboardingParakeet(root);

  const order = calls.map((c) => c.name).filter((n) => n === 'cmd_download_model' || n === 'cmd_install_parakeet');
  assert.deepEqual(order, ['cmd_download_model', 'cmd_install_parakeet']);
  assert.deepEqual(calls.find((c) => c.name === 'cmd_download_model').args, { model: 'base' });
  assert.deepEqual(calls.find((c) => c.name === 'cmd_set_setting').args, { section: 'transcription', key: 'model', value: 'base' });
  assert.match(nodes['.progress-text'].textContent, /^Parakeet v3 is ready/);
  assert.equal(nodes['.progress-fill'].style.width, '100%');
  assert(nodes['#step-model'].classList.contains('done'));
  assert.equal(nodes['#step-model-status'].textContent, 'Done');
});

test('a failed Parakeet download still finishes setup on Whisper Base and says so', async () => {
  const failed = { phase: 'error', error: 'Parakeet model file encoder.int8.onnx failed its checksum', done: 0, total: 639 * MB };
  const { context, root, nodes } = harness({ statuses: [failed] });
  await context.offerParakeetInOnboarding(root);
  await context.installOnboardingParakeet(root);
  assert.match(nodes['.progress-text'].textContent, /did not install \(.*checksum\)\. Whisper Base is ready/);
  assert(nodes['#step-model'].classList.contains('done'));
});
