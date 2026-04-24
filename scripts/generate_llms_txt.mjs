#!/usr/bin/env node

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = dirname(__dirname);
const manifestPath = join(repoRoot, "manifest.json");
const mcpSourcePath = join(repoRoot, "crates", "mcp", "src", "index.ts");
const llmsPath = join(repoRoot, "site", "public", "llms.txt");
const llmsFullPath = join(repoRoot, "site", "public", "llms-full.txt");
const productSurfacesPath = join(repoRoot, "site", "lib", "product-surfaces.json");
const skillsCatalogPath = join(repoRoot, "site", "lib", "skills-catalog.json");
const forAgentsBaseUrl = "https://useminutes.app/for-agents";
const mcpToolsMarkdownPath = join(repoRoot, "site", "public", "docs", "mcp", "tools.md");
const mcpToolsDataPath = join(repoRoot, "site", "app", "docs", "mcp", "tools", "data.json");
const errorsMarkdownPath = join(repoRoot, "site", "public", "docs", "errors.md");
const errorsDataPath = join(repoRoot, "site", "app", "docs", "errors", "data.json");
const mcpToolsBaseUrl = "https://useminutes.app/docs/mcp/tools";
const errorsBaseUrl = "https://useminutes.app/docs/errors";
const rustErrorSourcePaths = [
  join(repoRoot, "crates", "core", "src", "error.rs"),
  join(repoRoot, "crates", "core", "src", "graph.rs"),
  join(repoRoot, "crates", "core", "src", "voice.rs"),
];

const toolGroupOrder = [
  "Recording",
  "Search and recall",
  "People and relationships",
  "Insights",
  "Live and dictation",
  "Notes and processing",
  "Voice and speaker ID",
  "Integration",
];

function extractQuotedValue(input) {
  const match = input.match(/"((?:[^"\\]|\\.)*)"/);
  return match ? match[1].replace(/\\"/g, "\"") : null;
}

