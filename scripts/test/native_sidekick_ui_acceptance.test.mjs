import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';

import {
  appendBoundedOutput,
  assertMacSessionUnlocked,
  canonicalExistingPath,
  cleanupNativeSidekickProcessLanes,
  evaluateNativeSidekickUiAcceptance,
  nativeSidekickLaunchServicesArgs,
  nativeSidekickFailureWithLogs,
  nativeSidekickTemporaryParent,
  parseMacSessionLockProbe,
  parseLsofTextIdentities,
  readBoundedOutputFile,
  singleFlightAsync,
  terminateNewExactProcesses,
} from '../run_native_sidekick_ui_acceptance.mjs';
import { sidekickSemanticResponseSha256 } from '../lib/sidekick_exact_semantic_gate.mjs';
import { semanticJudgeCriteria } from '../lib/sidekick_semantic_judge.mjs';

const sidekickHtml = new URL('../../tauri/src/sidekick.html', import.meta.url);
const qualitySourceBinding = {
  git_commit: 'c'.repeat(40),
  quality_surface_sha256: 'e'.repeat(64),
  fixture_sha256: 'f'.repeat(64),
};

function semanticPassVerdict() {
  const turn = (criteria) => Object.fromEntries([
    ...criteria.map((name) => [name, true]),
    ['reason', 'Pass.'],
  ]);
  return {
    turn_1: turn(semanticJudgeCriteria.turn_1),
    turn_2: turn(semanticJudgeCriteria.turn_2),
    computed_pass: true,
    overall_pass: true,
    passed: true,
  };
}

function semanticResponses(payload) {
  const evidence = (index) => payload.turns[index].candidate_evidence.transcriptEvidenceIds
    .map((id) => `utterance-${String(id).match(/-(\d+)$/)[1]}`);
  return {
    turn_1: { text: payload.turns[0].response, evidence_ids: evidence(0) },
    turn_2: { text: payload.turns[1].response, evidence_ids: evidence(1) },
  };
}

test('native acceptance refuses a locked macOS session before capture', () => {
  assert.throws(
    () => assertMacSessionUnlocked({ platform: 'darwin', probe: () => 'locked\n' }),
    /Unlock the Mac before running native Sidekick acceptance/,
  );
  assert.doesNotThrow(
    () => assertMacSessionUnlocked({ platform: 'darwin', probe: () => 'unlocked\n' }),
  );
  assert.doesNotThrow(
    () => assertMacSessionUnlocked({ platform: 'linux', probe: () => {
      throw new Error('probe must not run off macOS');
    } }),
  );
  assert.equal(parseMacSessionLockProbe('locked'), true);
  assert.equal(parseMacSessionLockProbe('unlocked'), false);
  assert.throws(() => parseMacSessionLockProbe('unknown'), /could not determine/);
});

test('provider attestation canonicalizes an existing path alias', async () => {
  const temporaryRoot = await mkdtemp(path.join(os.tmpdir(), 'minutes-provider-path-'));
  try {
    const providerPath = path.join(temporaryRoot, 'codex-real');
    const aliasPath = path.join(temporaryRoot, 'codex-alias');
    await writeFile(providerPath, 'provider');
    await symlink(providerPath, aliasPath);

    assert.equal(await canonicalExistingPath(aliasPath), providerPath);
  } finally {
    await rm(temporaryRoot, { recursive: true, force: true });
  }
});

function inlineScript(source) {
  return [...source.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/gi)]
    .map((match) => match[1])
    .find((script) => script.includes('cmd_native_sidekick_send'));
}

class FakeElement {
  constructor(id = '', tagName = 'div') {
    this.id = id;
    this.tagName = tagName;
    this.className = '';
    this.textContent = '';
    this.value = '';
    this.hidden = false;
    this.disabled = false;
    this.readOnly = false;
    this.inert = false;
    this.style = {};
    this.scrollTop = 0;
    this.scrollHeight = 620;
    this.dataset = {};
    this.children = [];
    this.listeners = new Map();
    this.isConnected = true;
    const assignedClasses = new Set();
    const syncClassName = () => {
      this.className = [...assignedClasses].join(' ');
    };
    this.classList = {
      contains: (name) => (
        assignedClasses.has(name) || this.className.split(/\s+/).includes(name)
      ),
      toggle: (name, force) => {
        const next = force === undefined ? !this.classList.contains(name) : Boolean(force);
        if (next) assignedClasses.add(name);
        else assignedClasses.delete(name);
        syncClassName();
        return next;
      },
      add: (...names) => {
        names.forEach((name) => assignedClasses.add(name));
        syncClassName();
      },
      remove: (...names) => {
        names.forEach((name) => assignedClasses.delete(name));
        syncClassName();
      },
    };
  }

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) || [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  dispatchEvent(event) {
    for (const listener of this.listeners.get(event.type) || []) listener(event);
  }

  requestSubmit() {
    this.dispatchEvent({ type: 'submit', preventDefault() {} });
  }

  append(...children) {
    children.forEach((child) => { child.parentElement = this; });
    this.children.push(...children);
  }

  replaceChildren(...children) {
    children.forEach((child) => { child.parentElement = this; });
    this.children = children;
  }

  focus() {}

  setAttribute(name, value) {
    this[name] = String(value);
  }

  scrollIntoView(options) {
    this.lastScrollIntoViewOptions = options;
  }

  click() {
    this.dispatchEvent({ type: 'click', target: this, preventDefault() {} });
  }

  contains(candidate) {
    return candidate === this || this.children.some((child) => child.contains?.(candidate));
  }

  closest(selector) {
    if (selector !== '[inert]') return null;
    for (let node = this; node; node = node.parentElement) {
      if (node.inert) return node;
    }
    return null;
  }

  getBoundingClientRect() {
    if (this.id === 'input') {
      return { width: 780, height: 48, top: 650, left: 20, right: 800, bottom: 698 };
    }
    if (this.id === 'send') {
      return { width: 100, height: 48, top: 650, left: 820, right: 920, bottom: 698 };
    }
    if (this.id === 'messages') {
      return { width: 1_024, height: 620, top: 0, left: 0, right: 1_024, bottom: 620 };
    }
    return { width: 320, height: 72, top: 0, left: 0, right: 320, bottom: 72 };
  }

  querySelector(selector) {
    const match = selector.match(/^\[data-snapshot-index="(\d+)"\] \.bubble$/);
    if (!match) return null;
    const item = this.children.find((child) => child.dataset.snapshotIndex === match[1]);
    return item?.children.find((child) => child.className === 'bubble') || null;
  }
}

async function settle() {
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
}

