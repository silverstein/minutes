import { describe, expect, it } from "vitest";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { withStableCorpusLease } from "./corpus-lease.js";
import { retireBoundReadersForProcessShutdown } from "./secure-read.js";

// This test lives in its own file on purpose, and must stay alone here.
//
// A one-millisecond budget kills the corpus worker while it may still be
// spawning. When the kill misses the termination-grace window, the module
// deliberately retains the process-global memory reservation and poisons the
// worker path; that fail-closed design is correct product behavior, but it is
// process-wide, so in a shared test file it cascades into every later corpus
// test as "retained snapshots exceeded" failures. Observed on windows-latest
// in CI.
//
// Vitest runs each test file in its own isolated process (the default forks
// pool), so whatever state this test leaves behind dies with this process and
// can never reach the main corpus-lease suite. That containment is
// structural: no reset seam in production code, nothing exported that could
// disarm the fail-closed barrier, and nothing here that has to simulate the
// race. Adding further tests to this file would put them back inside the
// blast radius.
//
// The worker route is load-bearing. The raw-error escape this pins lived in
// the worker dispatcher's pre-spawn guards, and the in-process dispatcher
// wraps its own errors before the public entry ever sees them: an in-process
// variant of this test kept passing with the fix removed.
function withCorpus(run: (root: string) => Promise<void>): Promise<void> {
  const root = mkdtempSync(join(tmpdir(), "minutes-corpus-refusal-"));
  return run(root).finally(async () => {
    await retireBoundReadersForProcessShutdown();
    rmSync(root, { recursive: true, force: true });
  });
}

describe("stable corpus lease refusal contract", () => {
  it("reports the documented refusal when the budget expires, whichever guard trips", async () => {
    // An exhausted budget can be noticed by several guards, and only some sit
    // inside the worker machinery that wraps refusals. The ones outside it
    // used to escape as the raw "meeting corpus authorization deadline
    // elapsed", so callers saw two different sentences for one condition
    // depending only on which guard won. CI hit it on a contended Windows
    // runner, where a short budget can expire between computing the deadline
    // and the next statement checking it.
    //
    // This pins the contract rather than a route: the smallest budget the API
    // accepts is reliably gone by the time some guard notices, and every guard
    // has to produce the same sentence. Which one fires is timing-dependent
    // and deliberately not asserted -- claiming a specific one would be the
    // same kind of unverified detail this fix exists to remove.
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "elapsed budget canary");
      let projections = 0;
      await expect(
        withStableCorpusLease(
          root,
          () => {
            projections += 1;
            return "unreachable";
          },
          { timeoutMs: 1 }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(projections).toBe(0);
    });
  });
});
