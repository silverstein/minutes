import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

const indexHtml = new URL('../../tauri/src/index.html', import.meta.url);
const installDevApp = new URL('../install-dev-app.sh', import.meta.url);
const mainRust = new URL('../../tauri/src-tauri/src/main.rs', import.meta.url);
const commandsRust = new URL('../../tauri/src-tauri/src/commands.rs', import.meta.url);

function inlineScripts(source) {
  return [...source.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/gi)]
    .map((match) => match[1])
    .filter((script) => script.trim());
}

function eventTarget() {
  const listeners = new Map();
  return {
    addEventListener(type, listener) {
      const registered = listeners.get(type) || [];
      registered.push(listener);
      listeners.set(type, registered);
    },
    removeEventListener() {},
    dispatchEvent(event) {
      for (const listener of listeners.get(event.type) || []) listener(event);
      return true;
    },
  };
}

function fakeElement(id = '') {
  const classes = new Set();
  const events = eventTarget();
  const element = {
    ...events,
    id,
    textContent: '',
    innerHTML: '',
    value: '',
    checked: false,
    disabled: false,
    hidden: false,
    dataset: {},
    children: [],
    style: {
      setProperty() {},
      removeProperty() {},
    },
    classList: {
      add: (...names) => names.forEach((name) => classes.add(name)),
      remove: (...names) => names.forEach((name) => classes.delete(name)),
      toggle(name, force) {
        const enabled = force === undefined ? !classes.has(name) : Boolean(force);
        if (enabled) classes.add(name);
        else classes.delete(name);
        return enabled;
      },
      contains: (name) => classes.has(name),
    },
    appendChild(child) {
      this.children.push(child);
      return child;
    },
    replaceChildren(...children) {
      this.children = children;
    },
    remove() {},
    focus() {},
    blur() {},
    click() {},
    select() {},
    setAttribute() {},
    removeAttribute() {},
    getAttribute() { return null; },
    hasAttribute() { return false; },
    closest() { return null; },
    matches() { return false; },
    querySelector(selector) { return fakeElement(selector); },
    querySelectorAll() { return []; },
    scrollIntoView() {},
    getBoundingClientRect() {
      return { x: 0, y: 0, top: 0, left: 0, right: 560, bottom: 700, width: 560, height: 700 };
    },
    scrollHeight: 0,
    scrollTop: 0,
    clientHeight: 700,
    clientWidth: 560,
    offsetHeight: 0,
    offsetWidth: 0,
  };
  return element;
}

function coldThenable() {
  const value = {
    then() { return value; },
    catch() { return value; },
    finally() { return value; },
  };
  return value;
}

