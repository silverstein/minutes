import { spawn, type ChildProcessWithoutNullStreams } from "child_process";
import { createHash } from "node:crypto";
import {
  lstatSync,
  realpathSync,
  statSync,
  type BigIntStats,
} from "node:fs";
import { basename, dirname } from "path";
import { TextDecoder } from "node:util";

import { nodeChildEnvironment } from "./node-child.js";

/**
 * Decode security-policy-bearing text without Unicode replacement.
 *
 * Buffer.toString("utf8") silently replaces malformed input with U+FFFD. In
 * frontmatter, that can change `sensitivity` into an unknown key and make a
 * restricted document look like a legacy document with no designation. Keep
 * the BOM behavior byte-compatible with Buffer.toString while rejecting every
 * malformed UTF-8 sequence before policy parsing.
 */
export function decodePolicyUtf8(bytes: Uint8Array): string {
  try {
    return new TextDecoder("utf-8", {
      fatal: true,
      ignoreBOM: true,
    }).decode(bytes);
  } catch {
    throw new Error("Access denied: policy-bearing text is not valid UTF-8");
  }
}

export type BoundReadExpectation = {
  parentIdentity: string;
  leafFingerprint: string;
};

export type BoundReadHooks = {
  afterFirstRead?: () => void | Promise<void>;
  /** Test/diagnostic override; production reads use the 15 second bound. */
  timeoutMs?: number;
  /** Parent authorization cancellation propagated into the bound child read. */
  signal?: AbortSignal;
  /** Hard cap enforced inside the inode-bound reader process. */
  maxBytes?: number;
  /** Hard cap on cached/active inode-bound reader children. */
  maxReaders?: number;
  /** Test/diagnostic override for concurrent requests on one bound child. */
  maxInFlightPerReader?: number;
  /** Test/diagnostic override for concurrent requests across all children. */
  maxInFlightGlobal?: number;
  /** Test/diagnostic override for aggregate potentially retained bytes. */
  maxReservedBytes?: number;
  /** @internal Deterministic fail-closed retirement seam for hostile tests. */
  retireChildForTest?: (terminate: () => boolean) => boolean;
};

/** Default hard cap for any descriptor-bound file read. */
export const DEFAULT_BOUND_READ_MAX_BYTES = 16 * 1024 * 1024;
export const DEFAULT_BOUND_READER_MAX_CHILDREN = 2;
export const DEFAULT_BOUND_READ_TIMEOUT_MS = 15_000;
export const DEFAULT_BOUND_READER_MAX_IN_FLIGHT = 2;
export const DEFAULT_BOUND_READER_MAX_GLOBAL_IN_FLIGHT = 4;
export const DEFAULT_BOUND_READER_MAX_RESERVED_BYTES = 384 * 1024 * 1024;
const BOUND_READER_CHILD_BASELINE_BYTES = 32 * 1024 * 1024;
const BOUND_READER_CONTENT_AMPLIFICATION = 20;
const BOUND_READER_RETIRE_CONFIRM_MS = 2_000;
const MAX_BOUND_READER_LINE_CHARS =
  Math.ceil((DEFAULT_BOUND_READ_MAX_BYTES * 4) / 3) + 1024 * 1024;

export type BoundFileRevision = {
  byteLength: number;
  leafFingerprint: string;
  sha256: string;
};

export type BoundTextFileRead = {
  content: Buffer;
  revision: BoundFileRevision;
};

/**
 * Absorbed once, then never again: an 'error' event on stderr with no listener
 * is fatal to the host process.
 */
let stderrErrorsAbsorbed = false;

/**
 * Write one operator-visible diagnostic line, without letting a broken stderr
 * become fatal.
 *
 * A try/catch around the write is not sufficient. When the consumer of the
 * pipe has gone, the write fails with an asynchronous EPIPE delivered as an
 * 'error' event on a later tick, which no surrounding catch can see, and an
 * 'error' event with no listener kills the process. Absorbing it is the same
 * call corpus-lease.ts already makes for a killed worker's stdin, for the same
 * reason: a diagnostic that cannot be delivered must never escalate into a
 * failure of the thing it was describing.
 *
 * Callers defer this themselves so a blocked TTY or file cannot delay a
 * refusal that has already been decided.
 */
