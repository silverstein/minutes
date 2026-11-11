#!/usr/bin/env node
// Production tab controller and CSS, with native metadata/PTY boundaries stubbed.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
const require = createRequire(import.meta.url);
const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES || resolve('target/recall-browser/node_modules');
const { chromium } = require(require.resolve(resolve(modules, 'playwright')));
const source = readFileSync('tauri/src/index.html', 'utf8');
const css = source.match(/<style>([\s\S]*?)<\/style>/)[1];
const controller = readFileSync('tauri/src/scripts/recall-tabs.js', 'utf8');
const html = `<!doctype html><html><head><style>${css}</style></head><body>
<div id="recall-panel" class="recall-panel expanded"><div id="recall-tabs" class="recall-tabs" role="tablist"></div><div id="recall-tab-prompt" class="recall-tab-prompt" hidden></div>
<span data-recall-meeting="/live.md"></span><span data-recall-meeting="/prep.md"></span></div><script>${controller}</script><script>
const actions=[];let model={active:'live',tabs:[{id:'live',title:'Live call',meeting_path:'/live.md',status:'working',live:true},{id:'prep',title:'Board prep',meeting_path:'/prep.md',status:'working'},{id:'chat',title:'Chat',status:'done'},{id:'other',title:'Other',status:'done'}]};
const controls=MinutesRecallTabs.create({strip:document.getElementById('recall-tabs'),prompt:document.getElementById('recall-tab-prompt'),invoke:async()=>{},isRecording:()=>true,select:id=>{actions.push(['select',id]);model.active=id;controls.update(structuredClone(model));},create:()=>actions.push(['new']),resume:id=>actions.push(['resume',id]),close:(id,confirmed)=>{actions.push(['close',id,!!confirmed]);if(!confirmed)controls.confirmClose(id);}});
controls.update(structuredClone(model));window.fixture={actions,model,controls};</script></body></html>`;
const server = createServer((_, response) => { response.setHeader('Content-Type', 'text/html'); response.end(html); });
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1000, height: 700 } });
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  assert.equal(await page.getByRole('tab').count(), 4);
  for (const status of ['done', 'waiting']) {
    await page.evaluate(status => { fixture.model.tabs[1].status = status; fixture.controls.update(structuredClone(fixture.model)); }, status);
    const glyphs = await page.evaluate(() => [document.querySelector('#recall-tab-prep .recall-tab-glyph').className, document.querySelector('[data-recall-meeting="/prep.md"] .recall-tab-glyph').className]);
    assert.equal(glyphs[0], glyphs[1]); assert.ok(glyphs[0].includes(status));
  }
  await page.evaluate(() => document.dispatchEvent(new KeyboardEvent('keydown', { key: '2', ctrlKey: true, bubbles: true })));
  assert.equal(await page.getByRole('tab', { selected: true }).innerText(), 'Board prep');
  await page.getByRole('button', { name: 'Close Live call', exact: true }).click();
  assert.equal(await page.getByRole('button', { name: 'Close conversation', exact: true }).count(), 1);
  await page.getByRole('button', { name: 'Close conversation', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['close', 'live', true]);
  await page.evaluate(() => fixture.controls.chooseFinished(id => fixture.actions.push(['replace', id])));
  assert.equal(await page.getByRole('button', { name: 'Close Chat', exact: true }).count(), 2); // strip + chooser
  assert.equal(await page.getByRole('button', { name: 'Close Board prep', exact: true }).count(), 1); // waiting excluded
  await page.evaluate(() => { Object.assign(fixture.model.tabs[1], { shell: true, claude_session_id: 'saved-1', claude_active: false }); fixture.controls.update(structuredClone(fixture.model)); });
  assert.equal(await page.getByRole('button', { name: 'Resume previous conversation' }).count(), 1);
  assert.equal(await page.evaluate(() => fixture.actions.some(action => action[0] === 'resume')), false);
  await page.getByRole('button', { name: 'Resume previous conversation' }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['resume', 'prep']);
  await page.evaluate(() => { fixture.model.tabs[1].claude_active = true; fixture.controls.update(structuredClone(fixture.model)); });
  assert.equal(await page.getByRole('button', { name: 'Resume previous conversation' }).count(), 0);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  assert.equal(await page.locator('.recall-tab-glyph.working').first().evaluate(node => getComputedStyle(node).animationName), 'none');
  for (const scheme of ['dark', 'light']) {
    await page.emulateMedia({ colorScheme: scheme });
    assert.equal(await page.getByRole('tab').count(), 4);
  }
  console.log('Recall tabs rendered contracts passed: status parity, Ctrl shortcuts, in-panel close/cap, reduced motion, themes.');
} finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
