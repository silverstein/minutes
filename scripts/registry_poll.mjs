#!/usr/bin/env node

import { pathToFileURL } from "node:url";

const DEFAULT_NPM_REGISTRY_URL = "https://registry.npmjs.org/";
const DEFAULT_POLL_DELAYS_MS = [5_000, 10_000, 20_000, 40_000, 60_000, 60_000, 60_000, 45_000];

function registryUrl() {
  const configured = process.env.NPM_REGISTRY_URL ?? DEFAULT_NPM_REGISTRY_URL;
  return configured.endsWith("/") ? configured : `${configured}/`;
}

function pollDelays() {
  const configured = process.env.REGISTRY_POLL_DELAYS_MS;
  if (configured === undefined) return DEFAULT_POLL_DELAYS_MS;

  const delays = configured.split(",").map((value) => Number(value));
  if (delays.length === 0 || delays.some((value) => !Number.isFinite(value) || value < 0)) {
    throw new Error("REGISTRY_POLL_DELAYS_MS must be a comma-separated list of non-negative numbers");
  }
  return delays;
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export async function lookupNpmVersion(
  packageName,
  version,
  { fetchImpl = fetch, npmRegistryUrl = registryUrl(), timeoutMs = 10_000 } = {},
) {
  const baseUrl = npmRegistryUrl.endsWith("/") ? npmRegistryUrl : `${npmRegistryUrl}/`;
  const url = new URL(`${encodeURIComponent(packageName)}/${encodeURIComponent(version)}`, baseUrl);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);

  let response;
  try {
    response = await fetchImpl(url, {
      headers: { accept: "application/json" },
      signal: controller.signal,
    });
  } catch (error) {
    return { kind: "temporary-error", detail: error instanceof Error ? error.message : String(error) };
  } finally {
    clearTimeout(timeout);
  }

  if (response.status === 404) return { kind: "missing" };
  if (response.status >= 500 && response.status <= 599) {
    return { kind: "temporary-error", detail: `HTTP ${response.status}` };
  }
  if (!response.ok) {
    throw new Error(`npm registry lookup for ${packageName}@${version} failed with HTTP ${response.status}`);
  }

  let metadata;
  try {
    metadata = await response.json();
  } catch (error) {
    throw new Error(
      `npm registry returned invalid JSON for ${packageName}@${version}: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  return { kind: "found", integrity: metadata?.dist?.integrity };
}

export function assertNpmIntegrity(packageName, version, actual, expected) {
  if (typeof actual !== "string" || actual.length === 0) {
    throw new Error(`npm registry metadata for ${packageName}@${version} has no dist.integrity`);
  }
  if (actual !== expected) {
    throw new Error(
      `${packageName}@${version} already exists with different integrity\n` +
        `  registry: ${actual}\n  local:    ${expected}\nRefusing to replace published provenance.`,
    );
  }
}

export async function checkNpmVersion(packageName, version, expectedIntegrity, options = {}) {
  const result = await lookupNpmVersion(packageName, version, options);
  if (result.kind === "found") {
    assertNpmIntegrity(packageName, version, result.integrity, expectedIntegrity);
    return true;
  }
  if (result.kind === "missing") return false;
  throw new Error(
    `cannot safely determine whether ${packageName}@${version} already exists (${result.detail}); refusing to publish`,
  );
}

export async function pollForNpmVersion(
  packageName,
  version,
  expectedIntegrity,
  { delays = pollDelays(), logger = console.log, sleepImpl = sleep, ...lookupOptions } = {},
) {
  let lastResult;
  for (let attempt = 0; attempt <= delays.length; attempt += 1) {
    lastResult = await lookupNpmVersion(packageName, version, lookupOptions);
    if (lastResult.kind === "found") {
      assertNpmIntegrity(packageName, version, lastResult.integrity, expectedIntegrity);
      logger(`npm confirms ${packageName}@${version} (${expectedIntegrity}).`);
      return;
    }

    if (attempt < delays.length) {
      const description = lastResult.kind === "missing" ? "404 (not visible yet)" : lastResult.detail;
      logger(`npm registry poll ${attempt + 1}: ${description}; retrying in ${delays[attempt]}ms.`);
      await sleepImpl(delays[attempt]);
    }
  }

  const lastDescription = lastResult?.kind === "missing" ? "404" : lastResult?.detail ?? "unknown error";
  throw new Error(
    `${packageName}@${version} did not become visible before the npm registry polling timeout ` +
      `(last result: ${lastDescription})`,
  );
}

function usage() {
  return [
    "Usage:",
    "  node scripts/registry_poll.mjs npm-check <package> <version> <sha512-integrity>",
    "  node scripts/registry_poll.mjs npm-poll <package> <version> <sha512-integrity>",
  ].join("\n");
}

async function main(argv) {
  const [command, packageName, version, integrity, ...rest] = argv;
  if (
    !["npm-check", "npm-poll"].includes(command) ||
    [packageName, version, integrity].some((value) => value === undefined) ||
    rest.length > 0
  ) {
    throw new Error(usage());
  }
  if (!integrity.startsWith("sha512-")) {
    throw new Error(`expected a sha512 integrity for ${packageName}@${version}`);
  }

  if (command === "npm-check") {
    const alreadyPublished = await checkNpmVersion(packageName, version, integrity);
    process.stdout.write(`already=${alreadyPublished}\n`);
  } else {
    await pollForNpmVersion(packageName, version, integrity);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
