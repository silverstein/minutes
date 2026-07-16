import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const checker = path.resolve(path.dirname(process.argv[1]), "check_design_tokens.mjs");
const tokens = {
  colors: {
    background: "#F8F4ED",
    accent: "#C96B4E",
  },
  fonts: ["Instrument Serif", "Geist", "Geist Mono"],
  allowedKeywords: [
    "transparent",
    "currentColor",
    "inherit",
    "initial",
    "unset",
    "revert",
    "revert-layer",
    "none",
  ],
};

async function writeFixture(root, file, contents) {
  const destination = path.join(root, file);
  await mkdir(path.dirname(destination), { recursive: true });
  await writeFile(destination, contents);
}

async function writeJson(root, file, value) {
  await writeFixture(root, file, `${JSON.stringify(value, null, 2)}\n`);
}

async function makeRepo(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), "minutes-design-tokens-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeJson(root, "design/tokens.json", tokens);
  await writeJson(root, "design/token-baseline.json", []);
  await writeFixture(root, "site/app/clean.css", ".clean { color: var(--text); }\n");
  return root;
}

function runChecker(root, ...args) {
  return spawnSync(process.execPath, [checker, "--root", root, "--json", ...args], {
    encoding: "utf8",
  });
}

function report(result) {
  assert.equal(result.stdout.startsWith("{"), true, result.stderr || result.stdout);
  return JSON.parse(result.stdout);
}

function assertViolation(result, property, value) {
  assert.equal(result.status, 1, result.stderr || result.stdout);
  const parsed = report(result);
  assert.equal(parsed.ok, false);
  assert.ok(
    parsed.unbaselinedViolations.some(
      (entry) => entry.property === property && entry.value.toLowerCase() === value.toLowerCase(),
    ),
    result.stdout,
  );
}

test("clean fixture passes", async (t) => {
  const root = await makeRepo(t);
  const result = runChecker(root);
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.equal(report(result).ok, true);
});

test("raw color in a non-token CSS file fails", async (t) => {
  const root = await makeRepo(t);
  await writeFixture(root, "site/app/bad.css", ".bad { color: #ff0000; }\n");
  assertViolation(runChecker(root), "color", "#ff0000");
});

test("Tailwind arbitrary color class fails", async (t) => {
  const root = await makeRepo(t);
  await writeFixture(
    root,
    "site/app/page.tsx",
    "export const Page = () => <p className=\"text-[#ff0000]\">bad</p>;\n",
  );
  assertViolation(runChecker(root), "text", "#ff0000");
});

test("TSX style-object color fails", async (t) => {
  const root = await makeRepo(t);
  await writeFixture(
    root,
    "site/app/page.tsx",
    "export const Page = () => <p style={{ color: '#ff0000' }}>bad</p>;\n",
  );
  assertViolation(runChecker(root), "color", "#ff0000");
});

test("literal SVG fill attribute fails", async (t) => {
  const root = await makeRepo(t);
  await writeFixture(
    root,
    "site/app/icon.tsx",
    "export const Icon = () => <svg><path fill=\"#ff0000\" /></svg>;\n",
  );
  assertViolation(runChecker(root), "fill", "#ff0000");
});

test("sanctioned hex outside a token-definition file still fails", async (t) => {
  const root = await makeRepo(t);
  await writeFixture(root, "site/app/bad.css", ".bad { background: #F8F4ED; }\n");
  assertViolation(runChecker(root), "background", "#F8F4ED");
});

test("sanctioned hex inside a token-definition file passes", async (t) => {
  const root = await makeRepo(t);
  await writeFixture(root, "site/app/globals.css", ":root { --bg: #F8F4ED; }\n");
  const result = runChecker(root);
  assert.equal(result.status, 0, result.stderr || result.stdout);
});

test("an unsanctioned custom-property value in a token-definition file fails consistency", async (t) => {
  const root = await makeRepo(t);
  await writeFixture(root, "site/app/globals.css", ":root { --decorative: #ff0000; }\n");
  assertViolation(runChecker(root), "--decorative", "#ff0000");
});

test("comment-only mentions pass", async (t) => {
  const root = await makeRepo(t);
  await writeFixture(
    root,
    "site/app/comments.css",
    "/* .old { color: #ff0000; background: rgb(255, 0, 0); } */\n.clean { color: var(--text); }\n",
  );
  await writeFixture(
    root,
    "site/app/comments.tsx",
    "// <svg fill=\"#ff0000\" className=\"text-[#ff0000]\" />\nexport const ok = true;\n",
  );
  const result = runChecker(root);
  assert.equal(result.status, 0, result.stderr || result.stdout);
});

test("stale baseline entry fails", async (t) => {
  const root = await makeRepo(t);
  await writeJson(root, "design/token-baseline.json", [
    { file: "site/app/removed.css", property: "color", value: "#ff0000" },
  ]);
  const result = runChecker(root);
  assert.equal(result.status, 1, result.stderr || result.stdout);
  assert.deepEqual(report(result).staleBaselineEntries, [
    { file: "site/app/removed.css", property: "color", value: "#ff0000" },
  ]);
});

test("base-versus-head comparison rejects baseline additions", async (t) => {
  const root = await makeRepo(t);
  const entry = { file: "site/app/legacy.css", property: "color", value: "#ff0000" };
  await writeFixture(root, entry.file, ".legacy { color: #ff0000; }\n");
  await writeJson(root, "design/token-baseline.json", [entry]);
  await writeJson(root, "base-token-baseline.json", []);

  const result = runChecker(root, "--base-baseline", path.join(root, "base-token-baseline.json"));
  assert.equal(result.status, 1, result.stderr || result.stdout);
  assert.deepEqual(report(result).addedBaselineEntries, [entry]);
});
