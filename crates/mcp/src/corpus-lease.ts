import { createHash, randomBytes } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import {
  closeSync,
  constants,
  existsSync,
  fstatSync,
  lstatSync,
  openSync,
  readSync,
  realpathSync,
  statSync,
  watch,
  type Dirent,
  type FSWatcher,
} from "node:fs";
import { lstat, mkdir, open, opendir, realpath, stat } from "node:fs/promises";
import { basename, dirname, extname, isAbsolute, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import {
  decodePolicyUtf8,
  fingerprintTextFileFromBoundParent,
  readTextFileWithRevisionFromBoundParent,
  type BoundFileRevision,
  writeOperatorDiagnostic,
} from "./secure-read.js";
import { nodeChildEnvironment } from "./node-child.js";

const MAX_AUTHORIZATION_ATTEMPTS = 2;
const DEFAULT_FENCE_TIMEOUT_MS = 5_000;
const DEFAULT_AUTHORIZATION_TIMEOUT_MS = 15_000;
const MAX_ACTIVE_WATCHERS = 64;
// Snapshot content is retained as JavaScript strings, whose backing storage
// may require two bytes per source byte. Reserve that worst case for the full
// lease so concurrent agent requests cannot each retain an 80 MiB corpus.
const MAX_RETAINED_CORPUS_MEMORY_BYTES = 256 * 1024 * 1024;
const RETAINED_FILE_OBJECT_OVERHEAD_BYTES = 2 * 1024;
const RETAINED_DIRECTORY_ENTRY_OVERHEAD_BYTES = 2 * 1024;
const RETAINED_DIRECTORY_OVERHEAD_BYTES = 4 * 1024;
const MAX_RETAINED_SENTINELS = MAX_ACTIVE_WATCHERS * 2;
const MAX_SENTINEL_NAMESPACE_ENTRIES = 2;
const SENTINEL_NAMESPACE = ".minutes-corpus-lease-v1";
const SENTINEL_BASENAME = /^lease-shared-[01]\.fence$/;
const SENTINEL_TOKEN_BYTES = 32;
const INACTIVE_CORPUS_DIRS = new Set([
  "archive",
  "processed",
  "failed",
  "failed-captures",
]);
// Snapshot bytes cross the worker boundary in paced chunks. The fixed line cap
// makes transient protocol memory independent of maxCorpusBytes: in the worst
// case one line may coexist as a decoder chunk, the stdout accumulator, an
// extracted line, a parsed string, and parser/stream backing storage. Charge
// five widened UTF-16 copies plus the decoded raw chunk to every admission.
const CORPUS_WORKER_CONTENT_CHUNK_BYTES = 64 * 1024;
const CORPUS_WORKER_PROTOCOL_MAX_BYTES = 512 * 1024;
const CORPUS_WORKER_PROTOCOL_UTF16_COPIES = 5;
const CORPUS_WORKER_PROTOCOL_TRANSIENT_BYTES =
  CORPUS_WORKER_PROTOCOL_MAX_BYTES *
    2 *
    CORPUS_WORKER_PROTOCOL_UTF16_COPIES +
  CORPUS_WORKER_CONTENT_CHUNK_BYTES;
const CORPUS_WORKER_TERMINATION_GRACE_MS = 2_000;
const CORPUS_OPERATION_TERMINATION_GRACE_MS = 100;
let corpusLeaseWorkerProcess = false;

export type CorpusReadBudgets = {
  maxFileBytes: number;
  maxCorpusBytes: number;
  maxRetainedPathBytes: number;
  maxFileCount: number;
  maxDirectoryCount: number;
  maxDirectoryEntries: number;
  maxWatcherCount: number;
  maxReaderCount: number;
};

export const DEFAULT_CORPUS_READ_BUDGETS: Readonly<CorpusReadBudgets> =
  Object.freeze({
    maxFileBytes: 16 * 1024 * 1024,
    maxCorpusBytes: 80 * 1024 * 1024,
    maxRetainedPathBytes: 8 * 1024 * 1024,
    maxFileCount: 4_096,
    maxDirectoryCount: 512,
    maxDirectoryEntries: 8_192,
    maxWatcherCount: 512,
    maxReaderCount: 64,
  });

export type CorpusVerificationStats = Readonly<{
  fileCount: number;
  retainedContentBytes: number;
  totalBytes: number;
}>;

export type StableCorpusFile = Readonly<{
  readonly path: string;
  readonly relativePath: string;
  readonly content: string;
}>;

export type StableCorpusSnapshot = Readonly<{
  readonly canonicalRoot: string;
  files: readonly StableCorpusFile[];
}>;

export type CorpusLeaseControls = {
  failWatcher: (reason?: string) => void;
  suppressNextFence: () => void;
  requireRepulseForNextFence: () => void;
  failNextFencePulse: () => void;
  failNextSentinelOpen: () => void;
  pauseNextSentinelOpen: (
    until: Promise<void>,
    onReserved?: () => void
  ) => void;
  pauseNextFenceAfterPending: (
    until: Promise<void>,
    onPending?: () => void
  ) => void;
};

/**
 * Test/diagnostic hooks for deterministic authorization-race coverage.
 * Every awaited hook runs before the final sentinel fence; the successful
 * final fence remains the operation's linearization point.
 */
export type CorpusLeaseHooks = {
  /** Explicit corpus resource limits; omitted fields use the safe defaults. */
  budgets?: Partial<CorpusReadBudgets>;
  timeoutMs?: number;
  beforeSentinelCreate?: (
    context: {
      attempt: number;
      slot: number;
      capacity: Readonly<{
        globalReserved: number;
        globalRetained: number;
        rootReserved: number;
      }>;
    }
  ) => void | Promise<void>;
  onWatcherReady?: (
    context: { attempt: number; controls: CorpusLeaseControls }
  ) => void | Promise<void>;
  afterBaseline?: (
    context: { attempt: number; controls: CorpusLeaseControls }
  ) => void | Promise<void>;
  beforeFinalManifest?: (
    context: { attempt: number; controls: CorpusLeaseControls }
  ) => void | Promise<void>;
  afterFinalManifest?: (
    context: {
      attempt: number;
      controls: CorpusLeaseControls;
      verification: CorpusVerificationStats;
    }
  ) => void | Promise<void>;
  beforeFinalFence?: (
    context: { attempt: number; controls: CorpusLeaseControls }
  ) => void | Promise<void>;
  /** Test-only worker entry override for hostile protocol fixtures. */
  workerScriptForTest?: string;
  /** Test-only deterministic worker stall injection. */
  workerStallPhaseForTest?:
    | "before-baseline"
    | "after-baseline"
    | "before-finalize"
    | "before-authorized";
  /** Test-only deterministic parent deadline after the projection has started. */
  operationDeadlineForTest?: Promise<void>;
  /**
   * Test-only: treat worker termination as unconfirmed, whatever really happened.
   *
   * Whether a kill is confirmed inside the grace window is a race a test cannot
   * force: SIGKILL is untrappable, so no fixture can refuse to die on cue, and
   * a shortened window is not enough either. A never-fed child dies almost
   * immediately, so the termination promise usually wins even a zero-length
   * race, and a test written that way passes without ever entering the branch
   * it claims to cover.
   *
   * A boolean forces the branch instead. It is also the least powerful shape
   * available: no delay to coerce, nothing to hold cleanup open, and it can
   * only make the parent more conservative, since an unconfirmed kill of a
   * worker that knew the corpus still poisons.
   */
  forceUnconfirmedTerminationForTest?: boolean;

  /**
   * Held *in addition to* the real reap before a stranded child counts as
   * confirmed dead. Without it both halves of the contract race, because the
   * child is SIGKILLed and so reaped within milliseconds: a test cannot
   * observe "still refused" before recovery overtakes it.
   *
   * A promise the test never settles pins the refusing half; one it settles
   * pins the recovering half. Because recovery waits for this promise *and*
   * the real termination, it can only ever delay recovery relative to
   * production. It cannot unlock the process before the child is genuinely
   * reaped, so it cannot manufacture a pass out of a parent that fails closed
   * too weakly. An earlier draft used `??` here, which substituted the hook
   * for real termination and did exactly that.
   */
  confirmTerminationForTest?: Promise<void>;
};

type RootIdentity = {
  canonicalRoot: string;
  fingerprint: string;
};

type Manifest = {
  fingerprint: string;
  snapshot?: StableCorpusSnapshot;
  verification: CorpusVerificationStats;
};

type PendingFence = {
  reject: (error: Error) => void;
  resolve: () => void;
  sentinel: LiveSentinel;
  suppressEntireFence: boolean;
  token: Buffer;
};

type LiveSentinel = {
  directory: string;
  handle: Awaited<ReturnType<typeof open>>;
  inUse: boolean;
  lastUsed: number;
  name: string;
  path: string;
};

type SentinelOpenControl = {
  fail: boolean;
  onReserved?: () => void;
  pauseUntil?: Promise<void>;
};

type FencePendingControl = {
  onPending?: () => void;
  pauseUntil?: Promise<void>;
};

let activeWatcherCount = 0;
let reservedCorpusMemoryBytes = 0;
let retainedSentinelUseSequence = 0;
const retainedSentinels = new Map<string, LiveSentinel>();
let reservedSentinelCreations = 0;
const reservedSentinelCreationsByDirectory = new Map<string, number>();

type SentinelCapacityReservation = {
  directory: string;
  released: boolean;
};

class CorpusLeaseChangedError extends Error {}
class CorpusLeaseBudgetError extends Error {}

type CorpusMemoryReservation = {
  bytes: number;
  released: boolean;
};

function reserveCorpusMemory(
  budgets: Readonly<CorpusReadBudgets>
): CorpusMemoryReservation {
  // Retained UTF-8 may widen to two-byte JS strings. Path/name metadata has
  // the same widening risk, and each retained file has a fixed conservative
  // object/array/hash overhead. One max-sized source Buffer may coexist with
  // the already-retained strings while it is decoded. The final fixed term
  // reserves every simultaneous representation of the single paced protocol
  // line; the protocol rejects a second line until the first is dispatched.
  const bytes =
    budgets.maxCorpusBytes * 2 +
    budgets.maxFileBytes +
    budgets.maxRetainedPathBytes * 2 +
    budgets.maxFileCount * RETAINED_FILE_OBJECT_OVERHEAD_BYTES +
    budgets.maxDirectoryEntries * RETAINED_DIRECTORY_ENTRY_OVERHEAD_BYTES +
    budgets.maxDirectoryCount * RETAINED_DIRECTORY_OVERHEAD_BYTES +
    CORPUS_WORKER_PROTOCOL_TRANSIENT_BYTES;
  if (
    !Number.isSafeInteger(bytes) ||
    bytes < 0 ||
    reservedCorpusMemoryBytes > MAX_RETAINED_CORPUS_MEMORY_BYTES - bytes
  ) {
    throw new CorpusLeaseBudgetError(
      "meeting corpus retained snapshots exceeded their process budget"
    );
  }
  // Synchronous admission: no peer can interleave between the check and
  // charge, even when several MCP handlers begin in the same event-loop turn.
  reservedCorpusMemoryBytes += bytes;
  return { bytes, released: false };
}

/**
 * Deferred releases that have been scheduled but not yet completed.
 *
 * A lease that leaves unconfirmed hazards behind must not release its memory
 * charge until every hazard settles, because a child that may still be alive
 * may still hold corpus data. That is correct, and production wants exactly it.
 *
 * The charge is process-global, though, so in a test process the delay is
 * visible to whatever runs next: an early case whose hazard has not settled yet
 * leaves the charge standing, and later cases fail on the retained-snapshot
 * budget instead of on anything they did. That is one flake reported three
 * times on three platforms, always naming an innocent test.
 *
 * Tracking the deferrals lets a test await its own cleanup rather than race it.
 * Deliberately not a reset: resetting the counter would hide a real leak, while
 * awaiting the actual settlement proves the release happened.
 */
const pendingDeferredReleases = new Set<Promise<unknown>>();

/**
 * Test-only: give this process's deferred releases a chance to settle.
 *
 * Best-effort and bounded, deliberately. The flake this exists for is a release
 * that has not settled YET: a killed child whose termination lands as soon as
 * the event loop breathes. Awaiting gives it that chance, so the next case
 * starts from a clean charge instead of inheriting one.
 *
 * It does NOT throw on an unsettled deferral, because "never settles" is a
 * supported end state, not a defect: a projection that ignores cancellation
 * poisons admission permanently and by design, and the test covering that would
 * fail a stricter hook for doing its job. Returns whether everything settled,
 * for a caller that wants to know.
 *
 * Measured, so the budget is not a guess: one stranded child holds 15,335,424
 * bytes against MAX_RETAINED_CORPUS_MEMORY_BYTES, awaiting while the child is
 * still unreaped clears nothing, and awaiting after the reap lands clears it to
 * zero. The bound only has to cover the gap between a case ending and its
 * child's reap arriving, which is milliseconds unless the machine is saturated,
 * so it is set well above that and still far below vitest's hook timeout.
 */
export async function awaitDeferredCorpusReleasesForTests(
  timeoutMs = 8_000
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (pendingDeferredReleases.size > 0 && Date.now() < deadline) {
    await Promise.race([
      Promise.allSettled([...pendingDeferredReleases]),
      new Promise((resolve) => setTimeout(resolve, 25)),
    ]);
  }
  return pendingDeferredReleases.size === 0;
}

/** Test-only: the live retained-corpus charge, for leak assertions. */
export function reservedCorpusMemoryBytesForTests(): number {
  return reservedCorpusMemoryBytes;
}

function releaseCorpusMemory(reservation: CorpusMemoryReservation): void {
  if (reservation.released) return;
  reservation.released = true;
  reservedCorpusMemoryBytes = Math.max(
    0,
    reservedCorpusMemoryBytes - reservation.bytes
  );
}

function resolveCorpusReadBudgets(
  requested: Partial<CorpusReadBudgets> | undefined
): Readonly<CorpusReadBudgets> {
  const candidate = { ...DEFAULT_CORPUS_READ_BUDGETS, ...requested };
  if (
    !Number.isSafeInteger(candidate.maxFileBytes) ||
    candidate.maxFileBytes < 0 ||
    !Number.isSafeInteger(candidate.maxCorpusBytes) ||
    candidate.maxCorpusBytes < 0 ||
    !Number.isSafeInteger(candidate.maxRetainedPathBytes) ||
    candidate.maxRetainedPathBytes < 0 ||
    !Number.isSafeInteger(candidate.maxFileCount) ||
    candidate.maxFileCount < 0 ||
    !Number.isSafeInteger(candidate.maxDirectoryCount) ||
    candidate.maxDirectoryCount < 1 ||
    !Number.isSafeInteger(candidate.maxDirectoryEntries) ||
    candidate.maxDirectoryEntries < 0 ||
    !Number.isSafeInteger(candidate.maxWatcherCount) ||
    candidate.maxWatcherCount < 1 ||
    !Number.isSafeInteger(candidate.maxReaderCount) ||
    candidate.maxReaderCount < 1
  ) {
    throw new Error("Access denied: invalid meeting corpus read budget");
  }
  const budgets: CorpusReadBudgets = {
    maxFileBytes: Math.min(candidate.maxFileBytes, DEFAULT_CORPUS_READ_BUDGETS.maxFileBytes),
    maxCorpusBytes: Math.min(candidate.maxCorpusBytes, DEFAULT_CORPUS_READ_BUDGETS.maxCorpusBytes),
    maxRetainedPathBytes: Math.min(
      candidate.maxRetainedPathBytes,
      DEFAULT_CORPUS_READ_BUDGETS.maxRetainedPathBytes
    ),
    maxFileCount: Math.min(candidate.maxFileCount, DEFAULT_CORPUS_READ_BUDGETS.maxFileCount),
    maxDirectoryCount: Math.min(candidate.maxDirectoryCount, DEFAULT_CORPUS_READ_BUDGETS.maxDirectoryCount),
    maxDirectoryEntries: Math.min(candidate.maxDirectoryEntries, DEFAULT_CORPUS_READ_BUDGETS.maxDirectoryEntries),
    maxWatcherCount: Math.min(candidate.maxWatcherCount, DEFAULT_CORPUS_READ_BUDGETS.maxWatcherCount),
    maxReaderCount: Math.min(candidate.maxReaderCount, DEFAULT_CORPUS_READ_BUDGETS.maxReaderCount),
  };
  return Object.freeze(budgets);
}

function resolveFenceTimeout(timeoutMs: number | undefined): number {
  const requested = timeoutMs ?? DEFAULT_FENCE_TIMEOUT_MS;
  if (!Number.isSafeInteger(requested) || requested < 1) {
    throw new Error("Access denied: invalid meeting corpus fence timeout");
  }
  return Math.min(requested, DEFAULT_FENCE_TIMEOUT_MS);
}

function authorizationDeadline(timeoutMs: number | undefined): bigint {
  const requested = timeoutMs ?? DEFAULT_AUTHORIZATION_TIMEOUT_MS;
  if (!Number.isSafeInteger(requested) || requested < 1) {
    throw new Error("Access denied: invalid meeting corpus authorization timeout");
  }
  return process.hrtime.bigint() + BigInt(Math.min(requested, DEFAULT_AUTHORIZATION_TIMEOUT_MS)) * 1_000_000n;
}

function remainingAuthorizationMs(deadline: bigint): number {
  const remainingNs = deadline - process.hrtime.bigint();
  if (remainingNs <= 0n) {
    throw new CorpusLeaseChangedError("meeting corpus authorization deadline elapsed");
  }
  return Math.max(1, Number((remainingNs + 999_999n) / 1_000_000n));
}

function normalizedRelativePath(path: string): string {
  return path.replaceAll("\\", "/");
}

function activeRelativePath(path: string): boolean {
  if (!path || isAbsolute(path)) return false;
  return normalizedRelativePath(path)
    .split("/")
    .every(
      (component) =>
        component.length > 0 &&
        component !== ".." &&
        !component.startsWith(".") &&
        !INACTIVE_CORPUS_DIRS.has(component.toLowerCase())
    );
}

function metadataFingerprint(info: any): string {
  return [
    info.dev,
    info.ino,
    info.size,
    info.mtimeNs ?? info.mtimeMs,
    info.ctimeNs ?? info.ctimeMs,
    info.birthtimeNs ?? info.birthtimeMs,
    info.mode,
    info.nlink,
  ]
    .map(String)
    .join(":");
}

function sentinelIdentityMetadataAccepted(info: any): boolean {
  if (!info.isFile() || info.isSymbolicLink() || BigInt(info.nlink) !== 1n) {
    return false;
  }
  // Windows' mode bits do not describe its ACL. The empty sentinel is not an
  // authorization capability there: its event is only an ordering hint, and
  // the post-fence full root/manifest reread is the authorization boundary.
  // Path/handle identity still prevents accidental reuse. Never claim POSIX
  // owner/mode proof on Windows.
  if (process.platform === "win32") return true;
  const currentUid = process.getuid?.();
  return (
    currentUid === undefined ||
    (BigInt(info.uid) === BigInt(currentUid) &&
      (BigInt(info.mode) & 0o077n) === 0n)
  );
}

async function sentinelIsIdle(sentinel: LiveSentinel): Promise<boolean> {
  try {
    const pathBefore = await lstat(sentinel.path, { bigint: true });
    const exact = await sentinel.handle.stat({ bigint: true });
    const pathAfter = await lstat(sentinel.path, { bigint: true });
    return (
      sentinelIdentityMetadataAccepted(pathBefore) &&
      sentinelIdentityMetadataAccepted(exact) &&
      sentinelIdentityMetadataAccepted(pathAfter) &&
      BigInt(exact.size) === 0n &&
      metadataFingerprint(pathBefore) === metadataFingerprint(exact) &&
      metadataFingerprint(pathAfter) === metadataFingerprint(exact)
    );
  } catch {
    return false;
  }
}

async function sentinelIdentityStillBound(sentinel: LiveSentinel): Promise<boolean> {
  try {
    const pathBefore = await lstat(sentinel.path, { bigint: true });
    const exact = await sentinel.handle.stat({ bigint: true });
    const pathAfter = await lstat(sentinel.path, { bigint: true });
    return (
      sentinelIdentityMetadataAccepted(pathBefore) &&
      sentinelIdentityMetadataAccepted(exact) &&
      sentinelIdentityMetadataAccepted(pathAfter) &&
      metadataFingerprint(pathBefore) === metadataFingerprint(exact) &&
      metadataFingerprint(pathAfter) === metadataFingerprint(exact)
    );
  } catch {
    return false;
  }
}

async function restoreBoundSentinelToIdle(sentinel: LiveSentinel): Promise<boolean> {
  if (!(await sentinelIdentityStillBound(sentinel))) return false;
  await sentinel.handle.truncate(0);
  await sentinel.handle.sync();
  return sentinelIsIdle(sentinel);
}

async function sentinelCarriesToken(
  sentinel: LiveSentinel,
  token: Buffer
): Promise<boolean> {
  try {
    const pathBefore = await lstat(sentinel.path, { bigint: true });
    const exactBefore = await sentinel.handle.stat({ bigint: true });
    if (
      !sentinelIdentityMetadataAccepted(pathBefore) ||
      !sentinelIdentityMetadataAccepted(exactBefore) ||
      metadataFingerprint(pathBefore) !== metadataFingerprint(exactBefore) ||
      BigInt(exactBefore.size) !== BigInt(token.length)
    ) {
      return false;
    }
    const observed = Buffer.alloc(token.length);
    const { bytesRead } = await sentinel.handle.read(
      observed,
      0,
      observed.length,
      0
    );
    const exactAfter = await sentinel.handle.stat({ bigint: true });
    const pathAfter = await lstat(sentinel.path, { bigint: true });
    return (
      bytesRead === token.length &&
      observed.equals(token) &&
      sentinelIdentityMetadataAccepted(exactAfter) &&
      sentinelIdentityMetadataAccepted(pathAfter) &&
      metadataFingerprint(exactBefore) === metadataFingerprint(exactAfter) &&
      metadataFingerprint(pathAfter) === metadataFingerprint(exactAfter)
    );
  } catch {
    return false;
  }
}

async function sentinelNamespace(
  canonicalRoot: string
): Promise<{ directory: string; entryCount: number }> {
  const namespace = join(canonicalRoot, SENTINEL_NAMESPACE);
  try {
    await mkdir(namespace, { mode: 0o700 });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
  }
  const info = await lstat(namespace, { bigint: true });
  const canonical = await realpath(namespace);
  if (
    !info.isDirectory() ||
    info.isSymbolicLink() ||
    canonical !== namespace ||
    relative(canonicalRoot, canonical) !== SENTINEL_NAMESPACE
  ) {
    throw new CorpusLeaseChangedError("meeting corpus sentinel namespace changed");
  }
  if (process.platform !== "win32") {
    const currentUid = process.getuid?.();
    if (
      (currentUid !== undefined && BigInt(info.uid) !== BigInt(currentUid)) ||
      (BigInt(info.mode) & 0o077n) !== 0n
    ) {
      throw new CorpusLeaseChangedError("meeting corpus sentinel namespace is not private");
    }
  }
  const handle = await opendir(namespace);
  let entries = 0;
  try {
    for (;;) {
      const entry = await handle.read();
      if (!entry) break;
      entries += 1;
      if (
        entries > MAX_SENTINEL_NAMESPACE_ENTRIES ||
        !entry.isFile() ||
        !SENTINEL_BASENAME.test(entry.name)
      ) {
        throw new CorpusLeaseBudgetError(
          "meeting corpus sentinel namespace exceeded its retained budget"
        );
      }
    }
  } finally {
    await handle.close().catch(() => {});
  }
  return { directory: namespace, entryCount: entries };
}

function reserveSentinelCreation(
  directory: string
): SentinelCapacityReservation {
  // This function intentionally contains no await. Every async acquisition
  // owns its global and per-root capacity before it can yield to a peer.
  if (
    retainedSentinels.size + reservedSentinelCreations >=
    MAX_RETAINED_SENTINELS
  ) {
    throw new CorpusLeaseBudgetError(
      "meeting corpus retained sentinels exceeded their process budget"
    );
  }
  reservedSentinelCreations += 1;
  reservedSentinelCreationsByDirectory.set(
    directory,
    (reservedSentinelCreationsByDirectory.get(directory) ?? 0) + 1
  );
  return { directory, released: false };
}

function releaseSentinelCreation(
  reservation: SentinelCapacityReservation
): void {
  if (reservation.released) return;
  reservation.released = true;
  reservedSentinelCreations -= 1;
  const rootReserved =
    (reservedSentinelCreationsByDirectory.get(reservation.directory) ?? 0) - 1;
  if (rootReserved === 0) {
    reservedSentinelCreationsByDirectory.delete(reservation.directory);
  } else {
    reservedSentinelCreationsByDirectory.set(
      reservation.directory,
      rootReserved
    );
  }
}

function sentinelCapacitySnapshot(directory: string) {
  return Object.freeze({
    globalReserved: reservedSentinelCreations,
    globalRetained: retainedSentinels.size,
    rootReserved: reservedSentinelCreationsByDirectory.get(directory) ?? 0,
  });
}

async function evictIdleSentinelForCapacity(): Promise<boolean> {
  let oldest: LiveSentinel | undefined;
  for (const sentinel of retainedSentinels.values()) {
    if (
      !sentinel.inUse &&
      (!oldest || sentinel.lastUsed < oldest.lastUsed)
    ) {
      oldest = sentinel;
    }
  }
  if (!oldest) return false;
  oldest.inUse = true;
  try {
    // Eviction only releases this process's descriptor accounting; it never
    // mutates the ambient pathname. A displaced or already-removed leaf is
    // safe to close and forget; authorization still fails if it is reopened.
    await oldest.handle.close();
    retainedSentinels.delete(oldest.path);
    return true;
  } catch (error) {
    oldest.inUse = false;
    oldest.lastUsed = ++retainedSentinelUseSequence;
    throw error;
  }
}

async function acquireSentinel(
  canonicalRoot: string,
  slot: number,
  openControl?: SentinelOpenControl,
  afterReserved?: (
    capacity: Readonly<{
      globalReserved: number;
      globalRetained: number;
      rootReserved: number;
    }>
  ) => void | Promise<void>
): Promise<LiveSentinel> {
  const directory = join(canonicalRoot, SENTINEL_NAMESPACE);
  const name = `lease-shared-${slot}.fence`;
  if (!SENTINEL_BASENAME.test(name)) {
    throw new CorpusLeaseBudgetError("meeting corpus sentinel slot was invalid");
  }
  const path = join(directory, name);
  const retained = retainedSentinels.get(path);
  if (retained) {
    if (retained.inUse) {
      throw new CorpusLeaseBudgetError("meeting corpus sentinel slot is already active");
    }
    retained.inUse = true;
    if (await sentinelIsIdle(retained)) return retained;
    if (await restoreBoundSentinelToIdle(retained)) return retained;
    try {
      await retained.handle.close();
      retainedSentinels.delete(retained.path);
    } catch (error) {
      retained.inUse = true;
      throw error;
    }
    throw new CorpusLeaseChangedError("meeting corpus sentinel identity changed");
  }
  while (
    retainedSentinels.size + reservedSentinelCreations >=
    MAX_RETAINED_SENTINELS
  ) {
    if (!(await evictIdleSentinelForCapacity())) {
      throw new CorpusLeaseBudgetError(
        "meeting corpus retained sentinels exceeded their process budget"
      );
    }
  }
  const reservation = reserveSentinelCreation(directory);
  try {
    await afterReserved?.(sentinelCapacitySnapshot(directory));
    await sentinelNamespace(canonicalRoot);
    openControl?.onReserved?.();
    await openControl?.pauseUntil;
    if (openControl?.fail) {
      throw new Error("injected meeting corpus sentinel open failure");
    }
    const handle = await open(
      path,
      constants.O_RDWR |
        constants.O_CREAT |
        (constants.O_NOFOLLOW ?? 0),
      0o600
    );
    const sentinel = {
      directory,
      handle,
      inUse: true,
      lastUsed: 0,
      name,
      path,
    };
    // Convert the reserved descriptor slot to retained capacity synchronously
    // before the next await. The namespace itself has only two fixed names.
    releaseSentinelCreation(reservation);
    retainedSentinels.set(path, sentinel);
    if (
      !(await sentinelIsIdle(sentinel)) &&
      !(await restoreBoundSentinelToIdle(sentinel))
    ) {
      try {
        await handle.close();
        retainedSentinels.delete(path);
      } catch {
        sentinel.inUse = true;
      }
      throw new CorpusLeaseChangedError(
        "meeting corpus sentinel identity changed"
      );
    }
    return sentinel;
  } catch (error) {
    releaseSentinelCreation(reservation);
    throw error;
  }
}

async function releaseSentinel(sentinel: LiveSentinel): Promise<void> {
  try {
    await sentinel.handle.sync();
    let identityStillBound = await sentinelIsIdle(sentinel);
    if (!identityStillBound) {
      identityStillBound = await restoreBoundSentinelToIdle(sentinel);
    }
    if (!identityStillBound) {
      throw new CorpusLeaseChangedError("meeting corpus sentinel changed during cleanup");
    }
    sentinel.inUse = false;
    sentinel.lastUsed = ++retainedSentinelUseSequence;
  } catch (error) {
    try {
      await sentinel.handle.close();
      retainedSentinels.delete(sentinel.path);
    } catch {
      // Failed-close handles remain charged and unavailable.
      sentinel.inUse = true;
    }
    throw error;
  }
}

async function resolveRootIdentity(root: string): Promise<RootIdentity> {
  const canonicalRoot = await realpath(root);
  const info = await stat(canonicalRoot, { bigint: true });
  if (!info.isDirectory()) {
    throw new Error("Access denied: meeting corpus root is not a directory");
  }
  return {
    canonicalRoot,
    fingerprint: `${canonicalRoot}\0${metadataFingerprint(info)}`,
  };
}

type TraversalResources = {
  directoryCount: number;
  entryCount: number;
  pathBytes: number;
};

function chargePathBytes(
  resources: TraversalResources,
  budgets: Readonly<CorpusReadBudgets>,
  ...values: string[]
): void {
  for (const value of values) {
    resources.pathBytes += Buffer.byteLength(value, "utf8");
    if (resources.pathBytes > budgets.maxRetainedPathBytes) {
      throw new CorpusLeaseBudgetError(
        "meeting corpus path metadata exceeded its budget"
      );
    }
  }
}

async function boundedDirectoryEntries(
  directory: string,
  resources: TraversalResources,
  budgets: Readonly<CorpusReadBudgets>,
  deadline: bigint
): Promise<Dirent[]> {
  remainingAuthorizationMs(deadline);
  const handle = await opendir(directory);
  const entries: Dirent[] = [];
  try {
    for (;;) {
      remainingAuthorizationMs(deadline);
      const entry = await handle.read();
      if (!entry) break;
      resources.entryCount += 1;
      if (resources.entryCount > budgets.maxDirectoryEntries) {
        throw new CorpusLeaseBudgetError("meeting corpus directory entries exceeded their budget");
      }
      chargePathBytes(resources, budgets, entry.name);
      entries.push(entry);
    }
  } finally {
    await handle.close().catch(() => {});
  }
  entries.sort((left, right) => left.name.localeCompare(right.name));
  return entries;
}

function scanWorkerFile(
  fd: number,
  maxBytes: number,
  retainContent: boolean
): { byteLength: number; content?: Buffer; sha256: string } {
  const chunks: Buffer[] | undefined = retainContent ? [] : undefined;
  const digest = createHash("sha256");
  const reusable = retainContent
    ? undefined
    : Buffer.allocUnsafe(Math.min(64 * 1024, Math.max(1, maxBytes)));
  let position = 0;
  for (;;) {
    if (position >= maxBytes) {
      const probe = Buffer.allocUnsafe(1);
      if (readSync(fd, probe, 0, 1, position) !== 0) {
        throw new CorpusLeaseBudgetError("meeting corpus file exceeded its byte budget");
      }
      break;
    }
    const length = Math.min(64 * 1024, maxBytes - position);
    const chunk = reusable ?? Buffer.allocUnsafe(length);
    const count = readSync(fd, chunk, 0, length, position);
    if (count === 0) break;
    const bytes = chunk.subarray(0, count);
    digest.update(bytes);
    chunks?.push(bytes);
    position += count;
  }
  return {
    byteLength: position,
    content: chunks ? Buffer.concat(chunks, position) : undefined,
    sha256: digest.digest("hex"),
  };
}

/**
 * The corpus worker is itself the killable filesystem boundary, so it reads
 * exact files directly rather than creating grandchildren. This matters on
 * Windows, where terminating the worker cannot otherwise retire a nested
 * bound-reader process that is blocked in the kernel.
 */
function readTextFileInsideCorpusWorker(
  canonicalPath: string,
  maxBytes: number,
  retainContent: boolean
): { content?: Buffer; revision: BoundFileRevision } {
  const parent = dirname(canonicalPath);
  const parentBefore = statSync(parent, { bigint: true });
  if (!parentBefore.isDirectory() || realpathSync(parent) !== parent) {
    throw new CorpusLeaseChangedError("meeting corpus parent changed");
  }
  const lexicalBefore = lstatSync(canonicalPath, { bigint: true });
  if (
    lexicalBefore.isSymbolicLink() ||
    !lexicalBefore.isFile() ||
    BigInt(lexicalBefore.nlink) !== 1n
  ) {
    throw new CorpusLeaseChangedError("meeting corpus member was not a regular file");
  }
  const fd = openSync(
    canonicalPath,
    constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0)
  );
  try {
    const openedBefore = fstatSync(fd, { bigint: true });
    if (!openedBefore.isFile() || BigInt(openedBefore.nlink) !== 1n) {
      throw new CorpusLeaseChangedError("meeting corpus member was not a regular file");
    }
    const first = scanWorkerFile(fd, maxBytes, retainContent);
    const second = scanWorkerFile(fd, maxBytes, false);
    const openedAfter = fstatSync(fd, { bigint: true });
    const lexicalAfter = lstatSync(canonicalPath, { bigint: true });
    const liveAfter = statSync(canonicalPath, { bigint: true });
    const parentAfter = statSync(parent, { bigint: true });
    if (
      !openedAfter.isFile() ||
      !lexicalAfter.isFile() ||
      lexicalAfter.isSymbolicLink() ||
      !liveAfter.isFile() ||
      BigInt(openedAfter.nlink) !== 1n ||
      BigInt(lexicalAfter.nlink) !== 1n ||
      BigInt(liveAfter.nlink) !== 1n ||
      realpathSync(canonicalPath) !== canonicalPath ||
      realpathSync(parent) !== parent ||
      metadataFingerprint(parentBefore) !== metadataFingerprint(parentAfter) ||
      metadataFingerprint(openedBefore) !== metadataFingerprint(openedAfter) ||
      metadataFingerprint(openedAfter) !== metadataFingerprint(lexicalAfter) ||
      metadataFingerprint(openedAfter) !== metadataFingerprint(liveAfter) ||
      first.byteLength !== second.byteLength ||
      first.sha256 !== second.sha256
    ) {
      throw new CorpusLeaseChangedError("meeting corpus member changed during manifest read");
    }
    return {
      content: first.content,
      revision: Object.freeze({
        byteLength: first.byteLength,
        leafFingerprint: metadataFingerprint(openedAfter),
        sha256: first.sha256,
      }),
    };
  } finally {
    closeSync(fd);
  }
}