function parseResources(source) {
  const resources = [];

  const appResourcePattern =
    /registerAppResource\(\s*server,\s*"([^"]+)",\s*([A-Z0-9_":/.{}-]+),\s*\{\s*description:\s*"([^"]+)"\s*\}/gs;
  for (const match of source.matchAll(appResourcePattern)) {
    const [, name, rawUri, description] = match;
    const uri = rawUri === "UI_RESOURCE_URI" ? "ui://minutes/dashboard" : rawUri.replace(/"/g, "");
    resources.push({ name, uri, description });
  }

  const directResourcePattern =
    /server\.resource\(\s*"([^"]+)",\s*"([^"]+)",\s*\{\s*description:\s*"([^"]+)"\s*\}/gs;
  for (const match of source.matchAll(directResourcePattern)) {
    const [, name, uri, description] = match;
    resources.push({ name, uri, description });
  }

  const templateResourcePattern =
    /server\.resource\(\s*"([^"]+)",\s*new ResourceTemplate\("([^"]+)",[\s\S]*?\),\s*\{\s*description:\s*"([^"]+)"\s*\}/gs;
  for (const match of source.matchAll(templateResourcePattern)) {
    const [, name, uri, description] = match;
    resources.push({ name, uri, description });
  }

  const seen = new Set();
  return resources.filter((resource) => {
    const key = `${resource.name}:${resource.uri}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function anchorSlug(value) {
  return String(value)
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function decodeRustStringLiterals(source) {
  const matches = [...source.matchAll(/"((?:[^"\\]|\\.)*)"/g)];
  return matches
    .map((match) =>
      match[1]
        .replace(/\\n/g, "\n")
        .replace(/\\"/g, "\"")
        .replace(/\\\\/g, "\\")
    )
    .join("");
}

function parseRustErrorDefinitions(source, sourceFile) {
  const lines = source.split("\n");
  const entries = [];
  let currentEnum = null;
  let braceDepth = 0;
  let pendingCfg = null;
  let pendingErrorAttr = null;
  let collectingErrorAttr = false;
  let errorAttrBuffer = [];

  for (const line of lines) {
    const enumMatch = line.match(/pub enum ([A-Za-z0-9_]+)\s*\{/);
    if (enumMatch) {
      currentEnum = enumMatch[1];
      braceDepth = 1;
      pendingCfg = null;
      pendingErrorAttr = null;
      continue;
    }

    if (!currentEnum) continue;

    braceDepth += (line.match(/\{/g) || []).length;
    braceDepth -= (line.match(/\}/g) || []).length;
    if (braceDepth <= 0) {
      currentEnum = null;
      pendingCfg = null;
      pendingErrorAttr = null;
      collectingErrorAttr = false;
      errorAttrBuffer = [];
      continue;
    }

    const cfgMatch = line.match(/#\[cfg\((.+)\)\]/);
    if (cfgMatch) {
      pendingCfg = cfgMatch[1].trim();
      continue;
    }

    if (line.includes("#[error(")) {
      collectingErrorAttr = true;
      errorAttrBuffer = [line];
      if (line.includes(")]")) {
        collectingErrorAttr = false;
        pendingErrorAttr = decodeRustStringLiterals(errorAttrBuffer.join("\n"));
        errorAttrBuffer = [];
      }
      continue;
    }

    if (collectingErrorAttr) {
      errorAttrBuffer.push(line);
      if (line.includes(")]")) {
        collectingErrorAttr = false;
        pendingErrorAttr = decodeRustStringLiterals(errorAttrBuffer.join("\n"));
        errorAttrBuffer = [];
      }
      continue;
    }

    if (pendingErrorAttr) {
      const variantMatch = line.match(/^\s*([A-Z][A-Za-z0-9_]*)\s*(?:\(|\{|,)/);
      if (variantMatch) {
        const variant = variantMatch[1];
        const cfgSuffix = pendingCfg ? `-${anchorSlug(pendingCfg)}` : "";
        entries.push({
          id: `${anchorSlug(currentEnum)}-${anchorSlug(variant)}${cfgSuffix}`,
          enumName: currentEnum,
          variant,
          cfg: pendingCfg,
          message: pendingErrorAttr.trim(),
          sourceFile,
        });
        pendingErrorAttr = null;
        pendingCfg = null;
      }
    }
  }

  return entries;
}

async function parseErrorCatalog() {
  const allEntries = [];
  for (const sourcePath of rustErrorSourcePaths) {
    const source = await readFile(sourcePath, "utf8");
    allEntries.push(...parseRustErrorDefinitions(source, sourcePath.replace(`${repoRoot}/`, "")));
  }
  return allEntries;
}

function classifyErrorEntry(entry) {
  const lowSignalVariants = new Set(["Io", "Sqlite", "Other"]);
  return {
    ...entry,
    hidden: lowSignalVariants.has(entry.variant),
  };
}

function categorizeTool(name) {
  const groups = {
    Recording: new Set(["start_recording", "stop_recording", "get_status", "list_processing_jobs"]),
    "Search and recall": new Set([
      "list_meetings",
      "get_meeting",
      "search_meetings",
      "activity_summary",
      "search_context",
      "get_moment",
      "research_topic",
    ]),
    "People and relationships": new Set(["get_person_profile", "relationship_map", "track_commitments", "consistency_report"]),
    Insights: new Set(["get_meeting_insights", "ingest_meeting", "knowledge_status"]),
    "Live and dictation": new Set(["start_live_transcript", "read_live_transcript", "start_dictation", "stop_dictation"]),
    "Notes and processing": new Set(["add_note", "process_audio", "open_dashboard"]),
    "Voice and speaker ID": new Set(["list_voices", "confirm_speaker"]),
    Integration: new Set(["qmd_collection_status", "register_qmd_collection"]),
  };

  for (const [group, names] of Object.entries(groups)) {
    if (names.has(name)) return group;
  }
  return "Other";
}

function categorizeResource(name) {
  const groups = {
    Dashboard: new Set(["Minutes Dashboard"]),
    Status: new Set(["recording_status", "recent_events"]),
    Meetings: new Set(["recent_meetings", "meeting"]),
    Memory: new Set(["open_actions", "recent-ideas"]),
  };

  for (const [group, names] of Object.entries(groups)) {
    if (names.has(name)) return group;
  }
  return "Other";
}

function categorizePrompt(name) {
  const groups = {
    Prep: new Set(["meeting_prep", "person_briefing", "topic_research"]),
    Review: new Set(["weekly_review", "find_action_items"]),
    Capture: new Set(["start_meeting"]),
  };

  for (const [group, names] of Object.entries(groups)) {
    if (names.has(name)) return group;
  }
  return "Other";
}

function buildLlmsTxt({ manifest, resources, surfaces, skillsCatalog }) {
  const generatedOn = new Date().toISOString().slice(0, 10);
  const installCommand = "npx minutes-mcp";
  const longDescription = manifest.long_description.split("\n\n")[0].trim();

  const toolLines = manifest.tools
    .map(
      (tool) =>
        `- \`${tool.name}\` — ${tool.description} Docs: ${mcpToolsBaseUrl}#tool-${anchorSlug(tool.name)}`
    )
    .join("\n");

  const skillLines = skillsCatalog
    .map((skill) => {
      // Concise llms.txt uses one line per skill; use bestFor (always short) as
      // the headline instead of shortDescription (which may embed trigger
      // phrases and run hundreds of characters).
      return `- \`/${skill.name}\` — ${skill.bestFor} Category: ${skill.category}. Example: \`${skill.example}\`. Docs: ${forAgentsBaseUrl}#${skill.name}`;
    })
    .join("\n");

  const resourceLines = resources
    .map(
      (resource) =>
        `- \`${resource.uri}\` — ${resource.description} Docs: ${mcpToolsBaseUrl}#resource-${anchorSlug(resource.name)}`
    )
    .join("\n");

  const promptLines = manifest.prompts
    .map(
      (prompt) =>
        `- \`${prompt.name}\` — ${prompt.description} Docs: ${mcpToolsBaseUrl}#prompt-${anchorSlug(prompt.name)}`
    )
    .join("\n");
  const surfaceLines = surfaces
    .map(
      (surface) =>
        `- ${surface.name} — When: ${surface.when} Install: \`${surface.install}\` Best for: ${surface.activation}`
    )
    .join("\n");

  return `# minutes

> Generated file. Do not edit by hand.
> Source: manifest.json + crates/mcp/src/index.ts + site/lib/skills-catalog.json
> Last generated: ${generatedOn}

${longDescription}

## Key Facts

- License: ${manifest.license}
- Languages: Rust (core engine), TypeScript (MCP server)
- Platforms: ${manifest.compatibility.platforms.join(", ")}
- Version: ${manifest.version}
- Source: ${manifest.repository.url}
- Website: ${manifest.homepage}
- Privacy: ${manifest.privacy_policies[0]}

## For AI Agents

minutes exposes a standard MCP server with ${manifest.tools.length} tools, ${resources.length} resources, and ${manifest.prompts.length} prompt templates. Any MCP-compatible client can use it as a conversation memory layer.

## Choose Your Surface

${surfaceLines}

Recommended install:

\`\`\`json
{
  "mcpServers": {
    "minutes": {
      "command": "npx",
      "args": ["minutes-mcp"]
    }
  }
}
\`\`\`

## MCP Tools

${toolLines}

## MCP Resources

${resourceLines}

## Prompt Templates

${promptLines}

## Claude Code Plugin Skills

Workflow-level skills that wrap MCP tools into operator motions. Install in Claude Code via \`claude plugin marketplace add silverstein/minutes\` then \`/plugin install minutes@minutes\`. The same skills ship as a portable pack at \`.agents/skills/minutes/\` for Codex / Gemini CLI and at \`.opencode/skills/\` + \`.opencode/commands/\` for OpenCode.

${skillLines}

## Output Format

Meetings are stored as markdown with YAML frontmatter:

\`\`\`yaml
---
title: Q2 Pricing Discussion
type: meeting
date: 2026-03-17T14:00:00
duration: 42m
attendees: [Alex K., Jordan M.]
action_items:
  - assignee: mat
    task: Send pricing doc
    due: Friday
    status: open
decisions:
  - text: Run pricing experiment at monthly billing
    topic: pricing
---
\`\`\`

## Capabilities For Agents

1. Meeting recall — Search and retrieve past meetings, memos, and transcripts.
2. Relationship memory — Build person profiles, find commitments, and detect losing-touch risk.
3. Decision and action-item tracking — Query structured decisions, commitments, and open follow-ups.
4. Recording and live transcript control — Start or stop capture and read live transcript deltas.
5. Local-first context — Audio processing happens on-device and the durable output is inspectable markdown.

## Documentation

- Agent entry point: ${manifest.documentation}
- Proof and eval caveats: ${manifest.homepage}/proof
- Full agent index: ${manifest.homepage}/llms-full.txt
- MCP tools reference: ${mcpToolsBaseUrl}
- MCP tools markdown: ${mcpToolsBaseUrl}.md
- Repository: ${manifest.repository.url}
- MCP server package: https://www.npmjs.com/package/minutes-mcp
- SDK package: https://www.npmjs.com/package/minutes-sdk
- Support: https://github.com/silverstein/minutes/discussions

## Notes

- This file is intentionally concise for retrieval.
- Public reference docs should eventually live at stable \`/docs\` and \`/docs/*.md\` URLs.
- Install command: \`${installCommand}\`
`;
}