function startupContext(declaredIds, {
  hangingCommands = new Set(),
  commandHandlers = new Map(),
  hangEventListen = false,
  eventListenHandler = null,
  fireTimeouts = new Set(),
  stallAnimationFrames = false,
} = {}) {
  const elements = new Map();
  const invocations = [];
  const consoleMessages = [];
  const intervals = [];
  const clock = { now: Date.now() };
  const windowEvents = eventTarget();
  const documentEvents = eventTarget();
  const documentElement = fakeElement('html');
  documentElement.dataset.platform = 'macos';
  const body = fakeElement('body');
  const document = {
    ...documentEvents,
    readyState: 'complete',
    documentElement,
    body,
    activeElement: body,
    getElementById(id) {
      if (!declaredIds.has(id)) return null;
      if (!elements.has(id)) elements.set(id, fakeElement(id));
      return elements.get(id);
    },
    createElement(tagName) { return fakeElement(tagName); },
    createTextNode(text) { return { textContent: text }; },
    querySelector(selector) { return fakeElement(selector); },
    querySelectorAll() { return []; },
  };
  const responses = {
    cmd_capture_status: { processingJobs: [], recording: false, starting: false, processing: false },
    cmd_copilot_surface_status: {},
    cmd_desktop_capabilities: {},
    cmd_get_recall_workspace_state: null,
    cmd_get_settings: { assistant: { agent: 'codex' }, notifications: {} },
    cmd_global_hotkey_settings: { choices: [], enabled: false, shortcut: 'capslock' },
    cmd_list_agents: [],
    cmd_list_meetings: [],
    cmd_needs_setup: {},
    cmd_set_global_hotkey: { choices: [], enabled: false, shortcut: 'capslock' },
  };
  const currentWindow = new Proxy({
    listen: () => Promise.resolve(() => {}),
    setSize: () => coldThenable(),
    setPosition: () => coldThenable(),
    outerPosition: () => coldThenable(),
    outerSize: () => coldThenable(),
  }, {
    get(target, property) {
      return property in target ? target[property] : () => coldThenable();
    },
  });
  const invoke = (command, args) => {
    invocations.push({ command, args });
    if (commandHandlers.has(command)) return commandHandlers.get(command)(args);
    if (hangingCommands.has(command)) return new Promise(() => {});
    if (command === 'cmd_shortcut_status') {
      const shortcut = args?.slot === 'dictation'
        ? 'CmdOrCtrl+Shift+Space'
        : 'CmdOrCtrl+Shift+M';
      return Promise.resolve({
        slot: args?.slot,
        enabled: false,
        shortcut,
        keycode: -1,
        message: 'Off.',
      });
    }
    return Promise.resolve(responses[command] ?? {});
  };
  const tauri = {
    core: { invoke },
    event: {
      listen: (eventName, handler) => {
        if (eventListenHandler) return eventListenHandler(eventName, handler);
        return hangEventListen ? new Promise(() => {}) : Promise.resolve(() => {});
      },
    },
    window: { getCurrentWindow: () => currentWindow, LogicalSize: class {}, LogicalPosition: class {} },
    dialog: new Proxy({}, { get: () => () => coldThenable() }),
    shell: new Proxy({}, { get: () => () => coldThenable() }),
    app: new Proxy({}, { get: () => () => coldThenable() }),
  };
  const window = {
    ...windowEvents,
    document,
    __TAURI__: tauri,
    innerWidth: 560,
    innerHeight: 700,
    devicePixelRatio: 2,
    getSelection: () => ({ toString: () => '' }),
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
  };
  window.window = window;
  window.self = window;

  class Observer {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  class FakeTerminal {
    loadAddon() {}
    open() {}
    focus() {}
    write() {}
    writeln() {}
    onData() { return { dispose() {} }; }
    onResize() { return { dispose() {} }; }
    dispose() {}
  }
  class FakeAudio {
    constructor() {
      this.volume = 1;
      this.currentTime = 0;
    }
    play() { return Promise.resolve(); }
    pause() {}
    addEventListener() {}
    removeEventListener() {}
  }

  const fakeConsole = {
    debug: (...args) => consoleMessages.push({ level: 'debug', args }),
    error: (...args) => consoleMessages.push({ level: 'error', args }),
    info: (...args) => consoleMessages.push({ level: 'info', args }),
    log: (...args) => consoleMessages.push({ level: 'log', args }),
    warn: (...args) => consoleMessages.push({ level: 'warn', args }),
  };
  class FakeDate extends Date {
    static now() { return clock.now; }
  }

  const context = vm.createContext({
    window,
    self: window,
    document,
    navigator: { platform: 'MacIntel', clipboard: { writeText: async () => {} } },
    location: { reload() {} },
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    sessionStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    crypto: { randomUUID: () => '00000000-0000-4000-8000-000000000000' },
    performance: { now: () => 0 },
    getComputedStyle: () => ({ getPropertyValue: () => '', display: 'block' }),
    requestAnimationFrame: (callback) => {
      if (!stallAnimationFrames) callback(0);
      return 1;
    },
    cancelAnimationFrame() {},
    setTimeout: (callback, delay) => {
      if (fireTimeouts.has(delay)) queueMicrotask(callback);
      return 1;
    },
    clearTimeout() {},
    setInterval: (callback, delay) => {
      intervals.push({ callback, delay });
      return intervals.length;
    },
    clearInterval() {},
    ResizeObserver: Observer,
    MutationObserver: Observer,
    IntersectionObserver: Observer,
    Audio: FakeAudio,
    Terminal: FakeTerminal,
    FitAddon: { FitAddon: class { fit() {} } },
    marked: { parse: (value) => String(value ?? ''), setOptions() {} },
    CSS: { escape: (value) => String(value) },
    URL,
    URLSearchParams,
    Blob,
    TextDecoder,
    TextEncoder,
    AbortController,
    Promise,
    Map,
    Set,
    WeakMap,
    WeakSet,
    Array,
    Object,
    String,
    Number,
    Boolean,
    Date: FakeDate,
    Math,
    JSON,
    RegExp,
    Error,
    ReferenceError,
    TypeError,
    RangeError,
    console: fakeConsole,
  });

  return { clock, consoleMessages, context, elements, intervals, invocations };
}

test('complete desktop frontend registers without a startup ReferenceError', async () => {
  const source = await readFile(indexHtml, 'utf8');
  const declaredIds = new Set([...source.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]));
  const scripts = inlineScripts(source);
  const bootstrapScript = scripts.find((script) => script.includes('window.__MINUTES_STARTUP__'));
  const mainScript = scripts.find((script) => script.includes('const { invoke }'));
  assert.ok(bootstrapScript, 'startup recovery bootstrap should be present');
  assert.ok(mainScript, 'main desktop inline script should be present');
  const { context, invocations } = startupContext(declaredIds);

  assert.doesNotThrow(
    () => {
      new vm.Script(bootstrapScript, { filename: 'tauri/src/index.html#startup-recovery' }).runInContext(context);
      new vm.Script(mainScript, { filename: 'tauri/src/index.html#main' }).runInContext(context);
    },
    'the complete desktop frontend must register without an undefined global or startup exception',
  );
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  assert.ok(
    invocations.some(({ command }) => command === 'cmd_frontend_ready'),
    'a fully registered frontend should complete the native readiness handshake',
  );
});

