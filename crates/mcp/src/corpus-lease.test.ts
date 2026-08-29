import { afterEach, describe, expect, it } from "vitest";
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  linkSync,
  mkdtempSync,
  readFileSync,
  existsSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  unlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { realpath } from "node:fs/promises";

import {
  DEFAULT_CORPUS_READ_BUDGETS,
  awaitDeferredCorpusReleasesForTests,
  resolveAuthorizationTimeoutMsForTest,
  withStableCorpusLease,
} from "./corpus-lease.js";

// A lease that leaves an unconfirmed hazard behind defers its memory release
// until that hazard settles, which is correct: a child that may still be alive
// may still hold corpus data. The charge is process-global though, so a case
// whose hazard has not settled *yet* leaves it standing and later cases fail on
// the retained-snapshot budget rather than on anything they did. That is the
// flake that has been reported three times on three platforms, each time naming
// an innocent test.
//
// Waiting for our own cleanup makes the boundary between cases real, so a case
// is charged for what it did rather than for what the previous one had not
// finished undoing.
//
// Deliberately no "charge is back to zero" assertion. Poisoning is expressed AS
// a permanently retained charge, not as a flag, so "poisons admission when an
// asynchronous projection ignores cancellation" is supposed to end holding one.
// An unconditional assertion fails that test for doing its job, which is how
// this version of the hook was caught on its first run.
afterEach(async () => {
  await awaitDeferredCorpusReleasesForTests();
});
import {
  readTextFileFromBoundParent,
  retireBoundReadersForProcessShutdown,
} from "./secure-read.js";

function withCorpus(run: (root: string) => Promise<void>): Promise<void> {
  const root = mkdtempSync(join(tmpdir(), "minutes-corpus-lease-"));
  return run(root).finally(async () => {
    await retireBoundReadersForProcessShutdown();
    rmSync(root, { recursive: true, force: true });
  });
}

function publishInterruptedSentinel(root: string): void {
  const child = spawnSync(
    process.execPath,
    [
      "-e",
      `const { createHash } = require("node:crypto");
const { fsyncSync, openSync } = require("node:fs");
const { join } = require("node:path");
const root = process.argv[1];
const slot = createHash("sha256").update("minutes-corpus-lease-v1\\0shared-slot").digest("hex").slice(0, 32);
const path = join(root, ".minutes-corpus-lease-v1-0-" + slot + ".fence");
const fd = openSync(path, "wx", 0o600);
fsyncSync(fd);
process.exit(91);`,
      root,
    ],
    { encoding: "utf8" }
  );
  expect(child.status).toBe(91);
}

