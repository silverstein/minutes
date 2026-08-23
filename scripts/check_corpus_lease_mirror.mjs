#!/usr/bin/env node

/**
 * Parity gate for the files `crates/mcp/src` and `crates/sdk/src` mirror.
 *
 * The corpus lease exists twice, once per package, and the two copies are
 * maintained as byte-identical mirrors: every commit that touched either one
 * kept them the same, until a determinism fix landed in the MCP copy alone
 * (640ad42e, then 244cc558). Nothing noticed. The SDK kept the wall-clock
 * variant of a cancellation test, which measured elapsed time across lease
 * setup, and it failed whenever a hosted Windows runner was slow enough --
 * surfacing as an intermittent CI failure on branches whose diffs could not
 * reach this code (issue #617).
 *
 * A drift that only shows up as a flake on one platform is expensive to
 * diagnose and easy to misread as flaky infrastructure. Comparing the files
 * costs nothing and names the problem exactly.
 *
 * Files legitimately differ where the packages differ: `index.ts` is each
 * package's own entry point, and `secure-read.test.ts` has carried separate
 * cases in each package for many commits. Neither is mirrored, so neither is
 * listed here.
 */

import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const MIRRORED_FILES = [
  "corpus-lease.ts",
  "corpus-lease.test.ts",
  "corpus-lease-refusal.test.ts",
  "corpus-lease-poisoning.test.ts",
  "corpus-lease-worker.ts",
  "node-child.ts",
  "node-child.test.ts",
  "secure-read.ts",
];

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const digest = (contents) => createHash("sha256").update(contents).digest("hex");

const drifted = [];
for (const file of MIRRORED_FILES) {
  const mcpPath = join(repositoryRoot, "crates", "mcp", "src", file);
  const sdkPath = join(repositoryRoot, "crates", "sdk", "src", file);
  // A missing file is a failure, not a skip: deleting one side is exactly the
  // drift this gate exists to catch, and reporting "0 files checked, all
  // identical" would be the most misleading thing it could say.
  const [mcp, sdk] = await Promise.all([readFile(mcpPath), readFile(sdkPath)]);
  if (!mcp.equals(sdk)) {
    drifted.push({ file, mcp: digest(mcp).slice(0, 12), sdk: digest(sdk).slice(0, 12) });
  }
}

if (drifted.length > 0) {
  const detail = drifted
    .map(({ file, mcp, sdk }) => `  ${file}\n    crates/mcp/src  ${mcp}\n    crates/sdk/src  ${sdk}`)
    .join("\n");
  console.error(
    `corpus lease mirror drift in ${drifted.length} file(s):\n${detail}\n\n` +
      "These files are byte-identical mirrors. Apply the change to both copies,\n" +
      "or if the divergence is deliberate, remove the file from MIRRORED_FILES in\n" +
      "scripts/check_corpus_lease_mirror.mjs and say why in the commit message.\n" +
      "Diff them with:\n" +
      drifted.map(({ file }) => `  diff crates/mcp/src/${file} crates/sdk/src/${file}`).join("\n")
  );
  process.exit(1);
}

console.log(`corpus lease mirror: ${MIRRORED_FILES.length} files identical across mcp and sdk`);
