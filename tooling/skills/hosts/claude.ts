import type { HostConfig } from "../schema.js";

export const claudeHost: HostConfig = {
  name: "claude",
  displayName: "Claude Code Plugin",
  outputRoot: ".claude/plugins/minutes",
  pathPolicy: {
    defaultSkillDir: "skills",
    pathRewrites: [],
  },
  frontmatterPolicy: {
    mode: "denylist",
    stripFields: [
      "triggers",
      "phase",
      "summary",
      "assets",
      "host_overrides",
      "output",
      "tests",
      "tags",
    ],
  },
  descriptionPolicy: {
    maxLength: null,
    onOverflow: "error",
  },
  metadataPolicy: {
    generateSidecar: false,
    format: null,
    relativeDir: null,
  },
  transformPolicy: {
    extraNotesPlacement: "append",
  },
  assetPolicy: {
    mode: "copy",
  },
};
