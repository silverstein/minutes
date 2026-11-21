import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtemp, mkdir, writeFile, rm, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { discoverCanonicalSkills } from "./discover.js";
import { renderOpenAIPlugin, OPENAI_MCP_VERSION, OPENAI_PLUGIN_ROOT } from "./openai-plugin.js";
import { findUnownedGeneratedArtifacts } from "./ownership.js";

const root = fileURLToPath(new URL("../../", import.meta.url));

test("packaged skills resolve installed helpers without depending on a checkout", async () => {
  const skills = await discoverCanonicalSkills(root);
  const artifacts = await renderOpenAIPlugin(root, skills);
  const packaged = [...artifacts.keys()].filter(target => target.endsWith("/SKILL.md"));
  assert.equal(packaged.length, skills.length);
  for (const skill of skills) {
    const name = skill.frontmatter.name;
    const body = artifacts.get(`${OPENAI_PLUGIN_ROOT}/skills/${name}/SKILL.md`)!;
    assert.ok(body.startsWith(`---\nname: ${name}\n`));
    // Release-note authoring legitimately verifies Git refs. Installed helper
    // paths, however, must never depend on finding a repository checkout.
    assert.ok(!body.includes("$(git rev-parse --show-toplevel)"));
    assert.ok(!body.includes("${CLAUDE_PLUGIN_ROOT}"));
    assert.ok(!body.includes(".agents/skills/minutes"));
    assert.ok(body.includes("## Local Minutes Host"));
    if (body.includes("$MINUTES_SKILLS_ROOT")) assert.ok(body.includes("installed SKILL.md"));
  }
});

test("local marketplace stays contained and pins the published MCP package", async () => {
  const artifacts = await renderOpenAIPlugin(root, await discoverCanonicalSkills(root));
  const marketplace = JSON.parse(artifacts.get(".agents/plugins/marketplace.json")!);
  const plugin = JSON.parse(artifacts.get(`${OPENAI_PLUGIN_ROOT}/plugin.json`)!);
  const mcp = JSON.parse(artifacts.get(`${OPENAI_PLUGIN_ROOT}/mcp.json`)!);
  assert.equal(marketplace.plugins[0].name, plugin.name);
  assert.equal(marketplace.plugins[0].source.path, `./${OPENAI_PLUGIN_ROOT}`);
  assert.equal(mcp.mcpServers.minutes.type, "stdio");
  assert.deepEqual(mcp.mcpServers.minutes.args, ["-y", `minutes-mcp@${OPENAI_MCP_VERSION}`]);
  assert.equal(mcp.mcpServers.minutes.env.MINUTES_MCP_AUTO_SETUP, "0");
  const runtime = JSON.parse(await readFile(path.join(root, "../../integrations/openai-plugin/package.json"), "utf8"));
  assert.equal(runtime.dependencies["minutes-mcp"], OPENAI_MCP_VERSION, "installer and generated package must use the same published runtime");
  assert.ok([...artifacts.keys()].every(target => !path.isAbsolute(target) && !target.split("/").includes("..")));
  const learningRuntime = artifacts.get(`${OPENAI_PLUGIN_ROOT}/skills/_runtime/hooks/lib/minutes-learn.mjs`);
  assert.ok(learningRuntime?.includes("export"));
});

test("desktop compatibility entrypoint declares the installed MCP file", async () => {
  const artifacts = await renderOpenAIPlugin(root, await discoverCanonicalSkills(root));
  const portable = JSON.parse(artifacts.get(`${OPENAI_PLUGIN_ROOT}/plugin.json`)!);
  const compatibility = JSON.parse(artifacts.get(`${OPENAI_PLUGIN_ROOT}/.codex-plugin/plugin.json`)!);
  assert.equal(compatibility.name, portable.name);
  assert.equal(compatibility.version, portable.version);
  assert.equal(compatibility.skills, "./skills/");
  // The desktop runtime must load the very file rewritten by install-local,
  // rather than an unpatched duplicate that would launch npx through GUI PATH.
  assert.equal(compatibility.mcpServers, "./mcp.json");
  const referenced = path.posix.join(OPENAI_PLUGIN_ROOT, compatibility.mcpServers);
  const mcp = JSON.parse(artifacts.get(referenced)!);
  assert.deepEqual(Object.keys(mcp.mcpServers), ["minutes"]);
  assert.equal(mcp.mcpServers.minutes.type, "stdio");
});

test("retired or stray plugin skills cannot survive the ownership check", async t => {
  const repo = await mkdtemp(path.join(tmpdir(), "minutes-plugin-ownership-"));
  t.after(() => rm(repo, { recursive: true, force: true }));
  const skills = await discoverCanonicalSkills(root);
  const artifacts = await renderOpenAIPlugin(root, skills);
  for (const [target, content] of artifacts) {
    await mkdir(path.dirname(path.join(repo, target)), { recursive: true });
    await writeFile(path.join(repo, target), content);
  }
  assert.deepEqual(await findUnownedGeneratedArtifacts(repo, skills, artifacts.keys()), []);
  const unexpected = `${OPENAI_PLUGIN_ROOT}/skills/obsolete/SKILL.md`;
  await mkdir(path.dirname(path.join(repo, unexpected)), { recursive: true });
  await writeFile(path.join(repo, unexpected), "obsolete");
  assert.ok((await findUnownedGeneratedArtifacts(repo, skills, artifacts.keys())).includes(unexpected));
});
