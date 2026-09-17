import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { test } from 'node:test';

test('artifact inspection pages controls and exact option values without changing snapshot', async () => {
  const controls = Array.from({ length: 6 }, (_, i) => ({
    tagName: 'SELECT', value: `option-${i}-0`, id: `select-${i}`, labels: [],
    isConnected: true, dispatchEvent: () => {},
    getClientRects: () => [1], getAttribute: () => null,
    options: Array.from({ length: 64 }, (_, j) => ({ value: `option-${i}-${j}`, label: 'x'.repeat(180), disabled: false })),
  }));
  let handler;
  let result;
  const parent = { postMessage: message => { result = message.result; } };
  class Select {}
  Object.defineProperty(Select.prototype, 'value', { set(value) { this.value = value; } });
  const context = {
    crypto: { randomUUID: () => 'fixture-instance' }, parent,
    document: { querySelectorAll: selector => selector.startsWith('input') ? controls : [], getElementById: () => null },
    getComputedStyle: () => ({ visibility: 'visible' }),
    addEventListener: (_, callback) => { handler = callback; },
    HTMLSelectElement: Select, Event: class {}, setTimeout,
  };
  vm.runInNewContext(readFileSync(new URL('../../crates/core/src/voice_live/artifact_bridge.js', import.meta.url), 'utf8'), context);
  const inspect = async args => {
    await handler({ source: parent, data: { kind: 'minutes-control', id: 'request', command: { operation: 'inspect', ...args } } });
    return JSON.parse(JSON.stringify(result));
  };
  const first = await inspect({});
  assert.equal(first.controls.length, 4);
  assert.equal(first.total_controls, 6);
  assert.equal(first.controls[0].options.length, 4);
  assert.equal(first.controls[0].total_options, 64);
  assert.ok(JSON.stringify(first).length < 12000);
  const next = await inspect({ control_offset: 4 });
  assert.equal(next.controls.length, 2);
  assert.equal(next.controls[0].control_id, 'control-5');
  assert.equal(next.snapshot_id, first.snapshot_id);
  const focused = await inspect({ control_id: 'control-2', option_offset: 4 });
  assert.equal(focused.controls.length, 1);
  assert.equal(focused.controls[0].options[0].value, 'option-1-4');
  assert.equal(focused.controls[0].option_offset, 4);
  assert.equal(focused.snapshot_id, first.snapshot_id);
  await handler({ source: parent, data: { kind: 'minutes-control', id: 'edit', command: {
    operation: 'set', snapshot_id: first.snapshot_id, control_id: 'control-5', value: 'option-4-3',
  } } });
  const changed = JSON.parse(JSON.stringify(result));
  assert.equal(changed.controls.length, 2);
  assert.equal(changed.controls[0].control_id, 'control-5');
  assert.equal(changed.controls[0].value, 'option-4-3');
  assert.equal(changed.changed.after, 'option-4-3');
  assert.notEqual(changed.snapshot_id, first.snapshot_id);
  await handler({ source: parent, data: { kind: 'minutes-control', id: 'undo', command: {
    operation: 'undo', snapshot_id: changed.snapshot_id, undo_id: changed.undo_id,
  } } });
  assert.equal(result.controls[0].control_id, 'control-5');
  assert.equal(result.controls[0].value, 'option-4-0');
});

test('Unicode-heavy mutation receipts pass through the actual host byte boundary', { timeout: 3000 }, async () => {
  const controls = Array.from({ length: 4 }, (_, i) => ({
    tagName: 'SELECT', value: '語'.repeat(255) + '0', id: `select-${i}`, labels: [],
    isConnected: true, dispatchEvent: () => {}, getClientRects: () => [1],
    getAttribute: key => key === 'aria-label' ? '語'.repeat(180) : null,
    options: Array.from({ length: 4 }, (_, j) => ({ value: '語'.repeat(255) + j, label: '語'.repeat(180), disabled: false })),
  }));
  let bridgeHandler, hostHandler, poll, command, delivered, rawBytes;
  let resolveReceipt;
  const parent = { postMessage: async data => {
    rawBytes = new TextEncoder().encode(JSON.stringify({ id: data.id, result: data.result })).length;
    await hostHandler({ source: frame.contentWindow, data });
  } };
  const frame = {
    contentWindow: { postMessage: data => { void bridgeHandler({ source: parent, data }); } },
    addEventListener: (_, callback) => { poll = callback; },
  };
  class Select {}
  Object.defineProperty(Select.prototype, 'value', { set(value) { this.value = value; } });
  vm.runInNewContext(readFileSync(new URL('../../crates/core/src/voice_live/artifact_bridge.js', import.meta.url), 'utf8'), {
    crypto: { randomUUID: () => 'fixture-instance' }, parent, HTMLSelectElement: Select, Event: class {}, setTimeout,
    document: { querySelectorAll: selector => selector.startsWith('input') ? controls : Array.from({ length: 12 }, () => ({ textContent: '語'.repeat(180) })), getElementById: () => null },
    getComputedStyle: () => ({ visibility: 'visible' }),
    addEventListener: (_, callback) => { bridgeHandler = callback; },
  });
  vm.runInNewContext(readFileSync(new URL('../../crates/core/src/voice_live/artifact_host.js', import.meta.url), 'utf8'), {
    location: { hash: '#fixture-token' }, history: { replaceState: () => {} },
    document: { querySelector: () => frame }, TextEncoder, setTimeout: () => {},
    btoa: value => Buffer.from(value, 'binary').toString('base64'),
    addEventListener: (_, callback) => { hostHandler = callback; },
    fetch: async (url, init) => {
      if (url === '/command') return { ok: true, json: async () => ({ id: 'request-id', command }) };
      assert.equal(url, '/result');
      const bytes = Buffer.from(init.headers['X-Minutes-Result'], 'base64');
      assert.ok(bytes.length <= 34000);
      assert.ok(init.headers['X-Minutes-Result'].length <= 48000);
      delivered = JSON.parse(bytes.toString('utf8')).result;
      resolveReceipt();
      return { ok: true };
    },
  });
  const request = async next => {
    command = next;
    const received = new Promise(resolve => { resolveReceipt = resolve; });
    await poll();
    await received;
    return delivered;
  };
  const initial = await request({ operation: 'inspect', control_id: 'control-4' });
  const desired = '語'.repeat(255) + '1';
  const changed = await request({ operation: 'set', snapshot_id: initial.snapshot_id, control_id: 'control-4', value: desired });
  assert.ok(rawBytes > 34000, 'fixture must reproduce the original dropped receipt');
  assert.equal(changed.changed.after, desired);
  assert.equal(changed.changed.before, '語'.repeat(255) + '0');
  assert.equal(changed.transport_shortened, true);
  assert.ok(changed.undo_id);
  assert.notEqual(changed.snapshot_id, initial.snapshot_id);
});
