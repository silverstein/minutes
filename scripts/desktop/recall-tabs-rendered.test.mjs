#!/usr/bin/env node
// Production tab controller and CSS, with native metadata/PTY boundaries stubbed.
import assert from 'node:assert/strict';
import { herdrLiveContracts } from './herdr-live-rendered.mjs';
import { createServer } from 'node:http';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve, extname } from 'node:path';
import { gridFixture } from './recall-tabs-grid-fixture.mjs';
import { testMeetingOpen, testProcessExit, testStripResize, testHerdrRestore } from './recall-tabs-meeting-open.test.mjs';
const require = createRequire(import.meta.url);
const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES || resolve('target/recall-browser/node_modules');
const { chromium } = require(require.resolve(resolve(modules, 'playwright')));
const source = readFileSync('tauri/src/index.html', 'utf8');
const css = source.match(/<style>([\s\S]*?)<\/style>/)[1];
const controller = readFileSync('tauri/src/scripts/recall-tabs.js', 'utf8');
const html = `<!doctype html><html><head><meta charset="utf-8"><style>${css}</style></head><body>
<div id="recall-panel" class="recall-panel expanded"><div class="recall-header"><div id="recall-tabs" class="recall-tabs" role="tablist"></div><div class="recall-header-actions"></div></div><div id="recall-tab-prompt" class="recall-tab-prompt" hidden></div>
<span data-recall-meeting="/live.md"></span><span data-recall-meeting="/prep.md"></span></div><script>${controller}</script><script>
const actions=[];let model={active:'live',tabs:[{id:'live',title:'Live call',meeting_path:'/live.md',status:'working',live:true},{id:'prep',title:'Board prep',meeting_path:'/prep.md',status:'working'},{id:'chat',title:'Chat',status:'done'},{id:'other',title:'Other',status:'done'}]};
const controls=MinutesRecallTabs.create({strip:document.getElementById('recall-tabs'),prompt:document.getElementById('recall-tab-prompt'),invoke:async()=>{},isRecording:()=>true,select:id=>{actions.push(['select',id]);model.active=id;controls.update(structuredClone(model));},create:()=>actions.push(['new']),resume:id=>actions.push(['resume',id]),openInHerdr:id=>actions.push(['herdr',id]),takeBack:(id,end,missing)=>actions.push(missing?['take-back',id,!!end,true]:['take-back',id,!!end]),copyAddress:address=>actions.push(['copy',address]),close:(id,confirmed)=>{actions.push(['close',id,!!confirmed]);return !confirmed && ['live','prep'].includes(id) ? 'confirm' : true;}});
controls.update(structuredClone(model));window.fixture={actions,model,controls};</script></body></html>`;
const server = createServer((request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;
  if (path.startsWith('/vendor/xterm/') || path.startsWith('/fonts/') || path === '/styles/ui-zoom-tokens.css') {
    const file = resolve('tauri/src', '.' + path);
    const mime = { '.css': 'text/css', '.woff2': 'font/woff2', '.js': 'text/javascript' }[extname(file)];
    if (!mime || !existsSync(file)) { response.writeHead(404).end(); return; }
    response.setHeader('Content-Type', mime);
    response.end(readFileSync(file)); return;
  }
  response.setHeader('Content-Type', 'text/html; charset=utf-8');
  response.end(path === '/meeting-open' ? gridFixture(source, css, controller, { meetingOpen: true }) : path === '/grid' ? gridFixture(source, css, controller) : html);
});
await new Promise(resolve => server.listen(Number(process.env.MINUTES_RECALL_TAB_PREVIEW) || 0, '127.0.0.1', resolve));
if (process.env.MINUTES_RECALL_TAB_PREVIEW) {
  console.log(`Recall tabs preview: http://127.0.0.1:${server.address().port}/grid`);
  await new Promise(() => {});
}
async function testHeaderLayout(page, url) {
  await page.goto(url);
  await page.waitForFunction(() => fixture.ready);
  await page.evaluate(() => document.fonts.ready);
  const meetingTitle = 'Quarterly operating review and customer onboarding follow-up '.repeat(4).trim();
  await page.evaluate(async () => {
    document.documentElement.dataset.platform = 'macos';
    document.body.classList.add('sidebar-collapsed');
    document.getElementById('recall-panel').style.flex = 'none';
    Object.assign(fixture.model.tabs[0], { claude_session_id: 'saved-meeting' });
    await fixture.activate('one');
    window.allTabs = structuredClone(fixture.model.tabs);
  });
  for (const width of [560, 720, 960]) {
    for (const count of [1, 3, 4]) {
      const layout = await page.evaluate(({ width, count, meetingTitle }) => {
        const model = structuredClone(fixture.model);
        model.tabs = structuredClone(allTabs.slice(0, count));
        model.tabs[0].title = 'Chat 1';
        if (count > 1) Object.assign(model.tabs[1], { title: 'Live call', live: true });
        if (count > 2) model.tabs[2].title = meetingTitle;
        if (count > 3) model.tabs[3].title = 'Another conversation about long-term planning priorities';
        document.getElementById('recall-panel').style.width = width + 'px';
        fixture.applyModel(model);
        fixture.setPhase('debrief', meetingTitle);
        const header = document.querySelector('.recall-header');
        const strip = document.getElementById('recall-tabs');
        const context = document.getElementById('recall-context-label');
        const actions = [...header.querySelectorAll('.recall-header-actions button')].filter(node => node.checkVisibility());
        const rect = node => node.getBoundingClientRect();
        const bounds = rect(header);
        const hit = node => {
          const box = rect(node);
          return node.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
        };
        const tab = strip.querySelector('[role="tab"]');
        const tabTop = rect(tab).top;
        const within = node => rect(node).left >= bounds.left && rect(node).right <= bounds.right + 1;
        const actionResults = actions.map(node => ({
          label: node.textContent, hit: hit(node), rowDelta: Math.abs(rect(node).top - tabTop),
        }));
        const tabResults = [...strip.querySelectorAll('button')].map(node => {
          node.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'instant' });
          return hit(node) && within(node);
        });
        tab.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'instant' });
        return {
          actionResults, tabResults, bounded: [...header.children].every(within) && header.scrollWidth <= header.clientWidth + 1,
          width: rect(document.getElementById('recall-panel')).width,
          shortTabWidth: rect(tab.closest('.recall-tab')).width,
          truncated: context.scrollWidth > context.clientWidth && getComputedStyle(context).textOverflow === 'ellipsis',
          tooltip: context.title, scrolls: strip.scrollWidth > strip.clientWidth,
          compactContextWidth: rect(context).width,
        };
      }, { width, count, meetingTitle });
      const scenario = width + 'px / ' + count + ' tabs';
      assert.equal(layout.width, width, scenario);
      assert.equal(layout.bounded, true, scenario + ' keeps every header group within the panel');
      assert.ok(layout.shortTabWidth < 120, scenario + ' gives Chat 1 a content-sized tab');
      assert.equal(layout.truncated, true, scenario + ' truncates the long context');
      assert.equal(layout.tooltip, 'Debrief — ' + meetingTitle);
      assert.deepEqual(layout.actionResults.map(item => item.label), ['Resume', 'Weekly', 'End Session']);
      assert.ok(layout.actionResults.every(item => item.hit && item.rowDelta <= 2), scenario + ' keeps every action on the tab row and clickable ' + JSON.stringify(layout.actionResults));
      assert.ok(layout.tabResults.every(Boolean), scenario + ' lets every tab button be reached with horizontal scrolling');
      if (width === 560 && count === 4) assert.ok(layout.scrolls, scenario + ' scrolls after the tabs reach their minimum');
      if (count === 1 || width === 960) assert.equal(layout.scrolls, false, scenario + ' fits the strip first');
      console.log(`Recall header ${scenario}: Chat 1 ${Math.round(layout.shortTabWidth)}px, context ${Math.round(layout.compactContextWidth)}px, strip ${layout.scrolls ? 'scrolls' : 'fits'}, all controls hit-tested.`);
    }
  }
  await page.setViewportSize({ width: 1000, height: 590 });
  assert.equal(await page.locator('#btn-recall-weekly').isVisible(), false);
  assert.equal(await page.locator('#recall-context-label').isVisible(), false);
  assert.equal(await page.locator('#recall-resume').isVisible(), true);
  assert.equal(await page.locator('#btn-recall-end').isVisible(), true);
  await page.setViewportSize({ width: 1000, height: 700 });
}