function chargeDirectory(
  resources: TraversalResources,
  budgets: Readonly<CorpusReadBudgets>
): void {
  resources.directoryCount += 1;
  if (resources.directoryCount > budgets.maxDirectoryCount) {
    throw new CorpusLeaseBudgetError("meeting corpus directory count exceeded its budget");
  }
}

async function collectManifest(
  canonicalRoot: string,
  budgets: Readonly<CorpusReadBudgets>,
  retainContent: boolean,
  deadline: bigint
): Promise<Manifest> {
  remainingAuthorizationMs(deadline);
  const files: StableCorpusFile[] | undefined = retainContent ? [] : undefined;
  const manifestHash = createHash("sha256");
  let fileCount = 0;
  let totalBytes = 0;
  const resources: TraversalResources = {
    directoryCount: 0,
    entryCount: 0,
    pathBytes: 0,
  };
  chargePathBytes(resources, budgets, canonicalRoot);

  const visit = async (directory: string): Promise<void> => {
    remainingAuthorizationMs(deadline);
    chargeDirectory(resources, budgets);
    const entries = await boundedDirectoryEntries(directory, resources, budgets, deadline);
    for (const entry of entries) {
      remainingAuthorizationMs(deadline);
      if (entry.name.startsWith(".")) continue;
      const lexicalPath = join(directory, entry.name);
      // Parent entry arrays remain live across recursive descent. Charge each
      // constructed full path, including non-Markdown and directory entries,
      // instead of accounting only for retained meeting files.
      chargePathBytes(resources, budgets, lexicalPath);
      if (entry.isDirectory()) {
        if (!INACTIVE_CORPUS_DIRS.has(entry.name.toLowerCase())) {
          await visit(lexicalPath);
        }
        continue;
      }
      if (!entry.isFile() || extname(entry.name).toLowerCase() !== ".md") {
        continue;
      }

      fileCount += 1;
      if (fileCount > budgets.maxFileCount) {
        throw new CorpusLeaseBudgetError("meeting corpus file count exceeded its budget");
      }

      const canonicalPath = await realpath(lexicalPath);
      const scoped = relative(canonicalRoot, canonicalPath);
      if (!activeRelativePath(scoped)) {
        throw new CorpusLeaseChangedError("meeting corpus membership escaped its root");
      }
      const before = await lstat(canonicalPath, { bigint: true });
      if (
        !before.isFile() ||
        before.isSymbolicLink() ||
        BigInt(before.nlink) !== 1n
      ) {
        throw new CorpusLeaseChangedError("meeting corpus member was not a regular file");
      }
      const remainingCorpusBytes = budgets.maxCorpusBytes - totalBytes;
      const maxBytes = Math.min(budgets.maxFileBytes, remainingCorpusBytes);
      let content: Buffer | undefined;
      let revision: BoundFileRevision;
      if (corpusLeaseWorkerProcess) {
        const read = readTextFileInsideCorpusWorker(
          canonicalPath,
          maxBytes,
          retainContent
        );
        content = read.content;
        revision = read.revision;
      } else if (retainContent) {
        const read = await readTextFileWithRevisionFromBoundParent(canonicalPath, {
          maxBytes,
          maxReaders: budgets.maxReaderCount,
          timeoutMs: remainingAuthorizationMs(deadline),
        });
        content = read.content;
        revision = read.revision;
      } else {
        revision = await fingerprintTextFileFromBoundParent(canonicalPath, {
          maxBytes,
          maxReaders: budgets.maxReaderCount,
          timeoutMs: remainingAuthorizationMs(deadline),
        });
      }
      const after = await lstat(canonicalPath, { bigint: true });
      const beforeFingerprint = metadataFingerprint(before);
      const afterFingerprint = metadataFingerprint(after);
      if (
        !after.isFile() ||
        BigInt(after.nlink) !== 1n ||
        beforeFingerprint !== afterFingerprint ||
        beforeFingerprint !== revision.leafFingerprint
      ) {
        throw new CorpusLeaseChangedError("meeting corpus member changed during manifest read");
      }

      totalBytes += revision.byteLength;
      if (totalBytes > budgets.maxCorpusBytes) {
        throw new CorpusLeaseBudgetError("meeting corpus bytes exceeded their budget");
      }

      const relativePath = normalizedRelativePath(scoped);
      chargePathBytes(resources, budgets, canonicalPath, relativePath);
      if (files && content) {
        files.push({
          path: canonicalPath,
          relativePath,
          content: decodePolicyUtf8(content),
        });
        content = undefined;
      }
      manifestHash.update(
        `${JSON.stringify(relativePath)}:${revision.leafFingerprint}:${revision.byteLength}:${revision.sha256}\n`
      );
    }
  };

  await visit(canonicalRoot);
  files?.sort((left, right) => left.relativePath.localeCompare(right.relativePath));
  const verification = Object.freeze({
    fileCount,
    retainedContentBytes: retainContent ? totalBytes : 0,
    totalBytes,
  });
  return {
    fingerprint: manifestHash.digest("hex"),
    verification,
    ...(files
      ? {
          snapshot: Object.freeze({
            canonicalRoot,
            files: Object.freeze(files.map((file) => Object.freeze(file))),
          }),
        }
      : {}),
  };
}