export function writeOperatorDiagnostic(line: string): void {
  try {
    if (!stderrErrorsAbsorbed) {
      stderrErrorsAbsorbed = true;
      process.stderr.on("error", () => {});
    }
    process.stderr.write(line);
  } catch {
    // A synchronous failure is equally non-fatal here.
  }
}

export function boundReadIdentity(info: BigIntStats): string {
  return `${info.dev}:${info.ino}`;
}

export function boundReadFingerprint(info: BigIntStats): string {
  return [
    info.dev,
    info.ino,
    info.size,
    info.mtimeNs,
    info.ctimeNs,
    info.birthtimeNs,
    info.mode,
    info.nlink,
  ].join(":");
}

/** Capture the exact already-canonical leaf revision before a deferred bound read. */
export function captureBoundReadExpectation(
  canonicalPath: string
): BoundReadExpectation {
  const parent = dirname(canonicalPath);
  const parentInfo = statSync(parent, { bigint: true });
  const lexical = lstatSync(canonicalPath, { bigint: true });
  const live = statSync(canonicalPath, { bigint: true });
  const parentAfter = statSync(parent, { bigint: true });
  if (
    !parentInfo.isDirectory() ||
    !parentAfter.isDirectory() ||
    realpathSync(parent) !== parent ||
    boundReadIdentity(parentInfo) !== boundReadIdentity(parentAfter) ||
    lexical.isSymbolicLink() ||
    !lexical.isFile() ||
    lexical.nlink !== 1n ||
    !live.isFile() ||
    live.nlink !== 1n ||
    realpathSync(canonicalPath) !== canonicalPath ||
    boundReadFingerprint(lexical) !== boundReadFingerprint(live)
  ) {
    throw new Error("Access denied: source identity is not stable");
  }
  return {
    parentIdentity: boundReadIdentity(parentInfo),
    leafFingerprint: boundReadFingerprint(live),
  };
}

type PendingRead = {
  resolve: (result: BoundReaderResult) => void;
  reject: (error: Error) => void;
  afterFirstRead?: () => void | Promise<void>;
  maxBytes: number;
  returnContent: boolean;
  reservedBytes: number;
  retireChildForTest?: (terminate: () => boolean) => boolean;
  admissionReleased: boolean;
  timeout: NodeJS.Timeout;
  signal?: AbortSignal;
  abortListener?: () => void;
};

type BoundReaderResult = {
  content?: Buffer;
  revision: BoundFileRevision;
};