function buildLlmsFull({ manifest, resources, surfaces, skillsCatalog }) {
  const generatedOn = new Date().toISOString().slice(0, 10);
  const toolLines = manifest.tools
    .map(
      (tool) =>
        `- \`${tool.name}\` — ${tool.description} Docs: ${mcpToolsBaseUrl}#tool-${anchorSlug(tool.name)}`
    )
    .join("\n");
  const resourceLines = resources
    .map(
      (resource) =>
        `- \`${resource.uri}\` — ${resource.description} Docs: ${mcpToolsBaseUrl}#resource-${anchorSlug(resource.name)}`
    )
    .join("\n");
  const surfaceLines = surfaces
    .map(
      (surface) =>
        `- ${surface.name}\n  - When: ${surface.when}\n  - Install: \`${surface.install}\`\n  - Best for: ${surface.activation}\n  - Notes: ${surface.note}`
    )
    .join("\n");
  const skillsByCategory = skillsCatalog.reduce((acc, skill) => {
    (acc[skill.category] ??= []).push(skill);
    return acc;
  }, {});
  const skillGroups = Object.keys(skillsByCategory)
    .sort()
    .map((category) => {
      const lines = skillsByCategory[category]
        .map(
          (skill) =>
            `- \`/${skill.name}\` — ${skill.bestFor}\n  - Description: ${skill.shortDescription}\n  - Example: \`${skill.example}\`\n  - Docs: ${forAgentsBaseUrl}#${skill.name}`
        )
        .join("\n");
      return `### ${category}\n\n${lines}`;
    })
    .join("\n\n");

  return `# minutes — full agent reference

> Generated file. Do not edit by hand.
> Source: manifest.json + crates/mcp/src/index.ts + site/lib/skills-catalog.json
> Last generated: ${generatedOn}

## Product

${manifest.long_description}

## Choose your surface

${surfaceLines}

## Canonical entry points

- Website: ${manifest.homepage}
- Agent entry point: ${manifest.documentation}
- Proof and eval caveats: ${manifest.homepage}/proof
- Concise agent index: ${manifest.homepage}/llms.txt
- MCP tools reference (HTML): ${manifest.homepage}/docs/mcp/tools
- MCP tools reference (Markdown): ${manifest.homepage}/docs/mcp/tools.md
- Error reference (HTML): ${manifest.homepage}/docs/errors
- Error reference (Markdown): ${manifest.homepage}/docs/errors.md
- Support: ${manifest.support}

## Tool surface

${toolLines}

## Resource surface

${resourceLines}

## Claude Code Plugin skill surface

Workflow-level skills that wrap MCP tools into operator motions. Install in Claude Code via \`claude plugin marketplace add silverstein/minutes\` then \`/plugin install minutes@minutes\`. The same skills ship as a portable pack at \`.agents/skills/minutes/\` for Codex / Gemini CLI and at \`.opencode/skills/\` + \`.opencode/commands/\` for OpenCode. New skills must declare \`metadata.site_category\`, \`metadata.site_example\`, and \`metadata.site_best_for\` in \`tooling/skills/sources/<name>/skill.md\`.

${skillGroups}
`;
}