class WatchedCorpusAttempt {
  private watcher: FSWatcher | undefined;
  private readonly pendingFences = new Map<string, PendingFence>();
  private readonly sentinels: LiveSentinel[] = [];
  private nextFenceSentinel = 0;
  private watcherReserved = false;
  private failure: Error | null = null;
  private suppressNext = false;
  private failNextPulse = false;
  private nextSentinelOpen: SentinelOpenControl | undefined;
  private nextFencePending: FencePendingControl | undefined;
  generation = 0;

  readonly controls: CorpusLeaseControls = Object.freeze({
    failWatcher: (reason = "injected watcher failure") => {
      this.fail(new Error(`Access denied: ${reason}`));
    },
    suppressNextFence: () => {
      this.suppressNext = true;
    },
    // Kept as a compatibility-only diagnostic hook. A fence now has exactly
    // one pulse and one acknowledgement; there are no outstanding repulses.
    requireRepulseForNextFence: () => {},
    failNextFencePulse: () => {
      this.failNextPulse = true;
    },
    failNextSentinelOpen: () => {
      this.nextSentinelOpen = {
        ...this.nextSentinelOpen,
        fail: true,
      };
    },
    pauseNextSentinelOpen: (pauseUntil, onReserved) => {
      this.nextSentinelOpen = {
        fail: this.nextSentinelOpen?.fail ?? false,
        onReserved,
        pauseUntil,
      };
    },
    pauseNextFenceAfterPending: (pauseUntil, onPending) => {
      this.nextFencePending = { onPending, pauseUntil };
    },
  });

