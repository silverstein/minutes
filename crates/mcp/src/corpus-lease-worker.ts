import { writeSync } from "node:fs";
import { createInterface } from "node:readline";

import {
  corpusDenialReason,
  validateCorpusDenialReason,
  markCorpusLeaseWorkerProcess,
  runCorpusLeaseWorkerRequest,
  type CorpusLeaseWorkerBridge,
  type CorpusLeaseWorkerRequest,
} from "./corpus-lease.js";
import { retireBoundReadersForProcessShutdown } from "./secure-read.js";

// Content is sent in 64 KiB raw chunks, so every control line stays far below
// this fixed ceiling and never scales with the full corpus size.
const MAX_CONTROL_LINE_BYTES = 512 * 1024;
markCorpusLeaseWorkerProcess();

function fail(reason: unknown = "internal"): never {
  try {
    const message = { type: "denied", reason: validateCorpusDenialReason(reason) };
    writeSync(1, JSON.stringify(message) + "\n");
  } catch {
    // A broken pipe must not expose an error or prevent termination.
  }
  process.exit(70);
}

function send(message: unknown): void {
  const serialized = JSON.stringify(message);
  if (Buffer.byteLength(serialized) > MAX_CONTROL_LINE_BYTES) fail();
  process.stdout.write(serialized + "\n");
}

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
const responses: any[] = [];
const responseWaiters: Array<(value: any) => void> = [];
const pauseWaiters = new Map<number, () => void>();

lines.on("line", (line) => {
  if (Buffer.byteLength(line) > MAX_CONTROL_LINE_BYTES) fail();
  let message: any;
  try {
    message = JSON.parse(line);
  } catch {
    fail();
  }
  if (message?.type === "resume" && Number.isSafeInteger(message.id)) {
    const resume = pauseWaiters.get(message.id);
    if (!resume) fail();
    pauseWaiters.delete(message.id);
    resume();
    return;
  }
  const waiter = responseWaiters.shift();
  if (waiter) waiter(message);
  else responses.push(message);
});
lines.on("close", () => fail());

function nextResponse(): Promise<any> {
  const ready = responses.shift();
  if (ready !== undefined) return Promise.resolve(ready);
  return new Promise((resolve) => responseWaiters.push(resolve));
}

const bridge: CorpusLeaseWorkerBridge = {
  exchange: async (message) => {
    send(message);
    return nextResponse();
  },
  pause: (id, reservedEvent) => {
    if (!Number.isSafeInteger(id) || pauseWaiters.has(id)) fail();
    let resume!: () => void;
    const promise = new Promise<void>((resolve) => {
      resume = resolve;
    });
    pauseWaiters.set(id, resume);
    return {
      promise,
      onReserved: () => send({ type: reservedEvent, id }),
    };
  },
};

let denialReason: string | undefined;
try {
  const first = await nextResponse();
  if (
    !first ||
    first.type !== "begin" ||
    !first.request ||
    typeof first.request.root !== "string" ||
    !first.request.budgets ||
    !Number.isSafeInteger(first.request.timeoutMs) ||
    !Array.isArray(first.request.hookNames)
  ) {
    fail();
  }
  await runCorpusLeaseWorkerRequest(
    first.request as CorpusLeaseWorkerRequest,
    bridge
  );
} catch (error) {
  denialReason = corpusDenialReason(error);
}
try {
  await retireBoundReadersForProcessShutdown();
} catch (error) {
  denialReason ??= corpusDenialReason(error);
}
if (denialReason !== undefined) fail(denialReason);
process.exit(0);