function buildErrorsMarkdown(entries) {
  const generatedOn = new Date().toISOString().slice(0, 10);
  const visibleEntries = entries.filter((entry) => !entry.hidden);
  const hiddenCount = entries.length - visibleEntries.length;
  const grouped = Object.entries(
    visibleEntries.reduce((acc, entry) => {
      acc[entry.enumName] ||= [];
      acc[entry.enumName].push(entry);
      return acc;
    }, {})
  );

  const sections = grouped
    .map(([enumName, groupEntries]) => {
      const block = groupEntries
        .map((entry) => {
          const cfgLine = entry.cfg ? `\n\nPlatform condition: \`${entry.cfg}\`` : "";
          return `<a id="error-${entry.id}"></a>\n\n## \`${entry.enumName}::${entry.variant}\`\n\nExact message:\n\n> ${entry.message.replace(/\n/g, "\n> ")}${cfgLine}\n\nSource: \`${entry.sourceFile}\`\n\nReference URL: ${errorsBaseUrl}#error-${entry.id}`;
        })
        .join("\n\n");

      return `# ${enumName}\n\n${block}`;
    })
    .join("\n\n")
    .trim();

  return `# Minutes error reference

> Generated file. Do not edit by hand.
> Source: crates/core thiserror definitions
> Last generated: ${generatedOn}

This is the generated public catalog of stable Minutes core errors. It intentionally favors actionable, user-facing errors over generic wrapper variants.

- Visible actionable errors: ${visibleEntries.length}
- Hidden low-signal wrappers: ${hiddenCount}

${sections}
`;
}