  private constructor(
    private readonly canonicalRoot: string,
    private readonly deadline: bigint,
    private readonly fenceTimeoutMs: number
  ) {}

  static async create(
    canonicalRoot: string,
    deadline: bigint,
    fenceTimeoutMs: number,
    budgets: Readonly<CorpusReadBudgets>,
    attempt: number,
    beforeSentinelCreate: CorpusLeaseHooks["beforeSentinelCreate"]
  ): Promise<WatchedCorpusAttempt> {
    const lease = new WatchedCorpusAttempt(canonicalRoot, deadline, fenceTimeoutMs);
    try {
      // Open both fixed shared slots before watcher registration. Each fence
      // carries a fresh random token, so a peer or delayed callback cannot
      // acknowledge a different operation.
      const processLimit = Math.min(MAX_ACTIVE_WATCHERS, budgets.maxWatcherCount);
      if (activeWatcherCount >= processLimit) {
        throw new CorpusLeaseBudgetError(
          "meeting corpus watcher attempts exceeded their process budget"
        );
      }
      activeWatcherCount += 1;
      lease.watcherReserved = true;
      // Exactly two authorization fences are used per attempt. Give each one
      // a distinct sentinel created before watcher registration, preventing a
      // delayed callback from fence N from acknowledging fence N+1.
      lease.sentinels.push(
        await acquireSentinel(canonicalRoot, 0, undefined, (capacity) =>
          beforeSentinelCreate?.(
            Object.freeze({ attempt, slot: 0, capacity })
          )
        ),
        await acquireSentinel(canonicalRoot, 1, undefined, (capacity) =>
          beforeSentinelCreate?.(
            Object.freeze({ attempt, slot: 1, capacity })
          )
        )
      );
      // Node 20+ supports recursive fs.watch on the supported desktop
      // platforms. If a runtime/backend cannot provide it, construction throws
      // and authorization fails closed instead of composing unordered handles.
      lease.watcher = watch(
        canonicalRoot,
        { encoding: "utf8", persistent: false, recursive: true },
        (_eventType, filename) => lease.onEvent(filename)
      );
      lease.watcher.on("error", () => {
        lease.fail(new Error("Access denied: meeting corpus watcher failed"));
      });
      lease.assertHealthy();
      return lease;
    } catch (error) {
      await lease.close().catch(() => {});
      throw error;
    }
  }

