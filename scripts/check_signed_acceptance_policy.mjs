#!/usr/bin/env node

import { readFileSync } from "node:fs";

const workflowPath =
  process.argv[2] ?? ".github/workflows/signed-dev-acceptance.yml";
const source = readFileSync(workflowPath, "utf8");
const errors = [];

function requirePattern(pattern, message) {
  if (!pattern.test(source)) errors.push(message);
}

function rejectPattern(pattern, message) {
  if (pattern.test(source)) errors.push(message);
}

requirePattern(
  /^on:\n  workflow_dispatch:\n/m,
  "signed acceptance must be dispatched from the protected default-branch workflow",
);
rejectPattern(
  /^  (?:push|pull_request|schedule|repository_dispatch):/m,
  "signed acceptance must not expose signing through an event-controlled ref",
);
requirePattern(
  /if: github\.ref == 'refs\/heads\/main' && github\.actor == 'silverstein'/,
  "candidate authorization must run only from protected main under the repository owner",
);
requirePattern(
  /refs\/tags\/acceptance-\$\{\{ needs\.authorize-candidate\.outputs\.candidate_sha \}\}/,
  "candidate checkout must be bound to its protected acceptance-<sha> tag",
);
requirePattern(
  /^  sign-reviewed-artifact:[\s\S]*?^    environment: signed-dev-acceptance$/m,
  "the secret-bearing signing job must use the reviewer-gated environment",
);
requirePattern(
  /^  build-unsigned:[\s\S]*?^  sign-reviewed-artifact:/m,
  "candidate code must build in a separate job before signing credentials are available",
);

const signingJob = source.match(
  /^  sign-reviewed-artifact:\n([\s\S]*)$/m,
)?.[1];
if (!signingJob) {
  errors.push("could not isolate the secret-bearing signing job");
} else {
  if (!/\$\{\{ secrets\./.test(signingJob)) {
    errors.push("signing job no longer consumes the expected protected secrets");
  }
  if (/uses:\s*actions\/checkout@/.test(signingJob)) {
    errors.push("the secret-bearing job must never check out or execute candidate source");
  }
}

const beforeSigningJob = source.split(/^  sign-reviewed-artifact:\n/m, 1)[0];
if (/\$\{\{ secrets\./.test(beforeSigningJob)) {
  errors.push("signing secrets must not be exposed to candidate authorization or build jobs");
}

for (const match of source.matchAll(/^\s*-?\s*uses:\s*([^\s#]+).*$/gm)) {
  const reference = match[1];
  if (reference.startsWith("./")) continue;
  if (!/@[0-9a-f]{40}$/.test(reference)) {
    errors.push(`third-party action is not pinned to a full commit SHA: ${reference}`);
  }
}

if (errors.length) {
  for (const error of errors) console.error(`${workflowPath}: ${error}`);
  process.exitCode = 1;
}
