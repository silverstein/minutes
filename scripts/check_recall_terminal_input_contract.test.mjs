import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const PANEL = "tauri/src/index.html";
const panel = readFileSync(PANEL, "utf8");
const xterm = readFileSync("tauri/src/vendor/xterm/xterm.js", "utf8");

function extractFunction(name) {
  const asyncStart = panel.indexOf(`async function ${name}(`);
  const start = asyncStart !== -1 ? asyncStart : panel.indexOf(`function ${name}(`);
  assert.notEqual(start, -1, `missing ${name} in ${PANEL}`);
  const bodyStart = panel.indexOf("{", start);
  let depth = 0;
  for (let index = bodyStart; index < panel.length; index += 1) {
    if (panel[index] === "{") depth += 1;
    if (panel[index] === "}") depth -= 1;
    if (depth === 0) return panel.slice(start, index + 1);
  }
  throw new Error(`unterminated ${name} in ${PANEL}`);
}

function createHarness({ meetingPath = "/meetings/private.md" } = {}) {
  const source = `
    (() => {
      let pendingRecallTerminalMeetingPath = ${JSON.stringify(meetingPath)};
      let recallTerminalContextPending = true;
      let recallTerminalInputDraft = '';
      let recallTerminalInputReliable = true;
      const SESSION_ID = 'recall';
      const calls = [];
      const notices = [];
      let prepareError = null;

      async function invoke(command, args) {
        calls.push({ command, args });
        if (command === 'cmd_prepare_recall_terminal_meeting' && prepareError) {
          throw new Error(prepareError);
        }
      }

      function setRecallProviderNotice(message, level = 'info') {
        notices.push({ message, level });
      }

      ${extractFunction("forwardRecallTerminalData")}
      ${extractFunction("updateRecallTerminalInputDraft")}
      ${extractFunction("forwardRecallTerminalInput")}

      return {
        send: forwardRecallTerminalData,
        state: () => ({
          pending: pendingRecallTerminalMeetingPath,
          contextPending: recallTerminalContextPending,
          draft: recallTerminalInputDraft,
          reliable: recallTerminalInputReliable,
          calls: structuredClone(calls),
          notices: structuredClone(notices),
        }),
        setPrepareError: (message) => { prepareError = message; },
      };
    })()
  `;
  return vm.runInNewContext(source, { structuredClone });
}

function commands(harness, command) {
  return harness.state().calls.filter((call) => call.command === command);
}

test("xterm capability replies bypass the human-input shadow line", async () => {
  const harness = createHarness();
  await harness.send("\x1b[12;34R", false);

  const state = harness.state();
  assert.equal(state.draft, "");
  assert.equal(state.reliable, true);
  assert.equal(state.notices.length, 0);
  assert.deepEqual(state.calls, [
    {
      command: "cmd_pty_input",
      args: { sessionId: "recall", data: "\x1b[12;34R" },
    },
  ]);
});

test("the first plain question prepares the meeting before Return", async () => {
  const harness = createHarness();
  await harness.send("What did we decide?\r", true);

  const state = harness.state();
  assert.equal(state.pending, null);
  assert.equal(state.draft, "");
  assert.equal(state.reliable, true);
  assert.deepEqual(
    state.calls.map((call) => call.command),
    ["cmd_pty_input", "cmd_prepare_recall_terminal_meeting", "cmd_pty_input"],
  );
  assert.equal(state.calls[2].args.data, "\r");
});

test("a general question with no selected meeting prepares the meeting directory once", async () => {
  const harness = createHarness({ meetingPath: null });
  await harness.send("What did I commit to this week?\r", true);
  await harness.send("And last week?\r", true);

  const state = harness.state();
  const prepares = commands(harness, "cmd_prepare_recall_terminal_meeting");
  assert.equal(prepares.length, 1, "the general context is materialized exactly once");
  assert.deepEqual(prepares[0].args, { meetingPath: null });
  assert.equal(state.pending, null);
  assert.equal(state.contextPending, false);
  assert.match(state.notices.at(-1).message, /meeting directory is ready/);
  assert.equal(state.calls.at(-1).args.data, "\r");
});

test("a general question still cannot bypass the privacy checks", async () => {
  const harness = createHarness({ meetingPath: null });
  await harness.send("\r", true);

  const state = harness.state();
  assert.equal(commands(harness, "cmd_prepare_recall_terminal_meeting").length, 0);
  assert.equal(state.contextPending, true);
  assert.match(state.notices.at(-1).message, /complete question or command/);
});

test("startup terminal replies do not poison the first real question", async () => {
  const harness = createHarness();
  await harness.send("\x1b[?1;2c", false);
  await harness.send("Summarize the risks", true);
  await harness.send("\r", true);

  assert.equal(commands(harness, "cmd_prepare_recall_terminal_meeting").length, 1);
  assert.equal(harness.state().pending, null);
});

test("slash commands pass through without reading the meeting", async () => {
  const harness = createHarness();
  await harness.send("/login\r", true);

  const state = harness.state();
  assert.equal(state.pending, "/meetings/private.md");
  assert.equal(commands(harness, "cmd_prepare_recall_terminal_meeting").length, 0);
  assert.equal(state.calls.at(-1).args.data, "\r");
});

test("Ctrl-U then Ctrl-Y cannot submit an untracked question", async () => {
  const harness = createHarness();
  await harness.send("secret question", true);
  await harness.send("\x15", true);
  await harness.send("\x19", true);
  await harness.send("\r", true);

  const state = harness.state();
  assert.equal(state.pending, "/meetings/private.md");
  assert.equal(commands(harness, "cmd_prepare_recall_terminal_meeting").length, 0);
  assert.notEqual(state.calls.at(-1).args.data, "\r");
  assert.match(state.notices.at(-1).message, /No meeting context was read/);
});

test("cursor editing holds Return until the line is cleanly retyped", async () => {
  const harness = createHarness();
  await harness.send("question", true);
  await harness.send("\x1b[D", true);
  await harness.send("\r", true);

  const state = harness.state();
  assert.equal(state.pending, "/meetings/private.md");
  assert.equal(commands(harness, "cmd_prepare_recall_terminal_meeting").length, 0);
  assert.notEqual(state.calls.at(-1).args.data, "\r");
});

test("an empty Return cannot bypass pending meeting preparation", async () => {
  const harness = createHarness();
  await harness.send("\r", true);

  const state = harness.state();
  assert.equal(state.pending, "/meetings/private.md");
  assert.equal(state.calls.length, 0);
  assert.match(state.notices.at(-1).message, /complete question or command/);
});

test("a preparation failure keeps both the meeting and Return pending", async () => {
  const harness = createHarness();
  harness.setPrepareError("not signed in");
  await harness.send("Question", true);
  await harness.send("\r", true);

  const state = harness.state();
  assert.equal(state.pending, "/meetings/private.md");
  assert.equal(commands(harness, "cmd_prepare_recall_terminal_meeting").length, 1);
  assert.notEqual(state.calls.at(-1).args?.data, "\r");
  assert.match(state.notices.at(-1).message, /not signed in/);
});

test("the panel wires xterm's user-input signal with a fail-closed fallback", () => {
  assert.ok(
    xterm.includes("onUserInput=this._onUserInput.event") &&
      xterm.includes("this.coreService=") &&
      xterm.includes("this._core=this.register"),
    "the vendored xterm internals used to distinguish human input changed",
  );
  assert.match(panel, /_core\?\.coreService\?\.onUserInput/);
  assert.match(
    panel,
    /const isUserInput = !recallTerminalHasUserInputSignal \|\| recallTerminalNextDataIsUserInput/,
  );
});