  assertHealthy(): void {
    if (this.failure) throw this.failure;
  }

  async fence(): Promise<void> {
    this.assertHealthy();
    await this.fenceSentinel();
    this.assertHealthy();
  }

  async close(): Promise<void> {
    this.watcher?.close();
    this.watcher = undefined;
    if (this.watcherReserved) {
      activeWatcherCount -= 1;
      this.watcherReserved = false;
    }
    for (const pending of this.pendingFences.values()) {
      pending.reject(new Error("Access denied: meeting corpus lease closed"));
    }
    this.pendingFences.clear();
    let cleanupFailed = false;
    for (const sentinel of this.sentinels.splice(0)) {
      try {
        await releaseSentinel(sentinel);
      } catch {
        cleanupFailed = true;
      }
    }
    if (cleanupFailed) {
      throw new Error("Access denied: meeting corpus sentinel cleanup failed");
    }
  }

  private onEvent(filename: string | Buffer | null): void {
    if (filename === null) {
      this.fail(new Error("Access denied: meeting corpus watcher omitted a filename"));
      return;
    }
    const normalized = normalizedRelativePath(filename.toString());
    const name = basename(normalized);
    const pending = this.pendingFences.get(normalized);
    if (pending) {
      if (!pending.suppressEntireFence) {
        void sentinelCarriesToken(pending.sentinel, pending.token).then(
          (matches) => {
            if (matches && this.pendingFences.get(normalized) === pending) {
              pending.resolve();
            }
          },
          () => {}
        );
      }
      return;
    }
    // Shared-slot peer events are internal noise. Token verification above is
    // what binds an acknowledgement to this exact operation.
    if (
      normalized.startsWith(`${SENTINEL_NAMESPACE}/`) &&
      SENTINEL_BASENAME.test(name)
    ) return;
    this.generation += 1;
  }

  private async fenceSentinel(): Promise<void> {
    this.assertHealthy();
    const suppressEntireFence = this.suppressNext;
    this.suppressNext = false;
    let finished = false;
    const sentinel = this.sentinels[this.nextFenceSentinel++];
    if (!sentinel) {
      throw new CorpusLeaseChangedError("meeting corpus sentinel was unavailable");
    }
    const openControl = this.nextSentinelOpen;
    this.nextSentinelOpen = undefined;
    openControl?.onReserved?.();
    await openControl?.pauseUntil;
    if (openControl?.fail) throw new Error("injected meeting corpus sentinel open failure");
    const { handle, name } = sentinel;
    if (!(await sentinelIsIdle(sentinel))) {
      throw new CorpusLeaseChangedError("meeting corpus sentinel was displaced");
    }
    const directory = join(this.canonicalRoot, SENTINEL_NAMESPACE);
    const directoryBefore = await lstat(directory, { bigint: true });
    if (!directoryBefore.isDirectory() || directoryBefore.isSymbolicLink()) {
      throw new CorpusLeaseChangedError("meeting corpus fence directory changed");
    }
    let resolveFence!: () => void;
    let rejectFence!: (error: Error) => void;
    const observed = new Promise<void>((resolve, reject) => {
      resolveFence = () => {
        if (finished) return;
        finished = true;
        resolve();
      };
      rejectFence = (error) => {
        if (finished) return;
        finished = true;
        reject(error);
      };
    });
    const pending: PendingFence = {
      resolve: resolveFence,
      reject: rejectFence,
      sentinel,
      suppressEntireFence,
      token: randomBytes(SENTINEL_TOKEN_BYTES),
    };
    const pendingKey = `${SENTINEL_NAMESPACE}/${name}`;
    this.pendingFences.set(pendingKey, pending);
    const timeout = setTimeout(() => {
      rejectFence(new Error("Access denied: meeting corpus sentinel fence timed out"));
    }, Math.min(this.fenceTimeoutMs, remainingAuthorizationMs(this.deadline)));
    timeout.unref();

    try {
      const pendingControl = this.nextFencePending;
      this.nextFencePending = undefined;
      pendingControl?.onPending?.();
      await pendingControl?.pauseUntil;
      if (this.failNextPulse) {
        this.failNextPulse = false;
        throw new Error("Access denied: meeting corpus sentinel pulse failed");
      }
      if (!(await sentinelIsIdle(sentinel))) {
        throw new CorpusLeaseChangedError(
          "meeting corpus sentinel changed before acknowledgement"
        );
      }
      await handle.truncate(0);
      await handle.write(pending.token, 0, pending.token.length, 0);
      await handle.sync();
      await observed;
      if (!(await sentinelCarriesToken(sentinel, pending.token))) {
        throw new CorpusLeaseChangedError(
          "meeting corpus sentinel token changed during acknowledgement"
        );
      }
      await handle.truncate(0);
      await handle.sync();
      const directoryAfter = await lstat(directory, { bigint: true });
      if (
        !directoryAfter.isDirectory() ||
        directoryAfter.isSymbolicLink() ||
        metadataFingerprint(directoryAfter) !== metadataFingerprint(directoryBefore) ||
        !(await sentinelIsIdle(sentinel))
      ) {
        throw new CorpusLeaseChangedError(
          "meeting corpus sentinel changed during acknowledgement"
        );
      }
      this.assertHealthy();
    } finally {
      finished = true;
      clearTimeout(timeout);
      if (this.pendingFences.get(pendingKey) === pending) {
        this.pendingFences.delete(pendingKey);
      }
    }
  }

  private fail(error: Error): void {
    if (this.failure) return;
    this.failure = error;
    for (const pending of this.pendingFences.values()) {
      pending.reject(error);
    }
  }
}

/**
 * Run a multi-source read against one bounded watcher-fenced corpus snapshot.
 * A supported watcher must observe each sentinel fence; root identity and the
 * complete in-budget manifest must also agree before return. No claim is made
 * that uncontrolled writers cannot mutate in the JS check-to-return gap.
 */
async function withStableCorpusLeaseInProcess<T>(
  root: string,
  operation: (
    snapshot: StableCorpusSnapshot,
    attempt: number,
    signal: AbortSignal
  ) => T | Promise<T>,
  hooks: CorpusLeaseHooks = {}
): Promise<T> {
  const budgets = resolveCorpusReadBudgets(hooks.budgets);
  const deadline = authorizationDeadline(hooks.timeoutMs);
  const fenceTimeoutMs = resolveFenceTimeout(hooks.timeoutMs);
  const memoryReservation = reserveCorpusMemory(budgets);

  try {
    for (let attempt = 1; attempt <= MAX_AUTHORIZATION_ATTEMPTS; attempt += 1) {
      let lease: WatchedCorpusAttempt | undefined;
      try {
      remainingAuthorizationMs(deadline);
      const initialRoot = await resolveRootIdentity(root);
      lease = await WatchedCorpusAttempt.create(
        initialRoot.canonicalRoot,
        deadline,
        fenceTimeoutMs,
        budgets,
        attempt,
        hooks.beforeSentinelCreate
      );
      const diagnosticContext = Object.freeze({
        attempt,
        controls: lease.controls,
      });
      await hooks.onWatcherReady?.(diagnosticContext);
      remainingAuthorizationMs(deadline);
      await lease.fence();
      // The coverage probe creates hidden sentinels, which changes directory
      // metadata. Establish the root baseline only after that probe.
      const authorizedRoot = await resolveRootIdentity(root);
      if (authorizedRoot.canonicalRoot !== initialRoot.canonicalRoot) {
        throw new CorpusLeaseChangedError("meeting corpus root changed during initial fence");
      }
      const baselineGeneration = lease.generation;
      const baseline = await collectManifest(
        authorizedRoot.canonicalRoot,
        budgets,
        true,
        deadline
      );
      if (!baseline.snapshot) {
        throw new CorpusLeaseChangedError("meeting corpus snapshot was unavailable");
      }
      if (lease.generation !== baselineGeneration) {
        throw new CorpusLeaseChangedError("meeting corpus changed during baseline");
      }
      await hooks.afterBaseline?.(diagnosticContext);
      remainingAuthorizationMs(deadline);

      const operationAbort = new AbortController();
      const operationTimeout = setTimeout(() => {
        operationAbort.abort(
          new CorpusLeaseChangedError("meeting corpus operation deadline elapsed")
        );
      }, remainingAuthorizationMs(deadline));
      operationTimeout.unref();
      let result: T;
      try {
        result = await Promise.race([
          Promise.resolve(
            operation(baseline.snapshot, attempt, operationAbort.signal)
          ),
          new Promise<never>((_resolve, reject) => {
            operationAbort.signal.addEventListener(
              "abort",
              () => reject(operationAbort.signal.reason),
              { once: true }
            );
          }),
        ]);
      } finally {
        clearTimeout(operationTimeout);
      }
      remainingAuthorizationMs(deadline);
      await hooks.beforeFinalManifest?.(diagnosticContext);
      const finalManifest = await collectManifest(
        authorizedRoot.canonicalRoot,
        budgets,
        false,
        deadline
      );
      await hooks.afterFinalManifest?.(Object.freeze({
        attempt,
        controls: lease.controls,
        verification: finalManifest.verification,
      }));
      const finalRoot = await resolveRootIdentity(root);
      if (
        lease.generation !== baselineGeneration ||
        finalManifest.fingerprint !== baseline.fingerprint ||
        finalRoot.fingerprint !== authorizedRoot.fingerprint
      ) {
        throw new CorpusLeaseChangedError("meeting corpus changed before final fence");
      }
      await hooks.beforeFinalFence?.(diagnosticContext);

      // This is deliberately the last awaited authorization action. Generation
      // is checked synchronously after the sentinel event before returning.
      await lease.fence();
      if (lease.generation !== baselineGeneration) {
        throw new CorpusLeaseChangedError("meeting corpus changed at final fence");
      }
      // The sentinel acknowledgement is never an authorization capability.
      // In particular, a Windows principal that can enumerate this corpus may
      // inject a sentinel event, but that only advances execution to this full
      // reread. Authorization still requires the exact root, single-link file
      // identities, bytes, and hashes to match the baseline after the event.
      // A genuine fence additionally orders recursive-root callbacks before
      // this snapshot. Thus an outside hard-link alias, restricted overwrite,
      // or root swap cannot inherit the stale result merely by forging an ack.
      const authorizedManifest = await collectManifest(
        authorizedRoot.canonicalRoot,
        budgets,
        false,
        deadline
      );
      const authorizationRoot = await resolveRootIdentity(root);
      if (
        lease.generation !== baselineGeneration ||
        authorizedManifest.fingerprint !== baseline.fingerprint ||
        authorizationRoot.fingerprint !== authorizedRoot.fingerprint
      ) {
        throw new CorpusLeaseChangedError(
          "meeting corpus changed at authorization point"
        );
      }
      await lease.close();
      lease = undefined;
      return result;
      } catch {
        // Retry without retaining path-bearing filesystem errors. The public
        // failure below is deliberately privacy-safe.
      } finally {
        // Body failures are retried behind the one path-free public error. A
        // cleanup failure on the successful path is handled above and therefore
        // also denies; here it must not replace that privacy-safe error.
        await lease?.close().catch(() => {});
      }
    }

    throw new Error("Access denied: stable meeting corpus authorization failed");
  } finally {
    releaseCorpusMemory(memoryReservation);
  }
}