async function testCloseControls(page, url) {
  const tab = page.locator('#recall-close-one');
  const end = page.locator('#btn-recall-end');
  const armed = page.locator('button[data-close-armed]');
  const closeCalls = () => page.evaluate(() => fixture.calls.filter(call => call.command === 'cmd_recall_tab_close').map(call => call.args));
  async function reset({ live = false, active = 'one' } = {}) {
    await page.goto(url);
    await page.waitForFunction(() => fixture.ready);
    await page.evaluate(async ({ live, active }) => {
      Object.assign(fixture.model.tabs[0], { live, title: live ? 'Live call' : 'Saved conversation' });
      await fixture.activate(active);
      // Activation schedules terminal focus for the next frame.
      await new Promise(resolve => requestAnimationFrame(resolve));
      fixture.calls.length = 0;
    }, { live, active });
  }
  const closesTab = (id, confirmed = true) => page.waitForFunction(({ id, confirmed }) =>
    fixture.calls.some(call => call.command === 'cmd_recall_tab_close' && call.args.tabId === id && call.args.confirmed === confirmed),
  { id, confirmed });

  await reset({ active: 'two' });
  await tab.click();
  await page.getByRole('button', { name: 'Confirm close for Saved conversation', exact: true }).waitFor();
  assert.equal(await armed.count(), 1);
  assert.equal(await tab.getAttribute('aria-describedby'), 'recall-close-description');
  assert.match(await page.locator('#recall-close-description').textContent(), /Your agent keeps the transcript/);
  assert.equal(await page.locator('#recall-tab-prompt').isVisible(), false);
  await page.evaluate(() => fixture.applyModel(structuredClone(fixture.model)));
  assert.equal(await tab.getAttribute('data-close-armed'), '', 'metadata refresh preserves the inline confirmation');
  assert.equal(await tab.evaluate(node => document.activeElement === node), true);
  await page.keyboard.press('Escape');
  assert.equal(await tab.evaluate(node => document.activeElement === node), true, 'Escape returns focus to the original button');
  assert.equal(await armed.count(), 0);

  await tab.click();
  await armed.waitFor();
  await tab.blur();
  assert.equal(await armed.count(), 0);
  await tab.click();
  await armed.waitFor();
  await page.locator('.recall-header').click({ position: { x: 1, y: 1 } });
  assert.equal(await armed.count(), 0, 'clicking elsewhere disarms the tab');
  assert.ok((await closeCalls()).every(call => !call.confirmed));
  await tab.click();
  await armed.waitFor();
  await tab.click();
  await closesTab('one');
  assert.deepEqual((await closeCalls()).slice(-2), [{ tabId: 'one', confirmed: false }, { tabId: 'one', confirmed: true }]);

  await reset();
  await end.focus();
  await page.keyboard.press('Enter');
  await page.getByRole('button', { name: 'Confirm End Session for Saved conversation', exact: true }).waitFor();
  await page.keyboard.press('Escape');
  assert.equal(await end.evaluate(node => document.activeElement === node), true);
  await end.click();
  await armed.waitFor();
  await page.waitForFunction(() => document.getElementById('btn-recall-end').textContent === 'End Session', null, { timeout: 6000 });
  assert.ok((await closeCalls()).every(call => !call.confirmed));
  await end.click();
  await armed.waitFor();
  await page.keyboard.press('Space');
  await closesTab('one');

  await reset();
  await end.focus();
  await page.keyboard.down('Enter');
  await armed.waitFor();
  await page.keyboard.down('Enter');
  await page.keyboard.up('Enter');
  assert.equal((await closeCalls()).some(call => call.confirmed), false, 'holding Enter cannot skip the second activation');
  await page.keyboard.press('Enter');
  await closesTab('one');

  for (const [route, live] of [['DOM', false], ['native menu', false], ['DOM', true], ['native menu', true]]) {
    await reset({ live });
    const shortcut = route === 'DOM' ? () => page.keyboard.press('Meta+w') : () => page.evaluate(() => fixture.emit('minutes://close-context'));
    await shortcut();
    await armed.waitFor();
    await page.keyboard.press('Escape');
    assert.equal(await page.evaluate(() => document.getElementById('recall-terminal-one').contains(document.activeElement)), true);
    await shortcut();
    await armed.waitFor();
    await shortcut();
    if (live) {
      assert.equal((await closeCalls()).some(call => call.confirmed), false, route + ' requires an explicit live-tab confirmation');
      await tab.click();
    }
    await closesTab('one');
  }
  await reset({ active: 'two' });
  await page.locator('#recall-close-two').click();
  await page.waitForFunction(() => !fixture.model.tabs.some(tab => tab.id === 'two'));
  assert.equal(await armed.count(), 0);
  assert.deepEqual(await closeCalls(), [{ tabId: 'two', confirmed: false }]);
  console.log('Recall close controls: inline click, Enter/Space, Escape, blur, outside click, metadata refresh, End timer, DOM and native-menu shortcut double press, explicit live close, and immediate unused close.');
}