test('startup smoke rejects the missing Sidekick listener binding regression', async () => {
  const source = await readFile(indexHtml, 'utf8');
  const mainScript = inlineScripts(source).find((script) => script.includes('const { invoke }'));
  assert.ok(mainScript);
  const requiredBinding = /const\s*{[^}]*\blisten\b[^}]*}\s*=\s*window\.__TAURI__\.event\s*;/;
  assert.match(
    mainScript,
    requiredBinding,
    'bare Sidekick listen(...) calls require a statically registered Tauri event binding',
  );

  const broken = mainScript.replace('const { listen } = window.__TAURI__.event;', '');
  assert.doesNotMatch(broken, requiredBinding, 'the regression mutation must be caught by the static guard');
});

test('frontend readiness is not held hostage by optional startup hydration', async () => {
  const source = await readFile(indexHtml, 'utf8');
  const declaredIds = new Set([...source.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]));
  const scripts = inlineScripts(source);
  const bootstrapScript = scripts.find((script) => script.includes('window.__MINUTES_STARTUP__'));
  const mainScript = scripts.find((script) => script.includes('const { invoke }'));
  const { context, invocations } = startupContext(declaredIds, {
    hangingCommands: new Set(['cmd_capture_status']),
    hangEventListen: true,
    stallAnimationFrames: true,
  });

  new vm.Script(bootstrapScript, { filename: 'tauri/src/index.html#startup-recovery' }).runInContext(context);
  new vm.Script(mainScript, { filename: 'tauri/src/index.html#main' }).runInContext(context);
  await new Promise((resolve) => setImmediate(resolve));

  const readyIndex = invocations.findIndex(({ command }) => command === 'cmd_frontend_ready');
  const hydrationIndex = invocations.findIndex(({ command }) => command === 'cmd_capture_status');
  assert.ok(readyIndex >= 0, 'the registered frontend must report ready even if hydration never settles');
  assert.ok(hydrationIndex > readyIndex, 'readiness must be sent before optional hydration begins');
});