type WorkerControlCommand =
  | { kind: "fail-watcher"; reason?: string }
  | { kind: "suppress-next-fence" }
  | { kind: "repulse-next-fence" }
  | { kind: "fail-next-pulse" }
  | { kind: "fail-next-sentinel-open" }
  | { kind: "pause-next-sentinel-open"; id: number }
  | { kind: "pause-next-fence-pending"; id: number };

export type CorpusLeaseWorkerRequest = Readonly<{
  root: string;
  budgets: Readonly<CorpusReadBudgets>;
  timeoutMs: number;
  hookNames: readonly string[];
  stallPhase?: CorpusLeaseHooks["workerStallPhaseForTest"];
}>;

export type CorpusLeaseWorkerBridge = {
  exchange: (message: unknown) => Promise<any>;
  pause: (id: number, reservedEvent: string) => {
    promise: Promise<void>;
    onReserved: () => void;
  };
};

/** Mark this short-lived helper before it touches the caller's corpus. */
export function markCorpusLeaseWorkerProcess(): void {
  corpusLeaseWorkerProcess = true;
}

function applyWorkerControlCommands(
  controls: CorpusLeaseControls,
  commands: readonly WorkerControlCommand[],
  bridge: CorpusLeaseWorkerBridge
): void {
  for (const command of commands) {
    switch (command.kind) {
      case "fail-watcher":
        controls.failWatcher(command.reason);
        break;
      case "suppress-next-fence":
        controls.suppressNextFence();
        break;
      case "repulse-next-fence":
        controls.requireRepulseForNextFence();
        break;
      case "fail-next-pulse":
        controls.failNextFencePulse();
        break;
      case "fail-next-sentinel-open":
        controls.failNextSentinelOpen();
        break;
      case "pause-next-sentinel-open": {
        const pause = bridge.pause(command.id, "sentinel-open-reserved");
        controls.pauseNextSentinelOpen(pause.promise, pause.onReserved);
        break;
      }
      case "pause-next-fence-pending": {
        const pause = bridge.pause(command.id, "fence-pending");
        controls.pauseNextFenceAfterPending(pause.promise, pause.onReserved);
        break;
      }
    }
  }
}

/** Worker-only entry point used by corpus-lease-worker.ts. */
export async function runCorpusLeaseWorkerRequest(
  request: CorpusLeaseWorkerRequest,
  bridge: CorpusLeaseWorkerBridge
): Promise<void> {
  const namedHooks = new Set(request.hookNames);
  const phase = async (
    name: string,
    context: { attempt: number; controls: CorpusLeaseControls },
    extra?: Record<string, unknown>
  ): Promise<void> => {
    if (!namedHooks.has(name)) return;
    const response = await bridge.exchange({
      type: "phase",
      name,
      attempt: context.attempt,
      ...extra,
    });
    if (!response || response.type !== "phase-result" || !Array.isArray(response.commands)) {
      throw new CorpusLeaseChangedError("meeting corpus worker protocol changed");
    }
    applyWorkerControlCommands(context.controls, response.commands, bridge);
  };
  const stall = async (name: CorpusLeaseHooks["workerStallPhaseForTest"]) => {
    if (request.stallPhase === name) await new Promise<void>(() => {});
  };
  const hooks: CorpusLeaseHooks = {
    budgets: request.budgets,
    timeoutMs: request.timeoutMs,
    beforeSentinelCreate: namedHooks.has("beforeSentinelCreate")
      ? async (context) => {
          const response = await bridge.exchange({
            type: "phase",
            name: "beforeSentinelCreate",
            attempt: context.attempt,
            slot: context.slot,
            capacity: context.capacity,
          });
          if (!response || response.type !== "phase-result") {
            throw new CorpusLeaseChangedError("meeting corpus worker protocol changed");
          }
        }
      : undefined,
    onWatcherReady: async (context) => {
      await stall("before-baseline");
      await phase("onWatcherReady", context);
    },
    afterBaseline: async (context) => {
      await phase("afterBaseline", context);
      await stall("after-baseline");
    },
    beforeFinalManifest: async (context) => {
      await phase("beforeFinalManifest", context);
    },
    afterFinalManifest: async (context) => {
      await phase("afterFinalManifest", context, {
        verification: context.verification,
      });
    },
    beforeFinalFence: async (context) => {
      await phase("beforeFinalFence", context);
      await stall("before-authorized");
    },
  };

  await withStableCorpusLeaseInProcess(
    request.root,
    async (snapshot, attempt) => {
      const exchangeStreamMessage = async (message: unknown): Promise<void> => {
        const response = await bridge.exchange(message);
        if (!response || response.type !== "stream-ack") {
          throw new CorpusLeaseChangedError(
            "meeting corpus worker protocol changed"
          );
        }
      };
      await exchangeStreamMessage({
        type: "snapshot-start",
        attempt,
        canonicalRoot: snapshot.canonicalRoot,
        fileCount: snapshot.files.length,
      });
      for (const file of snapshot.files) {
        const bytes = Buffer.from(file.content, "utf8");
        await exchangeStreamMessage({
          type: "file-start",
          path: file.path,
          relativePath: file.relativePath,
          byteLength: bytes.byteLength,
        });
        for (
          let offset = 0;
          offset < bytes.byteLength;
          offset += CORPUS_WORKER_CONTENT_CHUNK_BYTES
        ) {
          await exchangeStreamMessage({
            type: "file-chunk",
            content: bytes
              .subarray(offset, offset + CORPUS_WORKER_CONTENT_CHUNK_BYTES)
              .toString("base64"),
          });
        }
        await exchangeStreamMessage({ type: "file-end" });
      }
      const response = await bridge.exchange({
        type: "snapshot-end",
      });
      if (!response || response.type !== "finalize") {
        throw new CorpusLeaseChangedError("meeting corpus worker protocol changed");
      }
      await stall("before-finalize");
      return undefined;
    },
    hooks
  );
  await bridge.exchange({ type: "authorized" });
}

type ParentPause = {
  onReserved?: () => void;
  until: Promise<void>;
};

// How many children were killed without their death being confirmed inside
// the grace, and are still unreaped. Every one of them may still be alive and
// reading the corpus, so while this is above zero no further lease may run.
//
// A count rather than a flag because two leases can each strand a child, and
// the first one reaped must not clear the second one's refusal. Only the
// transition back to zero reopens the process.
let unconfirmedCorpusWorkers = 0;
let activeCorpusWorkerCount = 0;

function retainUnconfirmedCorpusWorker(): void {
  unconfirmedCorpusWorkers += 1;
}

function releaseUnconfirmedCorpusWorker(): void {
  unconfirmedCorpusWorkers = Math.max(0, unconfirmedCorpusWorkers - 1);
}
let nextPauseId = 1;

function corpusWorkerInvocation(scriptOverride?: string): {
  binary: string;
  args: string[];
} {
  if (scriptOverride) return { binary: process.execPath, args: [scriptOverride] };
  const sourceMode = import.meta.url.endsWith(".ts");
  const helper = fileURLToPath(
    new URL(`./corpus-lease-worker.${sourceMode ? "ts" : "js"}`, import.meta.url)
  );
  if (!sourceMode) return { binary: process.execPath, args: [helper] };
  try {
    import.meta.resolve("tsx");
    return { binary: process.execPath, args: ["--import", "tsx", helper] };
  } catch {
    const built = fileURLToPath(new URL("../dist/corpus-lease-worker.js", import.meta.url));
    if (existsSync(built)) return { binary: process.execPath, args: [built] };
    throw new Error("Access denied: meeting corpus worker is unavailable");
  }
}

function killCorpusWorker(child: ChildProcess): void {
  if (process.platform !== "win32" && child.pid) {
    try {
      process.kill(-child.pid, "SIGKILL");
      return;
    } catch {
      // Fall back to the direct handle during the spawn/group creation race.
    }
  }
  try {
    child.kill("SIGKILL");
  } catch {
    // The worker may already have exited.
  }
}

function workerHookNames(hooks: CorpusLeaseHooks): string[] {
  return [
    "beforeSentinelCreate",
    "onWatcherReady",
    "afterBaseline",
    "beforeFinalManifest",
    "afterFinalManifest",
    "beforeFinalFence",
  ].filter((name) => typeof (hooks as any)[name] === "function");
}

function parentControls(
  commands: WorkerControlCommand[],
  pauses: Map<number, ParentPause>
): CorpusLeaseControls {
  return Object.freeze({
    failWatcher: (reason?: string) => commands.push({ kind: "fail-watcher", reason }),
    suppressNextFence: () => commands.push({ kind: "suppress-next-fence" }),
    requireRepulseForNextFence: () => commands.push({ kind: "repulse-next-fence" }),
    failNextFencePulse: () => commands.push({ kind: "fail-next-pulse" }),
    failNextSentinelOpen: () => commands.push({ kind: "fail-next-sentinel-open" }),
    pauseNextSentinelOpen: (until: Promise<void>, onReserved?: () => void) => {
      const id = nextPauseId++;
      pauses.set(id, { until, onReserved });
      commands.push({ kind: "pause-next-sentinel-open", id });
    },
    pauseNextFenceAfterPending: (until: Promise<void>, onPending?: () => void) => {
      const id = nextPauseId++;
      pauses.set(id, { until, onReserved: onPending });
      commands.push({ kind: "pause-next-fence-pending", id });
    },
  });
}

/**
 * Run a multi-source projection while a killable worker owns every corpus
 * traversal, exact read, watcher, fence, and cleanup filesystem operation.
 */