function buildErrorsData(entries) {
  const visibleEntries = entries.filter((entry) => !entry.hidden);
  const hiddenEntries = entries.filter((entry) => entry.hidden);
  const groups = Object.entries(
    visibleEntries.reduce((acc, entry) => {
      acc[entry.enumName] ||= [];
      acc[entry.enumName].push(entry);
      return acc;
    }, {})
  ).map(([enumName, groupEntries]) => ({
    enumName,
    count: groupEntries.length,
    entries: groupEntries.map((entry) => ({
      ...entry,
      anchorId: `error-${entry.id}`,
      docsUrl: `${errorsBaseUrl}#error-${entry.id}`,
    })),
  }));

  return JSON.stringify(
    {
      generatedAt: new Date().toISOString().slice(0, 10),
      visibleCount: visibleEntries.length,
      hiddenCount: hiddenEntries.length,
      groups,
      hiddenEnums: hiddenEntries.map((entry) => ({
        enumName: entry.enumName,
        variant: entry.variant,
        sourceFile: entry.sourceFile,
      })),
    },
    null,
    2
  );
}

function buildMcpToolsMarkdown({ manifest, resources }) {
  const generatedOn = new Date().toISOString().slice(0, 10);

  const toolGroups = Object.entries(
    manifest.tools.reduce((acc, tool) => {
      const group = categorizeTool(tool.name);
      acc[group] ||= [];
      acc[group].push(tool);
      return acc;
    }, {})
  )
    .sort(([a], [b]) => toolGroupOrder.indexOf(a) - toolGroupOrder.indexOf(b))
    .map(
      ([group, tools]) =>
        `### ${group}\n\n` +
        tools
          .map(
            (tool) =>
              `<a id="tool-${anchorSlug(tool.name)}"></a>\n\n#### \`${tool.name}\`\n\n${tool.description}\n\nReference URL: ${mcpToolsBaseUrl}#tool-${anchorSlug(tool.name)}`
          )
          .join("\n\n")
    )
    .join("\n\n");

  const resourceGroups = Object.entries(
    resources.reduce((acc, resource) => {
      const group = categorizeResource(resource.name);
      acc[group] ||= [];
      acc[group].push(resource);
      return acc;
    }, {})
  )
    .map(
      ([group, resourceEntries]) =>
        `### ${group}\n\n` +
        resourceEntries
          .map(
            (resource) =>
              `<a id="resource-${anchorSlug(resource.name)}"></a>\n\n#### \`${resource.uri}\`\n\n${resource.description}\n\nReference URL: ${mcpToolsBaseUrl}#resource-${anchorSlug(resource.name)}`
          )
          .join("\n\n")
    )
    .join("\n\n");

  const promptGroups = Object.entries(
    manifest.prompts.reduce((acc, prompt) => {
      const group = categorizePrompt(prompt.name);
      acc[group] ||= [];
      acc[group].push(prompt);
      return acc;
    }, {})
  )
    .map(
      ([group, prompts]) =>
        `### ${group}\n\n` +
        prompts
          .map(
            (prompt) =>
              `<a id="prompt-${anchorSlug(prompt.name)}"></a>\n\n#### \`${prompt.name}\`\n\n${prompt.description}\n\nReference URL: ${mcpToolsBaseUrl}#prompt-${anchorSlug(prompt.name)}`
          )
          .join("\n\n")
    )
    .join("\n\n");

  return `# Minutes MCP tools

> Generated file. Do not edit by hand.
> Source: manifest.json + crates/mcp/src/index.ts
> Regenerate: node scripts/generate_llms_txt.mjs
> Last generated: ${generatedOn}

Minutes exposes ${manifest.tools.length} tools, ${resources.length} resources, and ${manifest.prompts.length} prompt templates through the MCP server.

## Install

\`\`\`json
{
  "mcpServers": {
    "minutes": {
      "command": "npx",
      "args": ["minutes-mcp"]
    }
  }
}
\`\`\`

## Tools

${toolGroups}

## Resources

${resourceGroups}

## Prompt templates

${promptGroups}
`;
}