test('a timed-out capture status read releases the poller and the real interval recovers', async () => {
  const source = await readFile(indexHtml, 'utf8');
  const declaredIds = new Set([...source.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]));
  const scripts = inlineScripts(source);
  const bootstrapScript = scripts.find((script) => script.includes('window.__MINUTES_STARTUP__'));
  const mainScript = scripts.find((script) => script.includes('const { invoke }'));
  let captureAttempts = 0;
  const commandHandlers = new Map([
    ['cmd_capture_status', () => {
      captureAttempts += 1;
      if (captureAttempts === 1) return new Promise(() => {});
      return Promise.resolve({ processingJobs: [], recording: false, starting: false, processing: false });
    }],
  ]);
  const { clock, context, intervals } = startupContext(declaredIds, {
    commandHandlers,
    fireTimeouts: new Set([4_000]),
  });

  new vm.Script(bootstrapScript, { filename: 'tauri/src/index.html#startup-recovery' }).runInContext(context);
  new vm.Script(mainScript, { filename: 'tauri/src/index.html#main' }).runInContext(context);
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));

  const retryAt = vm.runInContext('statusRetryAt', context);
  const statusInterval = intervals.find(({ callback, delay }) => (
    delay === 2_000 && String(callback).includes('checkStatus')
  ));
  assert.ok(statusInterval, 'the production status interval must be registered');
  assert.equal(captureAttempts, 1);
  assert.equal(vm.runInContext('statusCheckInFlight', context), false);

  statusInterval.callback();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(captureAttempts, 1, 'backoff must suppress an immediate retry');

  clock.now = retryAt;
  statusInterval.callback();
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(captureAttempts, 2, 'polling must retry after the timed-out request');
  assert.equal(vm.runInContext('statusCheckInFlight', context), false);
});

test('Sidekick listener registration retries and cleans a stale late listener', async () => {
  const source = await readFile(indexHtml, 'utf8');
  const declaredIds = new Set([...source.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]));
  const scripts = inlineScripts(source);
  const bootstrapScript = scripts.find((script) => script.includes('window.__MINUTES_STARTUP__'));
  const mainScript = scripts.find((script) => script.includes('const { invoke }'));
  let sidekickAttempts = 0;
  let resolveFirstListener;
  let staleUnlistenCalls = 0;
  const activeRegistrations = [];
  const installListener = (handler, stale = false) => {
    const registration = { active: true, handler };
    activeRegistrations.push(registration);
    return () => {
      if (!registration.active) return;
      registration.active = false;
      if (stale) staleUnlistenCalls += 1;
    };
  };
  const eventListenHandler = (eventName, handler) => {
    if (eventName !== 'sidekick:state') return Promise.resolve(() => {});
    sidekickAttempts += 1;
    if (sidekickAttempts === 1) {
      const unlisten = installListener(handler, true);
      return new Promise((resolve) => {
        resolveFirstListener = () => resolve(unlisten);
      });
    }
    return Promise.resolve(installListener(handler));
  };
  const { context } = startupContext(declaredIds, {
    eventListenHandler,
    fireTimeouts: new Set([6_000, 1_000]),
  });

  new vm.Script(bootstrapScript, { filename: 'tauri/src/index.html#startup-recovery' }).runInContext(context);
  new vm.Script(mainScript, { filename: 'tauri/src/index.html#main' }).runInContext(context);
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sidekickAttempts, 2, 'a timed-out listener registration must be retried');
  assert.equal(
    activeRegistrations.filter(({ active }) => active).length,
    2,
    'the harness must exercise the overlap before the stale unlisten handle arrives',
  );

  activeRegistrations[0].handler({ payload: { active: true } });
  assert.equal(
    vm.runInContext('sidekickSessionOpen', context),
    false,
    'an invalidated listener must ignore events even before it can be unregistered',
  );
  activeRegistrations[1].handler({ payload: { active: true } });
  assert.equal(
    vm.runInContext('sidekickSessionOpen', context),
    true,
    'the current replacement listener must continue delivering events during the overlap',
  );

  resolveFirstListener();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(staleUnlistenCalls, 1, 'a late stale registration must be retired');
  const activeListeners = activeRegistrations.filter(({ active }) => active);
  assert.equal(activeListeners.length, 1, 'exactly one replacement listener must remain active');
  activeListeners[0].handler({ payload: { active: false } });
  assert.equal(
    vm.runInContext('sidekickSessionOpen', context),
    false,
    'the surviving listener must remain connected to the Sidekick UI state',
  );
});