const browser = await chromium.launch({ headless: true });
try {
  await herdrLiveContracts(browser, source, css);
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
  assert.equal(await page.locator('.recall-header-actions > #recall-resume').count(), 1, 'resume sits with the header actions, not in a row of its own');
  assert.equal(await page.evaluate(() => fixture.actions.some(action => action[0] === 'resume')), false);
  await page.getByRole('button', { name: 'Resume previous conversation' }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['resume', 'prep']);
  await page.evaluate(() => { fixture.model.tabs[1].claude_active = true; fixture.controls.update(structuredClone(fixture.model)); });
  assert.equal(await page.getByRole('button', { name: 'Resume previous conversation' }).count(), 0);
  // Availability, explicit handoff confirmation and durable external-owner state.
  assert.equal(await page.getByRole('button', { name: 'Conversation actions for Board prep' }).count(), 0);
  await page.evaluate(() => fixture.controls.setHerdrAvailable(true));
  await page.evaluate(() => fixture.controls.confirmOpen('live'));
  assert.match(await page.locator('#recall-tab-prompt').innerText(), /Wait for this reply to finish/);
  assert.equal(await page.locator('#recall-tab-prompt').getByRole('button', { name: 'Open in Herdr', exact: true }).count(), 0);
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.getByRole('button', { name: 'Conversation actions for Board prep' }).click();
  await page.getByRole('button', { name: 'Open in Herdr', exact: true }).click();
  assert.match(await page.locator('#recall-tab-prompt').innerText(), /Move this conversation to Herdr/);
  assert.equal(await page.getByRole('button', { name: 'Cancel', exact: true }).evaluate(node => node === document.activeElement), true);
  await page.keyboard.press('Enter');
  assert.equal(await page.evaluate(() => fixture.actions.some(action => action[0] === 'herdr')), false);
  await page.evaluate(() => fixture.controls.confirmOpen('prep'));
  await page.getByRole('button', { name: 'Open in Herdr', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['herdr', 'prep']);
  await page.evaluate(() => { fixture.model.tabs[1].herdr_terminal = 't42'; fixture.controls.update(structuredClone(fixture.model)); });
  assert.match(await page.locator('#recall-herdr-state').innerText(), /In Herdr · t42/);
  assert.equal(await page.getByRole('button', { name: 'Resume previous conversation' }).count(), 0);
  await page.getByRole('button', { name: 'Copy address', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['copy', 't42']);
  await page.getByRole('button', { name: 'Open terminal', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['copy', 'hc attach t42']);
  await page.getByRole('button', { name: 'Take back', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['take-back', 'prep', false]);
  await page.evaluate(() => fixture.controls.confirmTakeBack('prep'));
  await page.getByRole('button', { name: 'End terminal and take back', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['take-back', 'prep', true]);
  await page.evaluate(() => fixture.controls.confirmMissing('prep'));
  assert.match(await page.locator('#recall-tab-prompt').innerText(), /Herdr has no record of this terminal/);
  assert.equal(await page.getByRole('button', { name: 'Cancel', exact: true }).evaluate(node => node === document.activeElement), true);
  const beforeMissingCancel = await page.evaluate(() => fixture.actions.length);
  await page.keyboard.press('Enter');
  assert.equal(await page.evaluate(() => fixture.actions.length), beforeMissingCancel);
  await page.evaluate(() => fixture.controls.confirmMissing('prep'));
  await page.getByRole('button', { name: 'Take conversation back', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => fixture.actions.at(-1)), ['take-back', 'prep', false, true]);
  await page.evaluate(() => { fixture.controls.setHerdrAvailable(false); });
  assert.equal(await page.locator('#recall-herdr-state').isVisible(), true, 'a saved owner stays visible even when discovery is down');
  await page.evaluate(() => { delete fixture.model.tabs[1].herdr_terminal; fixture.controls.update(structuredClone(fixture.model)); });
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
    assert.equal(await page.evaluate(()=>document.getElementById('recall-terminal-one').contains(document.activeElement)),true,`cancelling with ${cancelKey} returns the keyboard to the selected terminal`);
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
  // No explicit focus here: typing right after Cancel must reach the terminal.
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
  await testMeetingOpen(page, `http://127.0.0.1:${server.address().port}/meeting-open`);
  await testHerdrRestore(page, `http://127.0.0.1:${server.address().port}/meeting-open`);
  await testProcessExit(page, `http://127.0.0.1:${server.address().port}/meeting-open`);
  await testHeaderLayout(page, `http://127.0.0.1:${server.address().port}/meeting-open`);
  await testStripResize(page, `http://127.0.0.1:${server.address().port}/grid`);
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
  await testCloseControls(macPage, `http://127.0.0.1:${server.address().port}/meeting-open`);
  await macPage.close();
  console.log('Recall tabs rendered contracts passed: status, shortcuts, safe close focus, rejected cap open/cancel, close/cap labels, real xterm/PTY grids, resize deduplication, empty-chat reuse, themes.');
} finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
