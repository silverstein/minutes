import path from "node:path";
import type {
  CanonicalSkillSource,
  CompiledSkillArtifact,
  HostConfig,
  HostName,
} from "../schema.js";

function rewritePaths(body: string, host: HostConfig): string {
  return host.pathPolicy.pathRewrites.reduce(
    (current, rewrite) => current.split(rewrite.from).join(rewrite.to),
    body,
  );
}

function overflowDescription(description: string, host: HostConfig): string {
  const limit = host.descriptionPolicy.maxLength;
  if (!limit || description.length <= limit) return description;
  if (host.descriptionPolicy.onOverflow === "truncate") {
    return `${description.slice(0, limit - 3)}...`;
  }
  throw new Error(
    `Description for ${host.name} exceeds limit ${limit}: ${description.length} characters`,
  );
}

function applyHostFrontmatter(
  skill: CanonicalSkillSource,
  host: HostConfig,
): string {
  const override = skill.frontmatter.host_overrides?.[host.name as HostName];
  const description = overflowDescription(
    override?.description_override ?? skill.frontmatter.description,
    host,
  );
  const lines = [`---`, `name: ${skill.frontmatter.name}`, `description: ${description}`];
  const userInvocable = skill.frontmatter.user_invocable;
  if (
    host.frontmatterPolicy.mode === "denylist" &&
    userInvocable !== undefined &&
    !host.frontmatterPolicy.stripFields?.includes("user_invocable")
  ) {
    lines.push(`user_invocable: ${userInvocable ? "true" : "false"}`);
  }
  const allowedTools = skill.frontmatter.allowed_tools;
  if (
    host.frontmatterPolicy.mode === "denylist" &&
    Array.isArray(allowedTools) &&
    allowedTools.length > 0 &&
    !host.frontmatterPolicy.stripFields?.includes("allowed_tools")
  ) {
    lines.push(`allowed-tools:`);
    for (const tool of allowedTools) {
      lines.push(`  - ${tool}`);
    }
  }
  lines.push(`---`);
  return `${lines.join("\n")}\n\n`;
}

function makeOpenAIYaml(skillName: string, description: string): string {
  return `interface:
  display_name: ${JSON.stringify(skillName)}
  short_description: ${JSON.stringify(description)}
  default_prompt: ${JSON.stringify(`Use ${skillName} for this task.`)}
`;
}

export function renderSkillForHost(
  skill: CanonicalSkillSource,
  host: HostConfig,
): CompiledSkillArtifact {
  const override = skill.frontmatter.host_overrides?.[host.name];
  const rewrittenBody = rewritePaths(skill.body.trimStart(), host);
  const extraNotes = override?.extra_notes?.trim();
  const outputPath =
    skill.frontmatter.output?.[host.name]?.path ??
    (host.name === "claude"
      ? path.join(host.outputRoot, "skills", skill.frontmatter.name, "SKILL.md")
      : path.join(host.outputRoot, skill.frontmatter.name, "SKILL.md"));

  const frontmatter = applyHostFrontmatter(skill, host);
  const metadata = skill.frontmatter.metadata ?? {};
  const metadataDescription = overflowDescription(
    metadata.short_description ??
      override?.description_override ??
      skill.frontmatter.description,
    host,
  );
  const body =
    host.transformPolicy.extraNotesPlacement === "prepend" && extraNotes
      ? `${frontmatter}${extraNotes}\n\n${rewrittenBody}\n`
      : host.transformPolicy.extraNotesPlacement === "append" && extraNotes
        ? `${frontmatter}${rewrittenBody}\n\n## Host Notes\n\n${extraNotes}\n`
        : `${frontmatter}${rewrittenBody}\n`;

  const sidecarFiles =
    host.metadataPolicy.generateSidecar && host.metadataPolicy.format === "openai.yaml"
      ? [
          {
            relativePath: path.join(
              path.dirname(outputPath),
              host.metadataPolicy.relativeDir ?? "",
              "openai.yaml",
            ),
            content: [
              "interface:",
              `  display_name: ${JSON.stringify(metadata.display_name ?? skill.frontmatter.name)}`,
              `  short_description: ${JSON.stringify(metadataDescription)}`,
              ...(metadata.icon_small
                ? [`  icon_small: ${JSON.stringify(metadata.icon_small)}`]
                : []),
              ...(metadata.icon_large
                ? [`  icon_large: ${JSON.stringify(metadata.icon_large)}`]
                : []),
              `  default_prompt: ${JSON.stringify(
                metadata.default_prompt ?? `Use ${skill.frontmatter.name} for this task.`,
              )}`,
              "",
            ].join("\n"),
          },
        ]
      : [];

  return {
    host: host.name,
    skillName: skill.frontmatter.name,
    outputPath,
    body,
    sidecarFiles,
  };
}
