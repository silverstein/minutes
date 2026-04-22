export type HostName = "claude" | "codex" | "opencode";

export interface HostOverride {
  description_override?: string;
  extra_notes?: string;
  strip_sections?: string[];
}

export interface HostOutputOverride {
  path?: string;
}

export interface CanonicalSkillFrontmatter {
  name: string;
  description: string;
  triggers: string[];
  phase?: string;
  user_invocable?: boolean;
  allowed_tools?: string[];
  summary?: string;
  tags?: string[];
  metadata?: {
    display_name?: string;
    short_description?: string;
    default_prompt?: string;
    icon_small?: string;
    icon_large?: string;
    site_category?: string;
    site_example?: string;
    site_best_for?: string;
  };
  assets?: {
    scripts?: string[];
    templates?: string[];
    references?: string[];
  };
  host_overrides?: Partial<Record<HostName, HostOverride>>;
  output?: Partial<Record<HostName, HostOutputOverride>>;
  tests?: {
    golden?: boolean;
    lint_commands?: boolean;
  };
}

export interface CanonicalSkillSource {
  id: string;
  sourcePath: string;
  frontmatter: CanonicalSkillFrontmatter;
  body: string;
}

export interface HostPathPolicy {
  defaultSkillDir: string;
  pathRewrites: Array<{ from: string; to: string }>;
}

export interface HostFrontmatterPolicy {
  mode: "allowlist" | "denylist";
  keepFields?: string[];
  stripFields?: string[];
  extraFields?: Record<string, unknown>;
}

export interface HostDescriptionPolicy {
  maxLength?: number | null;
  onOverflow: "error" | "truncate";
}

export interface HostMetadataPolicy {
  generateSidecar: boolean;
  format?: "openai.yaml" | null;
  relativeDir?: string | null;
}

export interface HostTransformPolicy {
  extraNotesPlacement: "append" | "prepend" | "none";
  stripSectionTitles?: string[];
}

export interface HostAssetPolicy {
  mode: "reference" | "copy" | "symlink";
}

export interface HostConfig {
  name: HostName;
  displayName: string;
  outputRoot: string;
  pathPolicy: HostPathPolicy;
  frontmatterPolicy: HostFrontmatterPolicy;
  descriptionPolicy: HostDescriptionPolicy;
  metadataPolicy: HostMetadataPolicy;
  transformPolicy: HostTransformPolicy;
  assetPolicy: HostAssetPolicy;
}

export interface CompiledSkillArtifact {
  host: HostName;
  skillName: string;
  outputPath: string;
  body: string;
  assetFiles: Array<{
    sourceRelativePath: string;
    outputRelativePath: string;
  }>;
  sidecarFiles: Array<{
    relativePath: string;
    content: string;
  }>;
}
