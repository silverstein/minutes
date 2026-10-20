#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { test } from 'node:test';
const window = {};
vm.runInNewContext(readFileSync(new URL('../tauri/src/scripts/update-offer.js', import.meta.url), 'utf8'), { window });
const { compare, restore } = window.MinutesUpdateOffer;
test('SemVer precedence handles numeric versions, prereleases and metadata', () => {
  const ordered = ['0.30.9', '0.30.10-alpha', '0.30.10-alpha.1', '0.30.10-alpha.beta', '0.30.10-beta', '0.30.10-beta.2', '0.30.10-beta.11', '0.30.10-rc.1', '0.30.10', '1.0.0'];
  for (let i = 1; i < ordered.length; i++) {
    assert.equal(compare(ordered[i], ordered[i - 1]), 1);
    assert.equal(compare(ordered[i - 1], ordered[i]), -1);
  }
  assert.equal(compare('0.30.10+old', '0.30.10+new'), 0);
  assert.equal(compare('1.0.0-9007199254740993', '1.0.0-9007199254740992'), 1);
  for (const invalid of ['', null, 'v0.30.4', '0.30', '0.030.4', '1.0.0-01', '1.0.0+', 'wat']) assert.equal(compare(invalid, '0.30.4'), null);
});
test('restoration shows only newer offers and clears stale, corrupt and Dev storage', async () => {
  for (const [stored, name, expected] of [
    [JSON.stringify({version: '0.30.5', body: 'notes', downloadBytes: 42}), 'Minutes', true],
    [JSON.stringify({version: '0.30.4'}), 'Minutes', false],
    [JSON.stringify({version: '0.30.3'}), 'Minutes', false],
    [JSON.stringify({version: '0.30.5-rc.1'}), 'Minutes', true],
    [JSON.stringify({version: '0.30.5'}), 'Minutes Dev', false],
    [JSON.stringify({version: 'bad'}), 'Minutes', false],
    ['{', 'Minutes', false], ['null', 'Minutes', false], ['{}', 'Minutes', false],
  ]) {
    const shown = [], removed = [];
    await restore({ storage: { getItem: () => stored, removeItem: key => removed.push(key) }, app: { getName: async () => name, getVersion: async () => '0.30.4' }, show: (...args) => shown.push(args) });
    assert.equal(shown.length, expected ? 1 : 0, stored + name);
    assert.equal(removed.length, expected ? 0 : 1, stored + name);
  }
});
test('missing storage, blocked storage and unavailable metadata do not break startup', async () => {
  const show = () => assert.fail('must not show');
  await restore({ storage: { getItem: () => null }, app: {}, show });
  await restore({ storage: { getItem: () => { throw Error('blocked'); } }, app: {}, show });
  await restore({ storage: { getItem: () => '{"version":"0.30.5"}' }, app: { getVersion: async () => { throw Error('unavailable'); } }, show });
});