// The helper deliberately opens only a basename from a process whose cwd was
// established at the already-canonical parent. On Unix cwd is an inode-bound
// directory reference; on Windows the process keeps the directory open. A
// rename, symlink, or junction swap therefore cannot redirect the leaf open to
// a different parent. Rechecking realpath(".") before and after the read makes
// a displaced parent fail closed instead of silently retaining stale scope.
const BOUND_READER_SOURCE = String.raw`
const crypto = require("node:crypto");
const fs = require("node:fs");
const readline = require("node:readline");

const expectedParent = process.env.MINUTES_BOUND_PARENT;
if (!expectedParent) process.exit(70);
const noFollow = process.platform === "win32" ? 0 : (fs.constants.O_NOFOLLOW || 0);
const pending = new Map();
let idleTimer;
let pendingWrites = 0;

function send(value) {
  clearTimeout(idleTimer);
  pendingWrites += 1;
  process.stdout.write(JSON.stringify(value) + "\n", () => {
    pendingWrites -= 1;
    scheduleExit();
  });
}

function scheduleExit() {
  clearTimeout(idleTimer);
  if (pending.size !== 0 || pendingWrites !== 0) return;
  idleTimer = setTimeout(() => process.exit(0), 750);
}

function fingerprint(info) {
  return [
    String(info.dev),
    String(info.ino),
    String(info.size),
    String(info.mtimeNs ?? info.mtimeMs),
    String(info.ctimeNs ?? info.ctimeMs),
    String(info.birthtimeNs ?? info.birthtimeMs),
    String(info.mode),
    String(info.nlink),
  ].join(":");
}

function identity(info) {
  return String(info.dev) + ":" + String(info.ino);
}

function isSingleLink(info) {
  return String(info.nlink) === "1";
}

function parentIsBound() {
  const info = fs.statSync(".", { bigint: true });
  const expected = fs.statSync(expectedParent, { bigint: true });
  return (
    info.isDirectory() &&
    expected.isDirectory() &&
    identity(info) === identity(expected) &&
    fs.realpathSync(expectedParent) === expectedParent
  );
}

function scanFromStart(fd, maxBytes, retainContent) {
  const chunks = retainContent ? [] : null;
  const hash = crypto.createHash("sha256");
  const scratch = retainContent
    ? null
    : Buffer.allocUnsafe(Math.min(64 * 1024, Math.max(1, maxBytes)));
  let position = 0;
  for (;;) {
    if (position >= maxBytes) {
      const probe = Buffer.allocUnsafe(1);
      if (fs.readSync(fd, probe, 0, 1, position) !== 0) throw new Error("too-large");
      break;
    }
    const length = Math.min(64 * 1024, maxBytes - position);
    const chunk = scratch || Buffer.allocUnsafe(length);
    const count = fs.readSync(fd, chunk, 0, length, position);
    if (count === 0) break;
    const bytes = chunk.subarray(0, count);
    hash.update(bytes);
    if (chunks) chunks.push(bytes);
    position += count;
  }
  return {
    byteLength: position,
    content: chunks ? Buffer.concat(chunks, position) : undefined,
    sha256: hash.digest("hex"),
  };
}

function finishRead(state) {
  const { id, name, expectedPath, fd, before, first, maxBytes, returnContent } = state;
  try {
    // The validation pass streams only a digest. It never retains a second
    // content-sized allocation alongside the authorized snapshot bytes.
    const second = scanFromStart(fd, maxBytes, false);
    const after = fs.fstatSync(fd, { bigint: true });
    const lexical = fs.lstatSync(name, { bigint: true });
    const live = fs.statSync(name, { bigint: true });
    const canonical = fs.realpathSync(name);
    if (
      !parentIsBound() ||
      lexical.isSymbolicLink() ||
      !lexical.isFile() ||
      !isSingleLink(lexical) ||
      !after.isFile() ||
      !isSingleLink(after) ||
      !live.isFile() ||
      !isSingleLink(live) ||
      canonical !== expectedPath ||
      identity(before) !== identity(after) ||
      identity(after) !== identity(live) ||
      fingerprint(before) !== fingerprint(after) ||
      fingerprint(after) !== fingerprint(lexical) ||
      fingerprint(after) !== fingerprint(live) ||
      first.byteLength !== second.byteLength ||
      first.sha256 !== second.sha256
    ) {
      throw new Error("unstable");
    }
    send({
      id,
      ok: true,
      byteLength: first.byteLength,
      content: returnContent ? first.content.toString("base64") : undefined,
      leafFingerprint: fingerprint(after),
      sha256: first.sha256,
    });
  } catch {
    send({ id, ok: false });
  } finally {
    try { fs.closeSync(fd); } catch {}
    scheduleExit();
  }
}

function startRead(request) {
  const { id, name, expectedPath, pauseAfterFirst, maxBytes, returnContent } = request;
  let fd;
  try {
    if (
      typeof id !== "number" ||
      typeof name !== "string" ||
      typeof expectedPath !== "string" ||
      name.length === 0 ||
      name === "." ||
      name === ".." ||
      name.includes("/") ||
      name.includes("\\") ||
      !Number.isSafeInteger(maxBytes) ||
      maxBytes < 0 ||
      typeof returnContent !== "boolean" ||
      !parentIsBound()
    ) {
      throw new Error("invalid");
    }
    const lexical = fs.lstatSync(name, { bigint: true });
    if (lexical.isSymbolicLink() || !lexical.isFile() || !isSingleLink(lexical)) {
      throw new Error("not-file");
    }
    fd = fs.openSync(name, fs.constants.O_RDONLY | noFollow);
    const before = fs.fstatSync(fd, { bigint: true });
    if (!before.isFile() || !isSingleLink(before)) throw new Error("not-file");
    const first = scanFromStart(fd, maxBytes, returnContent);
    const state = { id, name, expectedPath, fd, before, first, maxBytes, returnContent };
    if (pauseAfterFirst) {
      pending.set(id, state);
      send({ id, stage: "first" });
    } else {
      finishRead(state);
    }
  } catch {
    if (fd !== undefined) {
      try { fs.closeSync(fd); } catch {}
    }
    send({ id, ok: false });
    scheduleExit();
  }
}

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on("line", (line) => {
  clearTimeout(idleTimer);
  try {
    const request = JSON.parse(line);
    if (request && request.continue === true) {
      const state = pending.get(request.id);
      pending.delete(request.id);
      if (state) finishRead(state);
      else send({ id: request.id, ok: false });
    } else {
      startRead(request);
    }
  } catch {
    send({ id: -1, ok: false });
    scheduleExit();
  }
});
lines.on("close", () => process.exit(0));
scheduleExit();
`;