test('startup bootstrap renders a recovery surface and reports the diagnostic', async () => {
  const source = await readFile(indexHtml, 'utf8');
  const declaredIds = new Set([...source.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]));
  const bootstrapScript = inlineScripts(source).find((script) => script.includes('window.__MINUTES_STARTUP__'));
  assert.ok(bootstrapScript);
  const { context, elements, invocations } = startupContext(declaredIds);
  new vm.Script(bootstrapScript, { filename: 'tauri/src/index.html#startup-recovery' }).runInContext(context);

  vm.runInContext("window.__MINUTES_STARTUP__.fail(new ReferenceError('listen is not defined'))", context);

  assert.equal(elements.get('startup-failure').classList.contains('active'), true);
  assert.match(elements.get('startup-failure-detail').textContent, /listen is not defined/);
  assert.ok(
    invocations.some(({ command, args }) =>
      command === 'cmd_frontend_startup_failed' && args.message.includes('listen is not defined')),
  );
});

test('startup smoke fails when a required control disappears from the real markup', async () => {
  const source = await readFile(indexHtml, 'utf8');
  const scripts = inlineScripts(source);
  const mainScript = scripts.find((script) => script.includes('const { invoke }'));
  const declaredIds = new Set([...source.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]));
  declaredIds.delete('btn-settings');
  const { context } = startupContext(declaredIds);

  assert.throws(
    () => new vm.Script(mainScript, { filename: 'tauri/src/index.html#missing-control' }).runInContext(context),
    /null|addEventListener|classList/,
  );
});

