#!/usr/bin/env node
/** Runs the production Recall DOM, CSS, controller and safe Markdown renderer
 * in Chromium, with a deliberately stubbed Tauri event/command boundary.
 * This is not native app / capture / permission / hotkey acceptance testing. */
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFileSync, existsSync, mkdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, extname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const require = createRequire(import.meta.url);
// Use an explicit or repo-owned runtime, never a shared ancestor installation.
const modules = process.env.CODEX_PRIMARY_RUNTIME_NODE_MODULES
  || resolve(root, 'target/recall-browser/node_modules');
let playwrightPath;
try { playwrightPath = require.resolve(resolve(modules, 'playwright')); }
catch { throw new Error('Install Playwright in target/recall-browser, or set CODEX_PRIMARY_RUNTIME_NODE_MODULES to an isolated node_modules directory. See scripts/desktop/README.md.'); }
const { chromium } = require(playwrightPath);
const source = readFileSync(resolve(root, 'tauri/src/index.html'), 'utf8');
const rendererStart = source.indexOf('    marked.setOptions({ breaks: true });');
const rendererEnd = source.indexOf('    function renderTextToElement(', rendererStart);
assert.ok(rendererStart > 0 && rendererEnd > rendererStart, 'Production Markdown renderer markers changed');
const css = [...source.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)].map(match => match[1]).join('\n');
const links = [...source.matchAll(/<link[^>]+rel="stylesheet"[^>]*>/g)].map(match => match[0]).join('\n');
const fixture = `<!doctype html><html data-platform="linux"><head><meta charset="utf-8">${links}<style>${css}
.fixture-isolated .recall-panel.expanded { width: 100%; position: absolute; inset: 0; }
</style></head><body class="fixture-isolated">
<script>const source = ${JSON.stringify(source).replace(/</g, '\\u003c')};
const original = new DOMParser().parseFromString(source, 'text/html');
document.body.append(document.importNode(original.getElementById('recall-panel'), true));
document.getElementById('recall-panel').classList.add('expanded');
document.getElementById('recall-panel').removeAttribute('inert');
document.getElementById('recall-panel').setAttribute('aria-hidden', 'false');
document.getElementById('recall-chat-panel').style.display = 'flex';
</script>
<script src="/vendor/marked.min.js"></script><script src="/scripts/recall-contract.js"></script><script src="/scripts/recall-chat.js"></script>
<script>${source.slice(rendererStart, rendererEnd)}
const handlers = new Map(), calls = [], reports = [];
let failSend = false, failListen = null, releaseClear = null, pauseClear = false;
const invoke = async (command, args) => {
  calls.push({command, args});
  if(command === 'cmd_recall_chat_send' && failSend) throw new Error('provider unavailable');
  if(command === 'cmd_recall_chat_clear' && pauseClear) await new Promise(resolve => { releaseClear = resolve; });
};
const listen = async (name, handler) => {
  if(name === failListen) { failListen = null; throw new Error('listener unavailable'); }
  const callbacks = handlers.get(name) || new Set(); callbacks.add(handler); handlers.set(name, callbacks);
  return () => callbacks.delete(handler);
};
const controller = window.MinutesRecallChat.create({
  messages: document.getElementById('recall-chat-messages'), input: document.getElementById('recall-chat-input'),
  sendButton: document.getElementById('recall-chat-send'), jumpButton: document.getElementById('recall-chat-jump'),
  invoke, listen, renderMarkdown: renderMarkdownToElement, report: (...args) => reports.push(args),
});
controller.showWelcome();
window.fixture = { controller, calls, reports,
  emit: (name, payload) => { for(const handler of handlers.get(name) || []) handler({payload}); },
  failSend: value => { failSend = value; }, failListen: name => { failListen = name; },
  pauseClear: () => { pauseClear = true; }, releaseClear: () => { pauseClear = false; releaseClear(); },
  listenerCount: () => [...handlers.values()].reduce((sum, callbacks) => sum + callbacks.size, 0),
};
</script></body></html>`;
const assetRoot = resolve(root, 'tauri/src');
const server = createServer((request, response) => {
  const path = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
  if (path === '/fixture') { response.setHeader('Content-Type', 'text/html; charset=utf-8'); response.end(fixture); return; }
  const file = resolve(assetRoot, '.' + path);
  if (!file.startsWith(assetRoot + '/') || !existsSync(file)) { response.writeHead(404).end(); return; }
  const type = { '.js': 'text/javascript', '.css': 'text/css', '.woff2': 'font/woff2', '.png': 'image/png' }[extname(file)] || 'application/octet-stream';
  response.setHeader('Content-Type', type); response.end(readFileSync(file));
});
await new Promise(done => server.listen(0, '127.0.0.1', done));
let browser;
try {
  browser = await chromium.launch({ headless: true, timeout: 15000, args: JSON.parse(process.env.PLAYWRIGHT_CHROMIUM_ARGS || '[]'), ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? {executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE} : {}) });
  const page = await browser.newPage({ viewport: { width: 640, height: 720 } });
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', error => { errors.push(error.message); console.error('Fixture page error:', error.message); });
  const url = `http://127.0.0.1:${server.address().port}/fixture`;
  const emit = (name, payload) => page.evaluate(({ name, payload }) => window.fixture.emit(name, payload), { name, payload });
  const send = async text => {
    const before = await page.evaluate(() => window.fixture.calls.filter(call => call.command === 'cmd_recall_chat_send').length);
    await page.locator('#recall-chat-input').fill(text);
    await page.locator('#recall-chat-send').click();
    await page.waitForFunction(count => window.fixture.calls.filter(call => call.command === 'cmd_recall_chat_send').length > count, before);
  };
  const text = () => page.locator('.chat-bubble-assistant').last().textContent();
  await page.goto(url); await page.waitForFunction(() => window.fixture);
  // Fast providers: listener registration precedes invoke; concurrent register is idempotent.
  await page.evaluate(() => Promise.all([window.fixture.controller.register(), window.fixture.controller.register()]));
  assert.equal(await page.evaluate(() => window.fixture.listenerCount()), 3);
  await send('What did we decide?');
  await emit('recall-chat-chunk', { type: 'thinking' });
  assert.equal(await page.locator('.streaming').getAttribute('data-status'), 'thinking');
  await emit('recall-chat-chunk', { type: 'assistant', message: { content: [{type: 'tool_use', name: 'mcp__minutes__search_meetings'}] } });
  assert.equal(await page.locator('.streaming').getAttribute('data-status'), 'searching your meetings');
  await emit('recall-chat-chunk', { type: 'text', text: '**Monthly** billing.' });
  await emit('recall-chat-done');
  assert.equal(await page.locator('.chat-bubble-assistant').last().locator('strong').textContent(), 'Monthly');
  assert.equal(await text(), 'Monthly billing.\n');
  assert.equal(await page.locator('#recall-chat-input').isDisabled(), false);

  // Claude partial + complete + result must render exactly once.
  await send('Summarize the choice');
  await emit('recall-chat-chunk', { type: 'stream_event', event: { type: 'content_block_delta', delta: { type: 'text_delta', text: 'A single answer.' } } });
  await emit('recall-chat-chunk', { type: 'assistant', message: { content: [{type: 'text', text: 'A single answer.'}] } });
  await emit('recall-chat-chunk', { type: 'result', subtype: 'success', result: 'A single answer.' });
  await emit('recall-chat-done');
  assert.equal(await text(), 'A single answer.\n');

  // Failure in the same event-loop turn as a chunk must survive the pending render and done.
  await send('What is the follow-up?');
  await page.evaluate(() => {
    window.fixture.emit('recall-chat-chunk', {type: 'text', text: 'Partial answer'});
    window.fixture.emit('recall-chat-error', 'Connection lost. Answer incomplete.');
    window.fixture.emit('recall-chat-done');
  });
  assert.match(await text(), /Partial answer[\s\S]*Connection lost/);
  assert.equal(await page.locator('.chat-incomplete-notice').count(), 1);
  // Error arriving after done remains visible, including an empty answer.
  await send('Try again'); await emit('recall-chat-done'); await emit('recall-chat-error', 'Provider unavailable');
  assert.equal(await text(), 'Provider unavailable');

  // Invalid envelopes and hostile model markup cannot crash or inject the DOM.
  await send('Render safely');
  for (const payload of [null, {type:'assistant', message:{content:[null,{type:'text',text:3}]}}, {type:'text',text:{}}, {type:'future_event'}, {type:'system'}]) await emit('recall-chat-chunk', payload);
  await emit('recall-chat-chunk', {type:'result', subtype:'success', result:'<img src=x onerror="window.pwned=true">\n\n![tracking](https://invalid.example/track.png)'});
  await emit('recall-chat-done');
  assert.equal(await page.locator('.chat-bubble img').count(), 0);
  assert.equal(await page.evaluate(() => Boolean(window.pwned)), false);
  assert.equal(await page.evaluate(() => window.fixture.reports.length), 3);

  // Long answers honor a reader's scroll position; the jump control reattaches.
  await send('Long answer');
  await emit('recall-chat-chunk', {type:'text', text: Array.from({length:80}, (_,i) => `Paragraph ${i}\n\n`).join('')});
  await page.waitForFunction(() => document.getElementById('recall-chat-messages').scrollHeight > 1000);
  await page.locator('#recall-chat-messages').evaluate(node => { node.scrollTop = 0; });
  await emit('recall-chat-chunk', {type:'text', text:'Final paragraph.'});
  await page.locator('#recall-chat-jump').waitFor({state:'visible'});
  assert.equal(await page.locator('#recall-chat-messages').evaluate(node => node.scrollTop), 0);
  await page.locator('#recall-chat-jump').click();
  assert.ok(await page.locator('#recall-chat-messages').evaluate(node => node.scrollTop > 0));
  await page.locator('#recall-chat-send').click();
  assert.equal(await page.evaluate(() => window.fixture.calls.at(-1).command), 'cmd_recall_chat_cancel');
  await emit('recall-chat-done');

  // Reset cancels a queued render and clears both presentation and backend history.
  await send('Old meeting');
  await page.evaluate(async () => { window.fixture.emit('recall-chat-chunk', {type:'text',text:'Old content'}); await window.fixture.controller.reset(); });
  assert.equal(await page.locator('.chat-bubble').count(), 1);
  assert.ok(!(await text()).includes('Old content'));
  assert.equal(await page.evaluate(() => window.fixture.calls.at(-1).command), 'cmd_recall_chat_clear');
  await page.evaluate(() => window.fixture.failSend(true));
  await send('Unavailable');
  await page.waitForFunction(() => document.getElementById('recall-chat-send').textContent === 'Send');
  assert.match(await text(), /provider unavailable/);

  // A delayed backend reset cannot race a new send and clear its history.
  await page.evaluate(() => { window.fixture.pauseClear(); window.resetDone = window.fixture.controller.reset(); });
  await emit('recall-chat-done');
  assert.equal(await page.locator('#recall-chat-input').isDisabled(), true);
  await page.evaluate(() => { window.fixture.releaseClear(); return window.resetDone; });
  assert.equal(await page.locator('#recall-chat-input').isDisabled(), false);

  // Actual DOM geometry in narrow and wide windows, with the production CSS.
  for (const width of [360, 640, 960]) {
    await page.setViewportSize({width, height:720});
    const box = await page.locator('#recall-chat-input').boundingBox();
    assert.ok(box && box.width > 100 && box.x >= 0 && box.x + box.width <= width && box.y + box.height <= 720, `composer fits ${width}px viewport`);
  }
  // Use the actual shell transitions to exclude collapsed controls from both
  // keyboard focus and accessibility, while preserving the unsent draft.
  const transitionsStart = source.indexOf('    async function expandRecallPanel() {');
  const transitionsEnd = source.indexOf('    async function toggleRecallPanel()', transitionsStart);
  const transitions = await page.evaluate(async code => {
    let recallExpanded = true, expandAnimating = false, userExplicitlyCollapsed = false;
    const recallPanel = document.getElementById('recall-panel');
    const splitHandle = document.createElement('div');
    const assistantBtn = document.createElement('button');
    const recallNudge = document.createElement('span');
    const currentWindow = { isFullscreen: async () => true };
    const EXPANDED_WIDTH = 960, COLLAPSED_WIDTH = 520;
    const applyRecallWindowLayout = async () => {};
    const applyRecallRatio = () => {}, getStoredRecallRatio = () => 0.5;
    const persistRecallWorkspaceState = () => {};
    const recallDevMode = false;
    const applyRecallMode = () => {};
    const registerChatListeners = async () => {};
    const syncNarrowLayout = () => window.fixture.syncLayout?.();
    const input = document.getElementById('recall-chat-input');
    input.value = 'Unsent draft';
    input.focus();
    const { expandRecallPanel, collapseRecallPanel } = eval('(function(){' + code
      + ';return {expandRecallPanel,collapseRecallPanel};})()');
    window.fixture.openRecall = expandRecallPanel;
    window.fixture.closeRecall = collapseRecallPanel;
    await collapseRecallPanel();
    await new Promise(done => setTimeout(done, 150));
    input.focus();
    const hidden = recallPanel.getAttribute('aria-hidden') === 'true' && recallPanel.inert;
    const cannotFocus = document.activeElement !== input;
    await expandRecallPanel();
    input.focus();
    return { hidden, cannotFocus, restored: document.activeElement === input
      && !recallPanel.inert && recallPanel.getAttribute('aria-hidden') === 'false',
      draft: input.value };
  }, source.slice(transitionsStart, transitionsEnd));
  assert.deepEqual(transitions, { hidden: true, cannotFocus: true, restored: true, draft: 'Unsent draft' });

  // The isolated streaming fixture must not mask the real sidebar/document
  // flex layout. Reuse the production shell and its responsive policy here.
  const policyStart = source.indexOf('    function applySidebarCollapsed(');
  const policyEnd = source.indexOf('    syncNarrowLayout();', policyStart);
  assert.ok(policyStart > 0 && policyEnd > policyStart, 'Production responsive policy markers exist');
  await page.evaluate(code => {
    const original = new DOMParser().parseFromString(source, 'text/html');
    const panel = document.getElementById('recall-panel');
    for (const selector of ['.app-left', '#btn-sidebar-expand', '#split-handle']) {
      document.body.insertBefore(document.importNode(original.querySelector(selector), true), panel);
    }
    const documentPane = document.importNode(original.getElementById('md-viewer'), true);
    documentPane.classList.add('active');
    document.body.append(documentPane);
    document.documentElement.dataset.platform = 'macos';
    document.body.className = 'doc-open';
    let sidebarAutoCollapsed = false;
    let userExpandedSidebarThisSession = false;
    const sidebarCollapseButton = document.getElementById('btn-sidebar-collapse');
    const policy = eval('(function(){' + code + ';return {syncNarrowLayout,applySidebarCollapsed};})()');
    window.fixture.syncLayout = policy.syncNarrowLayout;
    window.fixture.collapseSidebar = policy.applySidebarCollapsed;
  }, source.slice(policyStart, policyEnd));
  for (const width of [960, 1100, 640]) {
    await page.setViewportSize({ width, height: 780 });
    await page.evaluate(() => window.fixture.syncLayout());
    await page.waitForTimeout(200); // Wait for the shipped sidebar transition.
    const pane = await page.locator('#recall-panel').boundingBox();
    const composer = await page.locator('#recall-chat-input').boundingBox();
    assert.ok(pane && pane.width >= 280 && pane.x + pane.width <= width + 1,
      `Recall pane retains usable width beside a document at ${width}px`);
    assert.ok(composer && composer.width > 150 && composer.x + composer.width <= width + 1,
      `production-shell composer fits ${width}px viewport`);
  }
  // A secondary pane is required before hiding the primary Meetings pane.
  // Use the shipped policy and Recall transitions, including persisted state,
  // rather than a parallel model of the responsive rules.
  await page.evaluate(() => window.fixture.closeRecall());
  await page.waitForTimeout(150);
  await page.evaluate(() => {
    document.getElementById('md-viewer').classList.remove('active');
    document.body.classList.remove('doc-open', 'sidebar-collapsed');
    document.getElementById('recall-panel').classList.remove('expanded');
    localStorage.removeItem('minutes.sidebarCollapsed');
  });
  const primaryVisible = async () => {
    assert.equal(await page.locator('#btn-record').isVisible(), true, 'primary recording control remains visible');
    assert.equal(await page.locator('#btn-assistant').isVisible(), true, 'Recall remains reachable from primary pane');
    assert.equal(await page.evaluate(() => document.body.classList.contains('sidebar-collapsed')), false);
    const solePane = await page.evaluate(() => !document.querySelector('#recall-panel.expanded, #md-viewer.active'));
    if (solePane) {
      const box = await page.locator('#btn-record').boundingBox();
      const width = await page.evaluate(() => window.innerWidth);
      assert.ok(box && box.x >= 0 && box.x + box.width <= width, 'primary recording control fits the narrow window');
      assert.equal(await page.locator('#btn-sidebar-collapse').isDisabled(), true, 'sole pane cannot be collapsed');
    }
  };
  for (const width of [460, 560, 700, 900]) {
    await page.setViewportSize({ width, height: 780 });
    await page.evaluate(() => window.fixture.syncLayout());
    await primaryVisible();
  }
  await page.setViewportSize({ width: 560, height: 780 });
  await page.evaluate(() => window.fixture.collapseSidebar(true, { explicit: true }));
  await primaryVisible();
  await page.evaluate(() => {
    localStorage.setItem('minutes.sidebarCollapsed', 'true');
    window.fixture.collapseSidebar(true); // Saved collapse on startup.
    window.fixture.syncLayout();
  });
  await primaryVisible();
  assert.equal(await page.evaluate(() => localStorage.getItem('minutes.sidebarCollapsed')), 'true', 'saved preference survives while the sole pane stays usable');
  await page.evaluate(() => window.fixture.openRecall());
  assert.equal(await page.evaluate(() => document.body.classList.contains('sidebar-collapsed')), true, 'saved collapse applies when Recall becomes visible');
  assert.equal(await page.locator('#recall-panel').isVisible(), true);
  await page.evaluate(() => window.fixture.closeRecall());
  await page.waitForTimeout(150);
  await primaryVisible(); // Actual close handler, without a resize event.
  await page.evaluate(() => {
    document.getElementById('md-viewer').classList.add('active');
    document.body.classList.add('doc-open');
    window.fixture.syncLayout();
  });
  assert.equal(await page.evaluate(() => document.body.classList.contains('sidebar-collapsed')), true);
  assert.equal(await page.locator('#md-viewer').isVisible(), true, 'document may replace the list');
  await page.evaluate(() => {
    document.getElementById('md-viewer').classList.remove('active');
    document.body.classList.remove('doc-open');
    window.fixture.syncLayout(); // Same call made by the document close path.
  });
  await primaryVisible();
  await page.evaluate(() => {
    localStorage.removeItem('minutes.sidebarCollapsed');
    document.getElementById('recall-panel').classList.add('expanded');
    window.fixture.syncLayout();
  });
  // A prior manual expansion is respected until the window becomes wide.
  await page.setViewportSize({ width: 900, height: 780 });
  await page.evaluate(() => window.fixture.syncLayout());
  await page.setViewportSize({ width: 560, height: 780 });
  await page.evaluate(() => window.fixture.syncLayout());
  assert.equal(await page.evaluate(() => document.body.classList.contains('sidebar-collapsed')), true, 'automatic narrow collapse still works with Recall visible');
  await page.evaluate(() => window.fixture.collapseSidebar(false, { explicit: true }));
  await page.evaluate(() => window.fixture.syncLayout());
  await primaryVisible(); // Manual expansion still overrides narrow policy.
  await page.evaluate(() => {
    document.getElementById('recall-panel').classList.remove('expanded');
    window.fixture.syncLayout();
  });
  await primaryVisible();
  if (process.env.MINUTES_DESKTOP_SCREENSHOT) {
    mkdirSync(dirname(resolve(process.env.MINUTES_DESKTOP_SCREENSHOT)), {recursive:true});
    await page.screenshot({path:resolve(process.env.MINUTES_DESKTOP_SCREENSHOT)});
  }
  await page.evaluate(() => window.fixture.controller.dispose());
  assert.equal(await page.evaluate(() => window.fixture.listenerCount()), 0);

  // Partial listener failure rolls back all subscriptions, and retry works.
  await page.goto(url); await page.waitForFunction(() => window.fixture);
  await page.evaluate(async () => { window.fixture.failListen('recall-chat-error'); try { await window.fixture.controller.register(); } catch {} });
  assert.equal(await page.evaluate(() => window.fixture.listenerCount()), 0);
  await page.evaluate(() => window.fixture.controller.register());
  assert.equal(await page.evaluate(() => window.fixture.listenerCount()), 3);
  // Exercise the shell's actual native-mode handoff with a slow/failing IPC
  // subscription. The composer must remain visible before listeners settle.
  const modeStart = source.indexOf('    async function applyRecallMode() {');
  const modeEnd = source.indexOf('    // ── Expand / collapse', modeStart);
  assert.ok(modeStart > 0 && modeEnd > modeStart, 'Production mode handoff markers exist');
  const handoff = await page.evaluate(async code => {
    const recallDevMode = false;
    const recallTerminalContainer = document.getElementById('recall-terminal-container');
    const recallChatPanel = document.getElementById('recall-chat-panel');
    const recallLoading = document.getElementById('recall-loading');
    let fail, notice = '';
    const registerChatListeners = () => new Promise((_, reject) => { fail = reject; });
    const showChatWelcome = () => { recallChatPanel.dataset.welcome = 'shown'; };
    const setRecallProviderNotice = message => { notice = message; };
    const console = { error() {} };
    const apply = eval('(' + code + ')');
    recallChatPanel.style.display = 'none';
    const pending = apply();
    const visibleBeforeReady = recallChatPanel.style.display === 'flex'
      && recallChatPanel.dataset.welcome === 'shown';
    fail(new Error('listener unavailable'));
    await pending;
    return { visibleBeforeReady, notice, terminalHidden: recallTerminalContainer.classList.contains('hidden') };
  }, source.slice(modeStart, modeEnd));
  assert.equal(handoff.visibleBeforeReady, true);
  assert.equal(handoff.terminalHidden, true);
  assert.match(handoff.notice, /Chat could not start/);
  assert.deepEqual(errors, []);
  console.log('Recall rendered workflow: PASS (production DOM/CSS/controller/renderer; stubbed Tauri IPC)');
} finally {
  await browser?.close(); await new Promise(done => server.close(done));
}
