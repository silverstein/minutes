import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { cwd, exit } from "node:process";
import { discoverCanonicalSkills } from "./discover.js";
import { getHostConfig } from "../hosts/index.js";
import { renderSkillForHost } from "./render.js";

const PILOT_SKILLS = new Set(["minutes-brief", "minutes-prep", "minutes-debrief"]);

function getRootDir(): string {
  return cwd().endsWith(path.join("tooling", "skills"))
    ? cwd()
    : path.join(cwd(), "tooling", "skills");
}

function parseWriteFlag(argv: string[]): boolean {
  return argv.includes("--write");
}

async function compareOrWriteSnapshot(
  filePath: string,
  content: string,
  writeMode: boolean,
): Promise<boolean> {
  let current: string | null = null;
  try {
    current = await readFile(filePath, "utf8");
  } catch {
    current = null;
  }

  if (current === content) return false;
  if (writeMode) {
    await mkdir(path.dirname(filePath), { recursive: true });
    await writeFile(filePath, content, "utf8");
    return false;
  }
  return true;
}

async function main(): Promise<void> {
  const writeMode = parseWriteFlag(process.argv.slice(2));
  const rootDir = getRootDir();
  const goldenRoot = path.join(rootDir, "goldens");
  const skills = await discoverCanonicalSkills(rootDir);
  const targets = skills.filter((skill) => PILOT_SKILLS.has(skill.id));
  const drifts: string[] = [];

  for (const skill of targets) {
    for (const hostName of ["claude", "codex"] as const) {
      const artifact = renderSkillForHost(skill, getHostConfig(hostName));
      const bodyPath = path.join(goldenRoot, hostName, skill.id, "SKILL.md");
      if (await compareOrWriteSnapshot(bodyPath, artifact.body, writeMode)) {
        drifts.push(bodyPath);
      }
      for (const sidecar of artifact.sidecarFiles) {
        const relativeSidecar = path.relative(
          path.join(path.dirname(artifact.outputPath)),
          sidecar.relativePath,
        );
        const snapshotPath = path.join(goldenRoot, hostName, skill.id, relativeSidecar);
        if (await compareOrWriteSnapshot(snapshotPath, sidecar.content, writeMode)) {
          drifts.push(snapshotPath);
        }
      }
    }
  }

  if (drifts.length > 0) {
    console.error(JSON.stringify({ status: "drift", writeMode, drifts }, null, 2));
    exit(1);
  }

  console.log(
    JSON.stringify({
      status: writeMode ? "written" : "ok",
      pilotCount: targets.length,
    }),
  );
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
