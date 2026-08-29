#!/usr/bin/env node

import { spawnSync } from "node:child_process";

const [requestedCommand, ...args] = process.argv.slice(2);
if (!requestedCommand) {
  throw new Error("usage: run_corpus_test_harness.mjs <command> [...args]");
}

const result = spawnSync(requestedCommand, args, {
  env: {
    ...process.env,
    NODE_ENV: "test",
    MINUTES_TEST_HARNESS: "1",
    MINUTES_CORPUS_AUTH_TIMEOUT_MS: "60000",
  },
  shell: process.platform === "win32",
  stdio: "inherit",
});

if (result.error) throw result.error;
if (result.signal) process.kill(process.pid, result.signal);
process.exit(result.status ?? 1);
