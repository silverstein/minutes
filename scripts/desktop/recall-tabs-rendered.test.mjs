#!/usr/bin/env node
// Production tab controller and CSS, with native metadata/PTY boundaries stubbed.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve, extname } from 'node:path';
import { gridFixture } from './recall-tabs-grid-fixture.mjs';
const require = createRequire(import.meta.url);
const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES || resolve('target/recall-browser/node_modules');
const { chromium } = require(require.resolve(resolve(modules, 'playwright')));
const source = readFileSync('tauri/src/index.html', 'utf8');
const css = source.match(/<style>([\s\S]*?)<\/style>/)[1];
const controller = readFileSync('tauri/src/scripts/recall-tabs.js', 'utf8');
const html = `<!doctype html><html><head><meta charset="utf-8"><style>${css}</style></head><body>
<div id="recall-panel" class="recall-panel expanded"><div id="recall-tabs" class="recall-tabs" role="tablist"></div><div id="recall-tab-prompt" class="recall-tab-prompt" hidden></div>
<span data-recall-meeting="/live.md"></span><span data-recall-meeting="/prep.md"></span></div><script>${controller}</script><script>
const actions=[];let model={active:'live',tabs:[{id:'live',title:'Live call',meeting_path:'/live.md',status:'working',live:true},{id:'prep',title:'Board prep',meeting_path:'/prep.md',status:'working'},{id:'chat',title:'Chat',status:'done'},{id:'other',title:'Other',status:'done'}]};
const controls=MinutesRecallTabs.create({strip:document.getElementById('recall-tabs'),prompt:document.getElementById('recall-tab-prompt'),invoke:async()=>{},isRecording:()=>true,select:id=>{actions.push(['select',id]);model.active=id;controls.update(structuredClone(model));},create:()=>actions.push(['new']),resume:id=>actions.push(['resume',id]),close:(id,confirmed)=>{actions.push(['close',id,!!confirmed]);if(!confirmed)controls.confirmClose(id);}});
controls.update(structuredClone(model));window.fixture={actions,model,controls};</script></body></html>`;
const server = createServer((request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;
  if (path.startsWith('/vendor/xterm/')) {
    const file = resolve('tauri/src', '.' + path);
    if (!file.startsWith(resolve('tauri/src/vendor/xterm') + '/')) { response.writeHead(404).end(); return; }
    response.setHeader('Content-Type', extname(file) === '.css' ? 'text/css' : 'text/javascript');
    response.end(readFileSync(file)); return;
  }
  response.setHeader('Content-Type', 'text/html; charset=utf-8');
  response.end(path === '/grid' ? gridFixture(source, css, controller) : html);
});
await new Promise(resolve => server.listen(Number(process.env.MINUTES_RECALL_TAB_PREVIEW) || 0, '127.0.0.1', resolve));
if (process.env.MINUTES_RECALL_TAB_PREVIEW) {
  console.log(`Recall tabs preview: http://127.0.0.1:${server.address().port}/grid`);
  await new Promise(() => {});
}
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1000, height: 700 } });
  if (process.env.MINUTES_RECALL_TEST_CPU_RATE) {
    const session = await page.context().newCDPSession(page);
    await session.send('Emulation.setCPUThrottlingRate', { rate: Number(process.env.MINUTES_RECALL_TEST_CPU_RATE) });
  }
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  assert.equal(await page.getByRole('tab').count(), 4);
  for (const status of ['done', 'waiting']) {
    await page.evaluate(status => { fixture.model.tabs[1].status = status; fixture.controls.update(structuredClone(fixture.model)); }, status);
    const glyphs = await page.evaluate(() => [document.querySelector('#recall-tab-prep .recall-tab-glyph').className, document.querySelector('[data-recall-meeting="/prep.md"] .recall-tab-glyph').className]);
    assert.equal(glyphs[0], glyphs[1]); assert.ok(glyphs[0].includes(status));
  }
  await page.evaluate(() => document.dispatchEvent(new KeyboardEvent('keydown', { key: '2', ctrlKey: true, bubbles: true })));
  assert.equal(await page.getByRole('tab', { selected: true }).innerText(), 'Board prep');
  await page.locator('#recall-tab-prep').focus();
  await page.evaluate(() => fixture.controls.update(structuredClone(fixture.model)));
  assert.equal(await page.evaluate(() => document.activeElement?.id), 'recall-tab-prep', 'a metadata update keeps keyboard focus on the focused tab button, not its wrapper');
  await page.getByRole('button', { name: 'Close Live call', exact: true }).click();
  assert.equal(await page.getByRole('button', { name: 'Close conversation', exact: true }).count(), 1);
  assert.equal(await page.getByRole('button', { name: 'Cancel', exact: true }).evaluate(node => node === document.activeElement), true);
  await page.keyboard.press('Enter');
  assert.equal(await page.getByRole('button', { name: 'Close conversation', exact: true }).count(), 0);
  assert.equal(await page.evaluate(() => fixture.actions.some(action => action[0] === 'close' && action[2])), false, 'Return cancels rather than destroying the conversation');
  await page.getByRole('button', { name: 'Close Live call', exact: true }).click();
  await page.getByRole('button', { name: 'Close conversation', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['close', 'live', true]);
  await page.evaluate(() => fixture.controls.chooseFinished(id => fixture.actions.push(['replace', id])));
  assert.equal(await page.getByRole('button', { name: 'Close Chat', exact: true }).count(), 1); // strip
  assert.equal(await page.getByRole('button', { name: 'Close Chat and continue', exact: true }).count(), 1, 'one suggested close, the first inactive finished tab');
  assert.equal(await page.locator('#recall-tab-prompt .btn').count(), 2, 'the chooser is one suggestion plus Cancel, never a button per tab');
  assert.equal(await page.getByRole('button', { name: 'Close Board prep', exact: true }).count(), 1); // waiting excluded
  assert.deepEqual(await page.locator('.recall-tab.recall-tab-closable').evaluateAll(nodes => nodes.map(node => node.dataset.tabId)), ['chat', 'other'], 'finished tabs are highlighted in the strip; live and waiting are not');
  assert.equal(await page.locator('#recall-tab-prompt .btn').first().evaluate(node => getComputedStyle(node).textTransform), 'none', 'meeting names are not shouted');
  await page.getByRole('button', { name: 'Close Other', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['replace', 'other'], 'closing a highlighted tab from the strip continues the blocked open');
  assert.equal(await page.locator('#recall-tab-prompt').isVisible(), false);
  assert.equal(await page.locator('.recall-tab.recall-tab-closable').count(), 0, 'dismissal clears the strip highlight');
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
  await page.evaluate(() => {
    fixture.model.tabs.forEach((tab,index)=>Object.assign(tab,{title:`Chat ${index+1}`,live:false,status:'done'}));
    fixture.controls.update(structuredClone(fixture.model));
    fixture.controls.chooseFinished(()=>{});
  });
  assert.equal(await page.getByRole('button',{name:'Close Chat 1 and continue',exact:true}).count(),1,'the suggestion skips the active tab');
  assert.equal(await page.locator('.recall-tab.recall-tab-closable').count(),4);
  assert.match(await page.locator('#recall-tab-prompt p').innerText(),/Or close another highlighted tab\./);
  await page.evaluate(()=>{ fixture.model.active='live'; fixture.controls.update(structuredClone(fixture.model)); });
  assert.equal(await page.getByRole('button',{name:'Close Chat 1 and continue',exact:true}).count(),0,'switching onto the suggested tab withdraws it, with the tabs unchanged');
  assert.equal(await page.getByRole('button',{name:'Close Chat 2 and continue',exact:true}).count(),1);
  await page.evaluate(()=>{ fixture.model.active='prep'; fixture.controls.update(structuredClone(fixture.model)); });
  assert.equal(await page.getByRole('button',{name:'Close Chat 1 and continue',exact:true}).count(),1);
  await page.evaluate(()=>{
    fixture.controls.dismiss();
    fixture.controls.syncPendingLive('capture-a',id=>fixture.actions.push(['admit-live',id]));
    fixture.model.tabs[3].live=true;
    fixture.model.tabs.unshift(fixture.model.tabs.pop());
    fixture.controls.update(structuredClone(fixture.model));
  });
  assert.equal(await page.locator('#recall-tab-prompt').getByRole('button',{name:'Close Chat 4 and continue',exact:true}).count(),0,'live tabs are never suggested in an updated chooser');
  assert.equal(await page.locator('#recall-tab-prompt').getByRole('button',{name:'Close Chat 1 and continue',exact:true}).count(),1);
  assert.equal(await page.locator('.recall-tab-closable[data-tab-id="other"]').count(),0,'the live tab is not highlighted for closing');
  assert.match(await page.locator('#recall-tab-prompt p').innerText(),/^Recording\./);
  await page.evaluate(()=>fixture.controls.syncPendingLive(null,()=>{}));
  assert.equal(await page.locator('#recall-tab-prompt').isVisible(),false,'admitting live dismisses its old cap chooser');
  await page.evaluate(()=>fixture.controls.syncPendingLive('capture-b',()=>{}));
  await page.getByRole('button',{name:'Cancel',exact:true}).click();
  await page.evaluate(()=>{
    fixture.controls.update(structuredClone(fixture.model));
    fixture.controls.syncPendingLive('capture-b',()=>{});
  });
  assert.equal(await page.locator('#recall-tab-prompt').isVisible(),false,'metadata updates do not reopen a cancelled chooser for the same capture');
  await page.evaluate(()=>{
    fixture.model.tabs.forEach(tab=>{tab.status='working';});
    fixture.controls.update(structuredClone(fixture.model));
    fixture.controls.chooseFinished(()=>{});
  });
  assert.equal(await page.locator('#recall-tab-prompt .btn').count(),1,'with every tab busy only Cancel is offered');
  assert.match(await page.locator('#recall-tab-prompt p').innerText(),/it asks first/);
  await page.getByRole('button',{name:'Cancel',exact:true}).click();
  await page.goto(`http://127.0.0.1:${server.address().port}/grid`);
  await page.waitForFunction(() => fixture.ready);
  for (const id of ['one', 'two', 'three', 'one']) {
    await page.evaluate(id => {
      // openRecall hides the shared container before selecting a tab. An idle
      // shell emits no new data, so activation must reveal it itself.
      document.getElementById('recall-terminal-container').classList.add('hidden');
      return fixture.activate(id);
    }, id);
    await page.waitForFunction(id => {
      const {terminal,pty}=fixture.snapshot(id);
      return terminal.cols>40 && terminal.rows>10 && terminal.cols===pty.cols && terminal.rows===pty.rows;
    }, id);
  }
  // Re-selecting an unchanged xterm grid still repairs a newly spawned PTY.
  await page.evaluate(() => { fixture.tabs.get('one').ptySize={cols:10,rows:4};fixture.grids.set('one',{cols:10,rows:4}); });
  await page.evaluate(() => fixture.activate('one'));
  await page.waitForFunction(() => fixture.snapshot('one').pty.cols === fixture.snapshot('one').terminal.cols);
  const count = await page.evaluate(() => fixture.calls.filter(call=>call.command==='cmd_pty_resize').length);
  await page.evaluate(async()=>{await fixture.fit();await fixture.fit();});
  assert.equal(await page.evaluate(()=>fixture.calls.filter(call=>call.command==='cmd_pty_resize').length), count, 'same grid does not redraw the child');
  await page.evaluate(()=>document.getElementById('recall-panel').style.width='600px');
  await page.waitForFunction(()=>fixture.snapshot('one').pty.cols===fixture.snapshot('one').terminal.cols && fixture.snapshot('one').terminal.cols<80);
  await page.getByRole('button',{name:'New Recall chat'}).click();
  assert.deepEqual(await page.evaluate(()=>fixture.opens.at(-1)),['assistant',null,{tabId:'one'}], 'new chat reuses known untouched shell');
  await page.evaluate(()=>{for(const tab of fixture.tabs.values())tab.shell=false;fixture.newChat();});
  assert.deepEqual(await page.evaluate(()=>fixture.opens.at(-1)),['assistant',null,{newTab:true}], 'direct agent processes are not assumed to be unused shells');
  await page.evaluate(()=>{for(const tab of fixture.tabs.values())tab.shell=true;});
  await page.evaluate(()=>{for(const tab of fixture.tabs.values())tab.untouched=false;Object.assign(fixture.tabs.get('one'),{untouched:true,draft:'unsent shell input'});fixture.newChat();});
  assert.deepEqual(await page.evaluate(()=>fixture.opens.at(-1)),['assistant',null,{newTab:true}], 'a draft is never reused as an empty chat');
  await page.evaluate(()=>{for(const tab of fixture.tabs.values())tab.untouched=false;fixture.newChat();});
  assert.deepEqual(await page.evaluate(()=>fixture.opens.at(-1)),['assistant',null,{newTab:true}], 'used/restored sessions are not treated as empty');
  await page.evaluate(()=>fixture.tabs.get('one').terminal.focus());
  await page.keyboard.press('Control+u');
  await page.evaluate(()=>fixture.tabs.get('one').queue);
  await page.evaluate(async()=>{await fixture.activate('three');fixture.tabs.get('three').terminal.focus();});
  const beforeShortcut = await page.evaluate(()=>fixture.calls.filter(call=>call.command==='cmd_pty_input').length);
  await page.keyboard.press('Control+1');
  await page.waitForFunction(()=>document.querySelector('#recall-tab-one')?.getAttribute('aria-selected')==='true');
  await page.evaluate(()=>fixture.tabs.get('one').queue);
  assert.deepEqual(await page.evaluate(()=>({draft:fixture.tabs.get('one').draft,reliable:fixture.tabs.get('one').reliable})),{draft:'',reliable:true});
  assert.equal(await page.evaluate(()=>fixture.calls.filter(call=>call.command==='cmd_pty_input').length),beforeShortcut,'tab shortcut never reaches a PTY');
  await page.evaluate(()=>fixture.tabs.get('one').terminal.write('GRID-CANARY'));
  await page.waitForFunction(()=>document.querySelector('#recall-terminal-one .xterm-rows').textContent.includes('GRID-CANARY'));
  await page.evaluate(()=>{
    const metadata={id:'four',title:'Chat 4',status:'done',shell:true};
    fixture.model.tabs.push(metadata);
    return fixture.activate('one');
  });
  for (const cancelKey of ['Enter', 'Escape', 'click']) {
    // Exercise the actual rejected open, including its initial container hide.
    // Calling the chooser alone misses the native cancellation regression.
    await page.evaluate(()=>fixture.openAtCap());
    assert.equal(await page.getByRole('tab').count(),4);
    assert.equal(await page.locator('#recall-terminal-container').isVisible(),true,'cap chooser keeps the selected PTY visible');
    assert.equal(await page.getByRole('button',{name:'Cancel',exact:true}).evaluate(node=>node===document.activeElement),true);
    if (cancelKey === 'click') await page.getByRole('button',{name:'Cancel',exact:true}).click();
    else await page.keyboard.press(cancelKey);
    assert.equal(await page.locator('#recall-tab-prompt').isVisible(),false);
    assert.equal(await page.locator('#recall-terminal-container').isVisible(),true,'cancelling a rejected open preserves the selected PTY');
    await page.waitForFunction(()=>{
      const {terminal,pty}=fixture.snapshot('one');
      return terminal.cols>40 && terminal.rows>10 && terminal.cols===pty.cols && terminal.rows===pty.rows;
    });
    assert.ok(await page.evaluate(()=>{
      const buffer=fixture.tabs.get('one').terminal.buffer.active;
      return Array.from({length:buffer.length},(_,index)=>buffer.getLine(index).translateToString()).join('\n').includes('GRID-CANARY');
    }),'cancellation preserves terminal buffer contents');
    // PTY resize acknowledgement does not wait for xterm's next paint frame.
    // Require the visible text too, without racing that asynchronous repaint.
    await page.waitForFunction(()=>document.querySelector('#recall-terminal-one .xterm-rows').textContent.includes('GRID-CANARY'));
  }
  await page.evaluate(()=>fixture.tabs.get('one').terminal.focus());
  await page.keyboard.type('pwd');
  await page.evaluate(()=>fixture.tabs.get('one').queue);
  assert.equal(await page.evaluate(()=>fixture.tabs.get('one').draft),'pwd','the cancelled tab still accepts terminal input');
  await page.keyboard.press('Control+u');
  await page.evaluate(()=>fixture.tabs.get('one').queue);
  await page.evaluate(()=>fixture.notice('Return to an empty shell prompt before resuming.','error'));
  await page.waitForFunction(()=>fixture.snapshot('one').terminal.rows===fixture.snapshot('one').pty.rows);
  await page.evaluate(async()=>{await fixture.activate('two');await fixture.activate('one');fixture.notice('');});
  await page.waitForFunction(()=>document.querySelector('#recall-terminal-one .xterm-rows').textContent.includes('GRID-CANARY'));
  await page.evaluate(async()=>{fixture.notice('Earlier refusal','error');await fixture.resume('one');});
  assert.equal(await page.locator('#recall-provider-notice.active').count(),0,'successful explicit resume clears an earlier refusal');
  await page.evaluate(()=>{
    Object.assign(fixture.model.tabs[0],{meeting_path:'/synthetic/northwind.md',title:'2026 03 04 northwind call',phase:'prep'});
    fixture.inventory.push({path:'/synthetic/northwind.md',title:'Northwind Customer Call — SSO Friction'});
    return fixture.activate('one');
  });
  assert.equal(await page.getByRole('tab',{selected:true}).innerText(),'Northwind Customer Call — SSO Friction');
  assert.equal(await page.locator('#fixture-header').innerText(),'prep — Northwind Customer Call — SSO Friction');
  assert.equal(await page.evaluate(()=>fixture.calls.filter(call=>call.command==='cmd_recall_tab_select').at(-1).args.meetingTitle),'Northwind Customer Call — SSO Friction');
  await page.evaluate(()=>{
    document.getElementById('fixture-meetings').append(fixture.buildRow({path:'/synthetic/northwind.md',title:'A long meeting title that must truncate while its status stays visible '.repeat(3)}));
  });
  assert.deepEqual(await page.locator('#fixture-meetings .meeting-title-row').evaluate(row=>{
    const title=row.querySelector('.meeting-title'),glyph=row.querySelector('.recall-tab-glyph');
    const r=row.getBoundingClientRect(),g=glyph.getBoundingClientRect();
    return {truncated:title.scrollWidth>title.clientWidth,glyphInside:g.width>0&&g.right<=r.right&&g.left>=r.left};
  }),{truncated:true,glyphInside:true},'production meeting row reserves glyph space beside a truncated title');
  await page.evaluate(()=>{
    const model=structuredClone(fixture.model);
    Object.assign(model.tabs[0],{live:true,title:'Live call',phase:null,meeting_path:null});
    fixture.applyModel(model);
    document.getElementById('recall-terminal-container').classList.add('hidden');
    fixture.afterCapture();
    model.tabs[0].live=false;
    fixture.applyModel(model);
  });
  assert.equal(await page.locator('#fixture-header').innerText(),'recall —');
  assert.equal(await page.locator('#recall-terminal-container').isVisible(),true,'Stop metadata repaints the selected terminal without a tab switch');
  await page.waitForFunction(()=>document.querySelector('#recall-terminal-one .xterm-rows').textContent.includes('GRID-CANARY'));
  assert.equal(await page.locator('#fixture-meetings .meeting-item').isVisible(),true);
  assert.deepEqual(await page.evaluate(()=>fixture.refreshes),[0],'Stop schedules the list refresh while processing continues');
  const macPage = await browser.newPage();
  await macPage.addInitScript(()=>Object.defineProperty(navigator,'platform',{value:'MacIntel'}));
  await macPage.goto(`http://127.0.0.1:${server.address().port}/grid`);
  await macPage.waitForFunction(()=>fixture.ready);
  await macPage.evaluate(async()=>{await fixture.activate('three');fixture.tabs.get('three').terminal.focus();});
  await macPage.keyboard.press('Meta+1');
  await macPage.waitForFunction(()=>document.querySelector('#recall-tab-one')?.getAttribute('aria-selected')==='true');
  await macPage.evaluate(()=>fixture.tabs.get('one').queue);
  assert.deepEqual(await macPage.evaluate(()=>({draft:fixture.tabs.get('one').draft,reliable:fixture.tabs.get('one').reliable})),{draft:'',reliable:true});
  assert.equal(await macPage.evaluate(()=>fixture.calls.filter(call=>call.command==='cmd_pty_input').length),0,'Cmd+1 does not write into either terminal');
  await macPage.close();
  console.log('Recall tabs rendered contracts passed: status, shortcuts, safe close focus, rejected cap open/cancel, close/cap labels, real xterm/PTY grids, resize deduplication, empty-chat reuse, themes.');
} finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
