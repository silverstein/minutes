#!/usr/bin/env node
// Execute the shipped recovery controller with synthetic DOM/IPC.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { test } from 'node:test';
const script = readFileSync(new URL('../tauri/src/scripts/dictation-history.js', import.meta.url), 'utf8');
const deferred = () => { let resolve; const promise = new Promise(run => { resolve = run; }); return { promise, resolve }; };
function harness(values = {}) {
  const elements = new Map(), calls = [], observers = [];
  class Target {
    listeners = new Map();
    addEventListener(name, fn) { if (!this.listeners.has(name)) this.listeners.set(name, []); this.listeners.get(name).push(fn); }
    async emit(name, event = {}) { for (const fn of this.listeners.get(name) || []) await fn(event); }
  }
  class Element extends Target {
    hidden = false; disabled = false; value = ''; textContent = ''; dataset = {}; readOnly = false; plays = 0; pauses = 0;
    classList = { contains: () => true };
    focus() { document.activeElement = this; }
    removeAttribute(name) { delete this[name]; }
    load() {}
    pause() { this.pauses++; }
    async play() { this.plays++; }
  }
  const document = new Target(), window = {};
  const get = id => { if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id); };
  document.getElementById = get; get('dictation-pane-recent').hidden = false;
  let draft = false, allowSwitch = true, refreshes = 0;
  const responses = { cmd_dictation_audio: 'data:audio/wav;base64,c2FtcGxl', cmd_reprocess_dictation: { candidateId: 'token1', text: 'New transcript', message: 'Preview ready' }, cmd_accept_dictation_recovery: 'Saved. Nothing pasted.', cmd_delete_dictation_audio: 'Deleted audio; text kept.', ...values };
  window.__TAURI__ = { core: { invoke: async (command, args) => { calls.push({ command, args: structuredClone(args) }); const value = responses[command]; if (value instanceof Error) throw value; return typeof value === 'function' ? value(args) : value; } } };
  window.MinutesDictation = {
    edit(_text, record) { if (!allowSwitch) return false; for (const fn of document.listeners.get('minutes:dictation-review-source') || []) fn({ detail: record }); return true; },
    hasReviewDraft: () => draft,
    refreshReview(_text, record) { refreshes++; for (const fn of document.listeners.get('minutes:dictation-review-source') || []) fn({ detail: record }); },
  };
  window.loadRecentDictations = async () => {};
  const context = vm.createContext({ window, document, console, Error, MutationObserver: class { constructor(fn) { observers.push(fn); } observe() {} } });
  vm.runInContext(script, context);
  const record = { id: 'one', cleanedText: 'Saved transcript', rawText: 'saved transcript', recoveryAudioPath: '/synthetic/recovery.wav', insertion: { outcome: 'copied', verified: false } };
  get('settings-dictation-recents')._dictationRecords = [record];
  return { get, calls, responses, document, api: window.MinutesDictationHistory, record, observers, draft(value) { draft = value; }, switchAllowed(value) { allowSwitch = value; }, refreshes: () => refreshes };
}
test('opening a review performs no read, transcription, mutation or paste', async () => {
  const h = harness(); await h.api.act('delete-audio', h.record);
  assert.equal(h.calls.length, 0); assert.equal(h.get('dictation-audio-delete-confirm').hidden, false);
  await h.get('dictation-audio-keep').emit('click'); assert.equal(h.calls.length, 0);
});
test('listening resolves audio by history id and clears it when review closes', async () => {
  const h = harness(); await h.api.act('listen', h.record);
  assert.deepEqual(h.calls, [{ command: 'cmd_dictation_audio', args: { id: 'one' } }]);
  assert.equal(h.get('dictation-audio-player').plays, 1);
  await h.get('dictation-review-close').emit('click'); assert.equal(h.get('dictation-audio-player').src, undefined); assert.equal(h.get('dictation-audio-player').hidden, true);
});
test('audio resolving after a source switch cannot play in the new review', async () => {
  const pending = deferred(); const h = harness({ cmd_dictation_audio: () => pending.promise });
  const read = h.api.act('listen', h.record);
  await Promise.resolve(); await h.api.act('delete-audio', { ...h.record, id: 'two' });
  pending.resolve('data:audio/wav;base64,c2FtcGxl'); await read;
  assert.equal(h.get('dictation-audio-player').plays, 0);
});
test('failed audio load retains an actionable error without playing', async () => {
  const h = harness({ cmd_dictation_audio: new Error('File missing') }); await h.api.act('listen', h.record);
  assert.equal(h.get('dictation-audio-player').plays, 0); assert.match(h.get('dictation-audio-status').textContent, /File missing/); assert.equal(h.get('dictation-audio-listen').disabled, false);
});
test('preview transcription explicitly avoids replacement and paste', async () => {
  const h = harness(); await h.api.act('reprocess', h.record);
  assert.deepEqual(h.calls, [{ command: 'cmd_reprocess_dictation', args: { id: 'one', previewOnly: true } }]);
  assert.equal(h.record.cleanedText, 'Saved transcript'); assert.equal(h.get('dictation-recovery-saved').textContent, 'Saved transcript'); assert.equal(h.get('dictation-recovery-text').value, 'New transcript');
  await h.get('dictation-recovery-discard').emit('click'); assert.equal(h.calls.length, 1); assert.equal(h.get('dictation-recovery-preview').hidden, true);
});
test('late transcription preview is discarded when review closes', async () => {
  const pending = deferred(); const h = harness({ cmd_reprocess_dictation: () => pending.promise });
  const run = h.api.act('reprocess', h.record); await Promise.resolve(); await h.get('dictation-review-close').emit('click');
  pending.resolve({ candidateId: 'late', text: 'Wrong review' }); await run;
  assert.equal(h.get('dictation-recovery-preview').hidden, true); assert.equal(h.get('dictation-recovery-text').value, '');
});
test('use transcript sends only staged token, refreshes history and never pastes', async () => {
  const h = harness(); await h.api.act('reprocess', h.record); await h.get('dictation-recovery-accept').emit('click');
  assert.deepEqual(h.calls.map(x => x.command), ['cmd_reprocess_dictation', 'cmd_accept_dictation_recovery']);
  assert.deepEqual(h.calls[1].args, { candidateId: 'token1' }); assert.equal(h.refreshes(), 1); assert.equal(h.get('dictation-edit-original').readOnly, false);
});
test('changed/expired source cannot display save success or lose preview', async () => {
  const h = harness({ cmd_accept_dictation_recovery: new Error('This dictation changed') });
  await h.api.act('reprocess', h.record); await h.get('dictation-recovery-accept').emit('click');
  assert.match(h.get('dictation-audio-status').textContent, /changed/); assert.equal(h.refreshes(), 0); assert.equal(h.get('dictation-recovery-preview').hidden, false); assert.equal(h.get('dictation-recovery-accept').disabled, false);
});
test('draft edits prevent accepting a preview over user work', async () => {
  const h = harness(); await h.api.act('reprocess', h.record); h.draft(true); await h.get('dictation-edit-original').emit('input');
  assert.equal(h.get('dictation-recovery-accept').disabled, true); await h.get('dictation-recovery-accept').emit('click'); assert.equal(h.calls.length, 1);
});
test('unsaved source-switch guard prevents audio and transcription calls', async () => {
  const h = harness(); h.switchAllowed(false); await h.api.act('listen', h.record); await h.api.act('reprocess', h.record); assert.equal(h.calls.length, 0);
});
test('delete requires confirmation, clears playing bytes, retains text and never pastes', async () => {
  const h = harness(); await h.api.act('listen', h.record); await h.api.act('delete-audio', h.record);
  assert.equal(h.calls.filter(x => x.command === 'cmd_delete_dictation_audio').length, 0);
  await h.get('dictation-audio-delete').emit('click'); assert.equal(h.calls.at(-1).command, 'cmd_delete_dictation_audio'); assert.equal(h.get('dictation-audio-player').src, undefined); assert.equal(h.record.cleanedText, 'Saved transcript'); assert.match(h.get('dictation-audio-title').textContent, /deleted/);
});
test('failed delete keeps the confirmation and a retry action', async () => {
  const h = harness({ cmd_delete_dictation_audio: new Error('Disk failure') }); await h.api.act('delete-audio', h.record); await h.get('dictation-audio-delete').emit('click');
  assert.equal(h.get('dictation-audio-delete-confirm').hidden, false); assert.match(h.get('dictation-audio-status').textContent, /Disk failure/); assert.equal(h.get('dictation-audio-delete').disabled, false);
});
test('delivery labels require verification and distinguish clipboard from insertion', () => {
  const h = harness(); assert.equal(h.api.deliveryLabel(h.record), 'Copied only');
  assert.equal(h.api.deliveryLabel({ insertion: { outcome: 'typed', verified: false } }), 'Insertion unverified');
  assert.equal(h.api.deliveryLabel({ insertion: { outcome: 'pasted', verified: true } }), 'Pasted in destination');
  assert.equal(h.api.deliveryLabel({ insertion: { outcome: 'recovered' } }), 'Recovered · not pasted');
  assert.equal(h.api.needsRecovery(h.record), true);
});
