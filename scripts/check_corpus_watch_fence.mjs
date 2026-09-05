#!/usr/bin/env node

/**
 * Cross-platform availability gate for the production corpus lease.
 *
 * This intentionally imports the built SDK/MCP implementation instead of
 * approximating fs.watch. Each authorization therefore uses the real single
 * recursive root watcher, nested retained sentinel namespace, two distinct
 * sentinel slots, initial fence, final fence, manifest rereads, and cleanup.
 */

import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const moduleArgument = process.argv[2];
if (!moduleArgument) {
  throw new Error("usage: check_corpus_watch_fence.mjs <built-corpus-lease-module>");
}
const modulePath = isAbsolute(moduleArgument)
  ? moduleArgument
  : resolve(process.cwd(), moduleArgument);
const loaded = await import(pathToFileURL(modulePath).href);
if (typeof loaded.withStableCorpusLease !== "function") {
  throw new Error("built module does not export withStableCorpusLease");
}

const AUTHORIZATION_COUNT = 25;
const root = await mkdtemp(join(tmpdir(), "minutes-corpus-platform-gate-"));
try {
  const nested = join(root, "nested", "team");
  await mkdir(nested, { recursive: true });
  await writeFile(
    join(nested, "synthetic-meeting.md"),
    "---\ntitle: Synthetic platform gate\ntype: meeting\n---\n\nSynthetic content.\n"
  );

  for (let index = 0; index < AUTHORIZATION_COUNT; index += 1) {
    // Exercise the implementation's production deadline. A fixture-only
    // two-second override rejects valid authorizations on loaded hosts.
    const result = await loaded.withStableCorpusLease(
      root,
      (snapshot) => ({
        paths: snapshot.files.map((file) => file.relativePath),
        contents: snapshot.files.map((file) => file.content),
      })
    );
    if (
      result.paths.length !== 1 ||
      result.paths[0] !== "nested/team/synthetic-meeting.md" ||
      result.contents.length !== 1 ||
      !result.contents[0].includes("Synthetic content")
    ) {
      throw new Error("production corpus lease returned an invalid snapshot");
    }
  }

  console.log(
    JSON.stringify({
      authorizations: AUTHORIZATION_COUNT,
      implementation: modulePath,
      platform: process.platform,
      protocol: "production-recursive-two-sentinel",
    })
  );
} finally {
  await rm(root, { recursive: true, force: true });
}
