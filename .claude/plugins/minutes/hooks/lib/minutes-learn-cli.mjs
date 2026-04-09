#!/usr/bin/env node

import {
  clearLearning,
  finalizePendingMeetingPrepNudge,
  getAliasCluster,
  getLatestLearning,
  inferMeetingPrepModeFromUsage,
  normalizeLearnings,
  recordPendingMeetingPrepNudge,
  rememberAlias,
  rememberExplicit,
  rememberObserved,
  shouldSuppressMeetingPrepNudge,
} from "./minutes-learn.mjs";

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

  if (command === "set-alias") {
    const [nameA, nameB, ...notes] = args;
    const result = rememberAlias(nameA, nameB, notes.join(" "));
    console.log(JSON.stringify({ status: "ok", result }));
    process.exit(0);
  }

  if (command === "aliases") {
    const [name] = args;
    console.log(JSON.stringify({ status: "ok", result: getAliasCluster(name) }, null, 2));
    process.exit(0);
  }

  if (command === "infer-meeting-prep-mode") {
    console.log(JSON.stringify({ status: "ok", result: inferMeetingPrepModeFromUsage() }));
    process.exit(0);
  }

  if (command === "record-pending-nudge") {
    const [mode = "auto"] = args;
    console.log(JSON.stringify({ status: "ok", result: recordPendingMeetingPrepNudge(mode) }));
    process.exit(0);
  }

  if (command === "finalize-pending-nudge") {
    console.log(JSON.stringify({ status: "ok", result: finalizePendingMeetingPrepNudge() }, null, 2));
    process.exit(0);
  }

  if (command === "should-suppress-nudge") {
    console.log(JSON.stringify({ status: "ok", result: shouldSuppressMeetingPrepNudge() }));
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

  if (command === "clear") {
    const [type, key] = args;
    const result = clearLearning(type, key);
    console.log(JSON.stringify({ status: "ok", result }));
    process.exit(0);
  }

  console.error(
    JSON.stringify({
      status: "error",
      message:
        "Usage: minutes-learn-cli.mjs set-explicit <type> <key> <value> [notes...] | set-observed <type> <key> <value> <confidence> [notes...] | set-alias <nameA> <nameB> [notes...] | aliases <name> | get <type> <key> | clear <type> <key> | dump",
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
