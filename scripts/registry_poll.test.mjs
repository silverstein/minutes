import assert from "node:assert/strict";
import { createServer } from "node:http";
import test from "node:test";

import {
  assertNpmIntegrity,
  checkNpmVersion,
  lookupNpmVersion,
  pollForNpmVersion,
} from "./registry_poll.mjs";

const PACKAGE_NAME = "minutes-sdk";
const VERSION = "1.2.3";
const INTEGRITY = "sha512-local";

async function registryFixture(t, responses) {
  let requestCount = 0;
  const server = createServer((request, response) => {
    assert.equal(request.url, `/${PACKAGE_NAME}/${VERSION}`);
    const fixture = responses[Math.min(requestCount, responses.length - 1)];
    requestCount += 1;
    response.writeHead(fixture.status, { "content-type": "application/json" });
    response.end(JSON.stringify(fixture.body ?? {}));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve()))));

  const address = server.address();
  return {
    npmRegistryUrl: `http://127.0.0.1:${address.port}/`,
    requestCount: () => requestCount,
  };
}

test("lookup distinguishes a missing version from a matching published version", async (t) => {
  const fixture = await registryFixture(t, [
    { status: 404 },
    { status: 200, body: { dist: { integrity: INTEGRITY } } },
  ]);

  assert.deepEqual(await lookupNpmVersion(PACKAGE_NAME, VERSION, fixture), { kind: "missing" });
  assert.equal(await checkNpmVersion(PACKAGE_NAME, VERSION, INTEGRITY, fixture), true);
});

test("integrity mismatch refuses an idempotent skip", () => {
  assert.throws(
    () => assertNpmIntegrity(PACKAGE_NAME, VERSION, "sha512-registry", INTEGRITY),
    /already exists with different integrity[\s\S]*Refusing to replace published provenance/,
  );
});

test("check refuses to publish after a temporary registry error", async (t) => {
  const fixture = await registryFixture(t, [{ status: 503 }]);

  await assert.rejects(
    checkNpmVersion(PACKAGE_NAME, VERSION, INTEGRITY, fixture),
    /cannot safely determine.*HTTP 503.*refusing to publish/,
  );
});

test("poll retries 404 and server errors until exact integrity is visible", async (t) => {
  const fixture = await registryFixture(t, [
    { status: 404 },
    { status: 502 },
    { status: 200, body: { dist: { integrity: INTEGRITY } } },
  ]);
  const messages = [];

  await pollForNpmVersion(PACKAGE_NAME, VERSION, INTEGRITY, {
    ...fixture,
    delays: [0, 0],
    logger: (message) => messages.push(message),
    sleepImpl: async () => {},
  });

  assert.equal(fixture.requestCount(), 3);
  assert.match(messages[0], /404 \(not visible yet\)/);
  assert.match(messages[1], /HTTP 502/);
  assert.match(messages[2], /npm confirms minutes-sdk@1\.2\.3/);
});
