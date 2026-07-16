import { readdir } from "node:fs/promises";
import path from "node:path";
import type { CanonicalSkillSource } from "../schema.js";
import { HOSTS } from "../hosts/index.js";
import { renderSkillForHost } from "./render.js";

interface GeneratedArtifactScope {
  root: string;
  kind: "tree" | "opencode-commands";
}

const GENERATED_ARTIFACT_SCOPES: readonly GeneratedArtifactScope[] = [
  { root: ".claude/plugins/minutes", kind: "tree" },
  { root: ".agents/skills/minutes", kind: "tree" },
  { root: ".opencode/skills", kind: "tree" },
  { root: ".opencode/commands", kind: "opencode-commands" },
];

// These hand-maintained Claude plugin surfaces and mirrored runtime helpers are
// not rendered from an individual canonical skill. Keep every exception exact:
// an unexpected descendant of an allowed directory must still fail ownership.
export const GENERATED_ARTIFACT_ALLOWLIST = [
  ".claude/plugins/minutes/.claude-plugin",
  ".claude/plugins/minutes/.claude-plugin/plugin.json",
  ".claude/plugins/minutes/agents",
  ".claude/plugins/minutes/agents/meeting-analyst.md",
  ".claude/plugins/minutes/hooks",
  ".claude/plugins/minutes/hooks/lib",
  ".claude/plugins/minutes/hooks/lib/minutes-learn.mjs",
  ".claude/plugins/minutes/hooks/lib/minutes-learn-cli.mjs",
  ".claude/plugins/minutes/hooks/lib/proactive-context.mjs",
  ".claude/plugins/minutes/hooks/post-record.mjs",
  ".claude/plugins/minutes/hooks/session-reminder.mjs",
  ".claude/plugins/minutes/hooks/test",
  ".claude/plugins/minutes/hooks/test/minutes-learn.test.mjs",
  ".claude/plugins/minutes/hooks/test/post-record.test.mjs",
  ".claude/plugins/minutes/hooks/test/proactive-context.test.mjs",
  ".claude/plugins/minutes/packs",
  ".claude/plugins/minutes/packs/founder-weekly.json",
  ".claude/plugins/minutes/packs/README.md",
  ".claude/plugins/minutes/packs/relationship-intel.json",
  ".claude/plugins/minutes/packs/schema.json",
  ".claude/plugins/minutes/skill-metadata.generated.json",
  ".agents/skills/minutes/_runtime",
  ".agents/skills/minutes/_runtime/hooks",
  ".agents/skills/minutes/_runtime/hooks/lib",
  ".agents/skills/minutes/_runtime/hooks/lib/minutes-learn.mjs",
  ".agents/skills/minutes/_runtime/hooks/lib/minutes-learn-cli.mjs",
  ".opencode/skills/_runtime",
  ".opencode/skills/_runtime/hooks",
  ".opencode/skills/_runtime/hooks/lib",
  ".opencode/skills/_runtime/hooks/lib/minutes-learn.mjs",
  ".opencode/skills/_runtime/hooks/lib/minutes-learn-cli.mjs",
] as const;

function toRepoPath(value: string): string {
  return value.split(path.sep).join("/");
}

function addPathAndParents(paths: Set<string>, artifactPath: string): void {
  let current = toRepoPath(path.normalize(artifactPath));
  while (current !== "." && current !== "/") {
    paths.add(current);
    const parent = path.posix.dirname(current);
    if (parent === current) break;
    current = parent;
  }
}

function planOwnedArtifactPaths(skills: CanonicalSkillSource[]): Set<string> {
  const owned = new Set<string>(GENERATED_ARTIFACT_ALLOWLIST);

  // The manifest is generated once per compile, rather than once per skill.
  addPathAndParents(owned, ".claude/plugins/minutes/plugin.json");

  for (const skill of skills) {
    for (const host of Object.values(HOSTS)) {
      const artifact = renderSkillForHost(skill, host);
      addPathAndParents(owned, artifact.outputPath);
      for (const asset of artifact.assetFiles) {
        addPathAndParents(owned, asset.outputRelativePath);
      }
      for (const sidecar of artifact.sidecarFiles) {
        addPathAndParents(owned, sidecar.relativePath);
      }
    }
  }

  return owned;
}

async function listTreeEntries(
  repoRoot: string,
  relativeRoot: string,
): Promise<string[]> {
  const entries: string[] = [];

  async function visit(relativeDir: string): Promise<void> {
    let children;
    try {
      children = await readdir(path.join(repoRoot, relativeDir), { withFileTypes: true });
    } catch (error) {
      if (error instanceof Error && "code" in error && error.code === "ENOENT") return;
      throw error;
    }

    for (const child of children) {
      const relativePath = toRepoPath(path.join(relativeDir, child.name));
      entries.push(relativePath);
      if (child.isDirectory()) await visit(relativePath);
    }
  }

  await visit(relativeRoot);
  return entries;
}

async function listOpenCodeCommandEntries(
  repoRoot: string,
  relativeRoot: string,
): Promise<string[]> {
  let entries;
  try {
    entries = await readdir(path.join(repoRoot, relativeRoot), { withFileTypes: true });
  } catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT") return [];
    throw error;
  }

  return entries
    .filter(
      (entry) =>
        (entry.isFile() || entry.isSymbolicLink()) && /^minutes-.*\.md$/.test(entry.name),
    )
    .map((entry) => toRepoPath(path.join(relativeRoot, entry.name)));
}

export async function findUnownedGeneratedArtifacts(
  repoRoot: string,
  skills: CanonicalSkillSource[],
): Promise<string[]> {
  const owned = planOwnedArtifactPaths(skills);
  const generatedEntries: string[] = [];

  for (const scope of GENERATED_ARTIFACT_SCOPES) {
    generatedEntries.push(
      ...(scope.kind === "tree"
        ? await listTreeEntries(repoRoot, scope.root)
        : await listOpenCodeCommandEntries(repoRoot, scope.root)),
    );
  }

  return generatedEntries.filter((entry) => !owned.has(entry)).sort();
}
