// Exercise the production open queue, readiness deferral, Discuss/Prep, close
// confirmation and continuation together. Only native IPC/services are doubles;
// the controller, xterm, header rendering and terminal lifecycle are shipped code.
import assert from 'node:assert/strict';

export async function testMeetingOpen(page, url) {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  async function reset() {
    await page.goto(url);
    await page.waitForFunction(() => fixture.ready);
    await page.evaluate(async () => {
      // A direct Claude conversation would accept a starter if Discuss leaked
      // past the rejected open. Keep it empty and reliable to expose that bug.
      Object.assign(fixture.model.tabs[1], { shell: false, claude_active: true });
      await fixture.activate('two');
      const tab = fixture.tabs.get('two');
      tab.pending = false;
      window.pastes = [];
      const paste = tab.terminal.paste.bind(tab.terminal);
      tab.terminal.paste = text => { pastes.push(text); paste(text); };
      fixture.calls.length = 0;
    });
  }
  async function assertOriginalConversation() {
    assert.equal(await page.locator('#fixture-header').innerText(), 'recall —', 'a rejected meeting open preserves the plain-chat header');
    assert.deepEqual(await page.evaluate(() => ({
      active: fixture.model.active, tabs: fixture.model.tabs.length,
      meeting: fixture.model.tabs.find(tab => tab.id === 'two').meeting_path || null,
      draft: fixture.tabs.get('two').draft, pastes,
      input: fixture.calls.filter(call => call.command === 'cmd_pty_input'),
    })), { active: 'two', tabs: 4, meeting: null, draft: '', pastes: [], input: [] }, 'the blocked request neither changes focus nor inserts text into the current Claude conversation');
    assert.equal(await page.locator('#recall-terminal-container').isVisible(), true);
  }
  for (const intent of ['discuss', 'prep']) {
    await reset();
    const meeting = { path: '/synthetic/engineering-standup.md', title: 'Engineering Standup — SSO', intent };
    const discuss = () => page.evaluate(({ path, title, intent }) => fixture.discuss(path, title, intent), meeting);
    await discuss();
    await assertOriginalConversation();
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await assertOriginalConversation();
    assert.equal(await page.locator('#recall-tab-prompt').isVisible(), false);

    await discuss();
    await page.getByRole('button', { name: 'Close Chat 1 and continue', exact: true }).click();
    await page.getByRole('button', { name: 'Close conversation', exact: true }).waitFor();
    await assertOriginalConversation();
    // Cancelling the saved-conversation confirmation also leaves the original
    // request unapplied and all four conversations present.
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await assertOriginalConversation();

    await discuss();
    await page.getByRole('button', { name: 'Close Chat 1 and continue', exact: true }).click();
    await page.getByRole('button', { name: 'Close conversation', exact: true }).click();
    await page.waitForFunction(path => fixture.model.tabs.find(tab => tab.id === fixture.model.active)?.meeting_path === path, meeting.path);
    const phase = intent === 'prep' ? 'prep' : 'debrief';
    assert.equal(await page.getByRole('tab', { selected: true }).innerText(), meeting.title);
    assert.equal(await page.locator('#fixture-header').innerText(), `${phase} — ${meeting.title}`);
    assert.deepEqual(await page.evaluate(() => ({
      tabs: fixture.model.tabs.length, closed: !fixture.model.tabs.some(tab => tab.id === 'one'),
      phase: fixture.model.tabs.find(tab => tab.id === fixture.model.active).phase,
      request: fixture.calls.filter(call => call.command === 'cmd_recall_tab_open').at(-1).args,
      confirmed: fixture.calls.some(call => call.command === 'cmd_recall_tab_close' && call.args.confirmed), pastes,
    })), { tabs: 4, closed: true, phase, request: { meetingPath: meeting.path, newTab: false, captureId: null, meetingTitle: meeting.title, phase }, confirmed: true, pastes: [] }, 'close-and-confirm admits the originally requested meeting with its title and phase');
    await page.waitForFunction(() => fixture.calls.some(call => call.command === 'cmd_spawn_terminal' && call.args.tabId === 'meeting'));
    assert.equal(await page.evaluate(() => fixture.calls.filter(call => call.command === 'cmd_spawn_terminal').every(call => call.args.mode === 'deferred' && !call.args.meetingPath)), true, 'preserving the request does not materialize meeting context before a prompt');
    assert.equal(await page.locator('#recall-tab-prompt').isVisible(), false);
  }
  await reset();
  await page.evaluate(async () => {
    fixture.boundary.openError = 'fixture open failed';
    await fixture.discuss('/synthetic/rejected.md', 'Rejected meeting');
  });
  await assertOriginalConversation();
  assert.equal(await page.locator('#recall-tab-prompt').isVisible(), false);
  assert.deepEqual(errors, [], 'the full meeting-open lifecycle has no browser errors');
}