describe("stable corpus lease", () => {
  it("requires the Node runtime that provides one ordered recursive watcher", () => {
    const manifest = JSON.parse(
      readFileSync(new URL("../package.json", import.meta.url), "utf8")
    ) as { engines?: { node?: string } };
    expect(manifest.engines?.node).toBe(">=20");
  });

  it("evicts idle bound readers across more than 64 sequential parents", async () => {
    await withCorpus(async (root) => {
      for (let index = 0; index < 65; index += 1) {
        const parent = join(root, `parent-${index}`);
        const meetingPath = join(parent, "meeting.md");
        mkdirSync(parent);
        writeFileSync(meetingPath, `synthetic meeting ${index}`);
        const content = await readTextFileFromBoundParent(realpathSync(meetingPath), {
          maxReaders: 2,
          maxBytes: Number.MAX_SAFE_INTEGER,
          timeoutMs: Number.MAX_SAFE_INTEGER,
        });
        expect(content.toString("utf8")).toBe(`synthetic meeting ${index}`);
      }
    });
  });

  it("fails closed when every bounded reader at the cap is active", async () => {
    await withCorpus(async (root) => {
      const releases: Array<() => void> = [];
      const activeReads: Array<Promise<Buffer>> = [];

      for (let index = 0; index < 1; index += 1) {
        const parent = join(root, `active-${index}`);
        const meetingPath = join(parent, "meeting.md");
        mkdirSync(parent);
        writeFileSync(meetingPath, `synthetic active meeting ${index}`);
        let markStarted!: () => void;
        let release!: () => void;
        const didStart = new Promise<void>((resolve) => {
          markStarted = resolve;
        });
        const hold = new Promise<void>((resolve) => {
          release = resolve;
        });
        releases.push(release);
        activeReads.push(
          readTextFileFromBoundParent(realpathSync(meetingPath), {
            maxReaders: 2,
            afterFirstRead: async () => {
              markStarted();
              await hold;
            },
          })
        );
        // Racing the read surfaces its rejection. Awaiting `didStart` alone
        // hangs to the suite timeout whenever the first read is refused,
        // because `markStarted` runs inside the read that just failed. That
        // turned a named capacity refusal into an anonymous 15s timeout on
        // Windows, which is what made #617 undiagnosable from CI logs.
        await Promise.race([didStart, activeReads[activeReads.length - 1]]);
      }

      const thirdParent = join(root, "active-1");
      const thirdPath = join(thirdParent, "meeting.md");
      mkdirSync(thirdParent);
      writeFileSync(thirdPath, "synthetic active meeting 1");
      try {
        await expect(
          readTextFileFromBoundParent(realpathSync(thirdPath), { maxReaders: 2 })
        ).rejects.toThrow("bound reader capacity exceeded");
      } finally {
        for (const release of releases) release();
        await Promise.all(activeReads);
      }
    });
  });

  it("returns an immutable quiescent snapshot", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "stable corpus canary");

      const result = await withStableCorpusLease(root, (snapshot) => ({
        root: snapshot.canonicalRoot,
        files: snapshot.files.map((file) => [file.relativePath, file.content]),
      }));

      expect(result.files).toEqual([["meeting.md", "stable corpus canary"]]);
      expect(result.root).toBe(await realpath(root));
    });
  });

  it("ignores an ambient authorization override and keeps the production cap", () => {
    const requested = Number.MAX_SAFE_INTEGER;
    expect(
      resolveAuthorizationTimeoutMsForTest(requested, {
        MINUTES_CORPUS_AUTH_TIMEOUT_MS: "60000",
      })
    ).toBe(15_000);
    expect(
      resolveAuthorizationTimeoutMsForTest(requested, {
        NODE_ENV: "test",
        MINUTES_CORPUS_AUTH_TIMEOUT_MS: "60000",
      })
    ).toBe(15_000);
    expect(
      resolveAuthorizationTimeoutMsForTest(requested, {
        MINUTES_TEST_HARNESS: "1",
        MINUTES_CORPUS_AUTH_TIMEOUT_MS: "60000",
      })
    ).toBe(15_000);
  });

  it("applies and clamps the explicitly gated test-harness authorization override", () => {
    const harness = {
      NODE_ENV: "test",
      MINUTES_TEST_HARNESS: "1",
      MINUTES_CORPUS_AUTH_TIMEOUT_MS: "60000",
    };
    expect(resolveAuthorizationTimeoutMsForTest(undefined, harness)).toBe(60_000);
    expect(resolveAuthorizationTimeoutMsForTest(5_000, harness)).toBe(5_000);
    expect(
      resolveAuthorizationTimeoutMsForTest(Number.MAX_SAFE_INTEGER, {
        ...harness,
        MINUTES_CORPUS_AUTH_TIMEOUT_MS: "999999",
      })
    ).toBe(120_000);
    expect(() =>
      resolveAuthorizationTimeoutMsForTest(undefined, {
        ...harness,
        MINUTES_CORPUS_AUTH_TIMEOUT_MS: "invalid",
      })
    ).toThrow("invalid meeting corpus authorization timeout");
  });

  it("reassembles paced chunks before strict UTF-8 decoding", async () => {
    await withCorpus(async (root) => {
      const content = `${"a".repeat(64 * 1024 - 1)}😀${"z".repeat(64 * 1024)}`;
      writeFileSync(join(root, "chunked.md"), content);
      await expect(
        withStableCorpusLease(root, (snapshot) => snapshot.files[0].content)
      ).resolves.toBe(content);
    });
  });

  it("holds process-global retained-memory admission for the entire lease", async () => {
    const parent = mkdtempSync(join(tmpdir(), "minutes-corpus-memory-"));
    let release!: () => void;
    const hold = new Promise<void>((resolve) => {
      release = resolve;
    });
    try {
      const firstRoot = join(parent, "first");
      const secondRoot = join(parent, "second");
      mkdirSync(firstRoot);
      mkdirSync(secondRoot);
      writeFileSync(join(firstRoot, "meeting.md"), "first retained snapshot");
      writeFileSync(join(secondRoot, "meeting.md"), "second retained snapshot");
      let ready!: () => void;
      const didRetain = new Promise<void>((resolve) => {
        ready = resolve;
      });
      const active = withStableCorpusLease(firstRoot, () => "first", {
        afterBaseline: async () => {
          ready();
          await hold;
        },
      });
      await didRetain;

      await expect(
        withStableCorpusLease(secondRoot, () => "must not retain")
      ).rejects.toThrow("retained snapshots exceeded their process budget");
      release();
      await expect(active).resolves.toBe("first");
      await expect(
        withStableCorpusLease(secondRoot, () => "second")
      ).resolves.toBe("second");
    } finally {
      release();
      rmSync(parent, { recursive: true, force: true });
    }
  });

  it("deep-freezes snapshots while keeping every diagnostic hook path/content-free", async () => {
    await withCorpus(async (root) => {
      const privateCanary = "PRIVATE-DIAGNOSTIC-CONTENT-CANARY";
      writeFileSync(join(root, "private-meeting.md"), privateCanary);
      let capturedSnapshot: any;
      const contexts: any[] = [];
      const observe = (context: any) => {
        contexts.push(context);
        expect(Object.isFrozen(context)).toBe(true);
        expect(Object.isFrozen(context.controls)).toBe(true);
        expect("snapshot" in context).toBe(false);
      };

      const result = await withStableCorpusLease(
        root,
        (snapshot) => {
          capturedSnapshot = snapshot;
          expect(Object.isFrozen(snapshot)).toBe(true);
          expect(Object.isFrozen(snapshot.files)).toBe(true);
          expect(Object.isFrozen(snapshot.files[0])).toBe(true);
          return snapshot.files[0].content;
        },
        {
          onWatcherReady: observe,
          afterBaseline: observe,
          beforeFinalManifest: (context) => {
            observe(context);
            expect(() => {
              capturedSnapshot.files[0].content = "MUTATED";
            }).toThrow();
            expect(() => {
              capturedSnapshot.files.push({});
            }).toThrow();
          },
          afterFinalManifest: (context) => {
            observe(context);
            expect(Object.isFrozen(context.verification)).toBe(true);
          },
          beforeFinalFence: observe,
        }
      );

      expect(result).toBe(privateCanary);
      expect(contexts).toHaveLength(5);
      const serialized = JSON.stringify(contexts);
      expect(serialized).not.toContain(privateCanary);
      expect(serialized).not.toContain("private-meeting.md");
      expect(serialized).not.toContain(root);
    });
  });

  it("clamps overlarge resource budgets to safe defaults", async () => {
    await withCorpus(async (root) => {
      for (let index = 0; index < DEFAULT_CORPUS_READ_BUDGETS.maxDirectoryCount; index += 1) {
        mkdirSync(join(root, `directory-${index}`));
      }
      let operationCalls = 0;
      const overlarge = Number.MAX_SAFE_INTEGER;
      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          {
            budgets: {
              maxFileBytes: overlarge,
              maxCorpusBytes: overlarge,
              maxRetainedPathBytes: overlarge,
              maxFileCount: overlarge,
              maxDirectoryCount: overlarge,
              maxDirectoryEntries: overlarge,
              maxWatcherCount: overlarge,
              maxReaderCount: overlarge,
            },
            timeoutMs: overlarge,
          }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(operationCalls).toBe(0);
    });
  });

  it("rejects invalid authorization timeouts before touching the corpus", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "timeout canary");
      for (const timeoutMs of [0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY]) {
        await expect(
          withStableCorpusLease(root, () => "unreachable", { timeoutMs })
        ).rejects.toThrow("invalid meeting corpus authorization timeout");
      }
    });
  });

  it("fails closed before the operation when one file exceeds its byte budget", async () => {
    await withCorpus(async (root) => {
      const privateCanary = "PRIVATE_OVERSIZED_CANARY";
      writeFileSync(join(root, "oversized.md"), privateCanary);
      let operationCalls = 0;

      let failure: unknown;
      try {
        await withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          {
            budgets: {
              maxFileBytes: privateCanary.length - 1,
              maxCorpusBytes: 1_024,
              maxFileCount: 10,
            },
          }
        );
      } catch (error) {
        failure = error;
      }

      expect(failure).toBeInstanceOf(Error);
      expect((failure as Error).message).toBe(
        "Access denied: stable meeting corpus authorization failed"
      );
      expect((failure as Error).message).not.toContain(privateCanary);
      expect((failure as Error).message).not.toContain(root);
      expect((failure as Error).cause).toBeUndefined();
      expect(operationCalls).toBe(0);
    });
  });

  it("fails closed when individually valid files exceed the aggregate byte budget", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "first.md"), "12345");
      writeFileSync(join(root, "second.md"), "67890");
      let operationCalls = 0;

      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          {
            budgets: {
              maxFileBytes: 5,
              maxCorpusBytes: 9,
              maxFileCount: 10,
            },
          }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(operationCalls).toBe(0);

    });
  });

  it("fails closed when the active Markdown file count exceeds its budget", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "first.md"), "");
      writeFileSync(join(root, "second.md"), "");
      let operationCalls = 0;

      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          {
            budgets: {
              maxFileBytes: 0,
              maxCorpusBytes: 0,
              maxFileCount: 1,
            },
          }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(operationCalls).toBe(0);
    });
  });

  it("fails closed when retained path and entry-name metadata exceeds its budget", async () => {
    await withCorpus(async (root) => {
      const longName = `${"path-segment-".repeat(8)}meeting.md`;
      writeFileSync(join(root, longName), "");
      let operationCalls = 0;

      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          {
            budgets: {
              maxFileBytes: 0,
              maxCorpusBytes: 0,
              maxFileCount: 1,
              maxRetainedPathBytes: Buffer.byteLength(root, "utf8") + 8,
            },
          }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(operationCalls).toBe(0);
    });
  });

  it("charges full traversal paths for non-meeting entries", async () => {
    await withCorpus(async (root) => {
      const directoryName = "non-meeting-directory";
      mkdirSync(join(root, directoryName));
      writeFileSync(join(root, directoryName, "ignored.bin"), "");
      let operationCalls = 0;

      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          {
            budgets: {
              maxRetainedPathBytes:
                Buffer.byteLength(root, "utf8") +
                Buffer.byteLength(directoryName, "utf8") +
                4,
            },
          }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(operationCalls).toBe(0);
    });
  });

  it("bounds empty directory traversal and watcher allocation", async () => {
    await withCorpus(async (root) => {
      mkdirSync(join(root, "first"));
      mkdirSync(join(root, "second"));
      let operationCalls = 0;

      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          { budgets: { maxDirectoryCount: 2, maxWatcherCount: 2 } }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(operationCalls).toBe(0);

      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          { budgets: { maxDirectoryCount: 10, maxWatcherCount: 2 } }
        )
      ).resolves.toBeUndefined();
      expect(operationCalls).toBe(1);
    });
  });

  it(
    "enforces one process-global active watcher budget",
    async () => {
      const parent = mkdtempSync(join(tmpdir(), "minutes-corpus-watchers-"));
      let releaseAll!: () => void;
      const hold = new Promise<void>((resolve) => {
        releaseAll = resolve;
      });
      try {
        const root = join(parent, "active");
        mkdirSync(root);
        writeFileSync(join(root, "meeting.md"), "active watcher");
        let markReady!: () => void;
        const ready = new Promise<void>((resolve) => {
          markReady = resolve;
        });
        const active = withStableCorpusLease(root, () => "active", {
          budgets: { maxWatcherCount: 1, maxCorpusBytes: 1024 },
          afterBaseline: async () => {
            markReady();
            await hold;
          },
        });
        await ready;

        const overflow = join(parent, "overflow");
        mkdirSync(overflow);
        writeFileSync(join(overflow, "meeting.md"), "overflow");
        await expect(
          withStableCorpusLease(overflow, () => "must not authorize", {
            budgets: { maxWatcherCount: 1, maxCorpusBytes: 1024 },
          })
        ).rejects.toThrow("stable meeting corpus authorization failed");
        releaseAll();
        await expect(active).resolves.toBe("active");
      } finally {
        releaseAll();
        rmSync(parent, { recursive: true, force: true });
      }
    },
    30_000
  );

  it("atomically reserves near-capacity slots for one root", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "near-capacity canary");
      const namespace = join(root, ".minutes-corpus-lease-v1");
      mkdirSync(namespace, { mode: 0o700 });
      for (let index = 0; index < 127; index += 1) {
        writeFileSync(
          join(namespace, `lease-777-${index.toString(16).padStart(32, "0")}.fence`),
          "",
          { mode: 0o600 }
        );
      }

      let release!: () => void;
      const gate = new Promise<void>((resolve) => {
        release = resolve;
      });
      let arrivals = 0;
      let markAllReserved!: () => void;
      const allReserved = new Promise<void>((resolve) => {
        markAllReserved = resolve;
      });
      const observed: Array<{
        globalReserved: number;
        globalRetained: number;
        rootReserved: number;
      }> = [];
      const contenders = Array.from({ length: 4 }, () =>
        withStableCorpusLease(root, () => "must not authorize", {
          budgets: { maxCorpusBytes: 1024 },
          beforeSentinelCreate: async ({ attempt, slot, capacity }) => {
            observed.push(capacity);
            if (attempt === 1 && slot === 0) {
              arrivals += 1;
              if (arrivals === 4) markAllReserved();
              await gate;
            }
          },
        })
      );
      await allReserved;
      release();
      const results = await Promise.allSettled(contenders);
      expect(results.every((result) => result.status === "rejected")).toBe(true);
      expect(readdirSync(namespace).length).toBeLessThanOrEqual(128);
      expect(Math.max(...observed.map((entry) => entry.rootReserved))).toBe(4);
      expect(
        observed.every(
          (entry) => entry.globalRetained + entry.globalReserved <= 128
        )
      ).toBe(true);
    });
  });

  it("uses exactly two persistent shared slots across more than 64 corpus lifetimes", async () => {
    const parent = mkdtempSync(join(tmpdir(), "minutes-corpus-restarts-"));
    try {
      let firstRoot = "";
      for (let index = 0; index < 80; index += 1) {
        const root = join(parent, `lifetime-${index}`);
        if (index === 0) firstRoot = root;
        mkdirSync(root);
        writeFileSync(join(root, "meeting.md"), `lifetime ${index}`);
        await expect(withStableCorpusLease(root, () => index)).resolves.toBe(index);
        expect(readdirSync(join(root, ".minutes-corpus-lease-v1")).sort()).toEqual([
          "lease-shared-0.fence",
          "lease-shared-1.fence",
        ]);
      }
      await expect(withStableCorpusLease(firstRoot, () => "reopened"))
        .resolves.toBe("reopened");
    } finally {
      rmSync(parent, { recursive: true, force: true });
    }
    // 80 corpus lifetimes means 80 worker process lifecycles, and process
    // creation on the Windows runners is slow enough that this lands at
    // 30024ms against a 30s limit: failing by 24 milliseconds, then taking the
    // rest of the file down with it. The iteration count is load bearing, it
    // has to exceed the 64 slot bound this test exists to check, so the limit
    // is what gives. Four minutes is not a performance budget, it is headroom
    // against a slow runner; the assertion is about slot reuse, not speed.
  }, 240_000);

  // Not "across retry attempts": the parent's cumulative timer kills the
  // worker while it is still awaiting a hook, so the phase-result that would
  // let the worker advance into a retry is never sent.
  it("charges hook time against the one cumulative deadline", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "deadline canary");
      let operationCalls = 0;
      let baselineCalls = 0;
      let manifestCalls = 0;
      // Two hooks, each comfortably inside the budget on its own, together
      // over it. A per-phase timeout would admit both and let the lease
      // proceed; only a shared cumulative deadline refuses. A single
      // over-budget hook, which is what this test used to do, cannot tell
      // those apart, because a fresh 2s timeout would have rejected a 3s hook
      // just the same.
      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          {
            timeoutMs: 2_000,
            afterBaseline: () => {
              baselineCalls += 1;
              return new Promise((resolve) => setTimeout(resolve, 1_300));
            },
            beforeFinalManifest: () => {
              manifestCalls += 1;
              return new Promise((resolve) => setTimeout(resolve, 1_300));
            },
          }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      // Both hooks ran, so neither was individually over budget, and the
      // refusal still arrived: the time is shared.
      expect(baselineCalls).toBe(1);
      expect(manifestCalls).toBe(1);
      expect(operationCalls).toBe(1);
    });
  });

  it("still denies cleanly when the diagnostic sink is broken", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "stderr canary");
      // A denial writes its reason to stderr. That write happens after the
      // lease has settled precisely so it cannot matter, and this pins it: with
      // a stderr that throws on every write, the denial must still reject
      // normally rather than stranding the promise. The earlier arrangement,
      // which wrote before rejecting, would hang here forever.
      const original = process.stderr.write;
      let attempted = 0;
      (process.stderr as unknown as { write: unknown }).write = () => {
        attempted += 1;
        throw new Error("stderr is gone");
      };
      try {
        let forceOperationDeadline!: () => void;
        const operationDeadline = new Promise<void>((resolve) => {
          forceOperationDeadline = resolve;
        });
        await expect(
          withStableCorpusLease(
            root,
            (_snapshot, _attempt, signal) =>
              new Promise((_resolve, reject) => {
                signal.addEventListener("abort", () => reject(signal.reason), {
                  once: true,
                });
                forceOperationDeadline();
              }),
            { timeoutMs: 10_000, operationDeadlineForTest: operationDeadline }
          )
        ).rejects.toThrow("stable meeting corpus authorization failed");
        // The write is deferred so it cannot delay the denial, so let the
        // scheduled diagnostic run before asserting it was attempted.
        await new Promise((resolve) => setImmediate(resolve));
        expect(attempted).toBeGreaterThan(0);
      } finally {
        (process.stderr as unknown as { write: unknown }).write = original;
      }
    });
  });

  it("aborts an asynchronous operation at the cumulative authorization deadline", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "operation deadline canary");
      let operationStarted = false;
      let abortFired = false;
      let forceOperationDeadline!: () => void;
      const operationDeadline = new Promise<void>((resolve) => {
        forceOperationDeadline = resolve;
      });
      await expect(
        withStableCorpusLease(
          root,
          (_snapshot, _attempt, signal) =>
            new Promise((_resolve, reject) => {
              operationStarted = true;
              signal.addEventListener(
                "abort",
                () => {
                  abortFired = true;
                  reject(signal.reason);
                },
                { once: true }
              );
              forceOperationDeadline();
            }),
          { timeoutMs: 10_000, operationDeadlineForTest: operationDeadline }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      // The rejection alone proved nothing: with a 250ms budget a lease that
      // timed out during worker startup satisfied it without ever reaching an
      // operation, let alone cancelling one. These two say the named cause
      // actually happened.
      expect(operationStarted).toBe(true);
      expect(abortFired).toBe(true);
    });
  });

  it("kills a worker stalled before baseline publication and reuses admission", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "stalled baseline canary");
      let operationCalls = 0;
      const started = Date.now();
      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          { timeoutMs: 1_500, workerStallPhaseForTest: "before-baseline" }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      // Sentinels are acquired before the watcher is registered, and the
      // injected stall is later still, so this bounds the refusal rather than
      // pinpointing it: the worker got at least as far as sentinel
      // acquisition. That is what separates this from the failure mode being
      // fixed, where a budget consumed by startup refused with no worker
      // progress at all. Measured: a stalled worker leaves both fences behind,
      // while the same lease starved to 1ms refuses in 35ms with no namespace.
      // Elapsed time could not separate those, since startup also spends the
      // budget.
      const namespace = join(root, ".minutes-corpus-lease-v1");
      expect(existsSync(namespace)).toBe(true);
      expect(readdirSync(namespace).sort()).toEqual([
        "lease-shared-0.fence",
        "lease-shared-1.fence",
      ]);
      expect(Date.now() - started).toBeLessThan(6_000);
      expect(operationCalls).toBe(0);
      await expect(withStableCorpusLease(root, () => "reused")).resolves.toBe(
        "reused"
      );
    });
  });

  it("never publishes a projection when final authorization stalls", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "stalled finalize canary");
      let projections = 0;
      // End the operation from the projection itself rather than by running
      // out the clock. The assertion below is that the projection DID run, so
      // a wall-clock budget has to be longer than worker startup or the lease
      // fails first and the count is zero. That is a property of the runner,
      // not of the code: startup measures ~80ms unloaded here and this test
      // budgeted 500ms, which held locally and did not on a loaded macOS
      // runner. `operationDeadlineForTest` fires the same parent deadline the
      // timeout would, at a point the test controls.
      let forceOperationDeadline!: () => void;
      const operationDeadline = new Promise<void>((resolve) => {
        forceOperationDeadline = resolve;
      });
      await expect(
        withStableCorpusLease(
          root,
          () => {
            projections += 1;
            forceOperationDeadline();
            return "MUST_NOT_PUBLISH";
          },
          {
            // Below vitest's 15s testTimeout on purpose: if the
            // deterministic deadline ever fails to fire, the lease still
            // refuses with its own message instead of vitest killing the test
            // with a less useful timeout. Still ~75x the measured worst-case
            // startup, so it cannot race.
            timeoutMs: 10_000,
            workerStallPhaseForTest: "before-authorized",
            operationDeadlineForTest: operationDeadline,
          }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      // The rejection is the non-publication proof: a published projection
      // would have resolved to MUST_NOT_PUBLISH instead.
      expect(projections).toBe(1);
      // And the stalled worker has to be reaped, not merely abandoned. Without
      // this the deterministic deadline could satisfy the assertions above
      // while leaving a wedged child holding admission, which is the failure
      // this scenario exists to catch.
      await expect(
        withStableCorpusLease(root, () => "reused")
      ).resolves.toBe("reused");
    });
  });

  it("rejects an out-of-order worker protocol and remains reusable", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "protocol canary");
      const fixture = join(root, "invalid-worker.mjs");
      const emitted = join(root, "out-of-order.marker");
      writeFileSync(
        fixture,
        `import { writeFileSync } from "node:fs";\n` +
          `process.stdin.once("data", () => {\n` +
          `  writeFileSync(${JSON.stringify(emitted)}, "sent");\n` +
          `  process.stdout.write(JSON.stringify({ type: "authorized" }) + "\\n");\n` +
          `  setTimeout(() => {}, 60_000);\n` +
          `});\n`
      );
      const started = Date.now();
      await expect(
        withStableCorpusLease(root, () => "must not publish", {
          // Generous relative to startup, but not so generous that a hang
          // regression stops failing usefully: at 10s the wait plus worker
          // cleanup plus the recovery lease crowded vitest's 15s testTimeout,
          // which would replace the assertion below with a bare timeout.
          timeoutMs: 6_000,
          workerScriptForTest: fixture,
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
      // The marker is written immediately before the hostile write, so it
      // proves the fixture ran and reached that point. It cannot prove the
      // bytes were emitted and classified: the child could die in between, and
      // every refusal reads the same. Recording after the write would prove
      // more but is unreliable, because the parent kills the child on the
      // offending line. Together with a fast refusal against a budget far
      // larger than startup, this rules out the failure mode being fixed,
      // where the budget itself was the cause.
      expect(existsSync(emitted)).toBe(true);
      expect(Date.now() - started).toBeLessThan(3_000);
      await expect(withStableCorpusLease(root, () => "recovered")).resolves.toBe(
        "recovered"
      );
    });
  });

  it("accepts a bounded retry and reports its authorization duration", async () => {
    await withCorpus(async (root) => {
      const fixture = join(root, "skipped-attempt-worker.mjs");
      writeFileSync(
        fixture,
        `import { createInterface } from "node:readline";\n` +
          `const lines = createInterface({ input: process.stdin });\n` +
          `let state = 0;\n` +
          `const send = (message) => process.stdout.write(JSON.stringify(message) + "\\n");\n` +
          `lines.on("line", (line) => {\n` +
          `  const message = JSON.parse(line);\n` +
          `  if (state === 0 && message.type === "begin") {\n` +
          `    state = 1;\n` +
          `    send({ type: "phase", name: "onWatcherReady", attempt: 2 });\n` +
          `  } else if (state === 1 && message.type === "phase-result") {\n` +
          `    state = 2;\n` +
          `    send({ type: "snapshot-start", attempt: 2, canonicalRoot: "/synthetic", fileCount: 0 });\n` +
          `  } else if (state === 2 && message.type === "stream-ack") {\n` +
          `    state = 3;\n` +
          `    send({ type: "snapshot-end" });\n` +
          `  } else if (state === 3 && message.type === "finalize") {\n` +
          `    state = 4;\n` +
          `    send({ type: "authorized" });\n` +
          `  } else if (state === 4 && message.type === "acknowledged") {\n` +
          `    process.exit(0);\n` +
          `  } else {\n` +
          `    process.exit(70);\n` +
          `  }\n` +
          `});\n`
      );
      let phaseCalls = 0;
      let operationCalls = 0;
      const original = process.stderr.write;
      let diagnostics = "";
      (process.stderr as unknown as { write: unknown }).write = (chunk: unknown) => {
        diagnostics += String(chunk);
        return true;
      };
      try {
        await expect(
          withStableCorpusLease(
            root,
            (_snapshot, attempt) => {
              operationCalls += 1;
              expect(attempt).toBe(2);
              return "authorized retry";
            },
            {
              // De-raced only: this one already fails loudly rather than
              // passing falsely, because it asserts positive phase and
              // operation counts.
              timeoutMs: 5_000,
              workerScriptForTest: fixture,
              onWatcherReady: ({ attempt }) => {
                phaseCalls += 1;
                expect(attempt).toBe(2);
              },
            }
          )
        ).resolves.toBe("authorized retry");
        await new Promise((resolve) => setImmediate(resolve));
        expect(diagnostics).toMatch(
          /\[corpus-lease\] authorized \(authorization duration \d+ms\)/
        );
      } finally {
        (process.stderr as unknown as { write: unknown }).write = original;
      }
      expect(phaseCalls).toBe(1);
      expect(operationCalls).toBe(1);
    });
  });

  it("charges paced protocol transients against global memory admission", async () => {
    await withCorpus(async (root) => {
      let release!: () => void;
      const hold = new Promise<void>((resolve) => {
        release = resolve;
      });
      let ready!: () => void;
      const retained = new Promise<void>((resolve) => {
        ready = resolve;
      });
      const leanBudgets = {
        maxFileBytes: 0,
        // Without the charged 5 MiB protocol transient, two of these
        // reservations would fit below the 256 MiB process cap.
        maxCorpusBytes: 61 * 1024 * 1024,
        maxRetainedPathBytes: 1024 * 1024,
        maxFileCount: 0,
        maxDirectoryCount: 1,
        maxDirectoryEntries: 2,
        maxWatcherCount: 4,
        maxReaderCount: 1,
      };
      const first = withStableCorpusLease(root, () => 0, {
        budgets: leanBudgets,
        afterBaseline: async () => {
          ready();
          await hold;
        },
      });
      // Racing the lease surfaces an admission failure as its own error. A
      // bare `await retained` hangs to the suite timeout instead, because the
      // baseline hook that resolves it never runs.
      await Promise.race([retained, first]);
      await expect(
        withStableCorpusLease(root, () => "must not allocate", {
          budgets: leanBudgets,
        })
      ).rejects.toThrow("retained snapshots exceeded their process budget");
      release();
      await expect(first).resolves.toBe(0);
    });
  });

  it("rejects a worker line above the fixed protocol cap and recovers", async () => {
    await withCorpus(async (root) => {
      const fixture = join(root, "oversized-worker.mjs");
      const emitted = join(root, "oversized.marker");
      writeFileSync(
        fixture,
        `import { writeFileSync } from "node:fs";\n` +
          `process.stdin.once("data", () => {\n` +
          `  writeFileSync(${JSON.stringify(emitted)}, "sent");\n` +
          `  process.stdout.write("X".repeat(512 * 1024 + 1));\n` +
          `  setTimeout(() => {}, 60_000);\n` +
          `});\n`
      );
      const started = Date.now();
      await expect(
        withStableCorpusLease(root, () => "must not publish", {
          // See the out-of-order case for why 6s rather than more.
          timeoutMs: 6_000,
          workerScriptForTest: fixture,
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(existsSync(emitted)).toBe(true);
      expect(Date.now() - started).toBeLessThan(3_000);
      await expect(withStableCorpusLease(root, () => "recovered")).resolves.toBe(
        "recovered"
      );
    });
  });

  it("rejects coalesced phase, snapshot, and authorization transitions", async () => {
    await withCorpus(async (root) => {
      const fixture = join(root, "coalesced-worker.mjs");
      const emitted = join(root, "coalesced.marker");
      writeFileSync(
        fixture,
        `import { writeFileSync } from "node:fs";\n` +
          `process.stdin.once("data", () => {\n` +
          `  const lines = [\n` +
          `    { type: "phase", name: "afterBaseline", attempt: 1 },\n` +
          `    { type: "snapshot-start", attempt: 1, canonicalRoot: "/synthetic", fileCount: 0 },\n` +
          `    { type: "authorized" },\n` +
          `  ];\n` +
          `  writeFileSync(${JSON.stringify(emitted)}, "sent");\n` +
          `  process.stdout.write(lines.map(JSON.stringify).join("\\n") + "\\n");\n` +
          `  setTimeout(() => {}, 60_000);\n` +
          `});\n`
      );
      let phaseCalls = 0;
      let operationCalls = 0;
      const startedCoalesced = Date.now();
      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          {
            // See the out-of-order case for why 6s rather than more. Both
            // zero-state assertions below were equally true of a worker that
            // never started, so the marker and the fast refusal carry the
            // weight.
            timeoutMs: 6_000,
            workerScriptForTest: fixture,
            afterBaseline: () => {
              phaseCalls += 1;
            },
          }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(existsSync(emitted)).toBe(true);
      expect(Date.now() - startedCoalesced).toBeLessThan(3_000);
      expect(phaseCalls).toBe(0);
      expect(operationCalls).toBe(0);
      await expect(withStableCorpusLease(root, () => "recovered")).resolves.toBe(
        "recovered"
      );
    });
  });

  it("charges non-Markdown entries against the traversal budget", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "first.txt"), "ignored");
      writeFileSync(join(root, "second.json"), "ignored");
      writeFileSync(join(root, "third.bin"), "ignored");
      let operationCalls = 0;

      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          { budgets: { maxDirectoryEntries: 2 } }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(operationCalls).toBe(0);
    });
  });

  it("evicts idle readers while honoring a one-reader corpus cap", async () => {
    await withCorpus(async (root) => {
      mkdirSync(join(root, "first"));
      mkdirSync(join(root, "second"));
      writeFileSync(join(root, "first", "meeting.md"), "first");
      writeFileSync(join(root, "second", "meeting.md"), "second");
      let operationCalls = 0;

      const files = await withStableCorpusLease(
        root,
        (snapshot) => {
          operationCalls += 1;
          return snapshot.files.map((file) => file.relativePath);
        },
        { budgets: { maxReaderCount: 1 } }
      );
      expect(files).toEqual(["first/meeting.md", "second/meeting.md"]);
      expect(operationCalls).toBe(1);
    });
  });

  it("final verification streams fingerprints without retaining corpus content", async () => {
    await withCorpus(async (root) => {
      const firstContent = "a".repeat(64 * 1024 + 17);
      const secondContent = "b".repeat(128 * 1024 + 29);
      const totalBytes = firstContent.length + secondContent.length;
      writeFileSync(join(root, "first.md"), firstContent);
      writeFileSync(join(root, "second.md"), secondContent);
      const observations: Array<{
        fileCount: number;
        retainedContentBytes: number;
        totalBytes: number;
      }> = [];

      const result = await withStableCorpusLease(
        root,
        (snapshot) => snapshot.files.map((file) => file.content.length),
        {
          budgets: {
            maxFileBytes: secondContent.length,
            maxCorpusBytes: totalBytes,
            maxFileCount: 2,
          },
          afterFinalManifest: ({ verification }) => {
            observations.push(verification);
          },
        }
      );

      expect(result).toEqual([firstContent.length, secondContent.length]);
      expect(observations).toEqual([
        { fileCount: 2, retainedContentBytes: 0, totalBytes },
      ]);
    });
  });

  it("fails closed after watcher failure on both attempts", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "watcher failure canary");
      let watcherReadyCalls = 0;
      await expect(
        withStableCorpusLease(root, () => "unreachable", {
          onWatcherReady: ({ controls }) => {
            watcherReadyCalls += 1;
            controls.failWatcher("test watcher failure");
          },
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
      // Bounds what the refusal can be: the worker reached watcher-ready and
      // the failure was requested. It does not prove the injected failure is
      // what the parent then refused on, which would need the module to
      // distinguish its refusals.
      expect(watcherReadyCalls).toBeGreaterThanOrEqual(1);
    });
  });

  it("fails closed when the sentinel fence is not observed", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "fence timeout canary");
      // The old 25ms budget expired during worker startup, so the refusal came
      // from the authorization deadline and the fence path was never reached.
      // Timing cannot separate them here either: a suppressed fence is retried,
      // so the lease runs on to the deadline rather than ending at the fence
      // timeout. Count the hook instead. It fires once the worker reaches
      // watcher-ready, so a lease whose budget died during startup cannot
      // satisfy it. It does not prove the suppression took effect: the control
      // only queues a command, which the parent sends after the hook returns
      // and the worker applies later still.
      let watcherReadyCalls = 0;
      await expect(
        withStableCorpusLease(root, () => "unreachable", {
          timeoutMs: 3_000,
          onWatcherReady: ({ controls }) => {
            watcherReadyCalls += 1;
            controls.suppressNextFence();
          },
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(watcherReadyCalls).toBeGreaterThanOrEqual(1);
    });
  });

  it("requires and acknowledges a bounded sentinel repulse", async () => {
    await withCorpus(async (root) => {
      const nested = join(root, "nested");
      mkdirSync(nested);
      writeFileSync(join(nested, "meeting.md"), "repulse canary");
      let operationCalls = 0;

      const result = await withStableCorpusLease(
        root,
        () => {
          operationCalls += 1;
          return "authorized";
        },
        {
          timeoutMs: 2_000,
          onWatcherReady: ({ controls }) =>
            controls.requireRepulseForNextFence(),
        }
      );

      expect(result).toBe("authorized");
      expect(operationCalls).toBe(1);
    });
  });

  it("fails closed and retires empty sentinels after a pulse failure", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "pulse failure canary");
      let operationCalls = 0;

      await expect(
        withStableCorpusLease(
          root,
          () => {
            operationCalls += 1;
          },
          {
            // De-raced only. The sentinel names below show the worker
            // reached sentinel acquisition, which precedes watcher
            // registration, so they rule out a startup-consumed budget
            // without proving the injected pulse failure is what refused.
            timeoutMs: 5_000,
            onWatcherReady: ({ controls }) => controls.failNextFencePulse(),
          }
        )
      ).rejects.toThrow("stable meeting corpus authorization failed");
      expect(operationCalls).toBe(0);
      const namespace = join(root, ".minutes-corpus-lease-v1");
      const sentinels = readdirSync(namespace);
      expect(sentinels.sort()).toEqual([
        "lease-shared-0.fence",
        "lease-shared-1.fence",
      ]);
    });
  });

  it("never removes a replacement at a unique live sentinel name", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "replacement safety canary");
      const replacement = "REPLACEMENT_MUST_SURVIVE";
      const displaced: string[] = [];
      const writers: NodeJS.Timeout[] = [];

      try {
        await expect(
          withStableCorpusLease(root, () => "must not authorize", {
            // De-raced only: the hook below asserts as it runs, so a budget
            // consumed by startup fails this test rather than satisfying it.
            timeoutMs: 5_000,
            beforeFinalFence: ({ attempt, controls }) => {
              if (attempt > 1) {
                controls.failWatcher("replacement cleanup retry");
                return;
              }
              const namespace = join(root, ".minutes-corpus-lease-v1");
              const current = readdirSync(namespace).find((name) =>
                name.startsWith("lease-")
              );
              expect(current).toBeDefined();
              const original = join(namespace, current!);
              const moved = join(namespace, `displaced-${attempt}.fence`);
              renameSync(original, moved);
              displaced.push(moved);
              writeFileSync(original, replacement);
              const writer = setInterval(() => {
                writeFileSync(original, replacement);
              }, 1);
              writer.unref();
              writers.push(writer);
            },
          })
        ).rejects.toThrow("stable meeting corpus authorization failed");
      } finally {
        for (const writer of writers) clearInterval(writer);
      }

      const namespace = join(root, ".minutes-corpus-lease-v1");
      const replacements = readdirSync(namespace)
        .filter((name) => !displaced.includes(join(namespace, name)))
        .map((name) => readFileSync(join(namespace, name), "utf8"));
      expect(replacements).toContain(replacement);
      expect(displaced.map((path) => statSync(path).size)).toEqual([0]);
    });
  });

  it("reuses the same two shared slots after every authorization", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "bounded slot canary");
      for (let index = 0; index < 8; index += 1) {
        await expect(withStableCorpusLease(root, () => index)).resolves.toBe(index);
      }
      const namespace = join(root, ".minutes-corpus-lease-v1");
      const sentinels = readdirSync(namespace);
      expect(sentinels.sort()).toEqual([
        "lease-shared-0.fence",
        "lease-shared-1.fence",
      ]);
    });
  });

  it("fails closed when the reserved sentinel namespace exceeds its bound", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "sentinel budget canary");
      const namespace = join(root, ".minutes-corpus-lease-v1");
      mkdirSync(namespace, { mode: 0o700 });
      for (let index = 0; index < 129; index += 1) {
        writeFileSync(
          join(namespace, `lease-1-${index.toString(16).padStart(32, "0")}.fence`),
          "",
          { mode: 0o600 }
        );
      }
      await expect(
        withStableCorpusLease(root, () => "must not authorize")
      ).rejects.toThrow("stable meeting corpus authorization failed");
    });
  });

  it("does not let a stale legacy sentinel acknowledge a live lease", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "crash recovery canary");
      publishInterruptedSentinel(root);

      const before = readdirSync(root).filter((name) =>
        name.startsWith(".minutes-corpus-lease-v1-")
      );
      expect(before).toHaveLength(1);
      expect(statSync(join(root, before[0]!)).size).toBe(0);

      await expect(withStableCorpusLease(root, () => "recovered")).resolves.toBe(
        "recovered"
      );
      const after = readdirSync(root).filter((name) =>
        name.startsWith(".minutes-corpus-lease-v1-")
      );
      expect(after).toEqual(before);
      expect(statSync(join(root, after[0]!)).size).toBe(0);
    });
  });

  it("does not authorize a corpus mutation after an injected sentinel acknowledgement", async () => {
    await withCorpus(async (root) => {
      const source = join(root, "meeting.md");
      writeFileSync(source, "---\nsensitivity: normal\n---\npublic canary");
      let markPending!: () => void;
      let releaseFence!: () => void;
      const pending = new Promise<void>((resolve) => {
        markPending = resolve;
      });
      const hold = new Promise<void>((resolve) => {
        releaseFence = resolve;
      });
      const authorization = withStableCorpusLease(
        root,
        () => "must not authorize",
        {
          onWatcherReady: ({ attempt, controls }) => {
            if (attempt > 1) controls.failWatcher("injected acknowledgement retry");
          },
          beforeFinalFence: ({ controls }) => {
            controls.pauseNextFenceAfterPending(hold, markPending);
          },
        }
      );
      await pending;
      const namespace = join(root, ".minutes-corpus-lease-v1");
      const forgedTime = new Date(Date.now() + 10_000);
      for (const name of readdirSync(namespace)) {
        if (name.startsWith("lease-")) {
          utimesSync(join(namespace, name), forgedTime, forgedTime);
        }
      }
      writeFileSync(
        source,
        "---\nsensitivity: restricted\n---\nprivate canary"
      );
      releaseFence();
      await expect(authorization).rejects.toThrow(
        "stable meeting corpus authorization failed"
      );
    });
  });

  it("fails closed on foreign restart leftovers instead of deleting or adopting them", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "restart bound canary");
      const namespace = join(root, ".minutes-corpus-lease-v1");
      mkdirSync(namespace, { mode: 0o700 });
      for (let index = 0; index < 126; index += 1) {
        writeFileSync(
          join(namespace, `lease-999-${index.toString(16).padStart(32, "0")}.fence`),
          "",
          { mode: 0o600 }
        );
      }
      await expect(withStableCorpusLease(root, () => "authorized"))
        .rejects.toThrow("stable meeting corpus authorization failed");
      expect(readdirSync(namespace)).toHaveLength(126);
      await expect(withStableCorpusLease(root, () => "reused"))
        .rejects.toThrow("stable meeting corpus authorization failed");
      expect(readdirSync(namespace)).toHaveLength(126);
    });
  });

  it("rejects exact-byte ABA churn even when mtime is restored", async () => {
    await withCorpus(async (root) => {
      const path = join(root, "meeting.md");
      const original = "exact ABA corpus canary";
      writeFileSync(path, original);
      const initial = statSync(path);

      await expect(
        withStableCorpusLease(root, () => original, {
          beforeFinalManifest: () => {
            writeFileSync(path, "sensitivity: restricted");
            writeFileSync(path, original);
            utimesSync(path, initial.atime, initial.mtime);
          },
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
    });
  });

  it("rejects an outside hard link created after the final manifest", async () => {
    const outside = mkdtempSync(join(tmpdir(), "minutes-corpus-alias-"));
    try {
      await withCorpus(async (root) => {
        const source = join(root, "meeting.md");
        writeFileSync(source, "hard-link authorization canary");
        await expect(
          withStableCorpusLease(root, () => "must not authorize", {
            beforeFinalFence: () => {
              linkSync(source, join(outside, "alias.md"));
            },
          })
        ).rejects.toThrow("stable meeting corpus authorization failed");
      });
    } finally {
      rmSync(outside, { recursive: true, force: true });
    }
  });

  it("rejects a normal-to-restricted overwrite at the authorization fence", async () => {
    await withCorpus(async (root) => {
      const source = join(root, "meeting.md");
      writeFileSync(source, "---\nsensitivity: normal\n---\npublic canary");
      await expect(
        withStableCorpusLease(root, () => "must not authorize", {
          beforeFinalFence: () => {
            writeFileSync(
              source,
              "---\nsensitivity: restricted\n---\nprivate canary"
            );
          },
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
    });
  });

  it("orders a delayed nested mutation before the root fence", async () => {
    await withCorpus(async (root) => {
      const nested = join(root, "team");
      mkdirSync(nested);
      writeFileSync(join(nested, "meeting.md"), "nested baseline");
      await expect(
        withStableCorpusLease(root, () => "must not authorize", {
          beforeFinalFence: async () => {
            await new Promise<void>((resolve) => {
              setTimeout(() => {
                writeFileSync(join(nested, "late.md"), "late private canary");
                resolve();
              }, 5);
            });
          },
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
    });
  });

  it("rejects transient corpus membership churn", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "membership canary");
      const transient = join(root, "transient.md");

      await expect(
        withStableCorpusLease(root, () => "unreachable", {
          beforeFinalManifest: () => {
            writeFileSync(transient, "transient private canary");
            unlinkSync(transient);
          },
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
    });
  });

  it("rejects membership added inside an existing nested directory", async () => {
    await withCorpus(async (root) => {
      const nested = join(root, "team");
      mkdirSync(nested);
      writeFileSync(join(nested, "meeting.md"), "nested membership canary");
      const transient = join(nested, "transient.md");

      await expect(
        withStableCorpusLease(root, () => "unreachable", {
          beforeFinalManifest: () => {
            writeFileSync(transient, "nested transient private canary");
          },
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
    });
  });

  it("rejects replacement of the corpus root identity", async () => {
    const parent = mkdtempSync(join(tmpdir(), "minutes-corpus-root-parent-"));
    const root = join(parent, "meetings");
    mkdirSync(root);
    writeFileSync(join(root, "meeting.md"), "root identity canary");

    try {
      await expect(
        withStableCorpusLease(root, () => "unreachable", {
          beforeFinalManifest: ({ attempt }) => {
            renameSync(root, join(parent, `meetings-old-${attempt}`));
            mkdirSync(root);
          },
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
    } finally {
      rmSync(parent, { recursive: true, force: true });
    }
  });

  it("poisons admission when an asynchronous projection ignores cancellation", async () => {
    await withCorpus(async (root) => {
      writeFileSync(join(root, "meeting.md"), "uncancellable projection canary");
      let markOperationStarted!: () => void;
      const operationStarted = new Promise<void>((resolve) => {
        markOperationStarted = resolve;
      });
      let forceOperationDeadline!: () => void;
      const operationDeadline = new Promise<void>((resolve) => {
        forceOperationDeadline = resolve;
      });
      const lease = withStableCorpusLease(
        root,
        () => {
          markOperationStarted();
          return new Promise<never>(() => {});
        },
        {
          timeoutMs: 15_000,
          operationDeadlineForTest: operationDeadline,
        }
      );

      await expect(
        Promise.race([
          operationStarted.then(() => true),
          lease.then(
            () => false,
            () => false
          ),
        ])
      ).resolves.toBe(true);

      const started = Date.now();
      forceOperationDeadline();
      await expect(lease).rejects.toThrow("stable meeting corpus authorization failed");
      expect(Date.now() - started).toBeLessThan(2_500);
      await expect(
        withStableCorpusLease(root, () => "must not reuse")
      ).rejects.toThrow("killed without confirming it died");
    });
  });
});
