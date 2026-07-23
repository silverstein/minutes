#!/usr/bin/env node

// Compatibility entry point. The old one-model phrase-matching rehearsal was
// retired because a mechanical $800K check could false-green strategically
// useless prose. Delegate to the real product-path harness: independent
// evidence verification, calibrated semantic judge, and latency/model gates.
import { spawnSync } from "node:child_process";
import process from "node:process";
import { fileURLToPath } from "node:url";

const sessionEval = fileURLToPath(new URL("./sidekick_session_eval.mjs", import.meta.url));
const forwarded = process.argv.slice(2);
if (!forwarded.includes("--repeat")) forwarded.push("--repeat", "3");

const result = spawnSync(process.execPath, [sessionEval, ...forwarded], {
  stdio: "inherit",
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