async function sidekickHarness(options = {}) {
  const source = await readFile(sidekickHtml, 'utf8');
  const script = inlineScript(source);
  assert.ok(script, 'the real Sidekick application script must be present');
  const ids = new Set([...source.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]));
  const elements = new Map([...ids].map((id) => [id, new FakeElement(id)]));
  const stream = elements.get('stream');
  const form = elements.get('form');
  const input = elements.get('input');
  const send = elements.get('send');
  stream.parentElement = elements.get('messages');
  send.click = () => form.requestSubmit();
  const eventHandlers = new Map();
  const invocations = [];
  const frames = [];
  const readySnapshot = options.readySnapshot || {
    active: true,
    state: 'ready',
    detail: 'Ready',
    provider: 'Codex',
    privacy: 'Cloud',
    sessionId: 'session-1',
    sessionType: 'Recording',
    screenAvailable: true,
    messages: [],
  };
  const invoke = async (command, args) => {
    invocations.push({ command, args });
    const override = await options.invokeOverride?.(command, args);
    if (override?.handled) return override.value;
    if (command === 'cmd_native_sidekick_status') return options.statusPromise || readySnapshot;
    if (command === 'cmd_native_sidekick_ui_acceptance_ready') {
      return options.acceptanceReady || { active: true, pending: null };
    }
    if (command === 'cmd_native_sidekick_ui_acceptance_claim') return true;
    return undefined;
  };
  const document = {
    body: new FakeElement('body', 'body'),
    visibilityState: 'visible',
    getElementById(id) { return elements.get(id) || null; },
    createElement(tagName) { return new FakeElement('', tagName); },
    elementFromPoint(x, y) {
      if (typeof options.elementFromPoint === 'function') {
        return options.elementFromPoint(x, y, { elements, stream });
      }
      const bubbles = stream.children
        .flatMap((item) => item.children)
        .filter((child) => child.className === 'bubble');
      for (const element of [...bubbles.reverse(), input, send]) {
        const rect = element.getBoundingClientRect();
        if (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom) return element;
      }
      return null;
    },
  };
  const window = {
    document,
    innerWidth: 1_024,
    innerHeight: 768,
    __TAURI__: {
      core: { invoke },
      event: {
        async listen(name, handler) {
          eventHandlers.set(name, handler);
          return () => eventHandlers.delete(name);
        },
      },
      window: { getCurrentWindow: () => ({ close() {} }) },
    },
  };
  window.window = window;
  const context = vm.createContext({
    window,
    document,
    console,
    Number,
    String,
    Array,
    Object,
    RegExp,
    Promise,
    performance,
    getComputedStyle: (element) => ({
      display: 'block',
      visibility: 'visible',
      opacity: '1',
      overflowX: element?.id === 'messages' ? 'hidden' : 'visible',
      overflowY: element?.id === 'messages' ? 'auto' : 'visible',
    }),
    requestAnimationFrame: (callback) => {
      frames.push(callback);
      return frames.length;
    },
  });
  new vm.Script(script, { filename: 'tauri/src/sidekick.html' }).runInContext(context);
  await settle();
  frames.splice(0).forEach((frame) => frame(0));
  await settle();
  return {
    body: document.body,
    elements,
    eventHandlers,
    frames,
    form,
    invocations,
    stream,
  };
}

test('a proactive publication rests as one quiet signal with visible source receipts', async () => {
  const harness = await sidekickHarness({
    readySnapshot: {
      revision: 1,
      active: true,
      state: 'ready',
      detail: 'Current context loaded.',
      provider: 'Codex',
      privacy: 'Cloud',
      sessionId: 'session-quiet',
      meetingTitle: 'Meridian launch review',
      screenAvailable: true,
      contextLoaded: true,
      contextItemCount: 2,
      contextSources: [
        { kind: 'history', label: 'Prior meetings' },
        { kind: 'project', label: 'meridian-app' },
      ],
      messages: [{
        role: 'sidekick',
        kind: 'material_opening',
        text: 'The proposed rollout puts $800K at risk. Ask for the error distribution before choosing a binary ship decision.',
        sources: ['Live meeting', 'Prior meetings', 'Project · meridian-app'],
      }],
    },
  });

  assert.equal(harness.body.dataset.mode, 'signal');
  assert.equal(harness.elements.get('meeting-title').textContent, 'Meridian launch review');
  assert.equal(harness.elements.get('state').textContent, 'Listening');
  assert.equal(
    harness.elements.get('signal-title').textContent,
    'The proposed rollout puts $800K at risk.',
  );
  assert.match(harness.elements.get('signal-body').textContent, /error distribution/i);
  assert.deepEqual(
    harness.elements.get('signal-sources').children.map((child) => child.textContent),
    ['Live meeting', 'Prior meetings', 'Project · meridian-app'],
  );
  assert.equal(harness.elements.get('context-count').textContent, 'Grounded in 4 bounded sources');
  assert.equal(harness.elements.get('project-remove').hidden, false);
});

test('the quiet signal expands into a strategist thread as soon as the user engages', async () => {
  const harness = await sidekickHarness({
    readySnapshot: {
      revision: 2,
      active: true,
      state: 'ready',
      detail: 'Current context loaded.',
      provider: 'Codex',
      privacy: 'Cloud',
      sessionId: 'session-thread',
      meetingTitle: 'Meridian launch review',
      screenAvailable: false,
      contextLoaded: true,
      contextItemCount: 1,
      contextSources: [{ kind: 'person', label: 'Dylan Barth' }],
      messages: [
        {
          role: 'sidekick',
          kind: 'decision',
          text: 'The current plan exposes $800K.',
          sources: ['Live meeting', 'Prior meeting · Dylan Barth'],
        },
        { role: 'user', text: 'Give me the line.', sources: [] },
        {
          role: 'sidekick',
          kind: 'say_this',
          text: '“Before we approve a binary launch, show us the failure distribution.”',
          sources: ['Live meeting', 'Prior meeting · Dylan Barth'],
        },
      ],
    },
  });

  assert.equal(harness.body.dataset.mode, 'thread');
  assert.equal(harness.stream.children.length, 3);
  assert.equal(harness.stream.children[1].children[0].textContent, 'You');
  assert.equal(harness.stream.children[2].children[0].textContent, 'Say This');
  assert.deepEqual(
    harness.stream.children[2].children.at(-1).children.map((child) => child.textContent),
    ['Live meeting', 'Prior meeting · Dylan Barth'],
  );
  assert.equal(harness.elements.get('context-count').textContent, 'Grounded in 2 bounded sources');
});

test('native Sidekick acceptance traverses the real form and waits for two paints', async () => {
  const harness = await sidekickHarness();
  const nonce = '0123456789abcdef0123456789abcdef';
  const message = 'What is the decision-changing risk?';
  await harness.eventHandlers.get('sidekick:acceptance-submit')({
    payload: {
      nonce,
      turnId: 'vendor_strategy',
      message,
      baselineMessageCount: 0,
    },
  });
  await settle();

  assert.ok(
    harness.invocations.some(({ command, args }) => (
      command === 'cmd_native_sidekick_send' && args.message === message
    )),
    'the diagnostic must submit through the production form handler',
  );
  assert.equal(
    harness.invocations.some(({ command }) => command === 'cmd_native_sidekick_ui_acceptance_painted'),
    false,
  );

  harness.eventHandlers.get('sidekick:state')({
    payload: {
      active: true,
      state: 'ready',
      messages: [
        { role: 'sidekick', text: 'A separately published proactive observation.' },
        { role: 'user', text: message, acceptanceTurnId: 'vendor_strategy' },
        { role: 'sidekick', text: 'The downside is $800K; gate rollout by the error distribution.', acceptanceTurnId: 'vendor_strategy' },
      ],
    },
  });
  const firstFrameBatch = harness.frames.splice(0);
  firstFrameBatch.forEach((frame) => frame(1));
  await settle();
  assert.equal(
    harness.invocations.some(({ command }) => command === 'cmd_native_sidekick_ui_acceptance_painted'),
    false,
    'one animation frame is not a visible-paint acknowledgement',
  );

  const secondFrameBatch = harness.frames.splice(0);
  secondFrameBatch.forEach((frame) => frame(2));
  await settle();
  const paint = harness.invocations.find(
    ({ command }) => command === 'cmd_native_sidekick_ui_acceptance_painted',
  );
  assert.equal(paint.args.animationFrames, 2);
  assert.equal(paint.args.userText, message);
  assert.equal(paint.args.domText, 'The downside is $800K; gate rollout by the error distribution.');
  assert.ok(paint.args.width >= 24 && paint.args.height >= 24);
  assert.ok(harness.invocations.some(
    ({ command, args }) => command === 'cmd_native_sidekick_ui_acceptance_turn_settled'
      && args.turnId === 'vendor_strategy',
  ));
});