let nextRequestId = 1;
const readers = new Map<string, BoundParentReader>();
const liveReaders = new Set<BoundParentReader>();
let globalInFlightReads = 0;
let globalReservedReadBytes = 0;
let globalReaderProcessCount = 0;
let globalReaderProcessBytes = 0;

function releaseBoundReadAdmission(pending: PendingRead): void {
  if (pending.admissionReleased) return;
  pending.admissionReleased = true;
  globalInFlightReads = Math.max(0, globalInFlightReads - 1);
  globalReservedReadBytes = Math.max(
    0,
    globalReservedReadBytes - pending.reservedBytes
  );
}

function detachBoundReadAbort(pending: PendingRead): void {
  if (pending.signal && pending.abortListener) {
    pending.signal.removeEventListener("abort", pending.abortListener);
  }
  pending.abortListener = undefined;
}

class BoundParentReader {
  private readonly child: ChildProcessWithoutNullStreams;
  private readonly pending = new Map<number, PendingRead>();
  private stdout = "";
  private stdoutScanOffset = 0;
  private retiring = false;
  private terminated = false;
  private processReservationReleased = false;
  private poisonedReadReservationBytes = 0;
  private retireChildForTest?: (terminate: () => boolean) => boolean;
  private readonly termination: Promise<void>;
  private resolveTermination!: () => void;