/**
 * Authorize a stable corpus read, presenting one error for one condition.
 *
 * Every way the lease can refuse is a refusal, and callers get the same
 * sentence for all of them. Without this, which sentence you saw depended on
 * where the failure landed: `fail()` wraps refusals raised once the worker is
 * running, but a `CorpusLeaseChangedError` thrown before that, such as the
 * deadline guard at the top of the dispatch below, escaped raw.
 *
 * That is observable, not cosmetic. On a contended Windows runner a short
 * budget can elapse between computing the deadline and the very next
 * statement checking it, so the same timeout surfaced as
 * "meeting corpus authorization deadline elapsed" instead of the documented
 * refusal, and CI failed with a message mismatch rather than a real defect.
 *
 * `CorpusLeaseBudgetError` and the plain `Error` refusals keep their own text:
 * those name a caller-actionable limit rather than an authorization outcome.
 */
export async function withStableCorpusLease<T>(
  root: string,
  operation: (
    snapshot: StableCorpusSnapshot,
    attempt: number,
    signal: AbortSignal
  ) => T | Promise<T>,
  hooks: CorpusLeaseHooks = {}
): Promise<T> {
  try {
    return await dispatchStableCorpusLease(root, operation, hooks);
  } catch (error) {
    if (error instanceof CorpusLeaseChangedError) {
      throw new Error(
        "Access denied: stable meeting corpus authorization failed"
      );
    }
    throw error;
  }
}