test('native Sidekick acceptance fails closed instead of typing into a readonly input', async () => {
  const harness = await sidekickHarness();
  const nonce = '0123456789abcdef0123456789abcdef';
  const message = 'Do not submit this through a fake writable lane.';
  harness.elements.get('input').readOnly = true;

  await harness.eventHandlers.get('sidekick:acceptance-submit')({
    payload: {
      nonce,
      turnId: 'readonly_input',
      message,
      baselineMessageCount: 0,
    },
  });
  await settle();

  assert.equal(
    harness.invocations.some(({ command }) => command === 'cmd_native_sidekick_send'),
    false,
  );
  assert.ok(harness.invocations.some(({ command, args }) => (
    command === 'cmd_native_sidekick_ui_acceptance_failed'
      && args.turnId === 'readonly_input'
      && /writable/i.test(args.error)
  )));
});

test('native Sidekick acceptance will not acknowledge a zero-size response bubble', async () => {
  const harness = await sidekickHarness();
  const nonce = 'fedcba9876543210fedcba9876543210';
  const message = 'What should I do?';
  await harness.eventHandlers.get('sidekick:acceptance-submit')({
    payload: { nonce, turnId: 'vendor_strategy', message, baselineMessageCount: 0 },
  });
  await settle();
  harness.eventHandlers.get('sidekick:state')({
    payload: {
      active: true,
      state: 'ready',
      messages: [
        { role: 'user', text: message, acceptanceTurnId: 'vendor_strategy' },
        { role: 'sidekick', text: 'A response that was never laid out.', acceptanceTurnId: 'vendor_strategy' },
      ],
    },
  });
  const bubble = harness.stream.children[1].children.find((child) => child.className === 'bubble');
  bubble.getBoundingClientRect = () => ({
    width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0,
  });
  harness.frames.splice(0).forEach((frame) => frame(1));
  harness.frames.splice(0).forEach((frame) => frame(2));
  await settle();
  assert.equal(
    harness.invocations.some(({ command }) => command === 'cmd_native_sidekick_ui_acceptance_painted'),
    false,
  );
});

test('native Sidekick acceptance rejects a bubble clipped out of the message scroller', async () => {
  const harness = await sidekickHarness();
  const nonce = '00112233445566778899aabbccddeeff';
  const message = 'Show me the clipped answer';
  await harness.eventHandlers.get('sidekick:acceptance-submit')({
    payload: { nonce, turnId: 'clipped_turn', message, baselineMessageCount: 0 },
  });
  harness.elements.get('messages').getBoundingClientRect = () => ({
    width: 1_024, height: 30, top: 0, left: 0, right: 1_024, bottom: 30,
  });
  harness.eventHandlers.get('sidekick:state')({
    payload: {
      revision: 1,
      active: true,
      state: 'ready',
      messages: [
        { role: 'user', text: message, acceptanceTurnId: 'clipped_turn' },
        { role: 'sidekick', text: 'Below the clipping boundary.', acceptanceTurnId: 'clipped_turn' },
      ],
    },
  });
  const bubble = harness.stream.children[1].children.find((child) => child.className === 'bubble');
  bubble.getBoundingClientRect = () => ({
    width: 320, height: 32, top: 40, left: 0, right: 320, bottom: 72,
  });
  harness.frames.splice(0).forEach((frame) => frame(1));
  harness.frames.splice(0).forEach((frame) => frame(2));
  await settle();
  assert.equal(
    harness.invocations.some(({ command }) => command === 'cmd_native_sidekick_ui_acceptance_painted'),
    false,
  );
});

test('native Sidekick acceptance rejects a fractional response sliver', async () => {
  const harness = await sidekickHarness();
  const nonce = '102132435465768798a9bacbdcedfe0f';
  const message = 'Show me the almost hidden answer';
  await harness.eventHandlers.get('sidekick:acceptance-submit')({
    payload: { nonce, turnId: 'fractional_turn', message, baselineMessageCount: 0 },
  });
  harness.elements.get('messages').getBoundingClientRect = () => ({
    width: 1_024, height: 40.8, top: 0, left: 0, right: 1_024, bottom: 40.8,
  });
  harness.eventHandlers.get('sidekick:state')({
    payload: {
      revision: 1,
      active: true,
      state: 'ready',
      messages: [
        { role: 'user', text: message, acceptanceTurnId: 'fractional_turn' },
        { role: 'sidekick', text: 'Only a fractional sliver is visible.', acceptanceTurnId: 'fractional_turn' },
      ],
    },
  });
  const bubble = harness.stream.children[1].children.find((child) => child.className === 'bubble');
  bubble.getBoundingClientRect = () => ({
    width: 320, height: 32, top: 40, left: 0, right: 320, bottom: 72,
  });
  harness.frames.splice(0).forEach((frame) => frame(1));
  harness.frames.splice(0).forEach((frame) => frame(2));
  await settle();
  assert.equal(
    harness.invocations.some(({ command }) => command === 'cmd_native_sidekick_ui_acceptance_painted'),
    false,
  );
  assert.equal(harness.elements.get('messages').style.scrollBehavior, 'auto');
  assert.equal(bubble.lastScrollIntoViewOptions.behavior, 'instant');
  assert.equal(bubble.lastScrollIntoViewOptions.block, 'nearest');
  assert.equal(bubble.lastScrollIntoViewOptions.inline, 'nearest');
});

test('event plus replay of the same challenge submits exactly once', async () => {
  let releaseClaim;
  const claimBarrier = new Promise((resolve) => { releaseClaim = resolve; });
  const harness = await sidekickHarness({
    invokeOverride: async (command) => (
      command === 'cmd_native_sidekick_ui_acceptance_claim'
        ? { handled: true, value: claimBarrier }
        : null
    ),
  });
  const payload = {
    nonce: '11223344556677889900aabbccddeeff',
    turnId: 'deduped_turn',
    message: 'Only submit me once',
    baselineMessageCount: 0,
  };
  const first = harness.eventHandlers.get('sidekick:acceptance-submit')({ payload });
  const replay = harness.eventHandlers.get('sidekick:acceptance-submit')({ payload });
  releaseClaim(true);
  await Promise.all([first, replay]);
  await settle();
  assert.equal(
    harness.invocations.filter(({ command }) => command === 'cmd_native_sidekick_ui_acceptance_claim').length,
    1,
  );
  assert.equal(
    harness.invocations.filter(({ command }) => command === 'cmd_native_sidekick_send').length,
    1,
  );
});