  constructor(
    private readonly parent: string,
    private readonly processReservationBytes: number
  ) {
    this.termination = new Promise<void>((resolve) => {
      this.resolveTermination = resolve;
    });
    this.child = spawn(process.execPath, ["-e", BOUND_READER_SOURCE], {
      cwd: parent,
      env: nodeChildEnvironment({ ...process.env, MINUTES_BOUND_PARENT: parent }),
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    this.child.stdout.setEncoding("utf8");
    this.child.stderr.resume();
    this.child.stdout.on("data", (chunk: string) => this.onStdout(chunk));
    this.child.stdin.on("error", () => this.requestRetirement(true));
    this.child.on("error", () => this.requestRetirement(true));
    this.child.on("exit", () => this.finishTermination());
    this.child.on("close", () => this.finishTermination());
    liveReaders.add(this);
  }

  isUsable(): boolean {
    return !this.retiring && !this.terminated && this.child.stdin.writable;
  }

  evictIfIdle(): Promise<void> | null {
    if (this.pending.size !== 0 || this.terminated) return null;
    // Starting termination does not free a process slot. The caller fails
    // closed until exit/close confirms that the child is actually reaped.
    this.requestRetirement(true);
    return this.termination;
  }

  retireForProcessShutdown(): Promise<void> {
    // Process shutdown is an unconditional ownership boundary. Test seams may
    // simulate a failed kill while exercising admission accounting, but they
    // must not survive beyond the process that owns the helper.
    this.retireChildForTest = undefined;
    this.requestRetirement(true);
    return this.termination;
  }

  read(
    expectedPath: string,
    returnContent: boolean,
    hooks: BoundReadHooks = {}
  ): Promise<BoundReaderResult> {
    if (!this.isUsable()) {
      return Promise.reject(new Error("Access denied: bound reader unavailable"));
    }
    if (hooks.signal?.aborted) {
      return Promise.reject(new Error("Access denied: bound read aborted"));
    }
    const maxBytes = hooks.maxBytes ?? DEFAULT_BOUND_READ_MAX_BYTES;
    const timeoutMs = hooks.timeoutMs ?? DEFAULT_BOUND_READ_TIMEOUT_MS;
    const maxInFlightPerReader =
      hooks.maxInFlightPerReader ?? DEFAULT_BOUND_READER_MAX_IN_FLIGHT;
    const maxInFlightGlobal =
      hooks.maxInFlightGlobal ?? DEFAULT_BOUND_READER_MAX_GLOBAL_IN_FLIGHT;
    const maxReservedBytes =
      hooks.maxReservedBytes ?? DEFAULT_BOUND_READER_MAX_RESERVED_BYTES;
    // One response may coexist as child chunks, Buffer.concat, base64 and JSON
    // strings, pipe buffers, the parent's accumulated line/parser string, and
    // the decoded Buffer. Charge the whole process-tree peak rather than only
    // the final payload.
    const reservedBytes = returnContent
      ? maxBytes * BOUND_READER_CONTENT_AMPLIFICATION
      : Math.max(1024 * 1024, Math.min(maxBytes, 64 * 1024));
    // Which of these tripped is the whole diagnosis, and the thrown message
    // deliberately does not say: callers must not learn why a read was
    // refused. The reason goes to stderr, which is operator-visible only, the
    // same split corpus-lease.ts uses for authorization denials.
    //
    // Worth the detail because this refusal is reachable from process-global
    // counters, so the read that gets refused is often not the one at fault.
    // A CI failure here previously gave no way to tell an exhausted byte
    // reservation from an in-flight cap (#617).
    const refusal =
      !Number.isSafeInteger(reservedBytes) || reservedBytes < 0
        ? `implausible reservation ${reservedBytes}`
        : this.pending.size >= maxInFlightPerReader
          ? `reader in-flight ${this.pending.size}/${maxInFlightPerReader}`
          : globalInFlightReads >= maxInFlightGlobal
            ? `global in-flight ${globalInFlightReads}/${maxInFlightGlobal}`
            : globalReaderProcessBytes + globalReservedReadBytes >
                maxReservedBytes - reservedBytes
              ? `global bytes ${globalReaderProcessBytes + globalReservedReadBytes} + ` +
                `${reservedBytes} requested > ${maxReservedBytes}`
              : null;
    if (refusal !== null) {
      // Never inside the throw path's critical section, and never allowed to
      // turn a clean refusal into a crash.
      setImmediate(() => {
        writeOperatorDiagnostic(`[bound-reader] refused: ${refusal}\n`);
      });
      throw new Error("Access denied: bound reader capacity exceeded");
    }

    // Admission is reserved synchronously before any request id, timer,
    // promise, or child stdin allocation is created.
    globalInFlightReads += 1;
    globalReservedReadBytes += reservedBytes;
    const id = nextRequestId++;
    return new Promise<BoundReaderResult>((resolve, reject) => {
      const timeout = setTimeout(() => {
        const pending = this.pending.get(id);
        if (!pending) return;
        this.pending.delete(id);
        detachBoundReadAbort(pending);
        this.poisonBoundReadAdmission(pending);
        reject(new Error("Access denied: bound read timed out"));
        this.requestRetirement(true, pending.retireChildForTest);
      }, timeoutMs);
      timeout.unref();
      const pending: PendingRead = {
        resolve,
        reject,
        afterFirstRead: hooks.afterFirstRead,
        maxBytes,
        returnContent,
        reservedBytes,
        retireChildForTest: hooks.retireChildForTest,
        admissionReleased: false,
        timeout,
        signal: hooks.signal,
      };
      this.pending.set(id, pending);
      if (hooks.signal) {
        pending.abortListener = () => {
          if (this.pending.get(id) !== pending) return;
          clearTimeout(pending.timeout);
          this.pending.delete(id);
          detachBoundReadAbort(pending);
          this.poisonBoundReadAdmission(pending);
          reject(new Error("Access denied: bound read aborted"));
          this.requestRetirement(true, pending.retireChildForTest);
        };
        hooks.signal.addEventListener("abort", pending.abortListener, {
          once: true,
        });
        if (hooks.signal.aborted) pending.abortListener();
      }
      if (!this.pending.has(id)) return;
      try {
        this.child.stdin.write(
          `${JSON.stringify({
            id,
            name: basename(expectedPath),
            expectedPath,
            pauseAfterFirst: hooks.afterFirstRead !== undefined,
            maxBytes,
            returnContent,
          })}\n`,
          (error) => {
            if (error) this.requestRetirement(true);
          }
        );
      } catch {
        this.requestRetirement(true);
      }
    });
  }

  private onStdout(chunk: string): void {
    this.stdout += chunk;
    if (this.stdout.length > MAX_BOUND_READER_LINE_CHARS) {
      this.requestRetirement(true);
      return;
    }
    for (;;) {
      const newline = this.stdout.indexOf("\n", this.stdoutScanOffset);
      if (newline < 0) {
        this.stdoutScanOffset = this.stdout.length;
        return;
      }
      const line = this.stdout.slice(0, newline);
      this.stdout = this.stdout.slice(newline + 1);
      this.stdoutScanOffset = 0;
      void this.onMessage(line);
    }
  }

  private async onMessage(line: string): Promise<void> {
    let message: any;
    try {
      message = JSON.parse(line);
    } catch {
      this.requestRetirement(true);
      return;
    }
    const pending = this.pending.get(message?.id);
    if (!pending) return;
    if (message.stage === "first") {
      try {
        await pending.afterFirstRead?.();
        if (this.pending.get(message.id) !== pending || !this.isUsable()) return;
        this.child.stdin.write(
          `${JSON.stringify({ id: message.id, continue: true })}\n`,
          (error) => {
            if (error) this.requestRetirement(true);
          }
        );
      } catch {
        clearTimeout(pending.timeout);
        this.pending.delete(message.id);
        detachBoundReadAbort(pending);
        this.poisonBoundReadAdmission(pending);
        pending.reject(new Error("Access denied: read validation hook failed"));
        this.requestRetirement(true);
      }
      return;
    }
    clearTimeout(pending.timeout);
    this.pending.delete(message.id);
    detachBoundReadAbort(pending);
    try {
      if (
        message.ok !== true ||
        !Number.isSafeInteger(message.byteLength) ||
        message.byteLength < 0 ||
        message.byteLength > pending.maxBytes ||
        typeof message.leafFingerprint !== "string" ||
        !/^[a-f0-9]{64}$/.test(message.sha256) ||
        (pending.returnContent
          ? typeof message.content !== "string"
          : message.content !== undefined)
      ) {
        pending.reject(
          new Error("Access denied: file changed while it was being read")
        );
        return;
      }
      const content = pending.returnContent
        ? Buffer.from(message.content, "base64")
        : undefined;
      if (
        content &&
        (content.byteLength !== message.byteLength ||
          createHash("sha256").update(content).digest("hex") !== message.sha256)
      ) {
        pending.reject(
          new Error("Access denied: file changed while it was being read")
        );
        return;
      }
      pending.resolve({
        content,
        revision: Object.freeze({
          byteLength: message.byteLength,
          leafFingerprint: message.leafFingerprint,
          sha256: message.sha256,
        }),
      });
    } finally {
      releaseBoundReadAdmission(pending);
    }
  }

  private poisonBoundReadAdmission(pending: PendingRead): void {
    if (pending.admissionReleased) return;
    pending.admissionReleased = true;
    globalInFlightReads = Math.max(0, globalInFlightReads - 1);
    // The request promise is settled, but a killed or blocked helper may still
    // retain its content/base64/JSON/pipe buffers. Transfer the reservation to
    // the retiring reader and release it only after exit/close is confirmed.
    this.poisonedReadReservationBytes += pending.reservedBytes;
  }

  private failAll(retainReservationsUntilExit: boolean): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout);
      detachBoundReadAbort(pending);
      if (retainReservationsUntilExit) {
        this.poisonBoundReadAdmission(pending);
      } else {
        releaseBoundReadAdmission(pending);
      }
      pending.reject(new Error("Access denied: bound reader exited"));
    }
    this.pending.clear();
  }

  private requestRetirement(
    kill: boolean,
    retireChildForTest?: (terminate: () => boolean) => boolean
  ): void {
    if (this.terminated) return;
    if (retireChildForTest) this.retireChildForTest = retireChildForTest;
    if (!this.retiring) {
      this.retiring = true;
      this.failAll(true);
    }
    if (kill) {
      try {
        const terminate = () => this.child.kill();
        if (this.retireChildForTest) {
          this.retireChildForTest(terminate);
        } else {
          terminate();
        }
      } catch {
        // Keep the process reservation and map slot poisoned. Only a later
        // exit/close event may release capacity.
      }
    }
  }

  private finishTermination(): void {
    if (this.terminated) return;
    this.terminated = true;
    this.retiring = true;
    // A natural exit can arrive without a preceding retirement request.
    // Settle those requests normally now that no child memory can survive.
    this.failAll(false);
    if (this.poisonedReadReservationBytes !== 0) {
      globalReservedReadBytes = Math.max(
        0,
        globalReservedReadBytes - this.poisonedReadReservationBytes
      );
      this.poisonedReadReservationBytes = 0;
    }
    if (!this.processReservationReleased) {
      this.processReservationReleased = true;
      globalReaderProcessCount = Math.max(0, globalReaderProcessCount - 1);
      globalReaderProcessBytes = Math.max(
        0,
        globalReaderProcessBytes - this.processReservationBytes
      );
    }
    if (readers.get(this.parent) === this) readers.delete(this.parent);
    liveReaders.delete(this);
    this.resolveTermination();
  }
}