async function dispatchStableCorpusLease<T>(
  root: string,
  operation: (
    snapshot: StableCorpusSnapshot,
    attempt: number,
    signal: AbortSignal
  ) => T | Promise<T>,
  hooks: CorpusLeaseHooks = {}
): Promise<T> {
  // This reservation hook is a deterministic in-process concurrency probe.
  // Production call sites never provide it; keeping it local preserves its
  // atomic process-global observations without weakening production reads.
  if (hooks.beforeSentinelCreate) {
    return withStableCorpusLeaseInProcess(root, operation, hooks);
  }
  if (unconfirmedCorpusWorkers > 0) {
    throw new Error(
      "Access denied: a meeting corpus worker was killed without confirming it died"
    );
  }
  const budgets = resolveCorpusReadBudgets(hooks.budgets);
  const deadline = authorizationDeadline(hooks.timeoutMs);
  const timeoutMs = remainingAuthorizationMs(deadline);
  const invocation = corpusWorkerInvocation(hooks.workerScriptForTest);
  const reservation = reserveCorpusMemory(budgets);
  const workerLimit = Math.min(MAX_ACTIVE_WATCHERS, budgets.maxWatcherCount);
  if (activeCorpusWorkerCount >= workerLimit) {
    releaseCorpusMemory(reservation);
    throw new Error("Access denied: stable meeting corpus authorization failed");
  }
  activeCorpusWorkerCount += 1;
  const pauses = new Map<number, ParentPause>();
  const operationAbort = new AbortController();
  let operationResult!: T;
  let operationCompleted = false;
  let operationActive = false;
  let operationTermination: Promise<void> = Promise.resolve();
  let lastAttempt = 0;
  // A watched attempt may fail before it reaches snapshot streaming. Track
  // every visible retry so protocol attempts stay bounded and monotonic
  // without requiring those pre-snapshot failures to fabricate a snapshot.
  let lastObservedAttempt = 0;
  let authorized = false;
  let settled = false;
  let terminated = false;
  let protocolBytes = 0;
  let stdout = "";
  let protocolMessagePending = false;
  let protocolQueue: Promise<void> = Promise.resolve();
  let releaseReservation = true;
  let releaseWorkerAdmission = true;
  // Guards the admission slot against a double release when a late-reaped
  // child's termination promise resolves after the cleanup block already ran.
  let workerAdmissionReleased = false;
  // Everything this lease may have left running that could still be reading
  // the corpus. The process stays closed until all of them settle.
  const unconfirmedHazards: Promise<unknown>[] = [];
  const noteUnconfirmedHazard = (hazard: Promise<unknown>): void => {
    // The first hazard closes the process; later ones only extend how long it
    // stays closed, so the hold is taken exactly once per lease.
    if (unconfirmedHazards.length === 0) retainUnconfirmedCorpusWorker();
    unconfirmedHazards.push(hazard);
  };
  // Whether the `begin` message, the only thing that tells the child which
  // corpus to read, was actually handed to the child's stdin. Authority is the
  // write itself, never anything the child echoes back, and never merely
  // reaching the call site: `send` declines to write once the lease has
  // settled or the pipe is gone.
  let corpusRootDisclosed = false;

  const child = spawn(invocation.binary, invocation.args, {
    detached: process.platform !== "win32",
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
    env: nodeChildEnvironment(),
  });
  child.stderr?.resume();
  let resolveTermination!: () => void;
  const termination = new Promise<void>((resolve) => {
    resolveTermination = resolve;
  });
  child.once("close", () => {
    terminated = true;
    resolveTermination();
  });

  // A refusal kills the worker, but protocol lines already in flight are still
  // being handled, so a write can land on a pipe whose reader is gone. That
  // raises an asynchronous EPIPE on the stream, and an 'error' event with no
  // listener is fatal to the host process, not just to this lease. Absorbing
  // it is correct: by the time the pipe is gone the lease has already settled
  // and the message has nowhere useful to go.
  child.stdin?.on("error", () => {});

  /** Write a protocol line, reporting whether it actually reached the pipe. */
  const send = (message: unknown): boolean => {
    const serialized = JSON.stringify(message);
    if (Buffer.byteLength(serialized) > CORPUS_WORKER_PROTOCOL_MAX_BYTES || !child.stdin) {
      throw new CorpusLeaseBudgetError("meeting corpus worker protocol exceeded its budget");
    }
    if (settled || child.stdin.destroyed || child.killed) return false;
    child.stdin.write(serialized + "\n");
    return true;
  };

  try {
    const result = await new Promise<T>((resolve, reject) => {
      const fail = (message: string): void => {
        if (settled) return;
        settled = true;
        // Sampled before the teardown below so the number reflects the moment
        // of denial, not how long cleanup took.
        const remainingNs = deadline - process.hrtime.bigint();

        operationAbort.abort(new CorpusLeaseChangedError(message));
        killCorpusWorker(child);
        reject(new Error("Access denied: stable meeting corpus authorization failed"));

        // Diagnostics run last and cannot affect the lease. `settled` is already
        // true, so anything thrown here would strand the promise forever: no
        // later fail() would get past the guard, and the abort/kill/reject above
        // would never have run. Same shape as the bug that made the first
        // version of this call `remainingAuthorizationMs`, which throws once the
        // deadline has passed.
        //
        // The rejection above stays a single uniform denial on purpose: callers
        // must not learn why authorization failed. The reason goes to stderr,
        // which is operator-visible only. Every reason is a fixed internal
        // string, so no corpus path or content is disclosed.
        // Deferred as well as guarded. `fail()` must return before promise
        // handlers can run, so writing inline would let a blocked stderr, a
        // full pipe for instance, delay the caller's observation of a denial
        // that has already been decided.
        //
        // Not a total guarantee, and deliberately not chased further: the
        // scheduled write can still land during the lease's own async cleanup,
        // so a genuinely blocked stderr could delay the public rejection by
        // however long it blocks. Node writes to pipes asynchronously, leaving
        // only a wedged TTY or file, which is a broken host rather than a
        // failure mode this diagnostic should contort itself around.
        setImmediate(() => {
          try {
            // Sign is taken from nanoseconds: BigInt division truncates toward
            // zero, so a sub-millisecond overrun would otherwise render as
            // "0ms remained" rather than an overrun.
            const overran = remainingNs < 0n;
            const magnitudeMs = Number((overran ? -remainingNs : remainingNs) / 1_000_000n);
            const budget = overran
              ? `authorization budget overrun by ${magnitudeMs}ms`
              : `${magnitudeMs}ms of authorization budget remained`;
            // Shared with the bound-reader refusal path, because a plain
            // try/catch here never covered an asynchronous EPIPE: that
            // arrives as an 'error' event on a later tick and is fatal
            // without a listener.
            writeOperatorDiagnostic(`[corpus-lease] denied: ${message} (${budget})\n`);
          } catch {
            // A broken stderr must never turn a clean denial into a crash.
          }
        });
      };
      const timer = setTimeout(
        () => fail("meeting corpus authorization deadline elapsed"),
        remainingAuthorizationMs(deadline)
      );
      timer.unref();
      child.once("error", () => fail("meeting corpus worker failed"));
      child.once("close", (code) => {
        clearTimeout(timer);
        if (settled) return;
        if (code !== 0 || !authorized || !operationCompleted) {
          fail("meeting corpus worker exited before authorization");
          return;
        }
        settled = true;
        resolve(operationResult);
      });
      type StreamFile = {
        bytes: Buffer;
        offset: number;
        path: string;
        relativePath: string;
      };
      type SnapshotStream = {
        attempt: number;
        canonicalRoot: string;
        expectedFileCount: number;
        files: StableCorpusFile[];
        current?: StreamFile;
        retainedBytes: number;
        retainedPathBytes: number;
      };
      let stream: SnapshotStream | undefined;
      const streamAck = (): void => {
        send({ type: "stream-ack" });
      };
      const handleProtocolLine = async (line: string): Promise<void> => {
        if (settled) return;
        let message: any;
        try {
          message = JSON.parse(line);
        } catch {
          fail("meeting corpus worker protocol was invalid");
          return;
        }
        if (message?.type === "phase") {
          const attempt = Number(message.attempt);
          const hook = (hooks as any)[message.name];
          if (
            stream ||
            authorized ||
            !Number.isSafeInteger(attempt) ||
            attempt < Math.max(1, lastAttempt, lastObservedAttempt) ||
            attempt > MAX_AUTHORIZATION_ATTEMPTS ||
            typeof hook !== "function"
          ) {
            fail("meeting corpus worker phase protocol was invalid");
            return;
          }
          lastObservedAttempt = attempt;
          const commands: WorkerControlCommand[] = [];
          const context: any = {
            attempt,
            controls: parentControls(commands, pauses),
          };
          if (message.capacity) context.capacity = Object.freeze(message.capacity);
          if (message.slot !== undefined) context.slot = message.slot;
          if (message.verification) {
            context.verification = Object.freeze(message.verification);
          }
          await hook(Object.freeze(context));
          send({ type: "phase-result", commands });
          return;
        }
        if (
          message?.type === "sentinel-open-reserved" ||
          message?.type === "fence-pending"
        ) {
          if (stream || authorized) {
            fail("meeting corpus worker pause protocol was invalid");
            return;
          }
          const pause = pauses.get(message.id);
          if (!pause) {
            fail("meeting corpus worker pause protocol was invalid");
            return;
          }
          pause.onReserved?.();
          await pause.until;
          pauses.delete(message.id);
          send({ type: "resume", id: message.id });
          return;
        }
        if (message?.type === "snapshot-start") {
          const attempt = Number(message.attempt);
          const fileCount = Number(message.fileCount);
          if (
            stream ||
            authorized ||
            !operationCompleted && lastAttempt > 0 ||
            !Number.isSafeInteger(attempt) ||
            attempt <= lastAttempt ||
            attempt < lastObservedAttempt ||
            attempt > MAX_AUTHORIZATION_ATTEMPTS ||
            typeof message.canonicalRoot !== "string" ||
            !Number.isSafeInteger(fileCount) ||
            fileCount < 0 ||
            fileCount > budgets.maxFileCount
          ) {
            fail("meeting corpus worker snapshot protocol was invalid");
            return;
          }
          const rootBytes = Buffer.byteLength(message.canonicalRoot);
          if (rootBytes > budgets.maxRetainedPathBytes) {
            fail("meeting corpus worker snapshot protocol exceeded its budget");
            return;
          }
          lastAttempt = attempt;
          lastObservedAttempt = attempt;
          operationCompleted = false;
          stream = {
            attempt,
            canonicalRoot: message.canonicalRoot,
            expectedFileCount: fileCount,
            files: [],
            retainedBytes: 0,
            retainedPathBytes: rootBytes,
          };
          streamAck();
          return;
        }
        if (message?.type === "file-start") {
          const byteLength = Number(message.byteLength);
          if (
            !stream ||
            stream.current ||
            stream.files.length >= stream.expectedFileCount ||
            typeof message.path !== "string" ||
            typeof message.relativePath !== "string" ||
            !Number.isSafeInteger(byteLength) ||
            byteLength < 0 ||
            byteLength > budgets.maxFileBytes ||
            stream.retainedBytes > budgets.maxCorpusBytes - byteLength
          ) {
            fail("meeting corpus worker file protocol was invalid");
            return;
          }
          const pathBytes =
            Buffer.byteLength(message.path) +
            Buffer.byteLength(message.relativePath);
          if (
            stream.retainedPathBytes >
            budgets.maxRetainedPathBytes - pathBytes
          ) {
            fail("meeting corpus worker file protocol exceeded its budget");
            return;
          }
          stream.retainedBytes += byteLength;
          stream.retainedPathBytes += pathBytes;
          stream.current = {
            bytes: Buffer.allocUnsafe(byteLength),
            offset: 0,
            path: message.path,
            relativePath: message.relativePath,
          };
          streamAck();
          return;
        }
        if (message?.type === "file-chunk") {
          if (!stream?.current || typeof message.content !== "string") {
            fail("meeting corpus worker file protocol was invalid");
            return;
          }
          const bytes = Buffer.from(message.content, "base64");
          if (
            bytes.byteLength < 1 ||
            bytes.byteLength > CORPUS_WORKER_CONTENT_CHUNK_BYTES ||
            bytes.toString("base64") !== message.content ||
            stream.current.offset >
              stream.current.bytes.byteLength - bytes.byteLength
          ) {
            fail("meeting corpus worker file protocol was invalid");
            return;
          }
          bytes.copy(stream.current.bytes, stream.current.offset);
          stream.current.offset += bytes.byteLength;
          streamAck();
          return;
        }
        if (message?.type === "file-end") {
          if (
            !stream?.current ||
            stream.current.offset !== stream.current.bytes.byteLength
          ) {
            fail("meeting corpus worker file protocol was invalid");
            return;
          }
          const current = stream.current;
          stream.current = undefined;
          stream.files.push(
            Object.freeze({
              path: current.path,
              relativePath: current.relativePath,
              content: decodePolicyUtf8(current.bytes),
            })
          );
          streamAck();
          return;
        }
        if (message?.type === "snapshot-end") {
          if (
            !stream ||
            stream.current ||
            stream.files.length !== stream.expectedFileCount
          ) {
            fail("meeting corpus worker snapshot protocol was invalid");
            return;
          }
          const completedStream = stream;
          stream = undefined;
          const snapshot: StableCorpusSnapshot = Object.freeze({
            canonicalRoot: completedStream.canonicalRoot,
            files: Object.freeze(completedStream.files),
          });
          operationActive = true;
          let resolveOperationTermination!: () => void;
          operationTermination = new Promise<void>((resolve) => {
            resolveOperationTermination = resolve;
          });
          if (hooks.operationDeadlineForTest) {
            void hooks.operationDeadlineForTest.then(
              () => fail("meeting corpus authorization deadline elapsed"),
              () => fail("meeting corpus authorization deadline elapsed")
            );
          }
          try {
            operationResult = await operation(
              snapshot,
              completedStream.attempt,
              operationAbort.signal
            );
          } finally {
            operationActive = false;
            resolveOperationTermination();
          }
          operationCompleted = true;
          send({ type: "finalize" });
          return;
        }
        if (message?.type === "authorized") {
          if (stream || !operationCompleted || authorized || lastAttempt < 1) {
            fail("meeting corpus worker authorization protocol was invalid");
            return;
          }
          authorized = true;
          send({ type: "acknowledged" });
          return;
        }
        fail("meeting corpus worker protocol was invalid");
      };
      child.stdout?.setEncoding("utf8");
      child.stdout?.on("data", (chunk: string) => {
        if (settled) return;
        // The worker must await one response for every outbound line. Reject a
        // second complete or partial line while dispatch is pending so no two
        // async handlers can observe or advance protocol state concurrently.
        if (protocolMessagePending) {
          fail("meeting corpus worker protocol was not paced");
          return;
        }
        stdout += chunk;
        protocolBytes = Buffer.byteLength(stdout);
        if (protocolBytes > CORPUS_WORKER_PROTOCOL_MAX_BYTES) {
          fail("meeting corpus worker protocol exceeded its budget");
          return;
        }
        const newline = stdout.indexOf("\n");
        if (newline < 0) return;
        if (newline !== stdout.length - 1) {
          fail("meeting corpus worker protocol was not paced");
          return;
        }
        const line = stdout.slice(0, newline);
        stdout = "";
        protocolBytes = 0;
        protocolMessagePending = true;
        protocolQueue = protocolQueue
          .then(() => handleProtocolLine(line))
          .catch(() => fail("meeting corpus worker phase failed"))
          .finally(() => {
            protocolMessagePending = false;
          });
      });
      try {
        // Buffering means a `true` here proves the bytes were handed to the
        // pipe, not that the child read them. That asymmetry is deliberate:
        // over-reporting disclosure costs a conservative poisoning, while
        // under-reporting would skip one that is required.
        if (
          send({
            type: "begin",
            request: {
              root,
              budgets,
              timeoutMs,
              hookNames: workerHookNames(hooks),
              stallPhase: hooks.workerStallPhaseForTest,
            } satisfies CorpusLeaseWorkerRequest,
          })
        ) {
          corpusRootDisclosed = true;
        }
      } catch {
        fail("meeting corpus worker request was invalid");
      }
    });
    return result;
  } finally {
    if (!terminated) {
      killCorpusWorker(child);
      const confirmed = hooks.forceUnconfirmedTerminationForTest
        ? false
        : await Promise.race([
            termination.then(() => true),
            new Promise<boolean>((resolve) => {
              const timer = setTimeout(
                () => resolve(false),
                CORPUS_WORKER_TERMINATION_GRACE_MS
              );
              timer.unref();
            }),
          ]);
      // An unconfirmed kill means the child might still be running, and a
      // child that knows the corpus root might still be reading it. Retaining
      // its reservation and refusing further workers is the right answer to
      // that, and stays the answer.
      //
      // A child killed before `begin` reached it is a different animal: the
      // protocol is the only way it learns which directory to read, and it
      // does nothing before that message. It cannot be holding corpus bytes,
      // so charging the process for bytes it never read and blocking every
      // later lease until restart protects nothing. That case is reachable in
      // ordinary use, not just in tests, whenever a short budget expires while
      // the worker is still starting (issue #689).
      if (!confirmed) {
        if (corpusRootDisclosed) {
          // Fail closed for as long as the danger is real, which is exactly
          // as long as the child's death is unconfirmed. Reaping it later
          // ends that danger definitively: a reaped child reads nothing, and
          // it cannot have read anything after it died. Holding the refusal
          // past that point protects nothing and costs the whole process,
          // which is the same reasoning the admission slot below already
          // uses, applied to the poison and the reservation.
          //
          // SIGKILL cannot be caught, so a child outliving the grace is a
          // slow reap under load, not a child refusing to die. That is a
          // recoverable condition and must not require a process restart.
          noteUnconfirmedHazard(
            hooks.confirmTerminationForTest
              ? Promise.all([termination, hooks.confirmTerminationForTest])
              : termination
          );
          releaseReservation = false;
          releaseWorkerAdmission = false;
        } else {
          // The child holds no corpus bytes, so its memory reservation is
          // released below. Its admission slot is a different question: the
          // kill is unconfirmed, so the process may genuinely still exist, and
          // that cap counts live workers rather than retained bytes.
          //
          // Releasing it immediately would let repeated failures accumulate
          // unbounded uncharged children; holding it forever would leak a slot
          // for a child that, in the overwhelming majority of cases, died a
          // moment after the grace expired. So the slot is handed to the
          // termination promise and freed if and when the child is actually
          // reaped, which is neither of those extremes.
          releaseWorkerAdmission = false;
          void termination.then(() => {
            if (workerAdmissionReleased) return;
            workerAdmissionReleased = true;
            activeCorpusWorkerCount = Math.max(0, activeCorpusWorkerCount - 1);
          });
        }
      }
    }
    if (operationActive) {
      const operationConfirmed = await Promise.race([
        operationTermination.then(() => true),
        new Promise<boolean>((resolve) => {
          const timer = setTimeout(
            () => resolve(false),
            CORPUS_OPERATION_TERMINATION_GRACE_MS
          );
          timer.unref();
        }),
      ]);
      if (!operationConfirmed) {
        // Same bargain as the worker above: refuse while the projection may
        // still be running, recover once it is confirmed finished.
        noteUnconfirmedHazard(operationTermination);
        releaseReservation = false;
        releaseWorkerAdmission = false;
      }
    }
    if (unconfirmedHazards.length > 0) {
      // One hold for the whole lease, released only when every hazard it left
      // behind has settled. Retaining and releasing per hazard would let the
      // count fall to zero between the worker branch and the projection branch
      // above, which are separated by an await: the worker's reap can land
      // inside that window, and a lease admitted there would run while this
      // lease's projection still holds corpus data.
      //
      // allSettled, not all: a hazard that rejects is still a hazard that
      // finished, and a rejection here must not strand the process closed.
      const deferred = Promise.allSettled(unconfirmedHazards).then(() => {
        releaseUnconfirmedCorpusWorker();
        releaseCorpusMemory(reservation);
        if (workerAdmissionReleased) return;
        workerAdmissionReleased = true;
        activeCorpusWorkerCount = Math.max(0, activeCorpusWorkerCount - 1);
      });
      // Registered before the removal is attached, so the set can never be
      // observed empty while this release is still outstanding.
      pendingDeferredReleases.add(deferred);
      void deferred.finally(() => {
        pendingDeferredReleases.delete(deferred);
      });
    }
    if (releaseWorkerAdmission && !workerAdmissionReleased) {
      workerAdmissionReleased = true;
      activeCorpusWorkerCount = Math.max(0, activeCorpusWorkerCount - 1);
    }
    if (releaseReservation) releaseCorpusMemory(reservation);
  }
}