test('every native command invoked by the desktop frontend is registered', async () => {
  const [html, rust] = await Promise.all([readFile(indexHtml, 'utf8'), readFile(mainRust, 'utf8')]);
  const handler = rust.split('.invoke_handler(tauri::generate_handler![')[1]?.split('])')[0] || '';
  const commands = new Set([...html.matchAll(/\binvoke\(\s*['"](cmd_[a-zA-Z0-9_]+)['"]/g)].map((match) => match[1]));
  assert.ok(commands.size > 20, 'frontend command extraction should cover the real desktop surface');
  for (const command of commands) {
    assert.match(handler, new RegExp(`\\b${command}\\b`), `${command} must be in Tauri generate_handler!`);
  }
});

test('dev installer retires the old app and verifies the fresh frontend', async () => {
  const source = await readFile(installDevApp, 'utf8');

  assert.match(
    source,
    /export MINUTES_BUILD_COMMIT="\$\(git rev-parse --verify HEAD\)"/,
    'the installer must embed the exact checked-out commit for installed-build acceptance',
  );
  assert.match(
    source,
    /assert_clean_build_source[\s\S]*git status --porcelain=v1 --untracked-files=all[\s\S]*echo "=== Building CLI \(release\) ==="[\s\S]*assert_clean_build_source[\s\S]*cargo tauri build[\s\S]*assert_clean_build_source/,
    'the installer must reject dirty application or harness source before and after compilation',
  );
  assert.doesNotMatch(
    source.match(/assert_clean_build_source\(\)[\s\S]*?^}/m)?.[0] || '',
    /mic_check/,
    'the source cleanliness gate must not exempt generated helper binaries',
  );
  assert.match(
    source,
    /prepare_canonical_build_helpers[\s\S]*cp -p "\$helper" "\$HELPER_BACKUP_DIR\/\$index"[\s\S]*write_head_helper "\$helper"/,
    'the installer must preserve developer helper copies while resetting build inputs to HEAD',
  );
  assert.match(
    source,
    /cargo clean -p minutes-app[\s\S]*remove_generated_build_helpers[\s\S]*cargo tauri build[\s\S]*reset_tracked_build_helpers_to_head[\s\S]*assert_clean_build_source/,
    'a warm build must regenerate every bundled native helper before packaging and restore canonical source state afterward',
  );
  assert.match(
    source,
    /cleanup_install_artifacts[\s\S]*restore_user_build_helpers/,
    'developer helper copies must be restored on every success or failure exit path',
  );

  assert.match(
    source,
    /acquire_install_lock[\s\S]*cp -rf "\$BUILD_APP" "\$STAGED_APP"[\s\S]*codesign --verify --deep --strict "\$STAGED_APP"[\s\S]*stop_running_dev_app[\s\S]*\/bin\/mv -f "\$INSTALL_APP" "\$BACKUP_APP"[\s\S]*\/bin\/mv -f "\$STAGED_APP" "\$INSTALL_APP"/,
    'the installer must lock, seal a staged copy, stop the old process, and atomically swap bundles',
  );
  assert.match(
    source,
    /LAUNCH_STARTED_UNIX_MS=.*[\s\S]*open -n "\$INSTALL_APP"[\s\S]*verify_frontend_startup "\$LAUNCH_STARTED_UNIX_MS"/,
    'the installer must launch a new process and wait for a launch-fresh frontend readiness signal',
  );
  assert.match(
    source,
    /status_pid" == "\$pid"[\s\S]*frontend_ready" == "true"[\s\S]*process_started_unix_ms" -ge "\$launch_started_unix_ms"[\s\S]*frontend_ready_at_unix_ms" -ge "\$launch_started_unix_ms"/,
    'the readiness gate must reject a stale heartbeat or a different process',
  );
  assert.match(
    source,
    /local max_attempts=180[\s\S]*attempt < max_attempts[\s\S]*sleep 0\.5/,
    'the readiness gate must tolerate a 90-second cold LaunchServices registration without weakening PID freshness',
  );
  assert.match(
    source,
    /restore_previous_app 1/,
    'a launch or startup failure must restore the previous app',
  );
  assert.match(
    source,
    /with timeout of 5 seconds[\s\S]*force_failed_candidate[\s\S]*kill -TERM[\s\S]*kill -KILL/,
    'rollback must bound an unresponsive AppleEvent and retire only the verified failed candidate process',
  );
  assert.match(
    source,
    /verify_frontend_startup[\s\S]*restore_previous_app 1 1/,
    'a candidate that never reaches frontend-ready must use the hardened rollback path',
  );
  assert.match(
    source,
    /INSTALL_SWAP_ACTIVE[\s\S]*cleanup_install_artifacts[\s\S]*restored the previous/,
    'an interrupted swap must not strand the machine without its previous app',
  );
  assert.doesNotMatch(source, /open -a "\$INSTALL_APP"/);
});

test('desktop activation backfill cannot crawl the meetings tree before the UI starts', async () => {
  const source = await readFile(commandsRust, 'utf8');
  const indexLookup = source
    .split('fn latest_saved_artifact_from_index(config: &Config) -> Option<IndexedActivationArtifact> {')[1]
    ?.split('\n}')[0] || '';
  const backfill = source
    .split('fn backfill_activation_from_paths(')[1]
    ?.split('\n}')[0] || '';
  const executableIndexLookup = indexLookup.replace(/\/\/.*$/gm, '');

  assert.match(executableIndexLookup, /SearchIndex::open\(config\)[\s\S]*\.search\("", &filters, Some\(1\)\)/);
  assert.doesNotMatch(executableIndexLookup, /search_with_mode|output_dir|\.exists\(|metadata\(|path_timestamp/);
  assert.match(backfill, /artifact\.saved_at\.clone\(\)/);
  assert.doesNotMatch(backfill, /path_timestamp\(artifact|path_timestamp\(path/);
});
