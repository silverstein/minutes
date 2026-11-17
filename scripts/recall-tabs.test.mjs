import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
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

test('Herdr adapter remains at the desktop edge', () => {
  function scan(path) {
    for (const item of readdirSync(path, { withFileTypes: true })) {
      const next = `${path}/${item.name}`;
      if (item.isDirectory()) scan(next);
      else if (item.name.endsWith('.rs')) {
        const text = readFileSync(next, 'utf8');
        const herdrLines = text.split('\n').filter(line => /herdr/i.test(line)).map(line => line.trim());
        assert.deepEqual(herdrLines, next.endsWith('/dictation_context.rs') ? ['\"herdr\",', 'fn herdr_is_terminal_code() {', 'infer_target_text_mode(Some(\"Herdr\"), Some(\"dev.herdr.app\"), None),'] : [], next);
        assert.doesNotMatch(readFileSync(next, 'utf8'), /recall_herdr|herdr_terminal|handed_off_at|hc run|hc server status/i, next);
      }
    }
  }
  scan('crates/core/src');
});

function paletteFunction(source, name) {
  const start = source.indexOf(`    async function ${name}(`);
  assert.ok(start >= 0, name);
  const body = source.indexOf('{', source.indexOf(')', start));
  let depth = 0;
  for (let index = body; index < source.length; index++) {
    if (source[index] === '{') depth++;
    if (source[index] === '}' && --depth === 0) return source.slice(start, index + 1);
  }
  throw new Error(`Unterminated ${name}`);
}
function paletteFixture() {
  const source = readFileSync('tauri/src/palette/index.html', 'utf8');
  const calls = [], renders = [];
  let answer;
  const availability = new Promise(resolve => { answer = resolve; });
  const sandbox = vm.createContext({ console, calls, renders, invoke: async (name, args) => {
    calls.push({ name, args });
    if (name === 'cmd_recall_herdr_available') return availability;
    if (name === 'palette_list') return { commands: [{ id: 'ordinary', title: 'Ordinary command' }], recents: [] };
    return null;
  } });
  vm.runInContext(`
    let allCommands = [], recents = [], commandLoad = 0, focusedIndex = 0, mode = { kind: 'browsing' };
    const queryEl = { value: '' }, hintEl = { textContent: '' };
    const render = () => renders.push(allCommands.map(command => command.id));
    const resultsFromResponse = () => null;
    const enterResultsMode = () => { throw new Error('Unexpected results'); };
    ${['appendHerdrCommand', 'fetchCommands', 'closePalette', 'executeWithFreshVisibility'].map(name => paletteFunction(source, name)).join('\n')}
    globalThis.fixture = { fetch: fetchCommands, execute: executeWithFreshVisibility, commands: () => allCommands };
  `, sandbox);
  return { calls, renders, answer, ...sandbox.fixture };
}

test('a hung Herdr probe cannot delay the palette list or ordinary commands', { timeout: 1000 }, async () => {
  const fixture = paletteFixture();
  await fixture.fetch();
  assert.deepEqual(Array.from(fixture.renders[0]), ['ordinary']);
  await fixture.execute({ id: 'ordinary' });
  assert.equal(fixture.calls.filter(call => call.name === 'palette_execute').length, 1);
  assert.equal(fixture.calls.filter(call => call.name === 'cmd_recall_herdr_available').length, 1);
  fixture.answer(true);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(fixture.commands().length, 1, 'a discovery result from before execution cannot overwrite the fresh command state');
});

test('Herdr discovery appends later and execution delegates one authoritative edge check', async () => {
  const fixture = paletteFixture();
  await fixture.fetch();
  fixture.answer(true);
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(Array.from(fixture.commands(), command => command.id), ['ordinary', 'recall-open-in-herdr']);
  await fixture.execute({ id: 'recall-open-in-herdr' });
  assert.equal(fixture.calls.filter(call => call.name === 'cmd_recall_herdr_palette').length, 1);
  assert.equal(fixture.calls.filter(call => call.name === 'cmd_recall_herdr_available').length, 1, 'execution must not add a duplicate frontend probe');
  assert.equal(fixture.calls.filter(call => call.name === 'palette_execute').length, 0, 'the portable dispatcher never receives the edge-only command');
});
