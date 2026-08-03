#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

// Updated only after a line-by-line review of the complete unsigned-build
// boundary and secret-bearing job. Raw regex matches are intentionally not the
// authority: these goldens make comment/dead-step duplication fail closed.
const EXPECTED_SIGNING_JOB_SHA256 =
  "903c7c962b5eac067fbe423699943969b0405cec732306299366f172f5037894";
const EXPECTED_POST_SIGNING_BOUNDARY_SHA256 =
  "e0a1feef44fa29000affee57a51a55156c232d677307181a68e9cc2341594807";
const EXPECTED_PRE_SIGNING_BOUNDARY_SHA256 =
  "1ddf57258d26c54dab346639d179a4cce64e6d3e333bb92982b38706ac7fe333";
const EXPECTED_TRIGGER_BLOCK = `on:
  workflow_dispatch:
    inputs:
      candidate_sha:
        description: Full SHA protected by the matching acceptance-<sha> tag
        required: true
        type: string
`;
const SECRET_CONTEXT_EXPRESSION =
  /\$\{\{(?:(?!\}\})[\s\S])*?\bsecrets\b(?:(?!\}\})[\s\S])*?\}\}/i;

const signingJobFixture = process.argv[2] === "--signing-job-fixture";
const workflowPath = signingJobFixture
  ? process.argv[3]
  : process.argv[2] ?? ".github/workflows/signed-dev-acceptance.yml";
const source = readFileSync(workflowPath, "utf8");
const errors = [];

function requirePattern(pattern, message) {
  if (!pattern.test(source)) errors.push(message);
}

const preSigningBoundary = source.split(/^  sign-reviewed-artifact:\n/m, 1)[0];

if (!signingJobFixture) {
  const preSigningBoundaryHash = createHash("sha256")
    .update(preSigningBoundary)
    .digest("hex");
  if (preSigningBoundaryHash !== EXPECTED_PRE_SIGNING_BOUNDARY_SHA256) {
    errors.push(
      "the complete trigger, authorization, and unsigned-build boundary changed; review it and update its golden hash",
    );
  }

  const triggerBlock = source.match(/^on:\n[\s\S]*?(?=\npermissions:)/m)?.[0];
  if (triggerBlock !== EXPECTED_TRIGGER_BLOCK) {
    errors.push("signed acceptance must expose only the exact reviewed workflow_dispatch trigger");
  }
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
  requirePattern(
    /graph_worker_bundle="\$app\/Contents\/XPCServices\/com\.useminutes\.graph-worker\.xpc"[\s\S]*?test ! -e "\$app\/Contents\/MacOS\/minutes-graph-worker"[\s\S]*?--entitlements payload\/graph-worker-entitlements\.plist[\s\S]*?--identifier com\.useminutes\.graph-worker[\s\S]*?--sign "\$MINUTES_DEV_SIGNING_IDENTITY" "\$graph_worker_bundle"/,
    "the nested graph XPC service must receive only its dedicated App Sandbox identity",
  );
  requirePattern(
    /expected = \{"com\.apple\.security\.app-sandbox": True\}[\s\S]*?actual != expected/,
    "signed acceptance must enforce the graph helper's exact one-key entitlement allowlist",
  );
  requirePattern(
    /minutes-graph-worker\.cdhash[\s\S]*?observed_cdhash[\s\S]*?test "\$expected_cdhash" = "\$observed_cdhash"/,
    "signed acceptance must seal and verify the graph helper's exact CodeDirectory hash",
  );
  requirePattern(
    /MINUTES_GRAPH_WORKER_CDHASH_V1=[\s\S]*?contents\.count\(marker\) != 1[\s\S]*?invalid prior parent graph-worker seal[\s\S]*?os\.fsync/,
    "signed acceptance must bind one exact graph-worker hash into the parent before outer signing",
  );
  requirePattern(
    /signed parent is not bound to the exact graph worker/,
    "signed acceptance must verify the final parent-to-helper binding",
  );
  requirePattern(
    /apple_speech_worker_bundle="\$app\/Contents\/XPCServices\/com\.useminutes\.apple-speech-worker\.xpc"[\s\S]*?test ! -e "\$app\/Contents\/MacOS\/minutes-apple-speech-worker"[\s\S]*?--entitlements payload\/apple-speech-worker-entitlements\.plist[\s\S]*?--identifier com\.useminutes\.apple-speech-worker[\s\S]*?--sign "\$MINUTES_DEV_SIGNING_IDENTITY" "\$apple_speech_worker_bundle"/,
    "the nested Apple Speech XPC service must receive only its dedicated App Sandbox identity",
  );
  requirePattern(
    /apple-speech-worker-entitlements\.actual\.plist[\s\S]*?expected = \{"com\.apple\.security\.app-sandbox": True\}[\s\S]*?actual != expected/,
    "signed acceptance must enforce the Apple Speech worker's exact one-key entitlement allowlist",
  );
  requirePattern(
    /minutes-apple-speech-worker\.cdhash[\s\S]*?observed_apple_cdhash[\s\S]*?test "\$expected_apple_cdhash" = "\$observed_apple_cdhash"/,
    "signed acceptance must seal and verify the Apple Speech worker's exact CodeDirectory hash",
  );
  requirePattern(
    /MINUTES_APPLE_SPEECH_WORKER_CDHASH_V1=[\s\S]*?contents\.count\(marker\) != 1[\s\S]*?invalid prior parent Apple Speech worker seal[\s\S]*?os\.fsync/,
    "signed acceptance must bind one exact Apple Speech worker hash into the parent before outer signing",
  );
  requirePattern(
    /signed parent is not bound to the exact Apple Speech worker/,
    "signed acceptance must verify the final parent-to-Apple-Speech-worker binding",
  );
}