test('a rejected production send fails the acceptance turn immediately', async () => {
  const harness = await sidekickHarness({
    invokeOverride: async (command) => {
      if (command === 'cmd_native_sidekick_send') throw new Error('delivery rejected');
      return null;
    },
  });
  await harness.eventHandlers.get('sidekick:acceptance-submit')({
    payload: {
      nonce: '22334455667788990011aabbccddeeff',
      turnId: 'rejected_turn',
      message: 'Fail me now',
      baselineMessageCount: 0,
    },
  });
  await settle();
  assert.ok(harness.invocations.some(
    ({ command, args }) => command === 'cmd_native_sidekick_ui_acceptance_failed'
      && args.turnId === 'rejected_turn'
      && args.error.includes('delivery rejected'),
  ));
});

test('a delayed paint acknowledgement cannot drop the next acceptance turn', async () => {
  let releasePaint;
  const paintBarrier = new Promise((resolve) => { releasePaint = resolve; });
  const harness = await sidekickHarness({
    invokeOverride: async (command) => (
      command === 'cmd_native_sidekick_ui_acceptance_painted'
        ? { handled: true, value: paintBarrier }
        : null
    ),
  });
  const nonce = '1234567890abcdef1234567890abcdef';
  const first = 'First question';
  const second = 'Second question';
  await harness.eventHandlers.get('sidekick:acceptance-submit')({
    payload: { nonce, turnId: 'turn_one', message: first, baselineMessageCount: 0 },
  });
  harness.eventHandlers.get('sidekick:state')({
    payload: {
      revision: 1,
      active: true,
      state: 'ready',
      messages: [
        { role: 'user', text: first, acceptanceTurnId: 'turn_one' },
        { role: 'sidekick', text: 'First answer', acceptanceTurnId: 'turn_one' },
      ],
    },
  });
  harness.frames.splice(0).forEach((frame) => frame(1));
  harness.frames.splice(0).forEach((frame) => frame(2));
  await settle();

  await harness.eventHandlers.get('sidekick:acceptance-submit')({
    payload: { nonce, turnId: 'turn_two', message: second, baselineMessageCount: 2 },
  });
  assert.equal(
    harness.invocations.filter(({ command }) => command === 'cmd_native_sidekick_send').length,
    1,
    'turn two must remain queued until turn one settles',
  );

  releasePaint();
  await settle();
  assert.equal(
    harness.invocations.filter(({ command }) => command === 'cmd_native_sidekick_send').length,
    2,
    'turn two must submit after the first paint IPC has fully settled',
  );
});

test('an older status response cannot overwrite a newer event snapshot', async () => {
  let resolveStatus;
  const statusPromise = new Promise((resolve) => { resolveStatus = resolve; });
  const harness = await sidekickHarness({ statusPromise });
  harness.eventHandlers.get('sidekick:state')({
    payload: {
      revision: 8,
      active: true,
      state: 'ready',
      sessionId: 'new-session',
      messages: [{ role: 'sidekick', text: 'Newest authoritative answer.' }],
    },
  });
  resolveStatus({
    revision: 7,
    active: true,
    state: 'arming',
    sessionId: 'old-session',
    messages: [{ role: 'sidekick', text: 'Stale answer.' }],
  });
  await settle();
  assert.equal(harness.stream.children.length, 1);
  assert.equal(harness.stream.children[0].children[1].textContent, 'Newest authoritative answer.');
  assert.equal(harness.elements.get('state').textContent, 'Listening');
});

test('a reloaded Sidekick window recovers an already-submitted acceptance turn without resending it', async () => {
  const nonce = 'abcdef0123456789abcdef0123456789';
  const message = 'Recovered question';
  const harness = await sidekickHarness({
    readySnapshot: {
      revision: 4,
      active: true,
      state: 'ready',
      sessionId: 'session-1',
      messages: [
        { role: 'user', text: message, acceptanceTurnId: 'recovered_turn' },
        { role: 'sidekick', text: 'Recovered answer', acceptanceTurnId: 'recovered_turn' },
      ],
    },
    acceptanceReady: {
      active: true,
      pending: {
        nonce,
        turnId: 'recovered_turn',
        message,
        baselineMessageCount: 0,
        shouldSubmit: false,
      },
    },
  });
  harness.frames.splice(0).forEach((frame) => frame(2));
  await settle();
  assert.equal(
    harness.invocations.some(({ command }) => command === 'cmd_native_sidekick_send'),
    false,
  );
  assert.ok(harness.invocations.some(
    ({ command }) => command === 'cmd_native_sidekick_ui_acceptance_painted',
  ));
});

test('a reload before submission submits the authoritative pending turn once', async () => {
  const nonce = '33445566778899001122aabbccddeeff';
  const harness = await sidekickHarness({
    acceptanceReady: {
      active: true,
      pending: {
        nonce,
        turnId: 'before_submit',
        message: 'Submit after reload',
        baselineMessageCount: 0,
        shouldSubmit: true,
      },
    },
  });
  assert.equal(
    harness.invocations.filter(({ command }) => command === 'cmd_native_sidekick_send').length,
    1,
  );
});

test('a reload during provider work waits for the exact response without resending', async () => {
  const nonce = '44556677889900112233aabbccddeeff';
  const message = 'Already in flight';
  const harness = await sidekickHarness({
    readySnapshot: {
      revision: 3,
      active: true,
      state: 'thinking',
      sessionId: 'session-1',
      messages: [{ role: 'user', text: message, acceptanceTurnId: 'mid_provider' }],
    },
    acceptanceReady: {
      active: true,
      pending: {
        nonce,
        turnId: 'mid_provider',
        message,
        baselineMessageCount: 0,
        shouldSubmit: false,
      },
    },
  });
  assert.equal(
    harness.invocations.some(({ command }) => command === 'cmd_native_sidekick_send'),
    false,
  );
  harness.eventHandlers.get('sidekick:state')({
    payload: {
      revision: 4,
      active: true,
      state: 'ready',
      sessionId: 'session-1',
      messages: [
        { role: 'user', text: message, acceptanceTurnId: 'mid_provider' },
        { role: 'sidekick', text: 'Finished after reload.', acceptanceTurnId: 'mid_provider' },
      ],
    },
  });
  harness.frames.splice(0).forEach((frame) => frame(1));
  harness.frames.splice(0).forEach((frame) => frame(2));
  await settle();
  assert.ok(harness.invocations.some(
    ({ command }) => command === 'cmd_native_sidekick_ui_acceptance_painted',
  ));
});