export async function testProcessExit(page, url) {
  await page.goto(url);
  await page.waitForFunction(() => fixture.ready);
  await page.evaluate(async () => {
    await fixture.activate('two');
    const model = structuredClone(fixture.model);
    Object.assign(model.tabs[1], { process_exited: true, claude_active: false, status: 'error' });
    fixture.applyModel(model);
    fixture.calls.length = 0;
    fixture.tabs.get('two').terminal.paste('must not enter a dead process\r');
  });
  assert.equal(await page.locator('#fixture-exit').evaluate(node => node.classList.contains('active')), true);
  assert.deepEqual(await page.evaluate(() => ({ alive: fixture.tabs.get('two').alive, draft: fixture.tabs.get('two').draft, input: fixture.calls.filter(call => call.command === 'cmd_pty_input') })), { alive: false, draft: '', input: [] });
  await page.evaluate(() => fixture.open('assistant', null, { tabId: 'two' }));
  assert.equal(await page.evaluate(() => fixture.calls.filter(call => call.command === 'cmd_spawn_terminal').length), 0, 'selecting an ended conversation does not silently start a new process');
  assert.equal(await page.locator('#fixture-exit').evaluate(node => node.classList.contains('active')), true);
  await page.evaluate(() => fixture.restart('two'));
  assert.equal(await page.evaluate(() => fixture.calls.filter(call => call.command === 'cmd_spawn_terminal').length), 1, 'the explicit action starts the same tab again');
  assert.equal(await page.evaluate(() => fixture.calls.find(call => call.command === 'cmd_spawn_terminal').args.tabId), 'two');
  assert.equal(await page.evaluate(() => fixture.tabs.get('two').alive), true);
  assert.equal(await page.locator('#fixture-exit').evaluate(node => node.classList.contains('active')), false);
  // A native exit event can arrive before the observer's metadata snapshot.
  await page.evaluate(() => { fixture.end('two'); fixture.calls.length = 0; });
  await page.evaluate(() => fixture.open('assistant', null, { tabId: 'two' }));
  assert.equal(await page.evaluate(() => fixture.calls.filter(call => call.command === 'cmd_spawn_terminal').length), 0, 'an early exit event also requires the explicit restart action');
  assert.equal(await page.locator('#fixture-exit').evaluate(node => node.classList.contains('active')), true);
}

export async function testStripResize(page, url) {
  await page.goto(url);
  await page.waitForFunction(() => fixture.ready);
  await page.evaluate(async () => {
    const model = structuredClone(fixture.model);
    model.tabs.forEach((tab, index) => { tab.title = 'Conversation ' + (index + 1) + ' with a longer title'; });
    fixture.applyModel(model);
    await fixture.activate('three');
    fixture.tabs.get('three').terminal.focus();
    window.resizeFocus = document.activeElement;
    document.getElementById('recall-panel').style.width = '280px';
  });
  await page.waitForFunction(() => {
    const strip = document.getElementById('recall-tabs').getBoundingClientRect();
    const active = document.querySelector('#recall-tabs [aria-selected="true"]').getBoundingClientRect();
    return active.left >= strip.left - 1 && active.right <= strip.right + 1;
  }, null, { timeout: 5000 });
  assert.equal(await page.evaluate(() => document.getElementById('recall-tabs').scrollLeft > 0), true, 'resize reveals the selected rightmost tab without a metadata update');
  assert.equal(await page.evaluate(() => document.activeElement === resizeFocus), true, 'revealing a tab does not move keyboard focus');
}
