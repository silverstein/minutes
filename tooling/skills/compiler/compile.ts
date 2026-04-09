import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { cwd, exit } from "node:process";
import { discoverCanonicalSkills } from "./discover.js";
import { getHostConfig, HOSTS } from "../hosts/index.js";
import type { HostName } from "../schema.js";
import { renderSkillForHost } from "./render.js";
import { resolveSkillAssetSourcePath, validateSkillAssets } from "./validate.js";

interface CompileOptions {
  dryRun: boolean;
  hosts: HostName[];
}

function parseArgs(argv: string[]): CompileOptions {
  const dryRun = argv.includes("--dry-run");
  const hostValues: string[] = [];
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--host" && argv[i + 1]) {
      hostValues.push(argv[i + 1]);
      i += 1;
    }
  }
  const hosts =
    hostValues.length === 0 || hostValues.includes("all")
      ? (Object.keys(HOSTS) as HostName[])
      : (hostValues as HostName[]);

  for (const host of hosts) {
    if (!(host in HOSTS)) {
      throw new Error(`Unknown host "${host}". Expected one of: ${Object.keys(HOSTS).join(", ")}`);
    }
  }

  return { dryRun, hosts };
}

async function compareOrWrite(
  rootDir: string,
  targetPath: string,
  content: string,
  dryRun: boolean,
): Promise<"unchanged" | "changed"> {
  const absolute = path.join(rootDir, "..", "..", targetPath);
  let current: string | null = null;
  try {
    current = await readFile(absolute, "utf8");
  } catch {
    current = null;
  }

  if (current === content) {
    return "unchanged";
  }

  if (!dryRun) {
    await mkdir(path.dirname(absolute), { recursive: true });
    await writeFile(absolute, content, "utf8");
  }
  return "changed";
}

async function main(): Promise<void> {
  const options = parseArgs(process.argv.slice(2));
  const rootDir = cwd().endsWith(path.join("tooling", "skills"))
    ? cwd()
    : path.join(cwd(), "tooling", "skills");
  const skills = await discoverCanonicalSkills(rootDir);

  const changes: Array<{ host: string; path: string; kind: string }> = [];

  for (const skill of skills) {
    await validateSkillAssets(skill);
    for (const hostName of options.hosts) {
      const host = getHostConfig(hostName);
      const artifact = renderSkillForHost(skill, host);
      const status = await compareOrWrite(rootDir, artifact.outputPath, artifact.body, options.dryRun);
      if (status === "changed") {
        changes.push({ host: hostName, path: artifact.outputPath, kind: "skill" });
      }
      for (const asset of artifact.assetFiles) {
        const sourcePath = await resolveSkillAssetSourcePath(skill, asset.sourceRelativePath);
        const assetContent = await readFile(sourcePath, "utf8");
        const assetStatus = await compareOrWrite(
          rootDir,
          asset.outputRelativePath,
          assetContent,
          options.dryRun,
        );
        if (assetStatus === "changed") {
          changes.push({ host: hostName, path: asset.outputRelativePath, kind: "asset" });
        }
      }
      for (const sidecar of artifact.sidecarFiles) {
        const sidecarStatus = await compareOrWrite(
          rootDir,
          sidecar.relativePath,
          sidecar.content,
          options.dryRun,
        );
        if (sidecarStatus === "changed") {
          changes.push({ host: hostName, path: sidecar.relativePath, kind: "sidecar" });
        }
      }
    }
  }

  if (changes.length === 0) {
    console.log(
      JSON.stringify({
        status: options.dryRun ? "clean" : "up_to_date",
        dryRun: options.dryRun,
        hosts: options.hosts,
        skillCount: skills.length,
      }),
    );
    return;
  }

  console.log(
    JSON.stringify({
      status: options.dryRun ? "drift" : "written",
      dryRun: options.dryRun,
      hosts: options.hosts,
      skillCount: skills.length,
      changes,
    }),
  );

  if (options.dryRun) {
    exit(1);
  }
}

main().catch((error) => {
  console.error(
    JSON.stringify({
      status: "error",
      message: error instanceof Error ? error.message : String(error),
    }),
  );
  exit(1);
});