function passingProductPayload() {
  const turn1 = 'That 90% is a liability number, not a quality score: 40,000 x 10% x $200 is $800K per month in contractual credits. Gate full automation to high-confidence tickets and route the uncertain remainder to a human. What is the confidence distribution, and what volume clears a defensible threshold?';
  const turn2 = "For Meridian procurement, require every wrong automated resolution to make the vendor owe Meridian a $200 credit, a written confidence-threshold SLA, auditable error reporting, and Meridian's unilateral right to revert affected work to human handling without vendor permission.";
  const domLayout = (turnId, response) => ({
    turnId,
    responseSha256: createHash('sha256').update(response).digest('hex'),
    typedToPaintMs: 1_900,
    animationFrames: 2,
    width: 320,
    height: 72,
    windowVisible: true,
    onScreen: true,
  });
  const transcriptEvidenceIds = Array.from({ length: 6 }, (_, index) => (
    `acceptance-transcript-0123456789abcdef-${index + 1}`
  ));
  const visualEvidencePrefix = 'acceptance-screen-0123456789abcdef';
  const evidenceReceipt = (turnId) => ({
    turnId: `foreground-${turnId === 'vendor_strategy' ? 1 : 2}`,
    captureSessionId: 'recording-session-1',
    transcriptEvidenceIds: turnId === 'vendor_strategy'
      ? transcriptEvidenceIds.slice(0, 4)
      : transcriptEvidenceIds,
    visualEvidenceIds: [`${visualEvidencePrefix}-${turnId}`],
  });
  const adapterReceipt = (turnId) => ({
    transcriptAdapter: 'live_transcript_jsonl_delta',
    transcriptCursor: turnId === 'vendor_strategy' ? 4 : 6,
    transcriptSha256: turnId === 'vendor_strategy' ? '6'.repeat(64) : '7'.repeat(64),
    transcriptNewItems: turnId === 'vendor_strategy' ? 0 : 2,
    screenAdapter: 'context_store_exact_session',
    screenCaptureSha256: '8'.repeat(64),
    providerImageEvidenceId: `${visualEvidencePrefix}-${turnId}`,
    providerImagePath: `/private/provider-${turnId}.png`,
    providerImageSha256: 'a'.repeat(64),
    providerImageTransport: 'inline_data_url',
    providerImageDispatchedSha256: 'a'.repeat(64),
    captureSessionId: 'recording-session-1',
    perTurnRefreshCompleted: true,
  });
  return {
    mode: 'diagnose-native-sidekick-ui',
    passed_product_path: true,
    bundle_identifier: 'com.useminutes.desktop.dev',
    build_commit: 'c'.repeat(40),
    fixture_id: 'synthetic-meridian-ship-decision',
    fixture_sha256: 'f'.repeat(64),
    context_session_id: 'recording-session-1',
    context_session_type: 'recording',
    audio: {
      intent: 'room',
      growing: true,
      size_before: 8_044,
      size_after: 16_044,
      samples_inspected: 4_000,
      peak_amplitude: 42,
      nonzero_samples: 3_500,
      rms_amplitude: 12.5,
      nonzero_ratio: 0.875,
      scope: 'microphone_signal_smoke_only',
      speech_or_asr_claimed: false,
    },
    startup_latency: {
      recording_ready_ms: 4_000,
      screen_ready_ms: 8_000,
      sidekick_ready_ms: 12_000,
    },
    transcript: {
      source: 'acceptance_pinned_fixture',
      adapter: 'verified_bytes_live_transcript_jsonl_delta',
      fixture_jsonl_sha256: '7'.repeat(64),
      initial_jsonl_sha256: '6'.repeat(64),
      final_jsonl_sha256: '7'.repeat(64),
      items: 6,
      initial_items: 4,
      delta_items: 2,
      delta_turn_id: 'procurement_role_flip',
      approved_evidence_ids: transcriptEvidenceIds,
      ambient_live_transcript_allowed: false,
    },
    screen: {
      permission_capture_bytes: 20_000,
      permission_capture_sha256: '8'.repeat(64),
      provider_marker_sha256: 'a'.repeat(64),
      provider_marker_evidence_prefix: visualEvidencePrefix,
      marker_nonce_verified_from_pixels: true,
      provider_marker_is_generated_nonce_only: true,
      adapter: 'context_store_exact_session',
      capture_session_id: 'recording-session-1',
    },
    sidekick: {
      ready_session_id: 'recording-session-1',
      screen_available: true,
      launch_surface: 'main_sidekick_button_cloud_consent',
      main_launch_completed: true,
      interactable_targets: {
        main_sidekick_button: true,
        cloud_consent_confirm: true,
        'vendor_strategy:sidekick_input': true,
        'vendor_strategy:sidekick_send': true,
        'procurement_role_flip:sidekick_input': true,
        'procurement_role_flip:sidekick_send': true,
      },
      reasoning_sessions_started: 1,
      reasoning_session_correlation: 'b'.repeat(64),
      verifier_sessions_started: 2,
      provider_executable_path: '/opt/homebrew/bin/codex',
      provider_executable_sha256: '9'.repeat(64),
      provider_version: 'codex-cli 1.0.0',
      provider_executable_attestation_scope: 'trusted_host_path_pre_post',
      provider_requested_contract: {
        provider: 'codex-app-server',
        model: 'gpt-5.6-terra',
        privacy: 'cloud',
        persistent: true,
        steerable: true,
        streaming: true,
        image_input: true,
      },
      verifier_requested_contract: {
        provider: 'codex-app-server',
        model: 'gpt-5.6-terra',
        privacy: 'cloud',
        persistent: true,
        steerable: true,
        streaming: true,
        image_input: true,
      },
      provider_capabilities_exercised: {
        persistent_sequential_turns: true,
        streaming_delta_observed: true,
        steering: false,
        interruption: false,
      },
    },
    turns: [
      {
        id: 'vendor_strategy',
        prompt: "What's the real risk here, and the single best question I should ask before we decide?",
        response: turn1,
        dom_layout: domLayout('vendor_strategy', turn1),
        evidence_receipt: evidenceReceipt('vendor_strategy'),
        adapter_receipt: adapterReceipt('vendor_strategy'),
        candidate_evidence: {
          transcriptEvidenceIds: [
            transcriptEvidenceIds[0],
            transcriptEvidenceIds[2],
            transcriptEvidenceIds[3],
          ],
          visualEvidenceIds: [],
          claimsVisualObservation: false,
          firstTokenMs: 500,
          candidateSha256: '1'.repeat(64),
          candidateDigestVerified: true,
          verificationVerdict: { decision: 'allow', reason_code: 'supported' },
          verifierSessionCorrelation: '2'.repeat(64),
        },
      },
      {
        id: 'procurement_role_flip',
        prompt: "Now pretend I'm Meridian's procurement lead — what should I push the vendor for?",
        response: turn2,
        dom_layout: domLayout('procurement_role_flip', turn2),
        evidence_receipt: evidenceReceipt('procurement_role_flip'),
        adapter_receipt: adapterReceipt('procurement_role_flip'),
        candidate_evidence: {
          transcriptEvidenceIds: [transcriptEvidenceIds[2], transcriptEvidenceIds[5]],
          visualEvidenceIds: [],
          claimsVisualObservation: false,
          firstTokenMs: 420,
          candidateSha256: '3'.repeat(64),
          candidateDigestVerified: true,
          verificationVerdict: { decision: 'allow', reason_code: 'supported' },
          verifierSessionCorrelation: '4'.repeat(64),
        },
      },
    ],
    acceptance_scope: {
      kind: 'bounded_native_ui_provider_integration',
      host_threat_model: 'trusted_single_user_no_concurrent_hostile_same_uid_process',
      excludes: [
        'live_speech_recognition',
        'two_speaker_diarization',
        'semantic_desktop_screen_understanding',
        'compositor_or_occlusion_proof',
        'provider_steering_and_interruption',
        'normal_installed_app_cold_start',
        'hostile_same_user_filesystem_or_process_tampering',
        'provider_live_process_code_identity_attestation',
        'escaped_session_descendant_detection',
      ],
    },
    teardown: {
      sidekick_stopped: true,
      sidekick_control_cleared: true,
      recording_stop_requested: true,
      recording_stopped: true,
      recording_pid_removed: true,
      recording_metadata_cleared: true,
      disposable_wav_removed: true,
      processing_idle: true,
      context_discarded_and_screen_stopped: true,
      sensitive_paths_removed: true,
      cleanup_complete: true,
    },
  };
}

