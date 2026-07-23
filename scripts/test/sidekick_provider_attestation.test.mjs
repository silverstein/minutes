import assert from "node:assert/strict";
import test from "node:test";

import {
  attestSidekickProviderExecutable,
  sidekickProviderAttestationMatches,
} from "../lib/sidekick_provider_attestation.mjs";

test("provider attestation binds one canonical executable's path, bytes, and version", async () => {
  const attestation = await attestSidekickProviderExecutable(process.execPath);
  assert.equal(attestation.path, process.execPath);
  assert.match(attestation.sha256, /^[a-f0-9]{64}$/);
  assert.match(attestation.version, /^v?\d+/);
  assert.equal(sidekickProviderAttestationMatches(attestation, attestation), true);
  assert.equal(sidekickProviderAttestationMatches(
    { ...attestation, sha256: "0".repeat(64) },
    attestation,
  ), false);
  assert.equal(sidekickProviderAttestationMatches(
    { ...attestation, version: `${attestation.version}-changed` },
    attestation,
  ), false);
});