/**
 * Reap every helper owned by this process before its filesystem scope is
 * released. This is intentionally internal to worker/test lifecycle code.
 */
export async function retireBoundReadersForProcessShutdown(): Promise<void> {
  const terminations = [...liveReaders].map((reader) =>
    reader.retireForProcessShutdown()
  );
  const confirmations = await Promise.all(
    terminations.map((termination) => confirmReaderTermination(termination))
  );
  if (confirmations.some((confirmed) => !confirmed)) {
    throw new Error("Access denied: bound reader could not be retired");
  }
}

/**
 * Read one already-canonical file through an OS-bound parent directory.
 * Callers remain responsible for root containment and extension checks before
 * invoking this function.
 */
export async function readTextFileFromBoundParent(
  canonicalPath: string,
  hooks: BoundReadHooks = {}
): Promise<Buffer> {
  return (await readTextFileWithRevisionFromBoundParent(canonicalPath, hooks)).content;
}

/** Read snapshot content plus its descriptor-bound metadata and byte digest. */
export async function readTextFileWithRevisionFromBoundParent(
  canonicalPath: string,
  hooks: BoundReadHooks = {}
): Promise<BoundTextFileRead> {
  const result = await readFromBoundParent(canonicalPath, true, hooks);
  if (!result.content) {
    throw new Error("Access denied: bound reader returned no content");
  }
  return { content: result.content, revision: result.revision };
}