function passingRuntime(payload = passingProductPayload()) {
  const responses = semanticResponses(payload);
  const qualityProviderExecutable = {
    path: '/opt/homebrew/bin/codex',
    sha256: '9'.repeat(64),
    version: 'codex-cli 1.0.0',
  };
  return {
    exit_code: 0,
    executable_sha256: 'd'.repeat(64),
    expected_executable_sha256: 'd'.repeat(64),
    bundle_sha256: 'e'.repeat(64),
    expected_bundle_sha256: 'e'.repeat(64),
    expected_build_commit: 'c'.repeat(40),
    quality_source_binding: qualitySourceBinding,
    quality_provider_executable: qualityProviderExecutable,
    expected_provider_path: '/opt/homebrew/bin/codex',
    expected_provider_sha256: '9'.repeat(64),
    expected_provider_version: 'codex-cli 1.0.0',
    wall_ms: 25_000,
    launch_method: 'macos_launch_services',
    launch_services_exit_code: 0,
    app_exit_code: 0,
    app_exit_receipt_verified: true,
    temporary_root_removed: true,
    process_group_empty: true,
    provider_process_cleanup_scope: 'app_teardown_launchservices_wait_exact_executable_scan',
    app_processes_remaining: [],
    provider_processes_remaining: [],
    forced_process_signals: [],
    provider_copy_is_private: true,
    provider_copy_post_sha256: '9'.repeat(64),
    hybrid_quality_gate: {
      schema_version: 1,
      passed: true,
      producer_attested: true,
      fixture_id: 'synthetic-meridian-ship-decision',
      run_count: 3,
      requested_model: 'gpt-5.6-terra',
      requested_effort: 'none',
      requested_verifier_model: 'gpt-5.6-terra',
      requested_verifier_effort: 'none',
      mechanical_quality_passed: true,
      semantic_quality_passed: true,
      semantic_calibration_passed: true,
      verifier_calibration_passed: true,
      model_matched: true,
      latency_passed: true,
      first_token_p95_ms: 2_000,
      total_median_ms: 4_500,
      service_target_pass_count: 6,
      total_sample_count: 6,
      total_p95_ms: 5_000,
      max_first_token_p95_ms: 4_000,
      max_total_median_ms: 6_000,
      service_target_total_ms: 8_000,
      min_service_target_pass_count: 5,
      max_total_p95_ms: 10_000,
      artifact_sha256: 'f'.repeat(64),
      producer_artifact_sha256: 'f'.repeat(64),
      source_binding: qualitySourceBinding,
      provider_executable: qualityProviderExecutable,
    },
    exact_semantic_quality_gate: {
      schema_version: 1,
      provider: 'codex-app-server',
      model: 'gpt-5.6-terra',
      response_sha256: sidekickSemanticResponseSha256(responses),
      source_binding: qualitySourceBinding,
      provider_executable: qualityProviderExecutable,
      verdict: semanticPassVerdict(),
    },
  };
}

test('native UI acceptance delegates valid natural paraphrases to the calibrated hybrid gate', async () => {
  const payload = passingProductPayload();
  const fixtureBytes = await readFile(
    new URL('../../tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json', import.meta.url),
  );
  payload.fixture_sha256 = createHash('sha256').update(fixtureBytes).digest('hex');
  const texts = [
    'At 40,000 monthly tickets and a 10% miss rate, full automation produces 4,000 bad outcomes and $800,000/month in contractual exposure. The 90% headline cannot decide launch. Restrict automation to high-confidence bands; send lower bands to people. Which confidence-band error distribution defines the safe cutoff?',
    'Advise Meridian procurement: each incorrect automated disposition makes the supplier owe Meridian $200. Put the confidence cutoff and observed error ceiling in the SLA; expose underlying records for each case to audit; Meridian alone may immediately return affected work to people, with no supplier approval or delay.',
  ];
  payload.turns.forEach((turn, index) => {
    turn.response = texts[index];
    turn.dom_layout.responseSha256 = createHash('sha256').update(texts[index]).digest('hex');
  });

  const report = evaluateNativeSidekickUiAcceptance(payload, passingRuntime(payload));
  assert.equal(
    report.passed,
    true,
    JSON.stringify([
      ...report.source_checks,
      ...report.quality_checks,
      ...report.paint_checks,
    ].filter((item) => !item.passed)),
  );
  assert.equal(report.semantic_diagnostics.some((item) => !item.passed), true);
});

test('a preexisting hybrid artifact without a live producer witness cannot pass UI acceptance', () => {
  const runtime = passingRuntime();
  runtime.hybrid_quality_gate.producer_attested = false;
  const report = evaluateNativeSidekickUiAcceptance(passingProductPayload(), runtime);
  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === 'calibrated_hybrid_quality_artifact').passed,
    false,
  );
});

test('an old good hybrid artifact cannot bless bad current UI responses', async () => {
  const payload = passingProductPayload();
  const fixtureBytes = await readFile(
    new URL('../../tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json', import.meta.url),
  );
  payload.fixture_sha256 = createHash('sha256').update(fixtureBytes).digest('hex');
  const texts = ['The exposure is $800K/month. Automate everything.', 'Hello.'];
  payload.turns.forEach((turn, index) => {
    turn.response = texts[index];
    turn.dom_layout.responseSha256 = createHash('sha256').update(texts[index]).digest('hex');
  });

  const report = evaluateNativeSidekickUiAcceptance(payload, passingRuntime());
  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) =>
      item.name === 'exact_ui_responses_pass_semantic_judge').passed,
    false,
  );
});

