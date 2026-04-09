import { access } from "node:fs/promises";
import path from "node:path";
import { cwd, exit } from "node:process";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { discoverCanonicalSkills } from "./discover.js";
import { getHostConfig } from "../hosts/index.js";
import { renderSkillForHost } from "./render.js";

const execFileAsync = promisify(execFile);

function getRootDir(): string {
  return cwd().endsWith(path.join("tooling", "skills"))
    ? cwd()
    : path.join(cwd(), "tooling", "skills");
}

async function exists(filePath: string): Promise<boolean> {
  try {
    await access(filePath);
    return true;
  } catch {
    return false;
  }
}

async function minutesCliAvailable(): Promise<boolean> {
  try {
    await execFileAsync("minutes", ["--version"]);
    return true;
  } catch {
    return false;
  }
}

async function main(): Promise<void> {
  const rootDir = getRootDir();
  const repoRoot = path.join(rootDir, "..", "..");
  const skills = await discoverCanonicalSkills(rootDir);
  const problems: string[] = [];
  const warnings: string[] = [];

  if (!(await minutesCliAvailable())) {
    warnings.push("minutes CLI not found on PATH; skill docs may compile, but runtime flows may not work locally.");
  }

  if (skills.length === 0) {
    problems.push("No canonical skill sources were discovered under tooling/skills/sources.");
  }

  for (const skill of skills) {
    for (const hostName of ["claude", "codex"] as const) {
      const artifact = renderSkillForHost(skill, getHostConfig(hostName));
      const skillPath = path.join(repoRoot, artifact.outputPath);
      if (!(await exists(skillPath))) {
        problems.push(`Missing generated ${hostName} skill output: ${artifact.outputPath}`);
      }
      for (const sidecar of artifact.sidecarFiles) {
        const sidecarPath = path.join(repoRoot, sidecar.relativePath);
        if (!(await exists(sidecarPath))) {
          problems.push(`Missing generated ${hostName} sidecar output: ${sidecar.relativePath}`);
        }
      }
    }
  }

  if (problems.length > 0) {
    console.error(
      JSON.stringify(
        {
          status: "error",
          problems,
          warnings,
          next_steps: [
            "cd tooling/skills",
            "npm run build",
            "npm run compile",
            "npm run compile:dry",
          ],
        },
        null,
        2,
      ),
    );
    exit(1);
  }

  console.log(
    JSON.stringify(
      {
        status: "ok",
        skillCount: skills.length,
        warnings,
        verified_hosts: ["claude", "codex"],
      },
      null,
      2,
    ),
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
