// Model-free checks for remote disclosure wording and the settings surface.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const html = readFileSync(new URL('../tauri/src/index.html', import.meta.url), 'utf8');
const scripts = [...html.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/g)]
  .map((match) => match[1]).filter((script) => script.trim());
for (const script of scripts) new Function(script);

const begin = html.indexOf('    function backendLabel(');
const end = html.indexOf('    function currentModeLabel(', begin);
assert(begin > 0 && end > begin);
const describe = new Function(`${html.slice(begin, end)}; return describeLiveBackendSelection;`)();
const notice = 'Batch transcription is remote: audio is sent to provider.invalid.';
const detail = describe('inherit', 'whisper', ['whisper'], 'openai-compatible', false, notice);
assert(detail.includes('provider.invalid'));
assert(detail.includes('Standalone live transcript uses local Whisper'));
assert(detail.includes('Recording-sidecar live stays local'));
assert(!detail.includes('batch transcription stay on Whisper'));

const settings = html.indexOf('id="settings-transcription-engine"');
assert(settings > 0);
assert(html.slice(settings, html.indexOf('</select>', settings)).includes('value="openai-compatible"'));
assert.match(html, /id="settings-remote-transcription" hidden/);
assert.match(html, /#settings-remote-transcription\[hidden\] \{ display: none; \}/);
assert(html.includes('API key environment variable name (never the key)'));
assert(!html.includes('Local Whisper model (live transcript only)'));
const beforeSettings = html.slice(0, settings);
assert(!beforeSettings.includes('<option value="openai-compatible">OpenAI-compatible — remote, batch only'));
console.log('Remote transcription settings: JavaScript parses; host disclosure, local live wording, hidden controls, and opt-in option pass.');

// Execute shipped rendering and handlers with deterministic DOM/IPC doubles.
const elements = new Map();
const document = {
  getElementById(id) {
    if (!elements.has(id)) {
      const classes = new Set();
      elements.set(id, {
        value: '', hidden: false, textContent: '', handlers: {},
        classList: {
          toggle(name, on) { if (on) classes.add(name); else classes.delete(name); },
          contains(name) { return classes.has(name); },
        },
        addEventListener(type, handler) { this.handlers[type] = handler; },
      });
    }
    return elements.get(id);
  },
};
const fields = [
  ['settings-transcription-base-url', 'openai_compatible_base_url', 'http://127.0.0.1:9876/v1'],
  ['settings-transcription-remote-model', 'openai_compatible_model', 'synthetic-model'],
  ['settings-transcription-key-env', 'openai_compatible_api_key_env', 'SYNTHETIC_TRANSCRIPTION_KEY'],
  ['settings-transcription-upload-limit', 'openai_compatible_max_upload_bytes', '1234567'],
];
const state = { transcription: { engine: 'whisper', resolved_engine: 'whisper', model: 'small' } };
const renderBegin = html.indexOf('        const remoteTranscription =');
const renderEnd = html.indexOf('        refreshParakeetInstall();', renderBegin);
const modelBegin = html.indexOf('        const batchUsesWhisper =', renderEnd);
const modelEnd = html.indexOf("        document.getElementById('settings-parakeet-section')", modelBegin);
assert(renderBegin > 0 && renderEnd > renderBegin && modelEnd > modelBegin);
const render = new Function('s', 'document', `
  const resolvedBatchEngine = s.transcription.resolved_engine;
  const engineDetail = {};
  ${html.slice(renderBegin, renderEnd)}
  ${html.slice(modelBegin, modelEnd)}
`);
const renderSettings = async () => render(state, document);
const calls = [];
const invoke = async (command, args) => {
  assert.equal(command, 'cmd_set_setting');
  calls.push(args);
  assert.equal(args.section, 'transcription');
  state.transcription[args.key] = args.value;
  if (args.key === 'engine') state.transcription.resolved_engine = args.value;
};
const handlerBegin = html.indexOf("    document.getElementById('settings-transcription-base-url').addEventListener", html.indexOf('    // Settings change handlers'));
const handlerEnd = html.indexOf("    document.getElementById('settings-parakeet-model').addEventListener", handlerBegin);
assert(handlerBegin > 0 && handlerEnd > handlerBegin);
new Function('document', 'invoke', 'loadSettings', 'checkSetup', html.slice(handlerBegin, handlerEnd))(
  document, invoke, renderSettings, async () => {},
);
const change = async (id, value) => {
  const element = document.getElementById(id);
  element.value = value;
  await element.handlers.change({ target: element });
};
await renderSettings();
assert.equal(document.getElementById('settings-remote-transcription').hidden, true);
await change('settings-transcription-engine', 'openai-compatible');
assert.equal(document.getElementById('settings-remote-transcription').hidden, false);
assert(document.getElementById('settings-whisper-section').classList.contains('is-hidden'),
  'Remote mode must hide the batch-only model selector, whose preference does not select the live model');
for (const [id, key, value] of fields) {
  await change(id, value);
  assert.equal(state.transcription[key], value);
  document.getElementById(id).value = '';
  await renderSettings();
  assert.equal(document.getElementById(id).value, value);
  assert(calls.some(call => call.key === key && call.value === value));
}
await change('settings-transcription-engine', 'whisper');
assert.equal(document.getElementById('settings-remote-transcription').hidden, true);
assert.equal(document.getElementById('settings-whisper-section').classList.contains('is-hidden'), false);
assert.equal(document.getElementById('settings-whisper-model-label').textContent, 'Whisper model');
for (const [id, key, value] of fields) assert.equal(state.transcription[key], value);
console.log('Remote Settings behavior: engine switching, all four setter calls, reload persistence, batch-model visibility, and switching back pass (mock IPC).');