test('native UI acceptance launches the signed app through LaunchServices with a parent lease', () => {
  const args = nativeSidekickLaunchServicesArgs({
    app: '/Users/tester/Applications/Minutes Dev.app',
    appStdoutPath: '/tmp/acceptance/app.stdout',
    appStderrPath: '/tmp/acceptance/app.stderr',
    parentLeasePath: '/tmp/acceptance/parent-lease.fifo',
    isolatedHome: '/tmp/acceptance/home',
    isolatedTmp: '/tmp/acceptance/tmp',
    codeHome: '/Users/tester/.codex',
    providerDirectory: '/tmp/acceptance/provider',
    inheritedPath: '/opt/homebrew/bin:/usr/bin:/bin',
    nonce: 'a'.repeat(64),
    realHome: '/Users/tester',
  });

  assert.deepEqual(args.slice(0, 4), ['-n', '-W', '-i', '/tmp/acceptance/parent-lease.fifo']);
  assert.ok(args.includes('HOME=/tmp/acceptance/home'));
  assert.ok(args.includes('TMPDIR=/tmp/acceptance/tmp'));
  assert.ok(args.includes('PATH=/tmp/acceptance/provider:/opt/homebrew/bin:/usr/bin:/bin'));
  const appIndex = args.indexOf('/Users/tester/Applications/Minutes Dev.app');
  const argsIndex = args.indexOf('--args');
  assert.ok(appIndex > 0 && argsIndex === appIndex + 1, 'the bundle path must precede app argv');
  const parentFdIndex = args.indexOf('--acceptance-parent-fd');
  assert.equal(args[parentFdIndex + 1], '0', 'LaunchServices maps the inherited lease to app stdin');
  assert.equal(nativeSidekickTemporaryParent('darwin', '/var/folders/very/long/path'), '/tmp');
  assert.equal(nativeSidekickTemporaryParent('linux', '/var/tmp'), '/var/tmp');
});

test('native UI acceptance surfaces bounded launch logs before secure cleanup', () => {
  const failure = nativeSidekickFailureWithLogs(new Error('report missing'), {
    launchServicesStderr: 'open failed',
    launchServicesStdout: '',
    appStderr: 'native diagnostic rejected',
    appStdout: 'x'.repeat(2_000),
  });

  assert.match(failure.message, /report missing/);
  assert.match(failure.message, /LaunchServices stderr:\nopen failed/);
  assert.match(failure.message, /Minutes stderr:\nnative diagnostic rejected/);
  assert.equal(failure.message.includes('x'.repeat(1_001)), false);
  assert.equal(failure.cause.message, 'report missing');
});