const signingJobMarker = "  sign-reviewed-artifact:\n";
const signingJobStart = source.indexOf(signingJobMarker);
const signingJobTail =
  signingJobStart >= 0
    ? source.slice(signingJobStart + signingJobMarker.length)
    : "";
const nextJobOffset = signingJobTail.search(/^  [a-z][a-z0-9-]*:\n/m);
const signingJob =
  signingJobStart < 0
    ? undefined
    : nextJobOffset >= 0
      ? signingJobTail.slice(0, nextJobOffset).replace(/\n$/, "")
      : signingJobTail;
const afterSigningJob =
  signingJobStart >= 0 && nextJobOffset >= 0
    ? signingJobTail.slice(nextJobOffset)
    : "";
// The unstripped signing job, kept only so the three regions can be proved to
// tile the file exactly. `signingJob` above drops one trailing newline for its
// own golden, which would otherwise make the arithmetic below off by one.
const signingJobRegion =
  signingJobStart >= 0 && nextJobOffset >= 0
    ? signingJobTail.slice(0, nextJobOffset)
    : signingJobTail;
if (!signingJob) {
  errors.push("could not isolate the secret-bearing signing job");
} else {
  if (!signingJobFixture && !SECRET_CONTEXT_EXPRESSION.test(signingJob)) {
    errors.push("signing job no longer consumes the expected protected secrets");
  }
  if (/uses:\s*actions\/checkout@/.test(signingJob)) {
    errors.push("the secret-bearing job must never check out or execute candidate source");
  }
  if (/^\s*uses:\s*\.\//m.test(signingJob)) {
    errors.push("the secret-bearing job must never execute a repository-local action");
  }
  if (
    /^\s*(?:run:\s*)?(?:bash|sh|source|\.)\s+["']?(?:payload|signed)\//m.test(
      signingJob,
    ) ||
    /^\s*run:\s*["']?(?:payload|signed)\//m.test(signingJob)
  ) {
    errors.push("the secret-bearing job must never execute candidate-artifact content");
  }

  const expectedSigningSteps = [
    "Download exact unsigned candidate",
    "Verify artifact provenance before unlocking the identity",
    "Import Developer ID identity into an ephemeral keychain",
    "Sign nested executables and outer app inside-out",
    "Verify exact Team identity and package sealed app",
    "Remove ephemeral signing material",
    "Upload short-lived signed acceptance artifact",
  ];
  const signingSteps = [...signingJob.matchAll(/^      - name:\s*(.+)$/gm)].map(
    (match) => match[1].trim(),
  );
  if (JSON.stringify(signingSteps) !== JSON.stringify(expectedSigningSteps)) {
    errors.push("the secret-bearing job step allowlist changed");
  }
  if (!signingJobFixture) {
    const signingJobHash = createHash("sha256").update(signingJob).digest("hex");
    if (signingJobHash !== EXPECTED_SIGNING_JOB_SHA256) {
      errors.push(
        "the complete secret-bearing job changed; review it and update its golden hash",
      );
    }
  }
}

if (!signingJobFixture) {
  requirePattern(
    /^  run-signed-runtime:\n[\s\S]*?needs:\n\s+- authorize-candidate\n\s+- sign-reviewed-artifact[\s\S]*?refs\/tags\/acceptance-\$\{\{ needs\.authorize-candidate\.outputs\.candidate_sha \}\}\n\s+fetch-depth: 1\n\s+persist-credentials: false[\s\S]*?test_apple_speech_signed_transport\.py[\s\S]*?signed-runtime-provenance\.json/m,
    "signed Apple Speech runtime acceptance must run in a no-secret successor job against the exact protected candidate",
  );
  if (SECRET_CONTEXT_EXPRESSION.test(afterSigningJob)) {
    errors.push("post-signing runtime jobs must not receive signing secrets");
  }

  // Everything after the signing job was checked for secret references and
  // nothing else, so `run-signed-runtime` -- 2 KB that checks out the
  // candidate at its acceptance tag and executes a candidate-controlled script
  // to produce the provenance receipt acceptance relies on -- could be
  // rewritten freely and this guard still passed. Steps could be added,
  // removed, or altered, and the receipt made to say whatever the edit wanted.
  // It carries no secrets, so the pattern above never fired.
  const afterSigningJobHash = createHash("sha256").update(afterSigningJob).digest("hex");
  if (afterSigningJobHash !== EXPECTED_POST_SIGNING_BOUNDARY_SHA256) {
    errors.push(
      "the complete post-signing runtime boundary changed; review it and update its golden hash",
    );
  }

  // The three hashed regions must tile the file exactly. Without this, the
  // next job appended to this workflow lands outside all of them and is
  // unreviewed again -- which is precisely how the region above was
  // introduced. A coverage failure means a region boundary moved, so the
  // goldens no longer mean what they claim, whatever they hash to.
  //
  // Stated as a concatenation rather than as arithmetic on lengths: the
  // identity is then self-evident and cannot hold by coincidence. Comparing
  // sums also invited an off-by-encoding reading, since `.length` counts
  // UTF-16 code units and the message said "bytes".
  const covered =
    preSigningBoundary + signingJobMarker + signingJobRegion + afterSigningJob;
  if (covered !== source) {
    errors.push(
      `hashed regions reconstruct ${covered.length} of ${source.length} characters ` +
        "and do not reproduce this workflow; some of it is outside every golden",
    );
  }
  const beforeSigningJob = source.split(/^  sign-reviewed-artifact:\n/m, 1)[0];
  if (SECRET_CONTEXT_EXPRESSION.test(beforeSigningJob)) {
    errors.push("signing secrets must not be exposed to candidate authorization or build jobs");
  }

  for (const match of source.matchAll(/^\s*-?\s*uses:\s*([^\s#]+).*$/gm)) {
    const reference = match[1];
    if (!/@[0-9a-f]{40}$/.test(reference)) {
      errors.push(`action is not pinned to a full commit SHA: ${reference}`);
    }
  }
}

if (errors.length) {
  for (const error of errors) console.error(`${workflowPath}: ${error}`);
  process.exitCode = 1;
}
