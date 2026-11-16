import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
const context = vm.createContext({});
vm.runInContext(readFileSync('tauri/src/scripts/recall-tabs.js', 'utf8'), context);
const shortcut = (event, mac) => JSON.parse(JSON.stringify(context.MinutesRecallTabs.shortcut(event, mac)));

test('Recall shortcuts use the platform modifier and preserve normal terminal keys', () => {
  for (const mac of [true, false]) {
    const modifier = mac ? { metaKey: true } : { ctrlKey: true };
    for (let index = 1; index <= 4; index++) assert.deepEqual(shortcut({ ...modifier, key: String(index) }, mac), { type: 'select', index: index - 1 });
    assert.deepEqual(shortcut({ ...modifier, key: 't' }, mac), { type: 'create' });
    assert.deepEqual(shortcut({ ...modifier, key: 'w' }, mac), { type: 'close' });
    assert.deepEqual(shortcut({ ctrlKey: true, key: 'Tab' }, mac), { type: 'cycle', direction: 1 });
    assert.deepEqual(shortcut({ ctrlKey: true, shiftKey: true, key: 'Tab' }, mac), { type: 'cycle', direction: -1 });
    assert.equal(shortcut({ key: 'w' }, mac), null);
    assert.equal(shortcut({ ...modifier, key: 'c' }, mac), null);
  }
  assert.equal(shortcut({ metaKey: true, key: '1' }, false), null);
  assert.equal(shortcut({ ctrlKey: true, key: '1' }, true), null);
});