test('native UI acceptance bounds stream collection and file reads before formatting', async () => {
  const first = appendBoundedOutput(Buffer.alloc(0), Buffer.from('123456'), 8);
  const second = appendBoundedOutput(first.bytes, Buffer.from('7890'), 8);
  assert.equal(second.bytes.toString('utf8'), '12345678');
  assert.equal(second.overflowed, true);

  const directory = await mkdtemp(path.join(os.tmpdir(), 'minutes-bounded-output-'));
  const file = path.join(directory, 'app.stderr');
  try {
    await writeFile(file, 'x'.repeat(2_000));
    const output = await readBoundedOutputFile(file, 1_000);
    assert.equal(Buffer.byteLength(output.text), 1_000);
    assert.equal(output.overflowed, true);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test('concurrent parent lease close requests join one in-flight operation', async () => {
  let calls = 0;
  let release;
  const close = singleFlightAsync(() => {
    calls += 1;
    return new Promise((resolve) => { release = resolve; });
  });

  const first = close();
  const second = close();
  assert.equal(first, second);
  await Promise.resolve();
  assert.equal(calls, 1);
  release();
  await Promise.all([first, second, close()]);
  assert.equal(calls, 1);
});

test('exact process identity survives an unrelated stale executable pathname', () => {
  const identities = parseLsofTextIdentities([
    'p20799',
    'ftxt',
    'D0x1000012',
    'i2354314229',
    'n/vanished/ChatGPT.app/Contents/Resources/codex',
    'ftxt',
    'D0x1000012',
    'i1152921500312573174',
    'n/usr/lib/dyld',
    '',
  ].join('\n'));

  assert.deepEqual(identities, [
    {
      descriptor: 'txt',
      device: BigInt('0x1000012').toString(),
      inode: '2354314229',
      path: '/vanished/ChatGPT.app/Contents/Resources/codex',
    },
    {
      descriptor: 'txt',
      device: BigInt('0x1000012').toString(),
      inode: '1152921500312573174',
      path: '/usr/lib/dyld',
    },
  ]);
});

test('exact process identity fails closed when the primary lsof record is incomplete', () => {
  assert.throws(() => parseLsofTextIdentities([
    'p4242',
    'ftxt',
    'n/tmp/private-provider/codex',
    'ftxt',
    'D0x1000012',
    'i1152921500312573174',
    'n/usr/lib/dyld',
    '',
  ].join('\n')), /missing device or inode identity/);
});

test('LaunchServices cleanup retires only newly launched exact processes', async () => {
  let live = [7, 42];
  const signals = [];
  const cleanup = await terminateNewExactProcesses({
    executable: '/Applications/Minutes Dev.app/Contents/MacOS/minutes-app',
    baselinePids: [7],
    scan: () => [...live],
    signal(pid, name) {
      signals.push({ pid, name });
      if (pid === 42 && name === 'SIGTERM') live = [7];
    },
    pause: async () => {},
  });

  assert.deepEqual(cleanup.remaining, []);
  assert.deepEqual(cleanup.signals, [{ pid: 42, signal: 'SIGTERM' }]);
  assert.deepEqual(signals, [{ pid: 42, name: 'SIGTERM' }]);
});

test('LaunchServices cleanup escalates to SIGKILL and fails closed on scan uncertainty', async () => {
  let live = [91];
  const signals = [];
  const cleanup = await terminateNewExactProcesses({
    executable: '/tmp/provider/codex',
    scan: () => [...live],
    signal(pid, name) {
      signals.push({ pid, name });
      if (name === 'SIGKILL') live = [];
    },
    pause: async () => {},
  });
  assert.deepEqual(cleanup.remaining, []);
  assert.deepEqual(cleanup.signals, [
    { pid: 91, signal: 'SIGTERM' },
    { pid: 91, signal: 'SIGKILL' },
  ]);
  assert.deepEqual(signals, [
    { pid: 91, name: 'SIGTERM' },
    { pid: 91, name: 'SIGKILL' },
  ]);

  await assert.rejects(
    terminateNewExactProcesses({
      executable: '/tmp/provider/codex',
      scan: () => { throw new Error('lsof denied'); },
      pause: async () => {},
    }),
    /lsof denied/,
  );
});

test('LaunchServices cleanup attempts the provider lane even when the app lane is uncertain', async () => {
  const calls = [];
  const cleanup = await cleanupNativeSidekickProcessLanes({
    appExecutable: '/Applications/Minutes Dev.app/Contents/MacOS/minutes-app',
    providerExecutable: '/tmp/provider/codex',
    terminate: async ({ executable }) => {
      calls.push(executable);
      if (executable.endsWith('/minutes-app')) throw new Error('app lsof denied');
      return { remaining: [], signals: [] };
    },
  });

  assert.deepEqual(calls, [
    '/Applications/Minutes Dev.app/Contents/MacOS/minutes-app',
    '/tmp/provider/codex',
  ]);
  assert.equal(cleanup.errors.length, 1);
  assert.match(cleanup.errors[0].message, /app lsof denied/);
  assert.equal(cleanup.retainTemporaryRoot, true);
  assert.deepEqual(cleanup.provider, { remaining: [], signals: [] });
});

test('installed UI acceptance requires product path, quality, and paint latency together', async () => {
  const fixture = JSON.parse(await readFile(
    new URL('../../tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json', import.meta.url),
    'utf8',
  ));
  const fixtureBytes = await readFile(
    new URL('../../tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json', import.meta.url),
  );
  const payload = passingProductPayload();
  payload.fixture_id = fixture.id;
  payload.fixture_sha256 = createHash('sha256').update(fixtureBytes).digest('hex');

  const report = evaluateNativeSidekickUiAcceptance(payload, passingRuntime());

  assert.equal(report.passed, true);
  assert.ok(report.quality_score.numerator >= 14);
});

test('installed UI acceptance permits one fail-closed verifier recovery', async () => {
  const fixtureBytes = await readFile(
    new URL('../../tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json', import.meta.url),
  );
  const payload = passingProductPayload();
  payload.fixture_sha256 = createHash('sha256').update(fixtureBytes).digest('hex');
  payload.sidekick.verifier_sessions_started = 3;

  const report = evaluateNativeSidekickUiAcceptance(payload, passingRuntime(payload));

  assert.equal(report.passed, true);
});

test('an internally ready answer cannot impersonate a visibly painted answer', async () => {
  const fixtureBytes = await readFile(
    new URL('../../tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json', import.meta.url),
  );
  const payload = passingProductPayload();
  payload.fixture_sha256 = createHash('sha256').update(fixtureBytes).digest('hex');
  payload.turns[0].dom_layout.typedToPaintMs = 10_001;

  const report = evaluateNativeSidekickUiAcceptance(payload, passingRuntime());

  assert.equal(report.passed, false);
  assert.equal(report.paint_checks[0].passed, false);
});

test('the evaluator rejects every reviewed false-green mutation', async () => {
  const fixtureBytes = await readFile(
    new URL('../../tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json', import.meta.url),
  );
  const mutations = [
    ['ambient transcript ID', (payload) => payload.transcript.approved_evidence_ids.push('ambient-utterance-7')],
    ['wrong screen session', (payload) => { payload.screen.capture_session_id = 'other-session'; }],
    ['second provider session', (payload) => { payload.sidekick.reasoning_sessions_started = 2; }],
    ['excess verifier sessions', (payload) => { payload.sidekick.verifier_sessions_started = 5; }],
    ['missing verifier digest receipt', (payload) => {
      payload.turns[0].candidate_evidence.candidateDigestVerified = false;
    }],
    ['reused verifier session', (payload) => {
      payload.turns[1].candidate_evidence.verifierSessionCorrelation =
        payload.turns[0].candidate_evidence.verifierSessionCorrelation;
    }],
    ['verifier rejection replaced with paint', (payload) => {
      payload.turns[0].candidate_evidence.verificationVerdict.decision = 'reject';
    }],
    ['missing second turn', (payload) => { payload.turns.pop(); }],
    ['incomplete teardown', (payload) => { payload.teardown.recording_pid_removed = false; }],
    ['wrong provider binary', (payload) => { payload.sidekick.provider_executable_sha256 = '0'.repeat(64); }],
    ['wrong provider path', (payload) => { payload.sidekick.provider_executable_path = '/tmp/other/codex'; }],
    ['wrong provider version', (payload) => { payload.sidekick.provider_version = 'codex-cli other'; }],
    ['mismatched DOM turn', (payload) => { payload.turns[0].dom_layout.turnId = 'other'; }],
    ['hidden native window', (payload) => { payload.turns[0].dom_layout.windowVisible = false; }],
    ['fractional response sliver', (payload) => { payload.turns[1].dom_layout.height = 0.8; }],
    ['wrong provider evidence window', (payload) => {
      payload.turns[1].evidence_receipt.transcriptEvidenceIds = ['invented'];
    }],
    ['transcript adapter bypass', (payload) => { payload.transcript.adapter = 'direct_engine_injection'; }],
    ['hidden main control', (payload) => { payload.sidekick.interactable_targets.main_sidekick_button = false; }],
    ['slow startup', (payload) => { payload.startup_latency.sidekick_ready_ms = 75_001; }],
    ['one-LSB microphone glitch', (payload) => {
      payload.audio.peak_amplitude = 1;
      payload.audio.rms_amplitude = 0.001;
      payload.audio.nonzero_ratio = 0.0001;
    }],
    ['empty candidate citations', (payload) => {
      payload.turns[0].candidate_evidence.transcriptEvidenceIds = [];
    }],
    ['wrong approved subset for human reversion', (payload) => {
      payload.turns[1].candidate_evidence.transcriptEvidenceIds = [
        payload.transcript.approved_evidence_ids[2],
        payload.transcript.approved_evidence_ids[4],
      ];
    }],
    ['visual marker used as advice evidence', (payload) => {
      payload.turns[1].candidate_evidence.transcriptEvidenceIds = [];
      payload.turns[1].candidate_evidence.visualEvidenceIds = [
        payload.turns[1].adapter_receipt.providerImageEvidenceId,
      ];
      payload.turns[1].candidate_evidence.claimsVisualObservation = true;
    }],
    ['turn-two transcript delta is a no-op', (payload) => {
      payload.turns[1].adapter_receipt.transcriptNewItems = 0;
    }],
    ['duplicate foreground receipt', (payload) => {
      payload.turns[1].evidence_receipt.turnId = payload.turns[0].evidence_receipt.turnId;
    }],
    ['wrong provider descriptor', (payload) => { payload.sidekick.provider_requested_contract.model = 'unknown'; }],
    ['unbounded acceptance claim', (payload) => { payload.acceptance_scope.excludes = []; }],
    ['overclaimed hostile-host threat model', (payload) => {
      payload.acceptance_scope.host_threat_model = 'hostile_same_uid_resistant';
    }],
    ['image path reopened instead of exact bytes dispatched', (payload) => {
      payload.turns[0].adapter_receipt.providerImageTransport = 'local_path';
    }],
    ['dispatched image digest mismatch', (payload) => {
      payload.turns[1].adapter_receipt.providerImageDispatchedSha256 = '0'.repeat(64);
    }],
    ['sensitive scratch survived', (payload) => { payload.teardown.sensitive_paths_removed = false; }],
  ];
  for (const [name, mutate] of mutations) {
    const payload = passingProductPayload();
    payload.fixture_sha256 = createHash('sha256').update(fixtureBytes).digest('hex');
    mutate(payload);
    const report = evaluateNativeSidekickUiAcceptance(payload, passingRuntime());
    assert.equal(report.passed, false, name);
  }
});

test('the evaluator rejects a surviving provider process or changed private provider copy', async () => {
  const fixtureBytes = await readFile(
    new URL('../../tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json', import.meta.url),
  );
  for (const mutate of [
    (runtime) => { runtime.process_group_empty = false; },
    (runtime) => { runtime.provider_processes_remaining = [91]; },
    (runtime) => { runtime.forced_process_signals = [{ scope: 'provider', pid: 91, signal: 'SIGTERM' }]; },
    (runtime) => { runtime.app_exit_receipt_verified = false; },
    (runtime) => { runtime.app_exit_code = 1; },
    (runtime) => { runtime.provider_copy_post_sha256 = '0'.repeat(64); },
  ]) {
    const payload = passingProductPayload();
    payload.fixture_sha256 = createHash('sha256').update(fixtureBytes).digest('hex');
    const runtime = passingRuntime();
    mutate(runtime);
    assert.equal(evaluateNativeSidekickUiAcceptance(payload, runtime).passed, false);
  }
});
