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

function buildLlmsTxt({ manifest, resources }) {
  const generatedOn = new Date().toISOString().slice(0, 10);
  const installCommand = "npx minutes-mcp";
  const longDescription = manifest.long_description.split("\n\n")[0].trim();

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

  const promptLines = manifest.prompts
    .map(
      (prompt) =>
        `- \`${prompt.name}\` — ${prompt.description} Docs: ${mcpToolsBaseUrl}#prompt-${anchorSlug(prompt.name)}`
    )
    .join("\n");

  return `# minutes

> Generated file. Do not edit by hand.
> Source: manifest.json + crates/mcp/src/index.ts
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

function buildLlmsFull({ manifest, resources }) {
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

  return `# minutes — full agent reference

> Generated file. Do not edit by hand.
> Source: manifest.json + crates/mcp/src/index.ts
> Last generated: ${generatedOn}

## Product

${manifest.long_description}

## Canonical entry points

- Website: ${manifest.homepage}
- Agent entry point: ${manifest.documentation}
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

  const tools = manifest.tools
    .map(
      (tool) =>
        `<a id="tool-${anchorSlug(tool.name)}"></a>\n\n## \`${tool.name}\`\n\n${tool.description}\n\nReference URL: ${mcpToolsBaseUrl}#tool-${anchorSlug(tool.name)}`
    )
    .join("\n\n");

  const resourceDocs = resources
    .map(
      (resource) =>
        `<a id="resource-${anchorSlug(resource.name)}"></a>\n\n## \`${resource.uri}\`\n\n${resource.description}\n\nReference URL: ${mcpToolsBaseUrl}#resource-${anchorSlug(resource.name)}`
    )
    .join("\n\n");

  const prompts = manifest.prompts
    .map(
      (prompt) =>
        `<a id="prompt-${anchorSlug(prompt.name)}"></a>\n\n## \`${prompt.name}\`\n\n${prompt.description}\n\nReference URL: ${mcpToolsBaseUrl}#prompt-${anchorSlug(prompt.name)}`
    )
    .join("\n\n");

  return `# Minutes MCP tools

> Generated file. Do not edit by hand.
> Source: manifest.json + crates/mcp/src/index.ts
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

${tools}

## Resources

${resourceDocs}

## Prompt templates

${prompts}
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
        anchorId: `tool-${anchorSlug(tool.name)}`,
        docsUrl: `${mcpToolsBaseUrl}#tool-${anchorSlug(tool.name)}`,
      })),
      resources: resources.map((resource) => ({
        ...resource,
        anchorId: `resource-${anchorSlug(resource.name)}`,
        docsUrl: `${mcpToolsBaseUrl}#resource-${anchorSlug(resource.name)}`,
      })),
      prompts: manifest.prompts.map((prompt) => ({
        ...prompt,
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
  const mcpSource = await readFile(mcpSourcePath, "utf8");
  const resources = parseResources(mcpSource);
  const errorEntries = (await parseErrorCatalog()).map(classifyErrorEntry);

  if (resources.length === 0) {
    throw new Error("Failed to extract MCP resources from crates/mcp/src/index.ts");
  }
  if (errorEntries.length === 0) {
    throw new Error("Failed to extract error definitions from crates/core");
  }

  const next = buildLlmsTxt({ manifest, resources });
  const nextFull = buildLlmsFull({ manifest, resources });
  const nextMcpToolsMarkdown = buildMcpToolsMarkdown({ manifest, resources });
  const nextMcpToolsData = buildMcpToolsData({ manifest, resources });
  const nextErrorsMarkdown = buildErrorsMarkdown(errorEntries);
  const nextErrorsData = buildErrorsData(errorEntries);

  if (checkMode) {
    const current = await readFile(llmsPath, "utf8");
    const currentFull = await readFile(llmsFullPath, "utf8");
    const currentMcpToolsMarkdown = await readFile(mcpToolsMarkdownPath, "utf8");
    const currentMcpToolsData = await readFile(mcpToolsDataPath, "utf8");
    const currentErrorsMarkdown = await readFile(errorsMarkdownPath, "utf8");
    const currentErrorsData = await readFile(errorsDataPath, "utf8");

    if (
      current !== next ||
      currentFull !== nextFull ||
      currentMcpToolsMarkdown !== nextMcpToolsMarkdown ||
      currentMcpToolsData !== nextMcpToolsData ||
      currentErrorsMarkdown !== nextErrorsMarkdown ||
      currentErrorsData !== nextErrorsData
    ) {
      console.error(
        "Generated agent docs are stale. Run: node scripts/generate_llms_txt.mjs"
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
