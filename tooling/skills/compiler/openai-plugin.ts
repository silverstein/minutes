import path from "node:path";
import { readFile } from "node:fs/promises";
import type { CanonicalSkillSource } from "../schema.js";
import { codexHost } from "../hosts/codex.js";
import { renderSkillForHost } from "./render.js";
import { resolveSkillAssetSourcePath } from "./validate.js";

export const OPENAI_PLUGIN_ROOT = ".agents/plugins/minutes";
export const OPENAI_MCP_VERSION = "0.28.0";
export const OPENAI_PLUGIN_VERSION = "0.2.4";

export async function renderOpenAIPlugin(rootDir: string, skills: CanonicalSkillSource[]): Promise<Map<string, string>> {
  const artifacts = new Map<string, string>();
  const json = (value: unknown) => `${JSON.stringify(value, null, 2)}\n`;
  const identity = {
    name: "minutes", version: OPENAI_PLUGIN_VERSION,
    description: "Record conversations, search your local Minutes library, prepare meetings, and follow up. Includes the full Minutes skill set and MCP tools; local capabilities require the Minutes engine and OS permissions.",
    author: { name: "Mat Silverstein", url: "https://github.com/silverstein" },
    homepage: "https://useminutes.app", repository: "https://github.com/silverstein/minutes", license: "MIT",
    keywords: ["meetings", "conversation-memory", "local-first", "minutes"],
  };
  const presentation = { displayName: "Minutes", shortDescription: "Recording, conversation search, meeting prep, and follow-up with local Minutes." };
  artifacts.set(`${OPENAI_PLUGIN_ROOT}/plugin.json`, json({
    $schema: "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
    ...identity,
    extensions: { "com.openai": { interface: presentation } },
  }));
  // The local installer selects this compatibility entrypoint for desktop
  // builds that display portable skills but ignore portable MCP components.
  // Both layouts reference the file rewritten to absolute runtime paths.
  artifacts.set(`${OPENAI_PLUGIN_ROOT}/.codex-plugin/plugin.json`, json({
    ...identity, skills: "./skills/", mcpServers: "./mcp.json", interface: presentation,
  }));
  artifacts.set(`${OPENAI_PLUGIN_ROOT}/mcp.json`, json({
    $schema: "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
    mcpServers: { minutes: { type: "stdio", command: "npx", args: ["-y", `minutes-mcp@${OPENAI_MCP_VERSION}`], env: { MINUTES_MCP_AUTO_SETUP: "0" } } },
  }));
  artifacts.set(".agents/plugins/marketplace.json", json({
    name: "minutes", interface: { displayName: "Minutes" },
    plugins: [{ name: "minutes", source: { source: "local", path: `./${OPENAI_PLUGIN_ROOT}` },
      policy: { installation: "AVAILABLE", authentication: "ON_INSTALL" }, category: "Productivity" }],
  }));

  // Package every canonical skill. A curated subset silently hid capture,
  // copilot and other existing workflows from ChatGPT's local plugin surface.
  for (const source of skills) {
    const name = source.frontmatter.name;
    const rendered = renderSkillForHost(source, codexHost);
    const outputDir = `${OPENAI_PLUGIN_ROOT}/skills/${name}`;
    // A downloaded plugin need not be inside a Git repository. Resolve helpers
    // from the installed skill location, never from the user's working tree.
    const rootNote = `## Skill Path\n\nResolve this skill's installed SKILL.md path from the host's skill metadata.\nSet MINUTES_SKILL_ROOT to that file's absolute parent directory and\nMINUTES_SKILLS_ROOT to the absolute parent of MINUTES_SKILL_ROOT. Do this before\nrunning a helper. Do not infer either path from the current working directory\nor a Git checkout. The bundled runtime is under MINUTES_SKILLS_ROOT/_runtime.\n\n`;
    const localHostNote = `## Local Minutes Host\n\nThis plugin runs in ChatGPT Work or Codex with access to the local computer.\nUse the connected Minutes MCP tools when they cover the requested operation;\nresolve their exact registered names from the host's tool list. CLI commands\nand bundled helpers require a local shell on the same computer as Minutes.\nIf that runtime is unavailable, report the missing capability instead of\nfabricating results or treating a command as executed. Keep capture, audio\nprocessing, dictation insertion, and OS permissions in the local Minutes engine.\nThe plugin does not capture audio in ChatGPT's browser or upload a library.\nTool results used as model context are shared with the AI host. Preserve the\ncanonical skill's meeting-access, confirmation, and external-send rules.\n\n`;
    const body = rendered.body.replace(/## Skill Path\n\nBefore running helper scripts or opening bundled references, set:\n\n```bash\n[\s\S]*?```\n\n/, rootNote).replace(/(---\n[\s\S]*?\n---\n)/, `$1\n${localHostNote}`);
    if (body.includes('$(git rev-parse --show-toplevel)')) throw new Error(`Unresolved repository path in packaged skill ${name}`);
    artifacts.set(`${outputDir}/SKILL.md`, `${body.trimEnd()}\n`);
    for (const sidecar of rendered.sidecarFiles) {
      artifacts.set(path.join(outputDir, path.relative(path.dirname(rendered.outputPath), sidecar.relativePath)), sidecar.content);
    }
    for (const asset of rendered.assetFiles) {
      const absolute = await resolveSkillAssetSourcePath(source, asset.sourceRelativePath);
      artifacts.set(path.join(outputDir, asset.sourceRelativePath), await readFile(absolute, "utf8"));
    }
  }
  for (const name of ["minutes-learn.mjs", "minutes-learn-cli.mjs"]) {
    artifacts.set(`${OPENAI_PLUGIN_ROOT}/skills/_runtime/hooks/lib/${name}`, await readFile(path.join(rootDir, "..", "..", ".claude/plugins/minutes/hooks/lib", name), "utf8"));
  }
  return artifacts;
}
