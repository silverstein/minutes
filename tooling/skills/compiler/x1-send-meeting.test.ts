import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { discoverCanonicalSkills } from "./discover.js";
import { renderSkillForHost } from "./render.js";
import { getHostConfig } from "../hosts/index.js";

const REPO_ROOT = path.resolve(process.cwd(), "..", "..");
const EXPECTED_ACTIONS = [
  { title: "Send the reviewed schedule", dueDate: "2026-11-12" },
  {
    title: "Send the amendment only after the attorney confirms the trustee",
    dueStatement: "after the CPA confirms the 2023 return",
  },
  {
    title: "Review the account beneficiary",
    dueStatement: "next Friday, if the statement arrives",
  },
  {
    title: "Ask which tax year applies",
    dueStatement: "unknown until the source is checked",
  },
  { title: "Check the attachment" },
];

function assertSendContract(body: string) {
  const text = body.replace(/\s+/g, " ");
  assert.match(text, /Copy `task` verbatim to `title`, including prerequisites and conditions/);
  assert.match(text, /valid `YYYY-MM-DD` date, copy it unchanged to `dueDate`/);
  assert.match(text, /any nonempty `due` verbatim to `dueStatement`/);
  assert.match(text, /Never infer a calendar date/);
  assert.match(text, /never send both `dueDate` and `dueStatement`/);
  assert.match(text, /Omit both when `due` is absent or empty/);
  assert.match(text, /Skip items whose `status` is `done`/);
  assert.match(text, /Copy `detail` only if `get_meeting` actually returns a separate, nonempty `detail` field/);
  assert.match(text, /does not provide a separate detail field/);
  assert.match(text, /`tools\/list`.*`meeting.actionItems` item properties/);
  assert.match(text, /If a needed field is absent or neither discovery path supplies its schema, stop before requesting a send/);
  assert.match(text, /Do not silently drop conditional timing or rewrite a task/);
  assert.match(text, /`detail` and `dueStatement` are each nonempty and at most 1,000 UTF-8 bytes/);
  assert.match(text, /Never call `submit_my_meeting` directly, approve anything on the user's behalf/);
  assert.match(text, /restricted meeting is never sent/);
  assert.match(text, /Never paste transcript text from `body`/);
  const examples = [...body.matchAll(/\| `(\{"title".*?\})` \|/g)]
    .map((match) => JSON.parse(match[1]));
  assert.deepEqual(examples, EXPECTED_ACTIONS);
}

for (const host of ["claude", "codex", "opencode"] as const) {
  test(`${host} meeting send preserves conditional tasks and exact due wording`, async () => {
    const skills = await discoverCanonicalSkills(path.join(REPO_ROOT, "tooling", "skills"));
    const skill = skills.find((entry) => entry.id === "minutes-x1-send-meeting");
    assert.ok(skill);
    const rendered = renderSkillForHost(skill, getHostConfig(host));
    assertSendContract(rendered.body);
    const generated = await readFile(path.join(REPO_ROOT, rendered.outputPath), "utf8");
    assertSendContract(generated);
    assert.equal(generated, rendered.body);
    const golden = await readFile(path.join(REPO_ROOT, "tooling", "skills", "goldens", host,
      "minutes-x1-send-meeting", "SKILL.md"), "utf8");
    assertSendContract(golden);
    assert.equal(golden, rendered.body);
  });
}

test("send contract rejects lost timing, weakened approval, and inferred dates", async () => {
  const skills = await discoverCanonicalSkills(path.join(REPO_ROOT, "tooling", "skills"));
  const skill = skills.find((entry) => entry.id === "minutes-x1-send-meeting");
  assert.ok(skill);
  assertSendContract(skill.body);
  for (const [before, after] of [
    ["any nonempty `due` verbatim to `dueStatement`", "discard non-date due text"],
    ["Never infer a calendar date", "Infer a calendar date"],
    ["Never call `submit_my_meeting` directly", "Call `submit_my_meeting` directly"],
    ["after the CPA confirms the 2023 return", "2026-10-09"],
  ]) {
    assert.throws(() => assertSendContract(skill.body.replaceAll(before, after)));
  }
});