function buildMcpToolsData({ manifest, resources }) {
  return JSON.stringify(
    {
      generatedAt: new Date().toISOString().slice(0, 10),
      installCommand: "npx minutes-mcp",
      documentationUrl: manifest.documentation,
      supportUrl: manifest.support,
      tools: manifest.tools.map((tool) => ({
        ...tool,
        group: categorizeTool(tool.name),
        anchorId: `tool-${anchorSlug(tool.name)}`,
        docsUrl: `${mcpToolsBaseUrl}#tool-${anchorSlug(tool.name)}`,
      })),
      resources: resources.map((resource) => ({
        ...resource,
        group: categorizeResource(resource.name),
        anchorId: `resource-${anchorSlug(resource.name)}`,
        docsUrl: `${mcpToolsBaseUrl}#resource-${anchorSlug(resource.name)}`,
      })),
      prompts: manifest.prompts.map((prompt) => ({
        ...prompt,
        group: categorizePrompt(prompt.name),
        anchorId: `prompt-${anchorSlug(prompt.name)}`,
        docsUrl: `${mcpToolsBaseUrl}#prompt-${anchorSlug(prompt.name)}`,
      })),
    },
    null,
    2
  );
}

async function main() {
  const checkMode = process.argv.includes("--check");

  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const surfaces = JSON.parse(await readFile(productSurfacesPath, "utf8"));
  const skillsCatalog = JSON.parse(await readFile(skillsCatalogPath, "utf8"));
  const mcpSource = await readFile(mcpSourcePath, "utf8");
  const resources = parseResources(mcpSource);
  const errorEntries = (await parseErrorCatalog()).map(classifyErrorEntry);

  if (manifest.tools.length === 0) {
    throw new Error("manifest.json contains no tools");
  }
  if (resources.length === 0) {
    throw new Error("Failed to extract MCP resources from crates/mcp/src/index.ts");
  }
  if (errorEntries.length === 0) {
    throw new Error("Failed to extract error definitions from crates/core");
  }
  if (!Array.isArray(skillsCatalog) || skillsCatalog.length === 0) {
    throw new Error(
      `${skillsCatalogPath} is empty or missing. Run: npm --prefix tooling/skills run compile`
    );
  }
  const malformedSkills = skillsCatalog.filter(
    (skill) => !skill.name || !skill.category || !skill.shortDescription || !skill.example || !skill.bestFor
  );
  if (malformedSkills.length > 0) {
    throw new Error(
      `Skill catalog entries missing required fields: ${malformedSkills
        .map((s) => s.name ?? "<unnamed>")
        .join(", ")}. Regenerate with: npm --prefix tooling/skills run compile`
    );
  }

  const uncategorizedTools = manifest.tools.filter((t) => categorizeTool(t.name) === "Other");
  if (uncategorizedTools.length > 0) {
    throw new Error(
      `Uncategorized tools: ${uncategorizedTools.map((t) => t.name).join(", ")}. ` +
      `Update categorizeTool() in generate_llms_txt.mjs.`
    );
  }
  const uncategorizedResources = resources.filter((r) => categorizeResource(r.name) === "Other");
  if (uncategorizedResources.length > 0) {
    throw new Error(
      `Uncategorized resources: ${uncategorizedResources.map((r) => r.name).join(", ")}. ` +
      `Update categorizeResource() in generate_llms_txt.mjs.`
    );
  }
  const uncategorizedPrompts = manifest.prompts.filter((p) => categorizePrompt(p.name) === "Other");
  if (uncategorizedPrompts.length > 0) {
    throw new Error(
      `Uncategorized prompts: ${uncategorizedPrompts.map((p) => p.name).join(", ")}. ` +
      `Update categorizePrompt() in generate_llms_txt.mjs.`
    );
  }

  const next = buildLlmsTxt({ manifest, resources, surfaces, skillsCatalog });
  const nextFull = buildLlmsFull({ manifest, resources, surfaces, skillsCatalog });
  const nextMcpToolsMarkdown = buildMcpToolsMarkdown({ manifest, resources });
  const nextMcpToolsData = buildMcpToolsData({ manifest, resources });
  const nextErrorsMarkdown = buildErrorsMarkdown(errorEntries);
  const nextErrorsData = buildErrorsData(errorEntries);

  if (checkMode) {
    // Ignore the current date when comparing — the generator embeds
    // `new Date()` in every output, so a pure date-only diff would fail
    // `--check` on any day where nothing else actually changed.
    const stripGeneratedDate = (content) =>
      content
        .replace(/^> Last generated: \d{4}-\d{2}-\d{2}$/m, "> Last generated: <date>")
        .replace(/"generatedAt": "\d{4}-\d{2}-\d{2}"/, '"generatedAt": "<date>"');

    const compare = async (actualPath, expected) => {
      const current = await readFile(actualPath, "utf8");
      return stripGeneratedDate(current) !== stripGeneratedDate(expected);
    };

    const staleFiles = [];
    if (await compare(llmsPath, next)) staleFiles.push(llmsPath);
    if (await compare(llmsFullPath, nextFull)) staleFiles.push(llmsFullPath);
    if (await compare(mcpToolsMarkdownPath, nextMcpToolsMarkdown))
      staleFiles.push(mcpToolsMarkdownPath);
    if (await compare(mcpToolsDataPath, nextMcpToolsData))
      staleFiles.push(mcpToolsDataPath);
    if (await compare(errorsMarkdownPath, nextErrorsMarkdown))
      staleFiles.push(errorsMarkdownPath);
    if (await compare(errorsDataPath, nextErrorsData))
      staleFiles.push(errorsDataPath);

    if (staleFiles.length > 0) {
      console.error(
        `Generated agent docs are stale. Run: node scripts/generate_llms_txt.mjs\nStale:\n${staleFiles
          .map((f) => `  - ${f}`)
          .join("\n")}`
      );
      process.exit(1);
    }
    console.log("Generated agent docs are up to date.");
    return;
  }

  await mkdir(dirname(llmsPath), { recursive: true });
  await mkdir(dirname(llmsFullPath), { recursive: true });
  await mkdir(dirname(mcpToolsMarkdownPath), { recursive: true });
  await mkdir(dirname(mcpToolsDataPath), { recursive: true });
  await mkdir(dirname(errorsMarkdownPath), { recursive: true });
  await mkdir(dirname(errorsDataPath), { recursive: true });

  await writeFile(llmsPath, next, "utf8");
  await writeFile(llmsFullPath, nextFull, "utf8");
  await writeFile(mcpToolsMarkdownPath, nextMcpToolsMarkdown, "utf8");
  await writeFile(mcpToolsDataPath, nextMcpToolsData, "utf8");
  await writeFile(errorsMarkdownPath, nextErrorsMarkdown, "utf8");
  await writeFile(errorsDataPath, nextErrorsData, "utf8");
  console.log(`Updated ${llmsPath}`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
