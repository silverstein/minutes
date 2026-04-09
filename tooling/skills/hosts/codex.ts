import type { HostConfig } from "../schema.js";

export const codexHost: HostConfig = {
  name: "codex",
  displayName: "OpenAI Codex",
  outputRoot: ".agents/skills/minutes",
  pathPolicy: {
    defaultSkillDir: ".",
    pathRewrites: [
      { from: ".claude/plugins/minutes", to: ".agents/skills/minutes" },
    ],
  },
  frontmatterPolicy: {
    mode: "allowlist",
    keepFields: ["name", "description"],
  },
  descriptionPolicy: {
    maxLength: 1024,
    onOverflow: "error",
  },
  metadataPolicy: {
    generateSidecar: true,
    format: "openai.yaml",
    relativeDir: "agents",
  },
  transformPolicy: {
    extraNotesPlacement: "append",
  },
  assetPolicy: {
    mode: "copy",
  },
};