/** Verify one file without retaining or returning its content bytes. */
export async function fingerprintTextFileFromBoundParent(
  canonicalPath: string,
  hooks: BoundReadHooks = {}
): Promise<BoundFileRevision> {
  return (await readFromBoundParent(canonicalPath, false, hooks)).revision;
}

async function confirmReaderTermination(
  termination: Promise<void>
): Promise<boolean> {
  return Promise.race([
    termination.then(() => true),
    new Promise<boolean>((resolve) => {
      const timeout = setTimeout(
        () => resolve(false),
        BOUND_READER_RETIRE_CONFIRM_MS
      );
      timeout.unref();
    }),
  ]);
}

async function readFromBoundParent(
  canonicalPath: string,
  returnContent: boolean,
  hooks: BoundReadHooks
): Promise<BoundReaderResult> {
  const parent = dirname(canonicalPath);
  const requestedMaxReaders = hooks.maxReaders ?? DEFAULT_BOUND_READER_MAX_CHILDREN;
  const requestedMaxBytes = hooks.maxBytes ?? DEFAULT_BOUND_READ_MAX_BYTES;
  const requestedTimeoutMs = hooks.timeoutMs ?? DEFAULT_BOUND_READ_TIMEOUT_MS;
  const requestedMaxInFlightPerReader =
    hooks.maxInFlightPerReader ?? DEFAULT_BOUND_READER_MAX_IN_FLIGHT;
  const requestedMaxInFlightGlobal =
    hooks.maxInFlightGlobal ?? DEFAULT_BOUND_READER_MAX_GLOBAL_IN_FLIGHT;
  const requestedMaxReservedBytes =
    hooks.maxReservedBytes ?? DEFAULT_BOUND_READER_MAX_RESERVED_BYTES;
  if (!Number.isSafeInteger(requestedMaxReaders) || requestedMaxReaders < 1) {
    throw new Error("Access denied: invalid bound reader budget");
  }
  if (!Number.isSafeInteger(requestedMaxBytes) || requestedMaxBytes < 0) {
    throw new Error("Access denied: invalid bound read budget");
  }
  if (!Number.isSafeInteger(requestedTimeoutMs) || requestedTimeoutMs < 1) {
    throw new Error("Access denied: invalid bound read timeout");
  }
  if (
    !Number.isSafeInteger(requestedMaxInFlightPerReader) ||
    requestedMaxInFlightPerReader < 1 ||
    !Number.isSafeInteger(requestedMaxInFlightGlobal) ||
    requestedMaxInFlightGlobal < 1 ||
    !Number.isSafeInteger(requestedMaxReservedBytes) ||
    requestedMaxReservedBytes < 0
  ) {
    throw new Error("Access denied: invalid bound reader admission budget");
  }
  const maxReaders = Math.min(
    requestedMaxReaders,
    DEFAULT_BOUND_READER_MAX_CHILDREN
  );
  const boundedHooks: BoundReadHooks = {
    ...hooks,
    maxReaders,
    maxBytes: Math.min(requestedMaxBytes, DEFAULT_BOUND_READ_MAX_BYTES),
    timeoutMs: Math.min(requestedTimeoutMs, DEFAULT_BOUND_READ_TIMEOUT_MS),
    maxInFlightPerReader: Math.min(
      requestedMaxInFlightPerReader,
      DEFAULT_BOUND_READER_MAX_IN_FLIGHT
    ),
    maxInFlightGlobal: Math.min(
      requestedMaxInFlightGlobal,
      DEFAULT_BOUND_READER_MAX_GLOBAL_IN_FLIGHT
    ),
    maxReservedBytes: Math.min(
      requestedMaxReservedBytes,
      DEFAULT_BOUND_READER_MAX_RESERVED_BYTES
    ),
  };
  let reader = readers.get(parent);
  if (reader && !reader.isUsable()) {
    const termination = reader.evictIfIdle();
    if (!termination || !(await confirmReaderTermination(termination))) {
      throw new Error("Access denied: bound reader capacity exceeded");
    }
    reader = readers.get(parent);
  }
  if (!reader) {
    while (globalReaderProcessCount >= maxReaders) {
      let termination: Promise<void> | null = null;
      for (const candidate of liveReaders) {
        termination = candidate.evictIfIdle();
        if (termination) break;
      }
      if (!termination) {
        throw new Error("Access denied: bound reader capacity exceeded");
      }
      if (!(await confirmReaderTermination(termination))) {
        throw new Error("Access denied: bound reader capacity exceeded");
      }
    }
    if (
      globalReaderProcessBytes + globalReservedReadBytes >
      boundedHooks.maxReservedBytes! - BOUND_READER_CHILD_BASELINE_BYTES
    ) {
      throw new Error("Access denied: bound reader capacity exceeded");
    }
    globalReaderProcessCount += 1;
    globalReaderProcessBytes += BOUND_READER_CHILD_BASELINE_BYTES;
    try {
      reader = new BoundParentReader(parent, BOUND_READER_CHILD_BASELINE_BYTES);
    } catch (error) {
      globalReaderProcessCount = Math.max(0, globalReaderProcessCount - 1);
      globalReaderProcessBytes = Math.max(
        0,
        globalReaderProcessBytes - BOUND_READER_CHILD_BASELINE_BYTES
      );
      throw error;
    }
    readers.set(parent, reader);
  } else {
    // Map insertion order is the bounded LRU. Refresh the current parent after
    // every hit so eviction prefers the least-recently-used idle child.
    readers.delete(parent);
    readers.set(parent, reader);
  }
  return reader.read(canonicalPath, returnContent, boundedHooks);
}
