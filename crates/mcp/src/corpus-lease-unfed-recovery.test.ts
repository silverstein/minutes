import { describe, expect, it } from "vitest";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { withStableCorpusLease } from "./corpus-lease.js";
import { retireBoundReadersForProcessShutdown } from "./secure-read.js";

// Recovery must run in a separate Vitest process: the deadline refusal test
// can intentionally poison process-global worker state before this case starts.
function withCorpus(run: (root: string) => Promise<void>): Promise<void> {
  const root = mkdtempSync(join(tmpdir(), "minutes-corpus-refusal-"));
  return run(root).finally(async () => {
    await retireBoundReadersForProcessShutdown();
    rmSync(root, { recursive: true, force: true });
  });
}

describe("stable corpus lease recovery contract", () => {
  it("keeps working after an unconfirmed kill of a worker that never learned the corpus", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "never fed canary");
      // A one-millisecond budget expires while the worker is still starting,
      // so `begin` is never written and the child cannot hold corpus bytes.
      await expect(
        withStableCorpusLease(root, () => "unreachable", {
          timeoutMs: 1,
          forceUnconfirmedTerminationForTest: true,
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");

      // Issue #689: this used to refuse with "requires a process restart"
      // until the host process was restarted, and to keep the killed lease's
      // memory reservation charged forever.
      await expect(withStableCorpusLease(root, () => "recovered")).resolves.toBe(
        "recovered"
      );
    });
  });
});
