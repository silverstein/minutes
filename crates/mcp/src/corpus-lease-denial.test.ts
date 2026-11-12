import { afterEach, describe, expect, it, vi } from "vitest";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  corpusDenialReason,
  MeetingsRootError,
  validateCorpusDenialReason,
  withStableCorpusLease,
} from "./corpus-lease.js";
import { retireBoundReadersForProcessShutdown } from "./secure-read.js";

const roots: string[] = [];
function fixture() {
  const root = mkdtempSync(join(tmpdir(), "minutes-denial-private-"));
  roots.push(root);
  return root;
}
afterEach(async () => {
  await retireBoundReadersForProcessShutdown();
  vi.restoreAllMocks();
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function diagnostics() {
  let output = "";
  vi.spyOn(process.stderr, "write").mockImplementation((chunk: unknown) => {
    output += String(chunk);
    return true;
  });
  return () => output;
}
const flushDiagnostic = () => new Promise<void>((resolve) => setImmediate(resolve));
const genericDenial = /^Access denied: stable meeting corpus authorization failed$/;

describe("content-free corpus denial reasons", () => {
  it.each(["missing", "file", "file-parent"])("gives actionable guidance for a %s root before any projection or worker", async (kind) => {
    const parent = fixture();
    const root = join(parent, "PRIVATE_FOLDER");
    if (kind !== "missing") writeFileSync(root, "PRIVATE_CONTENT");
    const configured = kind === "file-parent" ? join(root, "child") : root;
    const operation = vi.fn();
    const output = diagnostics();
    await expect(withStableCorpusLease(configured, operation, {
      workerScriptForTest: join(parent, "worker-does-not-exist.mjs"),
    })).rejects.toThrow(MeetingsRootError);
    await expect(withStableCorpusLease(configured, operation)).rejects.toThrow(/Check the Meetings Directory setting.*default: ~\/meetings.*record a meeting first/);
    expect(operation).not.toHaveBeenCalled();
    await flushDiagnostic();
    expect(output()).toContain(kind === "missing" ? "root_missing" : "root_not_directory");
    expect(output()).not.toContain(parent);
    expect(output()).not.toContain("PRIVATE");
  });

  it.each([
    "root_missing", "root_not_directory", "budget_exceeded", "corpus_changed",
    "sentinel_not_private", "fs_error:EACCES", "internal",
  ])("carries the whitelisted worker reason %s to diagnostics", async (reason) => {
    const root = fixture();
    const worker = join(root, "refusing-worker.mjs");
    writeFileSync(worker, `process.stdin.once("data", () => { process.stdout.write(${JSON.stringify(JSON.stringify({ type: "denied", reason }) + "\n")}, () => process.exit(70)); });`);
    const output = diagnostics();
    const operation = vi.fn();
    await expect(withStableCorpusLease(root, operation, { workerScriptForTest: worker }))
      .rejects.toThrow(reason.startsWith("root_") ? MeetingsRootError : genericDenial);
    await flushDiagnostic();
    expect(output()).toContain(`worker refusal: ${reason}`);
    expect(output()).not.toContain(root);
    expect(output()).not.toContain("refusing-worker.mjs");
    expect(operation).not.toHaveBeenCalled();
  });

  it.each([
    "/private/PRIVATE_MEETING.md", "fs_error:/private/PRIVATE_MEETING.md",
    "fs_error:UNKNOWN", "corpus_changed\nPRIVATE_CONTENT", "x".repeat(65),
    "x".repeat(10_000), null, 42, { path: "/private/PRIVATE_MEETING.md" },
  ])("rejects forged or oversized worker reasons (%#)", async (reason) => {
    const root = fixture();
    const worker = join(root, "forged-worker.mjs");
    writeFileSync(worker, `process.stdin.once("data", () => { process.stdout.write(${JSON.stringify(JSON.stringify({ type: "denied", reason }) + "\n")}, () => process.exit(70)); });`);
    const output = diagnostics();
    expect(validateCorpusDenialReason(reason)).toBe("internal");
    await expect(withStableCorpusLease(root, () => "unreachable", { workerScriptForTest: worker })).rejects.toThrow(genericDenial);
    await flushDiagnostic();
    expect(output()).toContain("worker refusal: internal");
    expect(output()).not.toContain("PRIVATE");
    expect(output()).not.toContain(root);
    expect(output().length).toBeLessThan(512);
  });

  it("retains a real budget refusal through retries and the worker boundary", async () => {
    const root = fixture();
    writeFileSync(join(root, "PRIVATE_MEETING.md"), "PRIVATE_CONTENT");
    const output = diagnostics();
    await expect(withStableCorpusLease(root, () => "unreachable", { budgets: { maxFileBytes: 1 } })).rejects.toThrow(genericDenial);
    await flushDiagnostic();
    expect(output()).toContain("worker refusal: budget_exceeded");
    expect(output()).not.toContain("PRIVATE");
    expect(output()).not.toContain(root);
  });

  it.skipIf(process.platform === "win32")("retains a real non-private sentinel refusal through retries", async () => {
    const root = fixture();
    const namespace = join(root, ".minutes-corpus-lease-v1");
    mkdirSync(namespace);
    chmodSync(namespace, 0o755);
    const output = diagnostics();
    await expect(withStableCorpusLease(root, () => "unreachable")).rejects.toThrow(genericDenial);
    await flushDiagnostic();
    expect(output()).toContain("worker refusal: sentinel_not_private");
    expect(output()).not.toContain(root);
  });

  it("retains a real changed-corpus refusal through both attempts", async () => {
    const root = fixture();
    const file = join(root, "PRIVATE_MEETING.md");
    writeFileSync(file, "PRIVATE_CONTENT");
    const output = diagnostics();
    await expect(withStableCorpusLease(root, () => "unreachable", {
      afterBaseline: ({ attempt }) => { writeFileSync(file, `PRIVATE_CHANGED_${attempt}`); },
    })).rejects.toThrow(genericDenial);
    await flushDiagnostic();
    expect(output()).toContain("worker refusal: corpus_changed");
    expect(output()).not.toContain("PRIVATE");
    expect(output()).not.toContain(root);
  });

  it("maps filesystem codes without retaining their messages or paths", () => {
    expect(corpusDenialReason(Object.assign(new Error("PRIVATE_CONTENT /private/meeting.md"), { code: "EACCES", path: "/private/meeting.md" }))).toBe("fs_error:EACCES");
    expect(corpusDenialReason(new Error("PRIVATE_CONTENT"))).toBe("internal");
  });
});
