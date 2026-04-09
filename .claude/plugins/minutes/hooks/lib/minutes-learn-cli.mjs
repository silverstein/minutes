#!/usr/bin/env node

import { getLatestLearning, normalizeLearnings, rememberExplicit, rememberObserved } from "./minutes-learn.mjs";

const [, , command, ...args] = process.argv;

try {
  if (command === "set-explicit") {
    const [type, key, value, ...notes] = args;
    const result = rememberExplicit(type, key, value, notes.join(" "));
    console.log(JSON.stringify({ status: "ok", result }));
    process.exit(0);
  }

  if (command === "set-observed") {
    const [type, key, value, confidenceRaw, ...notes] = args;
    const confidence = Number(confidenceRaw);
    const result = rememberObserved(type, key, value, confidence, notes.join(" "));
    console.log(JSON.stringify({ status: "ok", result }));
    process.exit(0);
  }

  if (command === "get") {
    const [type, key] = args;
    console.log(JSON.stringify({ status: "ok", result: getLatestLearning(type, key) }));
    process.exit(0);
  }

  if (command === "dump") {
    console.log(JSON.stringify({ status: "ok", result: normalizeLearnings() }, null, 2));
    process.exit(0);
  }

  console.error(
    JSON.stringify({
      status: "error",
      message:
        "Usage: minutes-learn-cli.mjs set-explicit <type> <key> <value> [notes...] | set-observed <type> <key> <value> <confidence> [notes...] | get <type> <key> | dump",
    }),
  );
  process.exit(1);
} catch (error) {
  console.error(
    JSON.stringify({
      status: "error",
      message: error instanceof Error ? error.message : String(error),
    }),
  );
  process.exit(1);
}
