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
assert(html.includes('Local Whisper model (live transcript only)'));
const beforeSettings = html.slice(0, settings);
assert(!beforeSettings.includes('<option value="openai-compatible">OpenAI-compatible — remote, batch only'));
console.log('Remote transcription settings: JavaScript parses; host disclosure, local live wording, hidden controls, and opt-in option pass.');
