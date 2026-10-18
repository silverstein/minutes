#!/usr/bin/env node

import { readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = dirname(__dirname);
const pluginPath = join(repoRoot, ".claude", "plugins", "minutes", "plugin.json");
const packsDir = join(repoRoot, ".claude", "plugins", "minutes", "packs");
const outputPath = join(repoRoot, ".claude", "plugins", "minutes", "skill-metadata.generated.json");

function inferCategory(skillName) {
  if (["minutes-record", "minutes-note", "minutes-list", "minutes-recap", "minutes-cleanup", "minutes-verify", "minutes-setup"].includes(skillName)) return "capture";
  if (
    [
      "minutes-brief",
      "minutes-prep",
      "minutes-debrief",
      "minutes-weekly",
      "minutes-x1-closeout",
      "minutes-x1-send-meeting",
    ].includes(skillName)
  )
    return "lifecycle";
  if (["minutes-tag", "minutes-mirror"].includes(skillName)) return "coaching";
  if (["minutes-ideas", "minutes-ingest", "minutes-lint"].includes(skillName)) return "knowledge";
  if (["minutes-search", "minutes-graph"].includes(skillName)) return "intelligence";
  return "other";
}

async function readSkillDescription(skillPath) {
  const raw = await readFile(skillPath, "utf8");
  const match = raw.match(/^description:\s*(.+)$/m);
  return match ? match[1].trim() : "";
}

async function main() {
  const checkMode = process.argv.includes("--check");
  const plugin = JSON.parse(await readFile(pluginPath, "utf8"));
  const packFiles = (await readdir(packsDir))
    .filter((name) => name.endsWith(".json") && name !== "schema.json")
    .sort();
  const packs = await Promise.all(
    packFiles.map(async (name) => JSON.parse(await readFile(join(packsDir, name), "utf8")))
  );

  const metadata = [];
  for (const skill of plugin.skills || []) {
    const absoluteSkillPath = join(repoRoot, ".claude", "plugins", "minutes", skill.path);
    const description = await readSkillDescription(absoluteSkillPath);
    const packIds = packs
      .filter((pack) => Array.isArray(pack.skill_names) && pack.skill_names.includes(skill.name))
      .map((pack) => pack.pack_id)
      .sort();

    metadata.push({
      skill_name: skill.name,
      title: skill.name.replace(/^minutes-/, "").replace(/-/g, " "),
      description,
      category: inferCategory(skill.name),
      surface_support: ["claude-code-plugin"],
      installable_via: "claude-code-plugin",
      pack_ids: packIds,
      role_fit: packIds.length > 0 ? packIds : [],
      source_path: skill.path,
    });
  }

  const next = JSON.stringify(
    {
      generated_at: new Date().toISOString(),
      source: {
        plugin_json: ".claude/plugins/minutes/plugin.json",
        packs_dir: ".claude/plugins/minutes/packs",
      },
      skills: metadata,
    },
    null,
    2
  );

  if (checkMode) {
    const current = await readFile(outputPath, "utf8");
    const currentParsed = JSON.parse(current);
    const nextParsed = JSON.parse(next);
    currentParsed.generated_at = "IGNORED";
    nextParsed.generated_at = "IGNORED";
    if (JSON.stringify(currentParsed) !== JSON.stringify(nextParsed)) {
      console.error("Generated skill metadata is stale. Run: node scripts/generate_skill_metadata.mjs");
      process.exit(1);
    }
    console.log("Generated skill metadata is up to date.");
    return;
  }

  await writeFile(outputPath, next, "utf8");
  console.log(`updated ${outputPath}`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
