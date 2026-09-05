import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import {
  appendFileSync,
  chmodSync,
  existsSync,
  fstatSync,
  linkSync,
  mkdtempSync,
  mkdirSync,
  openSync,
  readSync,
  readFileSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmSync as nodeRmSync,
  statSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { delimiter, join, resolve } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { ResourceUpdatedNotificationSchema } from "@modelcontextprotocol/sdk/types.js";
import { afterEach, describe, expect, it } from "vitest";
import { z } from "zod";

import { retireBoundReadersForProcessShutdown } from "./secure-read.js";
import {
  afterRequiredCli,
  afterActiveCopilotReadiness,
  afterContentBearingToolReadiness,
  afterContentResourceReadiness,
  afterAgentTrustReadiness,
  assistantSafeContextLinks,
  boundedCorePersonProfile,
  buildMcpProcessAudioArgs,
  buildLiveCopilotResourcePayload,
  buildLiveEventsResourcePayload,
  buildPrivacySafeProcessingJobsResult,
  buildPrivacySafeStatusResource,
  buildPrivacySafeStatusText,
  canonicalPathWireEquals,
  collectPolicyVerifiedMeetingSnapshots,
  collectPolicyToolSearchSnapshots,
  contentBearingAgentToolNames,
  contentBearingAgentResourceNames,
  enforceRestrictedContentPolicy,
  enrichWithFrontmatter,
  extractMarkdownSection,
  getEffectiveMeetingsDir,
  getReleaseBinaryName,
  handleMcpProcessAudioRequest,
  historicalCommitmentRows,
  isActiveCorpusMeetingPath,
  isPathWithinCanonicalRoot,
  LIVE_COPILOT_RESOURCE_URI,
  LIVE_EVENTS_RESOURCE_URI,
  LIVE_EVENTS_SUBSCRIPTIONS_ENABLED,
  liveMeetingSnippet,
  meetingDetailPayload,
  verifiedCliSpeakerOverlay,
  verifiedStopRecordingSummary,
  meetingListItem,
  meetingSearchItem,
  mcpCliChildEnv,
  MCP_ADD_NOTE_INPUT_SCHEMA,
  MCP_ACTION_RESULT_MAX,
  MCP_AGENT_ANNOTATIONS_UNAVAILABLE_DESCRIPTION,
  releaseInsightsWithLiveSourcePolicy,
  resolveCorpusRelativeSourcePath,
  resolveInsightToolDeps,
  meetsInsightConfidence,
  insightMentionsParticipant,
  insightIsSince,
  parseInsightSinceFloor,
  revalidateDerivedRecordSource,
  MCP_INSIGHT_RESULT_MAX,
  MCP_INSIGHT_SCAN_WINDOW,
  MCP_INTENT_RESULT_MAX,
  MCP_MEETING_RESULT_MAX,
  MCP_MEETING_INSIGHTS_DESCRIPTION,
  MCP_PERSON_PROFILE_DECISION_MAX,
  MCP_PERSON_PROFILE_MEETING_MAX,
  MCP_PERSON_PROFILE_OPEN_ACTION_MAX,
  MCP_PERSON_PROFILE_TOPIC_MAX,
  MCP_POLICY_MEETING_RESULT_MAX,
  MCP_PROCESSING_JOB_RESULT_MAX,
  MCP_PROCESS_AUDIO_MAX_ACTIVE_JOBS,
  MCP_PROCESS_AUDIO_CLI_TIMEOUT_MS,
  MCP_PROCESS_AUDIO_MAX_STDERR_BYTES,
  MCP_PROCESS_AUDIO_MAX_STDOUT_BYTES,
  MCP_RELATIONSHIP_RESULT_MAX,
  MCP_RESEARCH_DECISION_RESULT_MAX,
  MCP_RESEARCH_MEETING_RESULT_MAX,
  MCP_RESEARCH_TOPIC_RESULT_MAX,
  MEETING_INSIGHT_KINDS,
  normalizeCanonicalPathWire,
  normalizeMcpMeetingResultLimit,
  relationshipMapStructuredContent,
  mcpProcessAudioPlatformPolicy,
  openActionsFromMeetings,
  personProfileFromMeetings,
  policyCommitmentResults,
  policyIntentResults,
  parseCopilotNudgeLog,
  parseCopilotStatusOutput,
  parseKnowledgeConfig,
  parseDictationModelMissingError,
  parseHealthOutput,
  parseMeetingsRootSnapshot,
  parseLiveEventsResourceUri,
  parsePolicyVerifiedMeeting,
  policyVerifiedExactMeetingSnapshot,
  policyListMeetings,
  policySearchMeetings,
  registerDocsAppToolWithRestrictedPolicy,
  registerLiveEventsSubscriptionHandlers,
  registerToolWithRestrictedPolicy,
  registerUnavailableCompatibilityTools,
  cliMissingGuidance,
  readAgentTrustReadiness,
  readCopilotStatusFromCli,
  readKnowledgeStatusSnapshot,
  readLiveEventsResource,
  readVerifiedScreenImage,
  createCapabilityRepairCoordinator,
  repairCliCapabilities,
  ensureWhisperModel,
  researchTopicProjection,
  runAgentToolPolicies,
  stopCopilotBeforeStatusRead,
  restrictedMeetingStubResult,
  restrictedContentPolicyFromEnv,
  requireAgentTrustReadiness,
  terminalControlBeforeContentReadiness,
  selectCopilotNudges,
  shouldRunMainEntry,
  runIsolatedMcpProcessAudio,
  validateMcpProcessAudioInput,
  withAuthorizedMcpProcessAudioInput,
  withPolicyBoundContextPath,
  type CopilotNudgeObservation,
  type AuthorizedMcpProcessAudioInput,
} from "./index.js";

const deferredWindowsCleanup = new Set<string>();

function rmSync(
  path: Parameters<typeof nodeRmSync>[0],
  options?: Parameters<typeof nodeRmSync>[1]
): void {
  try {
    nodeRmSync(path, options);
  } catch (error) {
    if (
      process.platform === "win32" &&
      typeof path === "string" &&
      options?.recursive === true &&
      options.force === true &&
      (error as NodeJS.ErrnoException)?.code === "EBUSY"
    ) {
      deferredWindowsCleanup.add(path);
      return;
    }
    throw error;
  }
}

afterEach(async () => {
  await retireBoundReadersForProcessShutdown();
  for (const path of deferredWindowsCleanup) {
    nodeRmSync(path, { recursive: true, force: true });
  }
  deferredWindowsCleanup.clear();
});

describe("dictation model preflight errors", () => {
  it("extracts the model, expected path, and interrupted-download repair command", () => {
    const error = [
      "Error: Dictation model not installed: small",
      "Expected: /Users/test/.minutes/models/ggml-small.bin",
      "Fix: rm \"/Users/test/.minutes/models/ggml-small.bin\" && minutes setup --model small",
    ].join("\n");

    expect(parseDictationModelMissingError(error)).toEqual({
      model: "small",
      expectedPath: "/Users/test/.minutes/models/ggml-small.bin",
      setupCommand:
        "rm \"/Users/test/.minutes/models/ggml-small.bin\" && minutes setup --model small",
    });
  });

  it("ignores unrelated startup errors", () => {
    expect(parseDictationModelMissingError("microphone permission denied")).toBeNull();
  });
});

describe("Whisper model auto-setup", () => {
  it("does not probe or download models when the host opts out", async () => {
    const checkState = { done: false };
    await ensureWhisperModel({
      autoSetup: "0",
      checkState,
      health: async () => { throw new Error("health must not run"); },
      setup: async () => { throw new Error("setup must not run"); },
      log: () => { throw new Error("no background setup should run"); },
    });
    expect(checkState.done).toBe(false);
  });

  const readyItem = { label: "Speech model", state: "ready", detail: "medium" };
  const missingItem = { label: "Speech model", state: "attention", detail: "missing" };

  async function runModelCheck(input: {
    health: unknown;
    configExists?: boolean;
  }): Promise<{ setups: Array<string | undefined>; logs: string[] }> {
    const setups: Array<string | undefined> = [];
    const logs: string[] = [];
    await ensureWhisperModel({
      checkState: { done: false },
      health: async () => {
        if (input.health instanceof Error) throw input.health;
        return typeof input.health === "string"
          ? input.health
          : JSON.stringify(input.health);
      },
      configFileExists: async () => input.configExists ?? false,
      setup: async (model) => {
        setups.push(model);
      },
      log: (message) => logs.push(message),
    });
    return { setups, logs };
  }

  it("parses legacy and current health output shapes", () => {
    expect(parseHealthOutput(JSON.stringify([readyItem]))).toEqual({
      ok: true,
      items: [readyItem],
    });
    expect(
      parseHealthOutput(
        JSON.stringify({
          ok: true,
          data: {
            engine: "parakeet",
            effective_engine: "whisper",
            reason: "whisper: configured engine unavailable",
            model: "small",
            items: [missingItem],
          },
        })
      )
    ).toEqual({
      ok: true,
      items: [missingItem],
      engine: "parakeet",
      effectiveEngine: "whisper",
      reason: "whisper: configured engine unavailable",
      model: "small",
    });
  });

  it.each([
    ["a failed health command", new Error("health failed")],
    ["an ok:false envelope", { ok: false, data: { items: [missingItem] } }],
    ["invalid health items", { ok: true, data: { items: ["invalid"] } }],
  ])("fails closed for %s", async (_label, health) => {
    const result = await runModelCheck({ health });

    expect(result.setups).toEqual([]);
    expect(result.logs).toHaveLength(1);
  });

  it.each([
    ["an empty legacy array", []],
    ["an array without the speech model item", [{ label: "CLI", state: "ready" }]],
  ])("fails closed for %s", async (_label, health) => {
    const result = await runModelCheck({ health });

    expect(result.setups).toEqual([]);
    expect(result.logs).toContain(
      "[Minutes] unrecognized health items — skipping Whisper auto-setup"
    );
  });

  it("skips setup when the speech model is ready", async () => {
    const result = await runModelCheck({
      health: { ok: true, data: { engine: "whisper", items: [readyItem] } },
    });

    expect(result.setups).toEqual([]);
    expect(result.logs).toContain("[Minutes] Whisper model ready");
  });

  it("keeps accepting the legacy bare health-item array", async () => {
    const result = await runModelCheck({ health: [readyItem] });

    expect(result.setups).toEqual([]);
    expect(result.logs).toContain("[Minutes] Whisper model ready");
  });

  it("fails closed for an unknown speech model state", async () => {
    const result = await runModelCheck({
      health: {
        ok: true,
        data: { engine: "whisper", items: [{ ...missingItem, state: "unknown" }] },
      },
    });

    expect(result.setups).toEqual([]);
    expect(result.logs).toContain(
      "[Minutes] Speech model health state is unknown — skipping Whisper auto-setup"
    );
  });

  it("uses the effective engine and reported model when configured parakeet resolves to Whisper", async () => {
    const result = await runModelCheck({
      health: {
        ok: true,
        data: {
          engine: "parakeet",
          effective_engine: "whisper",
          model: "small",
          items: [missingItem],
        },
      },
    });

    expect(result.setups).toEqual(["small"]);
  });

  it("skips setup when the effective engine is non-Whisper", async () => {
    const result = await runModelCheck({
      health: {
        ok: true,
        data: {
          engine: "parakeet",
          effective_engine: "parakeet",
          items: [missingItem],
        },
      },
    });

    expect(result.setups).toEqual([]);
    expect(result.logs.join("\n")).toContain("skipping Whisper auto-setup");
  });

  it("does not run tiny setup when auto resolves to sherpa", async () => {
    const result = await runModelCheck({
      health: {
        ok: true,
        data: {
          engine: "auto",
          effective_engine: "sherpa",
          items: [missingItem],
        },
      },
    });

    expect(result.setups).toEqual([]);
    expect(result.logs).toContain(
      "[Minutes] Auto transcription resolved to sherpa — skipping Whisper auto-setup"
    );
  });

  it("runs plain setup when auto has the plugin but the Parakeet model is missing", async () => {
    const result = await runModelCheck({
      health: {
        ok: true,
        data: {
          engine: "auto",
          effective_engine: "whisper",
          reason: "whisper: parakeet model missing — run minutes setup",
          model: "small",
          items: [readyItem],
        },
      },
    });

    expect(result.setups).toEqual([undefined]);
    expect(result.logs.join("\n")).toContain("running minutes setup");
  });

  it("keeps explicit Whisper on the model-specific setup path for the same reason text", async () => {
    const result = await runModelCheck({
      health: {
        ok: true,
        data: {
          engine: "whisper",
          effective_engine: "whisper",
          reason: "whisper: parakeet model missing — run minutes setup",
          model: "medium",
          items: [missingItem],
        },
      },
    });

    expect(result.setups).toEqual(["medium"]);
  });

  it("conservatively skips an old CLI non-Whisper engine with upgrade guidance", async () => {
    const result = await runModelCheck({
      health: { ok: true, data: { engine: "parakeet", items: [missingItem] } },
    });

    expect(result.setups).toEqual([]);
    expect(result.logs.join("\n")).toContain(
      "upgrade the CLI to let auto-setup resolve the effective engine"
    );
  });

  it("treats a capitalized effective Whisper engine as Whisper", async () => {
    const result = await runModelCheck({
      health: {
        ok: true,
        data: { effective_engine: "Whisper", model: "medium", items: [missingItem] },
      },
    });

    expect(result.setups).toEqual(["medium"]);
  });

  it("uses the reported model as the only mutation path for an existing config", async () => {
    const result = await runModelCheck({
      health: {
        ok: true,
        data: { effective_engine: "whisper", model: "large-v3", items: [missingItem] },
      },
      configExists: true,
    });

    expect(result.setups).toEqual(["large-v3"]);
  });

  it("does not mutate an existing config when an old CLI omits the model", async () => {
    const result = await runModelCheck({
      health: { ok: true, data: { engine: "whisper", items: [missingItem] } },
      configExists: true,
    });

    expect(result.setups).toEqual([]);
    expect(result.logs).toContain(
      "[Minutes] config exists but this CLI does not report the configured model; run `minutes setup --model <your model>` or upgrade the CLI"
    );
  });

  it("keeps zero-touch tiny setup for an old CLI on a fresh machine", async () => {
    const result = await runModelCheck({
      health: { ok: true, data: { engine: "whisper", items: [missingItem] } },
      configExists: false,
    });

    expect(result.setups).toEqual(["tiny"]);
  });
});

describe("release asset selection", () => {
  it.each([
    ["darwin", "arm64", "minutes-macos-arm64-sherpa.tar.gz"],
    ["darwin", "x64", "minutes-macos-arm64-sherpa.tar.gz"],
    ["linux", "x64", "minutes-linux-x64"],
    ["win32", "x64", "minutes-windows-x64.zip"],
    ["linux", "arm64", null],
  ] as const)("selects %s/%s", (platform, arch, expected) => {
    expect(getReleaseBinaryName(platform, arch)).toBe(expected);
  });
});

function rustCanonicalPathWire(path: string): string {
  if (process.platform !== "win32") return path;
  return path.startsWith("\\\\")
    ? `\\\\?\\UNC\\${path.slice(2)}`
    : `\\\\?\\${path}`;
}

type ProcessAudioFixtureOptions = {
  binary: string;
  timeoutMs?: number;
  maxStdoutBytes?: number;
  maxStderrBytes?: number;
  extraEnv?: Record<string, string>;
};

function boundedFixtureLimit(
  requested: number | undefined,
  productionLimit: number
): number {
  const value = requested ?? productionLimit;
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new Error("process_audio resource budget was invalid");
  }
  return Math.min(value, productionLimit);
}

function killFixtureGroup(child: ReturnType<typeof spawn>): void {
  const pid = child.pid;
  if (process.platform !== "win32" && Number.isSafeInteger(pid) && (pid ?? 0) > 0) {
    try {
      process.kill(-(pid as number), "SIGKILL");
      return;
    } catch {
      // Fall through to the exact child handle for a pre-group spawn race.
    }
  }
  try {
    child.kill("SIGKILL");
  } catch {
    // The fixture may already have exited.
  }
}

/** Synthetic-executable harness only; production has no direct fd-3 runner. */
async function runProcessAudioFixtureCli(
  input: AuthorizedMcpProcessAudioInput,
  contentType: "meeting" | "memo",
  language: string | undefined,
  options: ProcessAudioFixtureOptions
): Promise<{ stdout: string; stderr: string }> {
  const args = buildMcpProcessAudioArgs(input, contentType, language);
  const safeExtraEnv = { ...(options.extraEnv ?? {}) };
  delete safeExtraEnv.MINUTES_MCP_OUTER_PROCESS_GROUP;
  const timeoutMs = boundedFixtureLimit(
    options.timeoutMs,
    MCP_PROCESS_AUDIO_CLI_TIMEOUT_MS
  );
  const maxStdoutBytes = boundedFixtureLimit(
    options.maxStdoutBytes,
    MCP_PROCESS_AUDIO_MAX_STDOUT_BYTES
  );
  const maxStderrBytes = boundedFixtureLimit(
    options.maxStderrBytes,
    MCP_PROCESS_AUDIO_MAX_STDERR_BYTES
  );

  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(options.binary, args, {
      detached: true,
      stdio: ["ignore", "pipe", "pipe", input.fd],
      env: mcpCliChildEnv({ RUST_LOG: "info", ...safeExtraEnv }),
    });
    const stdoutChunks: Buffer[] = [];
    const stderrChunks: Buffer[] = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let failure: string | undefined;
    let settled = false;
    const requestFailure = (message: string): void => {
      if (failure === undefined) failure = message;
      killFixtureGroup(child);
    };
    const timer = setTimeout(
      () => requestFailure("process_audio CLI exceeded its time budget"),
      timeoutMs
    );
    child.stdout?.on("data", (value: Buffer | string) => {
      const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value);
      stdoutBytes += bytes.byteLength;
      if (stdoutBytes > maxStdoutBytes) {
        requestFailure("process_audio CLI stdout exceeded its byte budget");
      } else {
        stdoutChunks.push(bytes);
      }
    });
    child.stderr?.on("data", (value: Buffer | string) => {
      const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value);
      stderrBytes += bytes.byteLength;
      if (stderrBytes > maxStderrBytes) {
        requestFailure("process_audio CLI stderr exceeded its byte budget");
      } else {
        stderrChunks.push(bytes);
      }
    });
    child.once("error", () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      killFixtureGroup(child);
      rejectRun(new Error("process_audio CLI could not be started safely"));
    });
    child.once("close", (code) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      killFixtureGroup(child);
      if (failure !== undefined) {
        rejectRun(new Error(failure));
      } else if (code !== 0) {
        rejectRun(new Error("process_audio CLI failed safely"));
      } else {
        resolveRun({
          stdout: Buffer.concat(stdoutChunks).toString("utf8").trim(),
          stderr: Buffer.concat(stderrChunks).toString("utf8").trim(),
        });
      }
    });
  });
}

describe("verified screen image reads", () => {
  const png = (suffix = "") =>
    Buffer.concat([
      Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
      Buffer.from(suffix),
    ]);

  it("reads a stable PNG from an explicitly bound screen root", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-screen-root-"));
    try {
      const session = join(root, "session-a");
      mkdirSync(session);
      const image = join(session, "capture.png");
      const bytes = png("SCREEN_CANARY");
      writeFileSync(image, bytes);

      await expect(
        readVerifiedScreenImage(
          image,
          bytes.length,
          createHash("sha256").update(bytes).digest("hex"),
          root
        )
      ).resolves.toEqual(bytes);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("rejects a leaf replacement after the bound reader's first read", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-screen-leaf-swap-"));
    try {
      const session = join(root, "session-a");
      mkdirSync(session);
      const image = join(session, "capture.png");
      const original = png("ORIGINAL_SCREEN_CANARY");
      writeFileSync(image, original);

      await expect(
        readVerifiedScreenImage(
          image,
          original.length,
          createHash("sha256").update(original).digest("hex"),
          root,
          {
            afterFirstRead: () => {
              rmSync(image);
              writeFileSync(image, png("REPLACEMENT_SCREEN_CANARY"));
            },
          }
        )
      ).rejects.toThrow(/Access denied/i);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("rejects a parent replacement after validation", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-screen-parent-swap-"));
    try {
      const session = join(root, "session-a");
      const displaced = join(root, "session-displaced");
      mkdirSync(session);
      const image = join(session, "capture.png");
      const original = png("ORIGINAL_PARENT_CANARY");
      writeFileSync(image, original);

      await expect(
        readVerifiedScreenImage(
          image,
          original.length,
          createHash("sha256").update(original).digest("hex"),
          root,
          {
            afterFirstRead: () => {
              renameSync(session, displaced);
              mkdirSync(session);
              writeFileSync(image, png("REPLACEMENT_PARENT_CANARY"));
            },
          }
        )
      ).rejects.toThrow(/Access denied/i);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("rejects oversized and signature-invalid PNG paths", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-screen-invalid-"));
    try {
      const session = join(root, "session-a");
      mkdirSync(session);
      const fake = join(session, "fake.png");
      const fakeBytes = Buffer.from("NOT_A_PNG");
      writeFileSync(fake, fakeBytes);
      await expect(
        readVerifiedScreenImage(
          fake,
          fakeBytes.length,
          createHash("sha256").update(fakeBytes).digest("hex"),
          root
        )
      ).rejects.toThrow("not a verified PNG");

      const oversized = join(session, "oversized.png");
      const bytes = Buffer.alloc(10 * 1024 * 1024 + 1);
      png().copy(bytes);
      writeFileSync(oversized, bytes);
      await expect(
        readVerifiedScreenImage(
          oversized,
          bytes.length,
          createHash("sha256").update(bytes).digest("hex"),
          root
        )
      ).rejects.toThrow("invalid capture-time byte bound");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("rejects same-size bytes that no longer match the capture-time digest", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-screen-digest-"));
    try {
      const session = join(root, "session-a");
      mkdirSync(session);
      const image = join(session, "capture.png");
      const original = png("ORIGINAL_BYTES");
      const replacement = png("REPLACEMENT_BY");
      expect(replacement.length).toBe(original.length);
      writeFileSync(image, replacement);

      await expect(
        readVerifiedScreenImage(
          image,
          original.length,
          createHash("sha256").update(original).digest("hex"),
          root
        )
      ).rejects.toThrow("capture-time digest");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

describe("privacy-safe operational status", () => {
  const forbiddenCanaries = [
    "TITLE_PRIVATE_CANARY",
    "AUDIO_PATH_PRIVATE_CANARY",
    "OUTPUT_PATH_PRIVATE_CANARY",
    "USER_NOTES_PRIVATE_CANARY",
    "PRE_CONTEXT_PRIVATE_CANARY",
    "CONSENT_PRIVATE_CANARY",
    "CALENDAR_PRIVATE_CANARY",
    "TEMPLATE_PRIVATE_CANARY",
    "ERROR_PRIVATE_CANARY",
    "RAW_STAGE_PRIVATE_CANARY",
  ];

  it("projects job records into a closed path-free text and structured schema", () => {
    const result = buildPrivacySafeProcessingJobsResult([
      {
        id: "job-20260715123456789-4321-0",
        state: "Summarizing",
        stage: "RAW_STAGE_PRIVATE_CANARY",
        title: "TITLE_PRIVATE_CANARY",
        audio_path: "/private/AUDIO_PATH_PRIVATE_CANARY.wav",
        output_path: "/private/OUTPUT_PATH_PRIVATE_CANARY.md",
        user_notes: "USER_NOTES_PRIVATE_CANARY",
        pre_context: "PRE_CONTEXT_PRIVATE_CANARY",
        consent_notice: "CONSENT_PRIVATE_CANARY",
        calendar_event: { title: "CALENDAR_PRIVATE_CANARY" },
        template_slug: "TEMPLATE_PRIVATE_CANARY",
        error: "ERROR_PRIVATE_CANARY",
      },
    ]);

    expect(result).toEqual({
      content: [
        {
          type: "text",
          text: "Processing jobs:\n\n- job-20260715123456789-4321-0: summarizing — Generating summary",
        },
      ],
      structuredContent: {
        jobs: [
          {
            id: "job-20260715123456789-4321-0",
            state: "summarizing",
            stage: "Generating summary",
          },
        ],
      },
    });
    const serialized = JSON.stringify(result);
    for (const canary of forbiddenCanaries) {
      expect(serialized).not.toContain(canary);
    }
  });

  it("stops projecting jobs at the documented processing-result cap", () => {
    const jobs = Array.from(
      { length: MCP_PROCESSING_JOB_RESULT_MAX + 25 },
      (_, index) => ({
        id: `job-20260715123456789-4321-${index}`,
        state: "queued",
      })
    );
    const result = buildPrivacySafeProcessingJobsResult(jobs);
    expect(result.structuredContent.jobs).toHaveLength(
      MCP_PROCESSING_JOB_RESULT_MAX
    );
    expect(result.content[0].text).not.toContain(
      `job-20260715123456789-4321-${MCP_PROCESSING_JOB_RESULT_MAX}`
    );
  });

  it("drops source-derived fields from both status text and the status resource", () => {
    const rawStatus = {
      recording: false,
      processing: true,
      processing_stage: "Generating summary",
      recording_mode: "meeting",
      processing_job_count: 2,
      processing_title: "TITLE_PRIVATE_CANARY",
      processing_job_id: "OUTPUT_PATH_PRIVATE_CANARY",
      pid: 4321,
      duration_secs: 42,
      wav_path: "/private/AUDIO_PATH_PRIVATE_CANARY.wav",
      error: "ERROR_PRIVATE_CANARY",
    };

    const text = buildPrivacySafeStatusText(rawStatus);
    const resource = buildPrivacySafeStatusResource(rawStatus);
    expect(text).toBe("Processing: Generating summary (2 jobs queued)");
    expect(JSON.parse(resource.contents[0].text)).toEqual({
      schema_version: 1,
      status_available: true,
      recording: false,
      processing: true,
      recording_mode: "meeting",
      processing_stage: "Generating summary",
      processing_job_count: 2,
    });
    const serialized = JSON.stringify({ text, resource });
    for (const canary of forbiddenCanaries) {
      expect(serialized).not.toContain(canary);
    }
  });

  it("fails closed without echoing malformed CLI payloads", () => {
    expect(
      buildPrivacySafeProcessingJobsResult([
        {
          id: "TITLE_PRIVATE_CANARY",
          state: "RAW_STAGE_PRIVATE_CANARY",
          stage: "PRE_CONTEXT_PRIVATE_CANARY",
        },
      ])
    ).toEqual({
      content: [
        {
          type: "text",
          text: "Processing jobs:\n\n- job-1: unknown — Status unavailable",
        },
      ],
      structuredContent: {
        jobs: [{ id: "job-1", state: "unknown", stage: "Status unavailable" }],
      },
    });
    const unavailable = buildPrivacySafeStatusResource("ERROR_PRIVATE_CANARY");
    expect(unavailable.contents[0].text).not.toContain("ERROR_PRIVATE_CANARY");
    expect(JSON.parse(unavailable.contents[0].text)).toMatchObject({
      status_available: false,
      recording: false,
      processing: false,
    });
  });
});

describe.runIf(process.platform !== "win32")(
  "assistant child and derived-input policy",
  () => {
  it("compares only Rust and Node Windows canonical path wire spellings", () => {
    const drive = "C:\\Users\\test\\meetings\\normal.md";
    const unc = "\\\\server\\share\\meetings\\normal.md";

    expect(normalizeCanonicalPathWire(`\\\\?\\${drive}`)).toBe(drive);
    expect(
      normalizeCanonicalPathWire("\\\\?\\UNC\\server\\share\\meetings\\normal.md")
    ).toBe(unc);
    expect(canonicalPathWireEquals(`\\\\?\\${drive}`, drive)).toBe(true);
    expect(
      canonicalPathWireEquals(
        "\\\\?\\UNC\\server\\share\\meetings\\normal.md",
        unc
      )
    ).toBe(true);

    // Keep every non-namespace distinction exact. This is not general Windows
    // path normalization and cannot authorize case, separator, dot-segment,
    // trailing-separator, device-namespace, or relative-path differences.
    expect(
      canonicalPathWireEquals(`\\\\?\\${drive}`, drive.toLowerCase())
    ).toBe(false);
    expect(
      canonicalPathWireEquals(`\\\\?\\${drive}`, drive.replaceAll("\\", "/"))
    ).toBe(false);
    expect(canonicalPathWireEquals(`${drive}\\`, drive)).toBe(false);
    expect(
      canonicalPathWireEquals(
        "\\\\?\\GLOBALROOT\\Device\\x",
        "GLOBALROOT\\Device\\x"
      )
    ).toBe(false);
    expect(canonicalPathWireEquals("normal.md", drive)).toBe(false);
  });

  it("puts the Minutes-owned install directory on the child PATH", () => {
    // The Windows auto-installer targets ~/.minutes/bin (#657). This server
    // uses the absolute path, but plugin skills shell out to a bare `minutes`,
    // so the directory has to reach child processes or those fail immediately
    // after a successful install.
    const parts = (mcpCliChildEnv().PATH || "").split(delimiter);
    expect(parts).toContain(join(homedir(), ".minutes", "bin"));
  });

  it("forces the CLI deny policy after ambient and call-site overrides", () => {
    const previous = process.env.MINUTES_CLI_RESTRICTED_POLICY;
    try {
      process.env.MINUTES_CLI_RESTRICTED_POLICY = "logged-override";
      expect(mcpCliChildEnv().MINUTES_CLI_RESTRICTED_POLICY).toBe("deny");
      expect(
        mcpCliChildEnv({ MINUTES_CLI_RESTRICTED_POLICY: "allow" })
          .MINUTES_CLI_RESTRICTED_POLICY
      ).toBe("deny");
      expect(
        mcpCliChildEnv({ MINUTES_POLICY_GRAPH_CORPUS_ROOT: "/synthetic/meetings" })
          .MINUTES_POLICY_GRAPH_CORPUS_ROOT
      ).toBe("/synthetic/meetings");
      delete process.env.MINUTES_CLI_RESTRICTED_POLICY;
      expect(mcpCliChildEnv().MINUTES_CLI_RESTRICTED_POLICY).toBe("deny");
    } finally {
      if (previous === undefined) {
        delete process.env.MINUTES_CLI_RESTRICTED_POLICY;
      } else {
        process.env.MINUTES_CLI_RESTRICTED_POLICY = previous;
      }
    }
  });

  it("repairs an installed same-major CLI that lacks new graph capabilities", async () => {
    const oldProbe = {
      kind: "report" as const,
      report: {
        version: "0.23.0",
        api_version: 1,
        features: { policy_projection_worker_v1: false },
      },
    };
    const repairedProbe = {
      kind: "report" as const,
      report: {
        version: "0.23.1",
        api_version: 1,
        features: { policy_projection_worker_v1: true },
      },
    };
    let repairs = 0;
    let reprobes = 0;
    await expect(
      repairCliCapabilities(
        ["policy_projection_worker_v1"],
        oldProbe,
        async () => {
          repairs += 1;
          return true;
        },
        () => {
          reprobes += 1;
          return repairedProbe;
        }
      )
    ).resolves.toEqual(repairedProbe);
    expect(repairs).toBe(1);
    expect(reprobes).toBe(1);
  });

  it("deduplicates concurrent capability repair and permits one bounded retry", async () => {
    let finishFirst!: (value: boolean) => void;
    let attempts = 0;
    const first = new Promise<boolean>((resolve) => {
      finishFirst = resolve;
    });
    const repair = createCapabilityRepairCoordinator(async () => {
      attempts += 1;
      if (attempts === 1) return first;
      return true;
    }, 2);

    const left = repair();
    const right = repair();
    expect(attempts).toBe(1);
    finishFirst(false);
    await expect(Promise.all([left, right])).resolves.toEqual([false, false]);
    await expect(repair()).resolves.toBe(true);
    await expect(repair()).resolves.toBe(false);
    expect(attempts).toBe(2);
  });

  it("fails Windows process_audio closed before CLI, validation, reads, or fd retention", async () => {
    const pathCanary = "C:\\Synthetic\\Downloads\\PRIVATE-AUDIO-PATH-CANARY.wav";
    let cliChecks = 0;
    let executions = 0;
    const result = await handleMcpProcessAudioRequest(
      { file_path: pathCanary, type: "memo" },
      {
        isCliAvailable: async () => {
          cliChecks += 1;
          throw new Error("CLI availability must not be inspected");
        },
        execute: async () => {
          executions += 1;
          throw new Error("validation/read/fd retention must not execute");
        },
      },
      "win32"
    );

    expect(cliChecks).toBe(0);
    expect(executions).toBe(0);
    expect(result.isError).toBe(true);
    expect(result.structuredContent).toEqual({
      available: false,
      error: "windows-agent-audio-fd-unavailable",
    });
    expect(JSON.stringify(result)).not.toContain(pathCanary);
    expect(JSON.stringify(result)).toMatch(/No audio was read or passed/i);
    expect(mcpProcessAudioPlatformPolicy("darwin")).toEqual({ available: true });
    expect(mcpProcessAudioPlatformPolicy("linux")).toEqual({ available: true });
    expect(mcpProcessAudioPlatformPolicy("freebsd")).toMatchObject({
      available: false,
      error: expect.stringMatching(/only on macOS and Linux/i),
    });
  });

  it("returns an honest structured error when the CLI is unavailable", async () => {
    const pathCanary = "/synthetic/PRIVATE-CLI-UNAVAILABLE-PATH.wav";
    let executions = 0;
    const result = await handleMcpProcessAudioRequest(
      { file_path: pathCanary, type: "memo" },
      {
        isCliAvailable: async () => false,
        execute: async () => {
          executions += 1;
          throw new Error("must not execute");
        },
      },
      "linux"
    );

    expect(executions).toBe(0);
    expect(result.isError).toBe(true);
    expect(result.structuredContent).toEqual({
      available: false,
      error: "cli-unavailable",
    });
    expect(JSON.stringify(result)).not.toContain(pathCanary);
    expect(JSON.stringify(result)).toMatch(/No audio was read or passed/i);
  });

  it("accepts only a complete process_audio success contract", async () => {
    const result = await handleMcpProcessAudioRequest(
      { file_path: "/synthetic/input.wav", type: "meeting" },
      {
        isCliAvailable: async () => true,
        execute: async () => ({
          stdout: JSON.stringify({
            status: "done",
            file: " /synthetic/meeting.md ",
            title: " Synthetic review ",
            words: 42,
          }),
        }),
      },
      "linux"
    );

    expect(result.isError).not.toBe(true);
    expect(result.structuredContent).toEqual({
      available: true,
      status: "done",
      file: "/synthetic/meeting.md",
      title: "Synthetic review",
      words: 42,
    });
    expect(result.content).toEqual([{
      type: "text",
      text: "Processed: /synthetic/meeting.md\nTitle: Synthetic review\nWords: 42",
    }]);
  });

  it("bounds held CLI readiness work before any excess availability check", async () => {
    let availabilityChecks = 0;
    let executions = 0;
    let announceFull!: () => void;
    const full = new Promise<void>((resolve) => (announceFull = resolve));
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    const dependencies = {
      isCliAvailable: async () => {
        availabilityChecks += 1;
        if (availabilityChecks === MCP_PROCESS_AUDIO_MAX_ACTIVE_JOBS) {
          announceFull();
        }
        await held;
        return true;
      },
      execute: async () => {
        executions += 1;
        return {
          stdout: JSON.stringify({
            status: "done",
            file: "/synthetic/readiness.md",
            title: "Readiness bounded",
            words: 1,
          }),
        };
      },
    };
    const active = Array.from(
      { length: MCP_PROCESS_AUDIO_MAX_ACTIVE_JOBS },
      (_, index) =>
        handleMcpProcessAudioRequest(
          { file_path: `/synthetic/readiness-${index}.wav`, type: "memo" },
          dependencies,
          "linux"
        )
    );

    try {
      await full;
      const overflow = await handleMcpProcessAudioRequest(
        { file_path: "/synthetic/readiness-overflow.wav", type: "memo" },
        dependencies,
        "linux"
      );
      expect(overflow.isError).toBe(true);
      expect(overflow.structuredContent).toEqual({
        available: false,
        error: "processing-failed",
      });
      expect(availabilityChecks).toBe(MCP_PROCESS_AUDIO_MAX_ACTIVE_JOBS);
      expect(executions).toBe(0);
    } finally {
      release();
    }

    const settled = await Promise.all(active);
    expect(settled.every((result) => result.isError !== true)).toBe(true);
    expect(executions).toBe(MCP_PROCESS_AUDIO_MAX_ACTIVE_JOBS);
    const recovery = await handleMcpProcessAudioRequest(
      { file_path: "/synthetic/readiness-recovery.wav", type: "memo" },
      {
        isCliAvailable: async () => true,
        execute: async () => ({
          stdout: JSON.stringify({
            status: "done",
            file: "/synthetic/readiness-recovered.md",
            title: "Readiness recovered",
            words: 2,
          }),
        }),
      },
      "linux"
    );
    expect(recovery.isError).not.toBe(true);
  });

  it("fails excess active process_audio jobs immediately and recovers admission", async () => {
    let executions = 0;
    let announceFull!: () => void;
    const full = new Promise<void>((resolve) => (announceFull = resolve));
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    const dependencies = {
      isCliAvailable: async () => true,
      execute: async () => {
        executions += 1;
        if (executions === MCP_PROCESS_AUDIO_MAX_ACTIVE_JOBS) announceFull();
        await held;
        return {
          stdout: JSON.stringify({
            status: "done",
            file: "/synthetic/meeting.md",
            title: "Bounded job",
            words: 1,
          }),
        };
      },
    };
    const active = Array.from(
      { length: MCP_PROCESS_AUDIO_MAX_ACTIVE_JOBS },
      (_, index) =>
        handleMcpProcessAudioRequest(
          { file_path: `/synthetic/held-${index}.wav`, type: "memo" },
          dependencies,
          "linux"
        )
    );

    try {
      await full;
      const overflow = await handleMcpProcessAudioRequest(
        { file_path: "/synthetic/overflow.wav", type: "memo" },
        dependencies,
        "linux"
      );
      expect(overflow.isError).toBe(true);
      expect(overflow.structuredContent).toEqual({
        available: false,
        error: "processing-failed",
      });
      expect(executions).toBe(MCP_PROCESS_AUDIO_MAX_ACTIVE_JOBS);
    } finally {
      release();
    }

    const settled = await Promise.all(active);
    expect(settled.every((result) => result.isError !== true)).toBe(true);
    const recovery = await handleMcpProcessAudioRequest(
      { file_path: "/synthetic/recovery.wav", type: "memo" },
      {
        isCliAvailable: async () => true,
        execute: async () => ({
          stdout: JSON.stringify({
            status: "done",
            file: "/synthetic/recovered.md",
            title: "Recovered job",
            words: 2,
          }),
        }),
      },
      "linux"
    );
    expect(recovery.isError).not.toBe(true);
  });

  it("rejects malformed and contract-invalid CLI output without echoing it", async () => {
    const stdoutCanary = "PRIVATE-MALFORMED-STDOUT-CANARY";
    const invalidPayloads: string[] = [
      `not-json-${stdoutCanary}`,
      "null",
      "[]",
      JSON.stringify({ status: "pending", file: stdoutCanary, title: "Title", words: 1 }),
      JSON.stringify({ status: "done", file: "", title: "Title", words: 1, canary: stdoutCanary }),
      JSON.stringify({ status: "done", file: "/synthetic/out.md", title: "", words: 1, canary: stdoutCanary }),
      JSON.stringify({ status: "done", file: "/synthetic/out.md", title: "Title", words: -1, canary: stdoutCanary }),
      JSON.stringify({ status: "done", file: "/synthetic/out.md", title: "Title", words: 1.5, canary: stdoutCanary }),
      JSON.stringify({ status: "done", file: "/synthetic/out.md", title: "Title", words: Number.MAX_SAFE_INTEGER + 1, canary: stdoutCanary }),
      JSON.stringify({ status: "done", file: "/synthetic/out.md", title: "Title", words: "1", canary: stdoutCanary }),
    ];

    for (const stdout of invalidPayloads) {
      const result = await handleMcpProcessAudioRequest(
        { file_path: "/synthetic/input.wav", type: "memo" },
        {
          isCliAvailable: async () => true,
          execute: async () => ({ stdout }),
        },
        "linux"
      );
      expect(result.isError).toBe(true);
      expect(result.structuredContent).toEqual({
        available: false,
        error: "invalid-cli-response",
      });
      const serialized = JSON.stringify(result);
      expect(serialized).not.toContain(stdoutCanary);
      expect(serialized).not.toContain(stdout);
    }
  });

  it("redacts availability, authorization, and execution exceptions", async () => {
    const exceptionCanary = "/synthetic/PRIVATE-EXCEPTION-PATH-CANARY.wav";
    const cases = [
      {
        isCliAvailable: async () => {
          throw new Error(`availability failed at ${exceptionCanary}`);
        },
        execute: async () => ({ stdout: "must-not-run" }),
      },
      {
        isCliAvailable: async () => true,
        execute: async () => {
          throw new Error(`authorization/execution failed at ${exceptionCanary}`);
        },
      },
    ];

    for (const dependencies of cases) {
      const result = await handleMcpProcessAudioRequest(
        { file_path: exceptionCanary, type: "memo" },
        dependencies,
        "linux"
      );
      expect(result.isError).toBe(true);
      expect(result.structuredContent).toEqual({
        available: false,
        error: "processing-failed",
      });
      expect(JSON.stringify(result)).not.toContain(exceptionCanary);
    }
  });

  it("rejects every retained output-root descendant even when roots overlap", () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-process-audio-policy-"));
    try {
      const inbox = join(root, "inbox");
      const downloads = join(root, "downloads");
      const meetings = join(downloads, "meetings");
      mkdirSync(inbox);
      mkdirSync(downloads);
      mkdirSync(meetings);
      const inboxAudio = join(inbox, "new.wav");
      const retained = join(meetings, "private.voice.wav");
      writeFileSync(inboxAudio, "audio");
      writeFileSync(retained, "restricted audio");
      const extensions = [".wav"];

      expect(
        validateMcpProcessAudioInput(
          inboxAudio,
          [inbox, downloads],
          meetings,
          extensions
        )
      ).toBe(realpathSync(inboxAudio));
      expect(isPathWithinCanonicalRoot(retained, meetings)).toBe(true);
      expect(() =>
        validateMcpProcessAudioInput(
          retained,
          [inbox, downloads],
          meetings,
          extensions
        )
      ).toThrow(/retained meeting audio/i);
      expect(() =>
        validateMcpProcessAudioInput(
          retained,
          [inbox, downloads],
          downloads,
          extensions
        )
      ).toThrow(/retained meeting audio/i);

      if (process.platform !== "win32") {
        const alias = join(inbox, "alias.wav");
        symlinkSync(retained, alias);
        expect(() =>
          validateMcpProcessAudioInput(
            alias,
            [inbox, downloads],
            meetings,
            extensions
          )
        ).toThrow(/access denied|retained meeting audio/i);
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  function processAudioFixture(content = "synthetic-audio-bytes") {
    const root = mkdtempSync(join(tmpdir(), "minutes-authorized-fd-"));
    const inbox = join(root, "inbox");
    const meetings = join(root, "meetings");
    mkdirSync(inbox);
    mkdirSync(meetings);
    const source = join(inbox, "synthetic-input.wav");
    writeFileSync(source, content);
    return { root, inbox, meetings, source, content };
  }

  function writeProcessAudioFdChild(root: string): string {
    const childPath = join(root, "synthetic-fd-child.cjs");
    writeFileSync(
      childPath,
      [
        "#!/usr/bin/env node",
        "const fs = require('node:fs');",
        "const crypto = require('node:crypto');",
        "const childProcess = require('node:child_process');",
        "const mode = process.env.MINUTES_FD_CHILD_MODE || 'success';",
        "if (mode === 'timeout') { setInterval(() => {}, 1000); }",
        "else if (mode === 'descendant') {",
        "  const descendant = childProcess.spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });",
        "  fs.writeFileSync(process.env.MINUTES_DESCENDANT_PID_FILE, String(descendant.pid));",
        "  setInterval(() => {}, 1000);",
        "}",
        "else if (mode === 'success-descendant') {",
        // The real CLI marks fd 4 close-on-exec before launching engines. This
        // synthetic CLI cannot set FD_CLOEXEC from Node, so close its copy
        // before spawning to model the same descendant inheritance boundary.
        "  fs.closeSync(4);",
        "  const descendant = childProcess.spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });",
        "  descendant.unref();",
        "  fs.writeFileSync(process.env.MINUTES_DESCENDANT_PID_FILE, String(descendant.pid));",
        "  process.stdout.write('{}');",
        "}",
        "else if (mode === 'stdout') { process.stdout.write('S'.repeat(4096)); setInterval(() => {}, 1000); }",
        "else if (mode === 'stderr') { process.stderr.write('E'.repeat(4096)); setInterval(() => {}, 1000); }",
        "else {",
        "  const bytes = fs.readFileSync(3);",
        "  process.stdout.write(JSON.stringify({",
        "    argv: process.argv.slice(2),",
        "    bytes: bytes.length,",
        "    sha256: crypto.createHash('sha256').update(bytes).digest('hex'),",
        "    restrictedPolicy: process.env.MINUTES_CLI_RESTRICTED_POLICY,",
        "    outerProcessGroup: process.env.MINUTES_MCP_OUTER_PROCESS_GROUP",
        "  }));",
        "}",
      ].join("\n"),
      { mode: 0o700 }
    );
    chmodSync(childPath, 0o700);
    return childPath;
  }

  it(
    "retains one exact source fd at offset zero without named staging or registry state",
    async () => {
    const fixture = processAudioFixture("synthetic-offset-proof");
    let retainedFd = -1;
    try {
      const beforeInbox = readdirSync(fixture.inbox);
      const result = await withAuthorizedMcpProcessAudioInput(
        fixture.source,
        [fixture.inbox],
        [".wav"],
        async () => fixture.meetings,
        fixture.source,
        async (authorized) => {
          retainedFd = authorized.fd;
          const invalidInputs: AuthorizedMcpProcessAudioInput[] = [
            { ...authorized, fd: 1.5 },
            { ...authorized, digest: { byteLength: -1 } },
            {
              ...authorized,
              digest: {
                ...authorized.digest,
                byteLength: authorized.digest.byteLength + 1,
              },
            },
            { ...authorized, format: "wav/path" },
            { ...authorized, format: "m4a" },
            { ...authorized, safeTitle: "synthetic/path" },
          ];
          for (const invalid of invalidInputs) {
            expect(() => buildMcpProcessAudioArgs(invalid, "memo")).toThrow(
              /capability is invalid/i
            );
          }
          expect(() =>
            buildMcpProcessAudioArgs(
              authorized,
              "other" as "memo",
              "../private"
            )
          ).toThrow(/arguments are invalid/i);
          const first = Buffer.alloc(1);
          // A non-positional read here proves authorization metadata checks
          // left the shared file description at offset zero.
          expect(readSync(authorized.fd, first, 0, 1, null)).toBe(1);
          const args = buildMcpProcessAudioArgs(authorized, "memo", "en");
          return { first: first.toString("utf8"), authorized, args };
        }
      );

      expect(result.first).toBe(fixture.content[0]);
      expect(result.authorized.digest).toEqual({
        byteLength: Buffer.byteLength(fixture.content),
      });
      expect(result.authorized.format).toBe("wav");
      expect(result.authorized.safeTitle).toBe("synthetic-input");
      expect(result.args[1]).toBe("authorized-input.wav");
      expect(result.args).toContain("--authorized-input-fd");
      expect(result.args[result.args.indexOf("--authorized-input-fd") + 1]).toBe(
        "3"
      );
      expect(result.args.join(" ")).not.toContain(fixture.source);
      expect(readdirSync(fixture.inbox)).toEqual(beforeInbox);
      expect(() => fstatSync(retainedFd)).toThrow();

      let leakFd = -1;
      const leakFailure = await withAuthorizedMcpProcessAudioInput(
        fixture.source,
        [fixture.inbox],
        [".wav"],
        async () => fixture.meetings,
        undefined,
        async (authorized) => {
          leakFd = authorized.fd;
          return { stdout: fixture.source, stderr: "" };
        }
      ).then(
        () => "unexpected-success",
        (error) => String(error)
      );
      expect(leakFailure).toMatch(/result exposed its source/i);
      expect(leakFailure).not.toContain(fixture.source);
      expect(() => fstatSync(leakFd)).toThrow();

      const implementation = readFileSync(
        new URL("./index.ts", import.meta.url),
        "utf8"
      );
      expect(implementation).not.toMatch(
        /\.minutes-mcp-process-inputs|mcp-process-audio-reservations-v1|stageMcpProcessAudioInput/
      );
    } finally {
      rmSync(fixture.root, { recursive: true, force: true });
    }
  });

  it(
    "rejects compressed agent audio before the operation receives a capability",
    async () => {
    const fixture = processAudioFixture("synthetic-compressed-container");
    const compressed = join(fixture.inbox, "synthetic-input.m4a");
    renameSync(fixture.source, compressed);
    let operations = 0;
    try {
      await expect(
        withAuthorizedMcpProcessAudioInput(
          compressed,
          [fixture.inbox],
          [".m4a"],
          async () => fixture.meetings,
          undefined,
          async () => {
            operations += 1;
            return { unexpected: true };
          }
        )
      ).rejects.toThrow(/bounded WAV input only/i);
      expect(operations).toBe(0);
    } finally {
      rmSync(fixture.root, { recursive: true, force: true });
    }
  });

  it(
    "inherits only the authorized input as fd 3 with synthetic argv, exact proof, and deny-last env",
    async () => {
    const fixture = processAudioFixture("synthetic-child-proof");
    const childPath = writeProcessAudioFdChild(fixture.root);
    let retainedFd = -1;
    try {
      const result = await withAuthorizedMcpProcessAudioInput(
        fixture.source,
        [fixture.inbox],
        [".wav"],
        async () => fixture.meetings,
        "Synthetic review",
        async (authorized) => {
          retainedFd = authorized.fd;
          return runProcessAudioFixtureCli(authorized, "meeting", "en", {
            binary: childPath,
            extraEnv: {
              MINUTES_CLI_RESTRICTED_POLICY: "allow",
              MINUTES_MCP_OUTER_PROCESS_GROUP: "0",
              MINUTES_FD_CHILD_MODE: "success",
            },
          });
        }
      );
      const payload = JSON.parse(result.stdout);
      const args = payload.argv as string[];
      expect(args.slice(0, 2)).toEqual(["process", "authorized-input.wav"]);
      expect(args[args.indexOf("--authorized-input-fd") + 1]).toBe("3");
      expect(args[args.indexOf("--authorized-input-format") + 1]).toBe("wav");
      expect(args[args.indexOf("--authorized-input-bytes") + 1]).toBe(
        String(Buffer.byteLength(fixture.content))
      );
      expect(payload.sha256).toBe(
        createHash("sha256").update(fixture.content).digest("hex")
      );
      expect(payload.bytes).toBe(Buffer.byteLength(fixture.content));
      expect(payload.restrictedPolicy).toBe("deny");
      expect(payload.outerProcessGroup).toBeUndefined();
      expect(JSON.stringify(payload)).not.toContain(fixture.source);
      expect(result.stderr).toBe("");
      expect(() => fstatSync(retainedFd)).toThrow();
    } finally {
      rmSync(fixture.root, { recursive: true, force: true });
    }
  });

  it("isolates authorization and inherits only exact fd 3 with path-free argv", async () => {
    if (process.platform !== "linux" && process.platform !== "darwin") return;
    const fixture = processAudioFixture("synthetic-isolated-helper");
    const childPath = writeProcessAudioFdChild(fixture.root);
    try {
      const result = await runIsolatedMcpProcessAudio(
        fixture.source,
        [fixture.inbox],
        [".wav"],
        async () => fixture.meetings,
        "Synthetic isolated review",
        "meeting",
        "en",
        {
          binary: childPath,
          extraEnv: {
            MINUTES_CLI_RESTRICTED_POLICY: "allow",
            MINUTES_MCP_OUTER_PROCESS_GROUP: "1",
            MINUTES_FD_CHILD_MODE: "success",
          },
        }
      );
      const payload = JSON.parse(result.stdout);
      expect(payload.argv.slice(0, 2)).toEqual([
        "process",
        "authorized-input.wav",
      ]);
      expect(payload.argv.join(" ")).not.toContain(fixture.source);
      expect(payload.bytes).toBe(Buffer.byteLength(fixture.content));
      expect(payload.sha256).toBe(
        createHash("sha256").update(fixture.content).digest("hex")
      );
      expect(payload.restrictedPolicy).toBe("deny");
      expect(Number(payload.outerProcessGroup)).toBeGreaterThan(1);
      expect(JSON.stringify(result)).not.toContain(fixture.source);
    } finally {
      rmSync(fixture.root, { recursive: true, force: true });
    }
  });

  it(
    "routes the production process_audio tool only through the isolated helper",
    () => {
    const implementation = readFileSync(
      new URL("./index.ts", import.meta.url),
      "utf8"
    );
    const registrationStart = implementation.indexOf(
      'registerTool(\n  "process_audio"'
    );
    const registrationEnd = implementation.indexOf(
      "// ── Tool: add_note",
      registrationStart
    );
    expect(registrationStart).toBeGreaterThan(0);
    expect(registrationEnd).toBeGreaterThan(registrationStart);
    const registration = implementation.slice(
      registrationStart,
      registrationEnd
    );
    expect(registration).toContain("runIsolatedMcpProcessAudio(");
    expect(registration).not.toContain("runProcessAudioFixtureCli(");
    expect(implementation).not.toContain("runMcpProcessAudioCli");
  });

  it("isolated authorization rejects a post-open source replacement", async () => {
    if (process.platform !== "linux" && process.platform !== "darwin") return;
    const fixture = processAudioFixture("synthetic-isolated-race");
    const childPath = writeProcessAudioFdChild(fixture.root);
    try {
      let replacementRan = false;
      const failure = await runIsolatedMcpProcessAudio(
        fixture.source,
        [fixture.inbox],
        [".wav"],
        async () => fixture.meetings,
        undefined,
        "memo",
        undefined,
        { binary: childPath },
        {
          afterValidation: () => {
            replacementRan = true;
            renameSync(fixture.source, join(fixture.inbox, "displaced.wav"));
            writeFileSync(fixture.source, "synthetic-replacement");
          },
        }
      ).then(
        () => "unexpected-success",
        (error) => String(error)
      );
      expect(replacementRan).toBe(true);
      expect(failure).toMatch(/failed safely/i);
      expect(failure).not.toContain(fixture.source);
    } finally {
      rmSync(fixture.root, { recursive: true, force: true });
    }
  });

  it("retires same-group descendants after a normal isolated helper close", async () => {
    if (process.platform !== "linux" && process.platform !== "darwin") return;
    const fixture = processAudioFixture("synthetic-isolated-normal-close");
    const childPath = writeProcessAudioFdChild(fixture.root);
    const descendantPidFile = join(fixture.root, "normal-close-descendant.pid");
    try {
      const result = await runIsolatedMcpProcessAudio(
        fixture.source,
        [fixture.inbox],
        [".wav"],
        async () => fixture.meetings,
        undefined,
        "memo",
        undefined,
        {
          binary: childPath,
          timeoutMs: 2_000,
          extraEnv: {
            MINUTES_FD_CHILD_MODE: "success-descendant",
            MINUTES_DESCENDANT_PID_FILE: descendantPidFile,
          },
        }
      );
      expect(result.stdout).toBe("{}");
      expect(existsSync(descendantPidFile)).toBe(true);
      const descendantPid = Number.parseInt(
        readFileSync(descendantPidFile, "utf8"),
        10
      );
      let alive = true;
      for (let attempt = 0; attempt < 30 && alive; attempt += 1) {
        try {
          process.kill(descendantPid, 0);
          await new Promise((resolve) => setTimeout(resolve, 10));
        } catch {
          alive = false;
        }
      }
      expect(alive).toBe(false);
    } finally {
      rmSync(fixture.root, { recursive: true, force: true });
    }
  });

  it("fails source replacement, hard-linking, and mutation closed and closes every retained fd", async () => {
    for (const race of ["replace", "hardlink", "mutate"] as const) {
      const fixture = processAudioFixture("synthetic-race-proof");
      let retainedFd = -1;
      let operations = 0;
      try {
        const failure = await withAuthorizedMcpProcessAudioInput(
          fixture.source,
          [fixture.inbox],
          [".wav"],
          async () => fixture.meetings,
          undefined,
          async () => {
            operations += 1;
            return "must-not-run";
          },
          {
            onRetainedFd: (fd) => {
              retainedFd = fd;
            },
            afterHash: () => {
              if (race === "replace") {
                renameSync(fixture.source, join(fixture.inbox, "displaced.wav"));
                writeFileSync(fixture.source, "synthetic-replacement");
              } else if (race === "hardlink") {
                linkSync(fixture.source, join(fixture.inbox, "alias.wav"));
              } else {
                appendFileSync(fixture.source, "-changed");
              }
            },
          }
        ).then(
          () => "unexpected-success",
          (error) => String(error)
        );
        expect(failure).toMatch(/access denied/i);
        expect(failure).not.toContain(fixture.source);
        expect(operations).toBe(0);
        expect(() => fstatSync(retainedFd)).toThrow();
      } finally {
        rmSync(fixture.root, { recursive: true, force: true });
      }
    }
  });

  it(
    "bounds each input and aggregate retained capability admission, then recovers",
    async () => {
    const fixture = processAudioFixture("12345678");
    const second = join(fixture.inbox, "second.wav");
    writeFileSync(second, "abcdefgh");
    let releaseHeld!: () => void;
    let announceHeld!: () => void;
    const held = new Promise<void>((resolve) => (releaseHeld = resolve));
    const announced = new Promise<void>((resolve) => (announceHeld = resolve));
    const hooks = { maxBytes: 8, maxAggregateBytes: 8 };
    try {
      const active = withAuthorizedMcpProcessAudioInput(
        fixture.source,
        [fixture.inbox],
        [".wav"],
        async () => fixture.meetings,
        undefined,
        async () => {
          announceHeld();
          await held;
          return "done";
        },
        hooks
      );
      await announced;
      await expect(
        withAuthorizedMcpProcessAudioInput(
          second,
          [fixture.inbox],
          [".wav"],
          async () => fixture.meetings,
          undefined,
          async () => "must-not-run",
          hooks
        )
      ).rejects.toThrow(/resource budget/i);
      releaseHeld();
      await expect(active).resolves.toBe("done");

      await expect(
        withAuthorizedMcpProcessAudioInput(
          second,
          [fixture.inbox],
          [".wav"],
          async () => fixture.meetings,
          undefined,
          async () => "recovered",
          hooks
        )
      ).resolves.toBe("recovered");
      appendFileSync(second, "x");
      await expect(
        withAuthorizedMcpProcessAudioInput(
          second,
          [fixture.inbox],
          [".wav"],
          async () => fixture.meetings,
          undefined,
          async () => "must-not-run",
          hooks
        )
      ).rejects.toThrow(/resource budget/i);
    } finally {
      releaseHeld?.();
      rmSync(fixture.root, { recursive: true, force: true });
    }
  });

  it(
    "re-attests the live meeting root and its identity immediately before dispatch",
    async () => {
    const fixture = processAudioFixture();
    const alternate = join(fixture.root, "alternate-meetings");
    mkdirSync(alternate);
    try {
      let calls = 0;
      let operations = 0;
      await expect(
        withAuthorizedMcpProcessAudioInput(
          fixture.source,
          [fixture.inbox],
          [".wav"],
          async () => (++calls === 1 ? fixture.meetings : alternate),
          undefined,
          async () => {
            operations += 1;
          }
        )
      ).rejects.toThrow(/meeting root changed/i);
      expect(operations).toBe(0);

      calls = 0;
      const displaced = join(fixture.root, "displaced-meetings");
      await expect(
        withAuthorizedMcpProcessAudioInput(
          fixture.source,
          [fixture.inbox],
          [".wav"],
          async () => {
            calls += 1;
            return fixture.meetings;
          },
          undefined,
          async () => {
            operations += 1;
          },
          {
            beforeFinalAttestation: () => {
              renameSync(fixture.meetings, displaced);
              mkdirSync(fixture.meetings);
            },
          }
        )
      ).rejects.toThrow(/meeting root changed/i);
      expect(calls).toBe(2);
      expect(operations).toBe(0);
    } finally {
      rmSync(fixture.root, { recursive: true, force: true });
    }
  });

  it(
    "kills timeout and oversized-output children with path-free errors and closes the parent fd",
    async () => {
    const fixture = processAudioFixture("synthetic-bounded-child");
    const childPath = writeProcessAudioFdChild(fixture.root);
    try {
      for (const mode of ["timeout", "stdout", "stderr"] as const) {
        let retainedFd = -1;
        const failure = await withAuthorizedMcpProcessAudioInput(
          fixture.source,
          [fixture.inbox],
          [".wav"],
          async () => fixture.meetings,
          undefined,
          async (authorized) => {
            retainedFd = authorized.fd;
            return runProcessAudioFixtureCli(authorized, "memo", undefined, {
              binary: childPath,
              timeoutMs: mode === "timeout" ? 50 : 2_000,
              maxStdoutBytes: 64,
              maxStderrBytes: 64,
              extraEnv: { MINUTES_FD_CHILD_MODE: mode },
            });
          }
        ).then(
          () => "unexpected-success",
          (error) => String(error)
        );
        expect(failure).toMatch(
          mode === "timeout" ? /time budget/i : /byte budget/i
        );
        expect(failure).not.toContain(fixture.source);
        expect(() => fstatSync(retainedFd)).toThrow();
      }

      let retainedFd = -1;
      const spawnFailure = await withAuthorizedMcpProcessAudioInput(
        fixture.source,
        [fixture.inbox],
        [".wav"],
        async () => fixture.meetings,
        undefined,
        async (authorized) => {
          retainedFd = authorized.fd;
          return runProcessAudioFixtureCli(authorized, "memo", undefined, {
            binary: join(fixture.root, "missing-binary"),
          });
        }
      ).then(
        () => "unexpected-success",
        (error) => String(error)
      );
      expect(spawnFailure).toMatch(/could not be started safely/i);
      expect(spawnFailure).not.toContain(fixture.source);
      expect(() => fstatSync(retainedFd)).toThrow();

      const descendantPidFile = join(fixture.root, "synthetic-descendant.pid");
      const descendantFailure = await withAuthorizedMcpProcessAudioInput(
        fixture.source,
        [fixture.inbox],
        [".wav"],
        async () => fixture.meetings,
        undefined,
        (authorized) =>
          runProcessAudioFixtureCli(authorized, "memo", undefined, {
            binary: childPath,
            timeoutMs: 200,
            extraEnv: {
              MINUTES_FD_CHILD_MODE: "descendant",
              MINUTES_DESCENDANT_PID_FILE: descendantPidFile,
            },
          })
      ).then(
        () => "unexpected-success",
        (error) => String(error)
      );
      expect(descendantFailure).toMatch(/time budget/i);
      expect(existsSync(descendantPidFile)).toBe(true);
      const descendantPid = Number.parseInt(
        readFileSync(descendantPidFile, "utf8"),
        10
      );
      let descendantAlive = true;
      for (let attempt = 0; attempt < 20 && descendantAlive; attempt += 1) {
        try {
          process.kill(descendantPid, 0);
          await new Promise((resolve) => setTimeout(resolve, 10));
        } catch {
          descendantAlive = false;
        }
      }
      expect(descendantAlive).toBe(false);
    } finally {
      rmSync(fixture.root, { recursive: true, force: true });
    }
  });

  it("settles without a helper close event, bounds the tree, and poisons further audio after a forced kill", async () => {
    if (process.platform !== "linux" && process.platform !== "darwin") return;
    const fixture = processAudioFixture("synthetic-isolated-timeout");
    const childPath = writeProcessAudioFdChild(fixture.root);
    const descendantPidFile = join(fixture.root, "isolated-descendant.pid");
    try {
      let eventLoopTicked = false;
      const tick = setTimeout(() => {
        eventLoopTicked = true;
      }, 10);
      const startedAt = Date.now();
      const failure = await runIsolatedMcpProcessAudio(
        fixture.source,
        [fixture.inbox],
        [".wav"],
        async () => fixture.meetings,
        undefined,
        "memo",
        undefined,
        {
          binary: childPath,
          timeoutMs: 100,
          extraEnv: {
            MINUTES_FD_CHILD_MODE: "descendant",
            MINUTES_DESCENDANT_PID_FILE: descendantPidFile,
          },
        },
        { ignoreHelperCloseForTest: true }
      ).then(
        () => "unexpected-success",
        (error) => String(error)
      );
      clearTimeout(tick);
      expect(eventLoopTicked).toBe(true);
      expect(Date.now() - startedAt).toBeLessThan(2_000);
      expect(failure).toMatch(/time budget/i);
      expect(failure).not.toContain(fixture.source);

      if (existsSync(descendantPidFile)) {
        const descendantPid = Number.parseInt(
          readFileSync(descendantPidFile, "utf8"),
          10
        );
        let alive = true;
        for (let attempt = 0; attempt < 30 && alive; attempt += 1) {
          try {
            process.kill(descendantPid, 0);
            await new Promise((resolve) => setTimeout(resolve, 10));
          } catch {
            alive = false;
          }
        }
        expect(alive).toBe(false);
      }

      await expect(
        runIsolatedMcpProcessAudio(
          fixture.source,
          [fixture.inbox],
          [".wav"],
          async () => fixture.meetings,
          undefined,
          "memo",
          undefined,
          { binary: childPath }
        )
      ).rejects.toThrow(/requires an MCP restart/i);
    } finally {
      rmSync(fixture.root, { recursive: true, force: true });
    }
  });
  it("holds a normal context source through the final lease fence", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-context-policy-"));
    const source = join(root, "normal.md");
    const content = [
      "---",
      "title: Normal context",
      "type: meeting",
      "date: 2026-07-15T10:00:00Z",
      "duration: 1m",
      "sensitivity: normal",
      "---",
      "",
      "CONTEXT_SOURCE_CANARY",
    ].join("\n");
    writeFileSync(source, content);
    try {
      const result = await withPolicyBoundContextPath(
        source,
        root,
        async (canonicalPath) => ({
          source_authorization: {
            session_id: "session-normal",
            // On Windows this is the exact JSON spelling emitted from Rust's
            // std::fs::canonicalize, checked against Node's realpath spelling.
            path: rustCanonicalPathWire(canonicalPath),
            sha256: createHash("sha256").update(content).digest("hex"),
          },
          value: "safe",
        }),
        (value, sessionId) => ({ value: value.value, sessionId })
      );
      expect(result).toEqual({ value: "safe", sessionId: "session-normal" });
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("omits every private context capability while retaining the exact authorized artifact", () => {
    const source = "/synthetic/meetings/authorized.md";
    const links = assistantSafeContextLinks(
      [
        { session_id: "synthetic-session", kind: "job", target: "job-safe" },
        {
          session_id: "synthetic-session",
          kind: "markdown-artifact",
          target: source,
        },
        {
          session_id: "synthetic-session",
          kind: "audio-capture",
          target: "/private/PRIVATE_AUDIO_CANARY.wav",
        },
        {
          session_id: "synthetic-session",
          kind: "screenshot-directory",
          target: "/private/PRIVATE_SCREEN_CANARY",
        },
        {
          session_id: "synthetic-session",
          kind: "markdown-artifact",
          target: "/synthetic/meetings/sibling.md",
        },
      ],
      source
    );
    expect(links).toHaveLength(1);
    const rendered = JSON.stringify({ links, view: "context" });
    expect(rendered).not.toContain("job-safe");
    expect(rendered).toContain(source);
    expect(rendered).not.toContain("PRIVATE_AUDIO_CANARY");
    expect(rendered).not.toContain("PRIVATE_SCREEN_CANARY");
    expect(rendered).not.toContain("sibling.md");
  });

  it("rejects a stale capture revision after same-path replacement", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-context-policy-revision-"));
    const source = join(root, "meeting.md");
    const restrictedAtLink = [
      "---",
      "title: Restricted at link time",
      "type: meeting",
      "date: 2026-07-15T10:00:00Z",
      "duration: 1m",
      "sensitivity: restricted",
      "---",
      "",
      "RESTRICTED_LINK_REVISION",
    ].join("\n");
    const normalReplacement = restrictedAtLink
      .replace("Restricted at link time", "Normal replacement")
      .replace("sensitivity: restricted", "sensitivity: normal")
      .replace("RESTRICTED_LINK_REVISION", "NORMAL_REPLACEMENT_REVISION");
    writeFileSync(source, normalReplacement);
    try {
      await expect(
        withPolicyBoundContextPath(
          source,
          root,
          async (canonicalPath) => ({
            source_authorization: {
              session_id: "session-stale-revision",
              path: canonicalPath,
              sha256: createHash("sha256").update(restrictedAtLink).digest("hex"),
            },
          }),
          async () => "must-not-return"
        )
      ).rejects.toThrow(/stable meeting corpus authorization failed|authorization no longer matches/i);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("rejects a normal-to-restricted context transition at the final fence", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-context-policy-race-"));
    const source = join(root, "meeting.md");
    const normal = [
      "---",
      "title: Context race",
      "type: meeting",
      "date: 2026-07-15T10:00:00Z",
      "duration: 1m",
      "sensitivity: normal",
      "---",
      "",
      "NORMAL_CONTEXT_CANARY",
    ].join("\n");
    const restricted = normal
      .replace("sensitivity: normal", "sensitivity: restricted")
      .replace("NORMAL_CONTEXT_CANARY", "RESTRICTED_CONTEXT_CANARY");
    writeFileSync(source, normal);
    try {
      await expect(
        withPolicyBoundContextPath(
          source,
          root,
          async (canonicalPath) => ({
            source_authorization: {
              session_id: "session-race",
              path: canonicalPath,
              sha256: createHash("sha256").update(normal).digest("hex"),
            },
          }),
          async () => "must-not-return",
          {
            beforeFinalManifest: () => {
              writeFileSync(source, restricted);
            },
          }
        )
      ).rejects.toThrow(/access denied/i);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
  }
);

describe("derived-record tool availability", () => {
  it("advertises and invokes both names as path-free machine-readable errors", async () => {
    const mcpServer = new McpServer({
      name: "minutes-unavailable-compatibility-test",
      version: "0.0.0",
    });
    registerUnavailableCompatibilityTools(mcpServer);
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client(
      { name: "unavailable-compatibility-client", version: "0.0.0" },
      { capabilities: {} }
    );
    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      const listed = await client.listTools();
      const descriptions = new Map(
        listed.tools.map((tool) => [tool.name, tool.description || ""])
      );
      // Annotations stay unavailable: their source pointer and body are both
      // author-supplied, so revalidating the pointer cannot bound the body.
      expect(descriptions.get("get_agent_annotations")).toContain(
        MCP_AGENT_ANNOTATIONS_UNAVAILABLE_DESCRIPTION
      );
      // Insights are available: their source is written by the pipeline.
      expect(descriptions.get("get_meeting_insights")).toContain(
        MCP_MEETING_INSIGHTS_DESCRIPTION
      );
      expect(descriptions.get("get_meeting_insights")).not.toMatch(
        /compatibility name only/i
      );

      const pathCanary = "/synthetic/PRIVATE-ANNOTATION-PATH-CANARY.md";
      const participantCanary = "PRIVATE-PARTICIPANT-CANARY";
      const annotations = await client.callTool({
        name: "get_agent_annotations",
        arguments: {
          limit: 7,
          agent_id: "PRIVATE-AGENT-CANARY",
          meeting_id: "PRIVATE-MEETING-CANARY",
          meeting_path: pathCanary,
        },
      });
      const insights = await client.callTool({
        name: "get_meeting_insights",
        arguments: {
          kind: "decision",
          participant: participantCanary,
          since: "2026-01-01",
          limit: 9,
        },
      });

      // Whole-shape assertion, not merely substring absence: the unavailable
      // result must expose exactly these keys and nothing that could carry a
      // record or a caller argument.
      expect(annotations.isError).toBe(true);
      expect(Object.keys(annotations.structuredContent || {}).sort()).toEqual([
        "available",
        "error",
      ]);
      expect(annotations.structuredContent).toMatchObject({
        available: false,
        error: { code: "source-policy-provenance-required" },
      });
      const annotationsSerialized = JSON.stringify(annotations);
      expect(annotationsSerialized).not.toMatch(/"annotations"|"count"|"requested"/);
      expect(annotationsSerialized).not.toContain(pathCanary);
      expect(annotationsSerialized).not.toContain("PRIVATE-AGENT-CANARY");
      expect(annotationsSerialized).not.toContain("PRIVATE-MEETING-CANARY");
      expect(annotationsSerialized).toMatch(/unavailable/i);

      // The available tool must never echo caller filters back either.
      const insightsSerialized = JSON.stringify(insights);
      expect(insightsSerialized).not.toContain(participantCanary);
    } finally {
      await client.close();
      await mcpServer.close();
    }
  });
});

describe("insight filtering applied after policy", () => {
  it("orders the confidence floor and rejects unknown observed values", () => {
    expect(meetsInsightConfidence("explicit", "strong")).toBe(true);
    expect(meetsInsightConfidence("strong", "strong")).toBe(true);
    expect(meetsInsightConfidence("inferred", "strong")).toBe(false);
    expect(meetsInsightConfidence("tentative", "inferred")).toBe(false);
    // No floor requested means everything qualifies.
    expect(meetsInsightConfidence("tentative", undefined)).toBe(true);
    // An unrecognised observed value must not sneak past a real floor.
    expect(meetsInsightConfidence("bogus", "strong")).toBe(false);
  });

  it("matches a participant across participants and owner, case-insensitively", () => {
    const insight = { participants: ["Alex Kim", "Dana"], owner: "Priya Raman" };
    expect(insightMentionsParticipant(insight, "alex")).toBe(true);
    expect(insightMentionsParticipant(insight, "RAMAN")).toBe(true);
    expect(insightMentionsParticipant(insight, "dana")).toBe(true);
    expect(insightMentionsParticipant(insight, "nobody")).toBe(false);
    // Missing fields must not throw.
    expect(insightMentionsParticipant({}, "alex")).toBe(false);
  });

  it("parses the since floor as local midnight and refuses malformed dates", () => {
    const floor = parseInsightSinceFloor("2026-07-17");
    expect(floor).toBe(new Date(2026, 6, 17, 0, 0, 0, 0).getTime());
    expect(parseInsightSinceFloor(undefined)).toBeNull();
    expect(parseInsightSinceFloor("")).toBeNull();
    expect(() => parseInsightSinceFloor("2026-7-17")).toThrow(/YYYY-MM-DD/);
    expect(() => parseInsightSinceFloor("2026-02-30")).toThrow(/YYYY-MM-DD/);
    expect(() => parseInsightSinceFloor("0026-07-17")).toThrow(/YYYY-MM-DD/);
    expect(() => parseInsightSinceFloor(20260717 as never)).toThrow(/YYYY-MM-DD/);

    // A record this process cannot date is excluded whenever a floor is asked
    // for, and included when none is.
    expect(insightIsSince({ timestamp: "2026-07-17T00:00:00Z" }, null)).toBe(true);
    expect(insightIsSince({}, null)).toBe(true);
    expect(insightIsSince({}, floor)).toBe(false);
    expect(insightIsSince({ timestamp: "not a date" }, floor)).toBe(false);
    expect(insightIsSince({ timestamp: "2026-07-16T10:00:00Z" }, floor)).toBe(false);
    expect(insightIsSince({ timestamp: "2026-07-18T10:00:00Z" }, floor)).toBe(true);
    // The floor is inclusive, which is what the argument description promises
    // ("on or after this calendar date"). A record sitting exactly on local
    // midnight is the only case that distinguishes >= from >, and nothing else
    // in this suite has one.
    expect(insightIsSince({ timestamp: new Date(floor!).toISOString() }, floor)).toBe(true);
    expect(insightIsSince({ timestamp: new Date(floor! - 1).toISOString() }, floor)).toBe(false);
  });
});

describe("derived record source revalidation", () => {
  function meetingMarkdown(title: string, sensitivity?: string): string {
    return [
      "---",
      `title: ${title}`,
      "type: meeting",
      "date: 2026-07-15T10:00:00Z",
      ...(sensitivity ? [`sensitivity: ${sensitivity}`] : []),
      "---",
      "",
      "Body.",
    ].join("\n");
  }

  it("withholds a record that carries no source meeting to revalidate", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-annotation-source-"));
    try {
      for (const absent of [undefined, null, "", "   ", 42]) {
        const verdict = await revalidateDerivedRecordSource(absent, root, false);
        expect(verdict).toEqual({
          allowed: false,
          reason: "no-source-provenance",
        });
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("releases a normal source and withholds a restricted one unless overridden", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-annotation-source-"));
    try {
      const normal = join(root, "normal.md");
      const restricted = join(root, "restricted.md");
      writeFileSync(normal, meetingMarkdown("Normal review"));
      writeFileSync(restricted, meetingMarkdown("Restricted review", "restricted"));

      expect(await revalidateDerivedRecordSource(normal, root, false)).toEqual({
        allowed: true,
      });
      expect(await revalidateDerivedRecordSource(restricted, root, false)).toEqual({
        allowed: false,
        reason: "source-policy-denied",
      });
      // The audited override reaches the same record.
      expect(await revalidateDerivedRecordSource(restricted, root, true)).toEqual({
        allowed: true,
      });
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("re-reads policy live, so designating a source restricted withholds it on the next read", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-annotation-source-"));
    try {
      const path = join(root, "meeting.md");
      writeFileSync(path, meetingMarkdown("Review"));
      expect(await revalidateDerivedRecordSource(path, root, false)).toEqual({
        allowed: true,
      });

      // Provenance captured at write time must not be trusted: the source is
      // re-read from disk on every call.
      writeFileSync(path, meetingMarkdown("Review", "restricted"));
      expect(await revalidateDerivedRecordSource(path, root, false)).toEqual({
        allowed: false,
        reason: "source-policy-denied",
      });
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("revalidates insights through the meeting they name as their source", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-insight-source-"));
    try {
      const normal = join(root, "normal.md");
      const restricted = join(root, "restricted.md");
      writeFileSync(normal, meetingMarkdown("Normal review"));
      writeFileSync(restricted, meetingMarkdown("Restricted review", "restricted"));

      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [
          { kind: "decision", content: "kept", source_meeting: normal },
          { kind: "decision", content: "RESTRICTED-INSIGHT-CANARY", source_meeting: restricted },
          { kind: "question", content: "orphan" },
        ],
        root,
        false
      );

      expect(released.map((entry: any) => entry.content)).toEqual(["kept"]);
      expect(withheld).toEqual({
        total: 2,
        noSourceProvenance: 1,
        sourcePolicyDenied: 1,
      });
      expect(JSON.stringify(released)).not.toContain("RESTRICTED-INSIGHT-CANARY");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

});

/**
 * A corpus whose own final segment is `meetings`, which is what the
 * relative-path normaliser anchors on.
 */
function makeMovedCorpus(): { base: string; root: string } {
  const base = mkdtempSync(join(tmpdir(), "minutes-moved-corpus-"));
  const root = join(base, "meetings");
  mkdirSync(root, { recursive: true });
  return { base, root };
}

function meetingFixture(title: string, sensitivity?: string): string {
  return [
    "---",
    `title: ${title}`,
    "type: meeting",
    "date: 2026-07-15T10:00:00Z",
    ...(sensitivity ? [`sensitivity: ${sensitivity}`] : []),
    "---",
    "",
    "Body.",
  ].join("\n");
}

describe("insight source identity survives a moved corpus", () => {
  it("releases a record whose recorded source names a corpus root that no longer exists here", async () => {
    // The pipeline records an absolute path. Every historical record in a
    // corpus that has since moved machines names a root that is not this one,
    // which is why the exact-path check released nothing at all.
    const { base, root } = makeMovedCorpus();
    try {
      writeFileSync(join(root, "2026-07-15-review.md"), meetingFixture("Review"));

      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [
          {
            kind: "decision",
            content: "kept",
            source_meeting: "/Users/someone-else/meetings/2026-07-15-review.md",
          },
        ],
        root,
        false
      );

      expect(released.map((entry: any) => entry.content)).toEqual(["kept"]);
      expect(withheld.total).toBe(0);
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("resolves a corpus-relative source, including one in a subdirectory", async () => {
    const { base, root } = makeMovedCorpus();
    try {
      mkdirSync(join(root, "memos"), { recursive: true });
      writeFileSync(join(root, "2026-07-15-review.md"), meetingFixture("Review"));
      writeFileSync(join(root, "memos", "2026-07-15-memo.md"), meetingFixture("Memo"));

      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [
          { content: "flat", source_meeting: "2026-07-15-review.md" },
          { content: "nested", source_meeting: "memos/2026-07-15-memo.md" },
        ],
        root,
        false
      );

      expect(released.map((entry: any) => entry.content)).toEqual(["flat", "nested"]);
      expect(withheld.total).toBe(0);
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("refuses to bind a foreign path that names no corpus root to a live meeting of the same filename", async () => {
    // This is the misattribution the normaliser must not commit. Records from
    // a temp directory — the shape a test run leaves in the event log — name
    // no corpus root, so they must resolve to nothing rather than adopt the
    // identity, and therefore the policy, of whatever live meeting happens to
    // share their filename.
    const { base, root } = makeMovedCorpus();
    try {
      writeFileSync(
        join(root, "2026-04-07-test-meeting.md"),
        meetingFixture("Unrelated live meeting")
      );

      // Deliberately carries no dot-directory and no inactive directory, so the
      // root-segment anchor is the only rule that can refuse it. A fixture with
      // a `.tmpXXXX` component would be rejected by the hidden-segment guard
      // even if the anchoring were removed, and would not test anchoring at
      // all.
      const foreign =
        "/var/folders/27/jxpp0/T/tmpfbyC6x/output/2026-04-07-test-meeting.md";
      expect(resolveCorpusRelativeSourcePath(foreign, root)).toBeNull();

      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [{ content: "MISATTRIBUTION-CANARY", source_meeting: foreign }],
        root,
        false
      );

      expect(released).toEqual([]);
      expect(withheld.total).toBe(1);
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("still applies live sensitivity policy through the normalised path", async () => {
    // Normalising identity must not become a way around the policy check: the
    // resolved path is re-read and re-classified exactly as an exact path is.
    const { base, root } = makeMovedCorpus();
    try {
      writeFileSync(
        join(root, "2026-07-15-restricted.md"),
        meetingFixture("Restricted review", "restricted")
      );
      const foreignRestricted =
        "/Users/someone-else/meetings/2026-07-15-restricted.md";

      const denied = await releaseInsightsWithLiveSourcePolicy(
        [{ content: "RESTRICTED-VIA-MOVED-CORPUS-CANARY", source_meeting: foreignRestricted }],
        root,
        false
      );
      expect(denied.released).toEqual([]);
      expect(denied.withheld.total).toBe(1);
      expect(JSON.stringify(denied.released)).not.toContain(
        "RESTRICTED-VIA-MOVED-CORPUS-CANARY"
      );

      // The audited override reaches the same record through the same path.
      const overridden = await releaseInsightsWithLiveSourcePolicy(
        [{ content: "RESTRICTED-VIA-MOVED-CORPUS-CANARY", source_meeting: foreignRestricted }],
        root,
        true
      );
      expect(overridden.released).toHaveLength(1);
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("does not rebind a recorded path that still exists on this machine", async () => {
    // The restored-corpus leak. A duplicate corpus has the same relative tails
    // as the live one but may carry different sensitivity frontmatter, so
    // normalising a path that still exists would evaluate the wrong meeting's
    // policy and release a restricted meeting's insight under an unrestricted
    // namesake.
    const base = mkdtempSync(join(tmpdir(), "minutes-two-corpora-"));
    try {
      const live = join(base, "backup", "meetings");
      const primary = join(base, "primary", "meetings");
      mkdirSync(live, { recursive: true });
      mkdirSync(primary, { recursive: true });
      // Same relative tail in both corpora, divergent policy.
      writeFileSync(join(live, "2026-06-01-diligence.md"), meetingFixture("Diligence snapshot"));
      writeFileSync(
        join(primary, "2026-06-01-diligence.md"),
        meetingFixture("Diligence", "restricted")
      );

      const recorded = join(primary, "2026-06-01-diligence.md");
      expect(resolveCorpusRelativeSourcePath(recorded, live)).toBeNull();

      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [{ content: "CROSS-CORPUS-CANARY", source_meeting: recorded }],
        live,
        false
      );
      expect(released).toEqual([]);
      expect(withheld.total).toBe(1);
      expect(JSON.stringify(released)).not.toContain("CROSS-CORPUS-CANARY");
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("refuses a recorded path that sat outside the active corpus it came from", async () => {
    // What decides this is the record's own corpus-relative tail, not where its
    // corpus lived. A tail naming an inactive or hidden directory, or one that
    // traverses out, must not re-enter the live active corpus. Variants that
    // re-enter through a SECOND anchor are refused as ambiguous instead, which
    // the dedicated anchor test covers.
    const { base, root } = makeMovedCorpus();
    try {
      writeFileSync(join(root, "2026-06-01-board.md"), meetingFixture("Board"));
      for (const recorded of [
        "/elsewhere/meetings/archive/2026-06-01-board.md",
        "/elsewhere/meetings/processed/2026-06-01-board.md",
        "/elsewhere/meetings/failed/2026-06-01-board.md",
        "/elsewhere/meetings/failed-captures/2026-06-01-board.md",
        "/elsewhere/meetings/.trash/2026-06-01-board.md",
        "/elsewhere/meetings/../outside.md",
      ]) {
        expect(resolveCorpusRelativeSourcePath(recorded, root)).toBeNull();
      }

      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [
          {
            content: "ARCHIVED-REENTRY-CANARY",
            source_meeting: "/elsewhere/meetings/archive/2026-06-01-board.md",
          },
        ],
        root,
        false
      );
      expect(released).toEqual([]);
      expect(withheld.total).toBe(1);
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("carries the whole tail over, not just the filename", async () => {
    // The absolute branch is the one that synthesises a tail, and nothing else
    // pins that the tail survives intact. Binding `/elsewhere/meetings/memos/x.md`
    // to `<root>/x.md` would evaluate a different meeting's policy. The nested
    // case elsewhere in this file exercises the relative branch, which is a
    // different path through the resolver.
    const { base, root } = makeMovedCorpus();
    try {
      mkdirSync(join(root, "memos"), { recursive: true });
      writeFileSync(join(root, "memos", "2026-07-15-memo.md"), meetingFixture("Memo"));
      writeFileSync(join(root, "2026-07-15-memo.md"), meetingFixture("Namesake at the root"));

      expect(
        resolveCorpusRelativeSourcePath(
          "/home/someone/meetings/memos/2026-07-15-memo.md",
          root
        )
      ).toBe(join(realpathSync(root), "memos", "2026-07-15-memo.md"));

      // Released records must name the nested meeting, not the root namesake.
      const { released } = await releaseInsightsWithLiveSourcePolicy(
        [
          {
            content: "nested",
            source_meeting: "/home/someone/meetings/memos/2026-07-15-memo.md",
          },
        ],
        root,
        false
      );
      expect(released).toHaveLength(1);
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("counts an unresolvable source in the same bucket as a refused one", async () => {
    // The middle refusal path. A source that cannot be resolved into this
    // corpus at all must land in the SAME published bucket as one that resolved
    // and was refused by policy, because separating them would publish the
    // number of restricted meetings in the window as a clean count. The bucket
    // is what an agent sees; the reason value never reaches it.
    const { base, root } = makeMovedCorpus();
    try {
      writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
      writeFileSync(join(root, "restricted.md"), meetingFixture("Restricted", "restricted"));

      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [
          { content: "kept", source_meeting: "normal.md" },
          // Resolves, then refused by policy.
          { content: "refused", source_meeting: "restricted.md" },
          // Cannot be resolved into this corpus at all: no anchor segment.
          { content: "unresolvable", source_meeting: "/var/folders/x/T/tmp/out/notes.md" },
          // No pointer at all: the one bucket that is safe to report apart.
          { content: "orphan" },
        ],
        root,
        false
      );

      expect(released.map((r: any) => r.content)).toEqual(["kept"]);
      expect(withheld).toEqual({
        total: 3,
        noSourceProvenance: 1,
        sourcePolicyDenied: 2,
      });
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("refuses a dot-dot component instead of letting join cancel it lexically", async () => {
    // `join` cancels `..` lexically; the kernel does not. Measured on Linux:
    // `missing/../board.md` is ENOENT and `afile/../board.md` is ENOTDIR, so
    // neither names any file, while `alink/../board.md` with `alink` a symlink
    // out of the corpus opens a file OUTSIDE it. Lexical cancellation turns all
    // three into `<root>/board.md`, which binds a record to a meeting its
    // recorded path never named and, in the symlink case, launders an
    // out-of-corpus file past the active-corpus check.
    const base = mkdtempSync(join(tmpdir(), "minutes-dotdot-"));
    try {
      const root = join(base, "meetings");
      mkdirSync(root, { recursive: true });
      mkdirSync(join(base, "outside"), { recursive: true });
      writeFileSync(join(root, "board.md"), meetingFixture("Live board"));
      writeFileSync(join(base, "outside", "board.md"), meetingFixture("Outside board"));
      writeFileSync(join(root, "afile"), "not a directory");
      symlinkSync(join(base, "outside", "subdir"), join(root, "alink"));

      for (const tail of [
        "missing/../board.md",
        "afile/../board.md",
        "alink/../board.md",
        "./board.md",
      ]) {
        expect(resolveCorpusRelativeSourcePath(tail, root)).toBeNull();
        expect(
          resolveCorpusRelativeSourcePath(`/home/someone/meetings/${tail}`, root)
        ).toBeNull();
      }
      // The same filename without a cancelling component still resolves, so
      // this refuses the traversal rather than the meeting.
      expect(resolveCorpusRelativeSourcePath("board.md", root)).toBe(
        join(realpathSync(root), "board.md")
      );

      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [{ content: "DOTDOT-LAUNDER-CANARY", source_meeting: "alink/../board.md" }],
        root,
        false
      );
      expect(released).toEqual([]);
      expect(withheld.total).toBe(1);
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("refuses a source pointer whose surrounding whitespace is significant", async () => {
    // Trailing and leading spaces are legal in POSIX filenames, so trimming can
    // silently convert one real file's path into another real file's path.
    // Both files exist here, so a trim would bind the record to the wrong one.
    const { base, root } = makeMovedCorpus();
    try {
      writeFileSync(join(root, "notes.md "), meetingFixture("Trailing space file"));
      writeFileSync(join(root, "notes.md"), meetingFixture("Different meeting"));

      expect(resolveCorpusRelativeSourcePath(join(root, "notes.md "), root)).toBeNull();
      expect(resolveCorpusRelativeSourcePath(" notes.md", root)).toBeNull();
      // The unpadded form still resolves, so this refuses the ambiguity rather
      // than the filename.
      expect(resolveCorpusRelativeSourcePath("notes.md", root)).toBe(
        join(realpathSync(root), "notes.md")
      );

      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [{ content: "WHITESPACE-REBIND-CANARY", source_meeting: join(root, "notes.md ") }],
        root,
        false
      );
      expect(released).toEqual([]);
      expect(withheld.total).toBe(1);
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("refuses a source pointer containing a NUL byte", () => {
    const { base, root } = makeMovedCorpus();
    try {
      writeFileSync(join(root, "x.md"), meetingFixture("X"));
      expect(resolveCorpusRelativeSourcePath("x.md\0.png", root)).toBeNull();
      expect(
        resolveCorpusRelativeSourcePath("/home/someone/meetings/x.md\0.png", root)
      ).toBeNull();
      expect(
        resolveCorpusRelativeSourcePath("/home/someone/meetings/x.md\0.png", root)
      ).toBeNull();
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("recognises a corpus that used to live under a hidden or inactive ancestor", async () => {
    // The ancestors of the old corpus root say nothing about where a record sat
    // inside that corpus. Screening them refused ordinary moved corpora, which
    // is the case this normaliser exists to serve.
    const { base, root } = makeMovedCorpus();
    try {
      writeFileSync(join(root, "2026-06-01-board.md"), meetingFixture("Board"));
      for (const recorded of [
        "/home/someone/Archive/meetings/2026-06-01-board.md",
        "/home/someone/.local/share/meetings/2026-06-01-board.md",
        "/home/someone/processed/meetings/2026-06-01-board.md",
      ]) {
        expect(resolveCorpusRelativeSourcePath(recorded, root)).toBe(
          join(realpathSync(root), "2026-06-01-board.md")
        );
      }
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  describe.runIf(process.platform !== "win32")(
    "unreadable source parents",
    () => {
  it(
    "treats a recorded path it cannot stat as present rather than absent",
    async () => {
    // existsSync answers false for EVERY stat failure, so an unreadable parent
    // reads as "not there" and hands the path to the normaliser, reopening the
    // duplicate-corpus rebinding. Absence has to be proven, not inferred from a
    // failed syscall.
    const base = mkdtempSync(join(tmpdir(), "minutes-unreadable-corpus-"));
    const vaultParent = join(base, "vault");
    try {
      const live = join(base, "live", "meetings");
      const vault = join(vaultParent, "meetings");
      mkdirSync(live, { recursive: true });
      mkdirSync(vault, { recursive: true });
      writeFileSync(join(live, "2026-06-01-diligence.md"), meetingFixture("Namesake"));
      writeFileSync(
        join(vault, "2026-06-01-diligence.md"),
        meetingFixture("Diligence", "restricted")
      );
      const recorded = join(vault, "2026-06-01-diligence.md");

      // Readable: refused because the path exists.
      expect(resolveCorpusRelativeSourcePath(recorded, live)).toBeNull();

      // Unreadable parent: the stat now fails, but the answer must not change.
      chmodSync(vaultParent, 0o000);
      // Assert the precondition rather than assume it. Root traverses a 0o000
      // directory, so under root the stat would succeed and this test would
      // pass for the same reason as the readable case, silently losing the
      // EACCES path it exists to cover.
      let statBlocked = false;
      try {
        statSync(recorded);
      } catch {
        statBlocked = true;
      }
      expect(
        statBlocked,
        "precondition: the recorded path must be unstattable here (are you root?)"
      ).toBe(true);
      const stillRefused = resolveCorpusRelativeSourcePath(recorded, live);
      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [{ content: "UNREADABLE-PARENT-CANARY", source_meeting: recorded }],
        live,
        false
      );
      chmodSync(vaultParent, 0o755);
      expect(stillRefused).toBeNull();
      expect(released).toEqual([]);
      expect(withheld.total).toBe(1);
      expect(JSON.stringify(released)).not.toContain("UNREADABLE-PARENT-CANARY");
    } finally {
      try {
        chmodSync(vaultParent, 0o755);
      } catch {
        /* already restored */
      }
      rmSync(base, { recursive: true, force: true });
    }
  });
    }
  );

  it("counts anchors case-insensitively so a case variant cannot smuggle a second one", () => {
    // Exact-case counting sees one anchor here and binds the inner tail. On a
    // case-insensitive filesystem `Meetings` is an ordinary spelling of the same
    // directory, so this is the same wrong-meeting binding the two-anchor rule
    // exists to refuse.
    const { base, root } = makeMovedCorpus();
    try {
      expect(
        resolveCorpusRelativeSourcePath("/elsewhere/Meetings/meetings/x.md", root)
      ).toBeNull();
      expect(
        resolveCorpusRelativeSourcePath("/elsewhere/meetings/Meetings/x.md", root)
      ).toBeNull();
      // A single case-variant anchor is still recognised, so folding widens what
      // resolves rather than only refusing more.
      expect(resolveCorpusRelativeSourcePath("/elsewhere/Meetings/x.md", root)).toBe(
        join(realpathSync(root), "x.md")
      );
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("does not treat a backslash as a separator on POSIX", () => {
    // A POSIX filename may legitimately contain a backslash. Splitting on it
    // unconditionally re-segments the tail and binds a different meeting.
    const { base, root } = makeMovedCorpus();
    try {
      const resolved = resolveCorpusRelativeSourcePath(
        "/elsewhere/meetings/sub\\x.md",
        root
      );
      if (process.platform === "win32") {
        expect(resolved).toBe(join(realpathSync(root), "sub", "x.md"));
      } else {
        expect(resolved).toBe(join(realpathSync(root), "sub\\x.md"));
        expect(resolved).not.toBe(join(realpathSync(root), "sub", "x.md"));
      }
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("refuses a recorded path carrying more than one anchor segment", async () => {
    // Two anchors are ambiguous, and the last-anchor rule silently prefers the
    // inner one, which is a different meeting.
    const { base, root } = makeMovedCorpus();
    try {
      mkdirSync(join(root, "meetings"), { recursive: true });
      writeFileSync(join(root, "x.md"), meetingFixture("Outer"));
      writeFileSync(join(root, "meetings", "x.md"), meetingFixture("Inner"));

      expect(
        resolveCorpusRelativeSourcePath("/elsewhere/meetings/meetings/x.md", root)
      ).toBeNull();
      // The unambiguous single-anchor form still resolves, so the guard is not
      // simply refusing everything.
      expect(resolveCorpusRelativeSourcePath("/elsewhere/meetings/x.md", root)).toBe(
        join(realpathSync(root), "x.md")
      );
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });

  it("refuses traversal and inactive corpus directories after normalisation", async () => {
    const { base, root } = makeMovedCorpus();
    try {
      mkdirSync(join(root, "archive"), { recursive: true });
      writeFileSync(join(root, "archive", "old.md"), meetingFixture("Archived"));
      writeFileSync(join(base, "outside.md"), meetingFixture("Outside the corpus"));

      // Archived: the file exists and parses, so the inactive-directory rule is
      // the only thing refusing it.
      expect(
        resolveCorpusRelativeSourcePath("/Users/someone-else/meetings/archive/old.md", root)
      ).toBeNull();
      expect(resolveCorpusRelativeSourcePath("archive/old.md", root)).toBeNull();

      // Traversal out of the corpus, by either shape.
      expect(resolveCorpusRelativeSourcePath("../outside.md", root)).toBeNull();
      expect(
        resolveCorpusRelativeSourcePath("/Users/someone-else/meetings/../outside.md", root)
      ).toBeNull();

      // A value naming the root itself is not a meeting. This is refused by the
      // active-corpus check rejecting a candidate equal to the root, not by any
      // dedicated anchor-position guard.
      expect(resolveCorpusRelativeSourcePath("/Users/someone-else/meetings", root)).toBeNull();

      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        [
          { content: "archived", source_meeting: "archive/old.md" },
          { content: "escaped", source_meeting: "../outside.md" },
        ],
        root,
        false
      );
      expect(released).toEqual([]);
      expect(withheld.total).toBe(2);
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });
});

/**
 * Drive the real `get_meeting_insights` handler.
 *
 * The runner stands in for the `minutes insights` process and reproduces its
 * two window-shaping behaviours faithfully: `--since` filters by event
 * timestamp, and `--limit` returns the newest N from the tail. That fidelity is
 * what gives these tests teeth. A stub that ignored `--limit` would report a
 * constant tally no matter what the handler asked for, and every oracle
 * assertion below would pass with the oracle wide open.
 *
 * These tests are hermetic and must stay that way: CI runs this suite with no
 * `minutes` binary built, so anything that reaches the real CLI fails there.
 * `readiness` is stubbed below for exactly that reason, and two tests cover
 * what stubbing it removes.
 */
async function insightHarness(meetingsDir: string, records: any[]) {
  const mcpServer = new McpServer({
    name: "minutes-insight-handler-test",
    version: "0.0.0",
  });
  const calls: string[][] = [];
  registerUnavailableCompatibilityTools(mcpServer, {
    cliAvailable: async () => true,
    meetingsDir: async () => meetingsDir,
    // Insights are content-bearing, so the trust bridge runs before the handler
    // body, and the live bridge shells out to the CLI. Stubbing it is what makes
    // these tests hermetic: CI runs this suite with no `minutes` binary built.
    // Two separate tests below cover what this stub removes, namely that the
    // gate is really wired and that production binds the live bridge.
    readiness: async () => ({ ready: true }),
    runner: async (args: string[]) => {
      calls.push([...args]);
      let out = records;
      const sinceIndex = args.indexOf("--since");
      if (sinceIndex >= 0) {
        const floor = new Date(`${args[sinceIndex + 1]}T00:00:00`).getTime();
        out = out.filter((record) => Date.parse(record.timestamp) >= floor);
      }
      const limitIndex = args.indexOf("--limit");
      if (limitIndex >= 0) {
        const size = Number(args[limitIndex + 1]);
        out = out.slice(Math.max(0, out.length - size));
      }
      return { stdout: JSON.stringify(out), stderr: "" };
    },
  });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const client = new Client(
    { name: "insight-handler-client", version: "0.0.0" },
    { capabilities: {} }
  );
  await Promise.all([
    mcpServer.connect(serverTransport),
    client.connect(clientTransport),
  ]);
  return {
    calls,
    call: async (args: Record<string, unknown>) =>
      (await client.callTool({
        name: "get_meeting_insights",
        arguments: args,
      })) as any,
    close: async () => {
      await client.close();
      await mcpServer.close();
    },
  };
}

/** Ten records, alternating between a normal and a restricted source. */
function alternatingInsightWindow(normalPath: string, restrictedPath: string) {
  return Array.from({ length: 10 }, (_, index) => ({
    timestamp: `2026-07-${String(10 + index).padStart(2, "0")}T10:00:00Z`,
    kind: index % 3 === 0 ? "decision" : "commitment",
    content: `record-${index}`,
    confidence: "strong",
    participants: [index % 2 === 0 ? "Alex" : "Dana"],
    owner: null,
    source_meeting: index % 2 === 0 ? normalPath : restrictedPath,
  }));
}

describe("insight window is not shaped by the caller", () => {
  it("passes no caller-supplied value to the CLI", async () => {
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    const harness = await insightHarness(root, [
      {
        timestamp: "2026-07-15T10:00:00Z",
        kind: "decision",
        content: "only",
        confidence: "strong",
        participants: ["Alex"],
        source_meeting: "normal.md",
      },
    ]);
    try {
      await harness.call({
        kind: "commitment",
        confidence: "explicit",
        participant: "PRIVATE-SWEEP-CANARY",
        since: "2026-03-04",
        limit: 7,
        actionable_only: true,
      });

      // The window the CLI is asked for is a constant. Nothing the caller sent
      // appears in the argv at all.
      expect(harness.calls).toEqual([
        ["insights", "--limit", String(MCP_INSIGHT_SCAN_WINDOW)],
      ]);
      const argv = JSON.stringify(harness.calls);
      expect(argv).not.toContain("PRIVATE-SWEEP-CANARY");
      expect(argv).not.toContain("2026-03-04");
      expect(argv).not.toContain("--since");
      expect(argv).not.toContain("commitment");
      expect(argv).not.toContain("7");
    } finally {
      await harness.close();
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("holds the withheld tally still while the caller sweeps limit", async () => {
    // The limit-differencing oracle. When `limit` sized the fetched window, the
    // tally moved by one each time the newly included record was withheld, so
    // two calls read one record's policy verdict and a sweep mapped every
    // restricted meeting in the log.
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    writeFileSync(join(root, "restricted.md"), meetingFixture("Restricted", "restricted"));
    const harness = await insightHarness(
      root,
      alternatingInsightWindow("normal.md", "restricted.md")
    );
    try {
      const tallies: number[] = [];
      for (let limit = 1; limit <= 6; limit += 1) {
        const result = await harness.call({ limit });
        tallies.push(result.structuredContent.withheld.total);
      }
      // Five of the ten records name the restricted source, and that is the
      // answer for every limit. Sizing the window with `limit` instead would
      // walk this sequence 1,1,2,2,3,3 — one step per newly included record,
      // which is the leak stated as a number.
      expect(tallies).toEqual([5, 5, 5, 5, 5, 5]);
    } finally {
      await harness.close();
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("holds the withheld tally still while the caller sweeps since", async () => {
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    writeFileSync(join(root, "restricted.md"), meetingFixture("Restricted", "restricted"));
    const harness = await insightHarness(
      root,
      alternatingInsightWindow("normal.md", "restricted.md")
    );
    try {
      const tallies: number[] = [];
      for (const since of ["2026-07-10", "2026-07-14", "2026-07-17", "2026-07-20"]) {
        const result = await harness.call({ since });
        tallies.push(result.structuredContent.withheld.total);
      }
      expect(tallies).toEqual([5, 5, 5, 5]);
    } finally {
      await harness.close();
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("holds the withheld tally still while the caller sweeps content filters", async () => {
    // Rewritten from a version that captured the tally once and then asserted
    // the same immutable local inside a loop, which could not fail. Each sweep
    // step now issues its own request and reads the tally the handler computed
    // for that request.
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    writeFileSync(join(root, "restricted.md"), meetingFixture("Restricted", "restricted"));
    const harness = await insightHarness(
      root,
      alternatingInsightWindow("normal.md", "restricted.md")
    );
    try {
      const sweeps: Record<string, unknown>[] = [
        {},
        { participant: "Alex" },
        { participant: "nobody-at-all" },
        { kind: "decision" },
        { confidence: "explicit" },
      ];
      const tallies: number[] = [];
      const counts: number[] = [];
      for (const sweep of sweeps) {
        const result = await harness.call(sweep);
        tallies.push(result.structuredContent.withheld.total);
        counts.push(result.structuredContent.count);
      }
      // The tally never moves...
      expect(tallies).toEqual(sweeps.map(() => 5));
      // ...while the released set genuinely does, which is what makes the
      // constant tally evidence of anything.
      expect(new Set(counts).size).toBeGreaterThan(1);
    } finally {
      await harness.close();
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("lets a narrow filter reach records older than the requested limit", async () => {
    // `limit` caps the answer, not the search. Sizing the fetched window with it
    // meant a filtered question was answered from only the newest `limit`
    // records, so a participant whose only commitment was older than that
    // returned nothing and looked like an absence of evidence.
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    const records = Array.from({ length: 10 }, (_, index) => ({
      timestamp: `2026-07-${String(10 + index).padStart(2, "0")}T10:00:00Z`,
      kind: "commitment",
      content: `record-${index}`,
      confidence: "strong",
      participants: [index === 0 ? "Zola" : "Alex"],
      source_meeting: "normal.md",
    }));
    const harness = await insightHarness(root, records);
    try {
      const result = await harness.call({ participant: "Zola", limit: 1 });
      expect(result.structuredContent.count).toBe(1);
      expect(result.structuredContent.insights[0].content).toBe("record-0");
    } finally {
      await harness.close();
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("reports a capped answer as partial and keeps the newest matches", async () => {
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    const records = Array.from({ length: 10 }, (_, index) => ({
      timestamp: `2026-07-${String(10 + index).padStart(2, "0")}T10:00:00Z`,
      kind: "commitment",
      content: `record-${index}`,
      confidence: "strong",
      participants: ["Alex"],
      source_meeting: "normal.md",
    }));
    const harness = await insightHarness(root, records);
    try {
      const result = await harness.call({ limit: 3 });
      expect(result.structuredContent.count).toBe(3);
      expect(result.structuredContent.matched).toBe(10);
      expect(result.structuredContent.capped).toBe(true);
      expect(result.structuredContent.partial).toBe(true);
      // Capping the answer is not truncating the search. `truncated` compares
      // against the scan window, not against the caller's limit; comparing
      // against the limit would claim older records went unexamined whenever a
      // caller asked for fewer than were found.
      expect(result.structuredContent.truncated).toBe(false);
      expect(result.content[0].text).not.toMatch(/were not examined/);
      expect(
        result.structuredContent.insights.map((entry: any) => entry.content)
      ).toEqual(["record-7", "record-8", "record-9"]);
      // "releasable" is load-bearing: the count is over records that survived
      // policy, never over the whole window.
      expect(result.content[0].text).toContain("10 releasable record(s) matched");
    } finally {
      await harness.close();
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("applies since in this process with the CLI's calendar-day semantics", async () => {
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    // Records are a month apart on purpose. The floor is local midnight while
    // the records are UTC, so day-spaced fixtures would straddle the boundary
    // differently depending on the host's offset and this assertion would only
    // hold in some timezones.
    const records = Array.from({ length: 10 }, (_, index) => ({
      timestamp: `2026-${String(index + 1).padStart(2, "0")}-15T12:00:00Z`,
      kind: "commitment",
      content: `record-${index}`,
      confidence: "strong",
      participants: ["Alex"],
      source_meeting: "normal.md",
    }));
    const harness = await insightHarness(root, records);
    try {
      const result = await harness.call({ since: "2026-08-01", limit: 500 });
      expect(
        result.structuredContent.insights.map((entry: any) => entry.content)
      ).toEqual(["record-7", "record-8", "record-9"]);

      // A malformed date is refused rather than silently widening the query.
      for (const malformed of ["17-07-2026", "2026-02-30", "2026-7-17"]) {
        const refused = await harness.call({ since: malformed });
        expect(refused.isError).toBe(true);
        expect(refused.content[0].text).toMatch(/YYYY-MM-DD/);
        expect(refused.structuredContent).toBeUndefined();
      }
    } finally {
      await harness.close();
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("distinguishes a withheld partial view from a genuinely empty one", async () => {
    // Rewritten from a test whose name promised the partial-view contract but
    // whose body only exercised the release helper, which has no notion of
    // `partial` at all. The flag lives in the handler, so the handler is what
    // this asserts.
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    writeFileSync(join(root, "restricted.md"), meetingFixture("Restricted", "restricted"));
    try {
      const withRestricted = await insightHarness(root, [
        {
          timestamp: "2026-07-15T10:00:00Z",
          kind: "decision",
          content: "RESTRICTED-PARTIAL-CANARY",
          confidence: "strong",
          participants: ["Alex"],
          source_meeting: "restricted.md",
        },
      ]);
      try {
        const result = await withRestricted.call({ participant: "Alex" });
        // Nothing released, but the answer must not read as "there is nothing".
        // The filter was never evaluated against the withheld record, so the
        // text must not claim it failed to match.
        expect(result.content[0].text).toContain("No releasable meeting insights");
        expect(result.content[0].text).toContain("not filter-tested");
        expect(result.content[0].text).not.toMatch(
          /^No meeting insights matched the filter criteria/
        );
        expect(result.structuredContent.count).toBe(0);
        expect(result.structuredContent.partial).toBe(true);
        expect(result.structuredContent.withheld.total).toBe(1);
        expect(result.content[0].text).toContain("could not be released");
        expect(JSON.stringify(result)).not.toContain("RESTRICTED-PARTIAL-CANARY");
      } finally {
        await withRestricted.close();
      }

      const genuinelyEmpty = await insightHarness(root, []);
      try {
        const result = await genuinelyEmpty.call({});
        expect(result.structuredContent.count).toBe(0);
        expect(result.structuredContent.partial).toBe(false);
        expect(result.structuredContent.withheld.total).toBe(0);
        expect(result.content[0].text).not.toContain("could not be released");
        // A genuinely complete answer must not carry the withheld caveat, or it
        // reads as partial when it is not.
        expect(result.content[0].text).not.toContain("not filter-tested");
      } finally {
        await genuinelyEmpty.close();
      }
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("reports the empty branch's counters too", async () => {
    // `matched` and `capped` are set in both return branches. The non-empty
    // branch is asserted above; without this the empty branch's copies could be
    // dropped with the suite green.
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    const harness = await insightHarness(root, [
      {
        timestamp: "2026-07-15T10:00:00Z",
        kind: "decision",
        content: "only",
        confidence: "strong",
        participants: ["Alex"],
        source_meeting: "normal.md",
      },
    ]);
    try {
      const result = await harness.call({ participant: "nobody-at-all" });
      expect(result.structuredContent.count).toBe(0);
      expect(result.structuredContent.matched).toBe(0);
      expect(result.structuredContent.capped).toBe(false);
    } finally {
      await harness.close();
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("states the withheld and truncation reasons the agent is shown", async () => {
    // Both notes were rewritten because their previous wordings asserted things
    // that were not true. Nothing asserted either text, so both could be
    // reverted with the suite green. The truncation branch additionally needs a
    // saturated window, which no other test produces.
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    writeFileSync(join(root, "restricted.md"), meetingFixture("Restricted", "restricted"));
    const saturated = Array.from({ length: MCP_INSIGHT_SCAN_WINDOW }, (_, index) => ({
      timestamp: "2026-07-15T10:00:00Z",
      kind: "commitment",
      content: `record-${index}`,
      confidence: "strong",
      participants: ["Alex"],
      source_meeting: index === 0 ? "restricted.md" : "normal.md",
    }));
    const harness = await insightHarness(root, saturated);
    try {
      const result = await harness.call({ limit: 500 });
      expect(result.structuredContent.truncated).toBe(true);
      expect(result.structuredContent.withheld.total).toBe(1);

      const text = result.content[0].text;
      // The note must say what the number counts. The tally is over the scanned
      // window and not over the caller's filtered result, and saying so is what
      // stops an agent reading it as "200 of your matches were suppressed".
      expect(text).toContain("most recent record(s) examined");
      expect(text).toContain("independently of the filters in this request");
      // It must cover the missing-source case too, not only the policy one,
      // because the tally counts both.
      expect(text).toContain("the record names no source meeting");
      expect(text).toContain("could not be resolved to a meeting in the active corpus");
      expect(text).toContain("designated restricted, archived, or deleted");
      // The truncation note must not claim older records exist, and must not
      // offer a remedy this tool does not have.
      expect(text).toContain(
        `examined the newest ${MCP_INSIGHT_SCAN_WINDOW} record(s); any older ones were not examined`
      );
      expect(text).not.toMatch(/raise limit or narrow since/i);
      expect(text).not.toMatch(/anything older was not examined/i);
    } finally {
      await harness.close();
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("enforces its own window rather than trusting the CLI to honour --limit", async () => {
    // Both notes state the window as fact. If an over-long projection came back,
    // `matched` would exceed the window and the capped note would tell the agent
    // to raise `limit` past the schema's own maximum, which it cannot do.
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    const overlong = Array.from({ length: MCP_INSIGHT_SCAN_WINDOW + 50 }, (_, index) => ({
      timestamp: "2026-07-15T10:00:00Z",
      kind: "commitment",
      content: `record-${index}`,
      confidence: "strong",
      participants: ["Alex"],
      source_meeting: "normal.md",
    }));
    // Deliberately ignores --limit, which is the failure being guarded against.
    const mcpServer = new McpServer({ name: "minutes-overlong-test", version: "0.0.0" });
    registerUnavailableCompatibilityTools(mcpServer, {
      cliAvailable: async () => true,
      meetingsDir: async () => root,
      readiness: async () => ({ ready: true }),
      runner: async () => ({ stdout: JSON.stringify(overlong), stderr: "" }),
    });
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client(
      { name: "overlong-client", version: "0.0.0" },
      { capabilities: {} }
    );
    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      const result: any = await client.callTool({
        name: "get_meeting_insights",
        arguments: { limit: MCP_INSIGHT_RESULT_MAX },
      });
      expect(result.structuredContent.matched).toBe(MCP_INSIGHT_SCAN_WINDOW);
      expect(result.structuredContent.capped).toBe(false);
      expect(result.content[0].text).not.toMatch(/Raise limit for the rest/);
      // The newest records are the ones kept.
      expect(result.structuredContent.insights.at(-1).content).toBe(
        `record-${MCP_INSIGHT_SCAN_WINDOW + 49}`
      );
    } finally {
      await client.close();
      await mcpServer.close();
      rmSync(base, { recursive: true, force: true });
    }
  }, 60_000);

  it("describes its own arguments truthfully to the agent", async () => {
    // These strings are the only thing an agent reads before choosing arguments,
    // and `check:llms` cannot reach them: the generator takes tool descriptions
    // from manifest.json, not from the zod `.describe()` calls. Without this
    // they can be reverted to wordings the code has made false.
    const mcpServer = new McpServer({ name: "minutes-insight-schema-test", version: "0.0.0" });
    registerUnavailableCompatibilityTools(mcpServer, {
      cliAvailable: async () => true,
      meetingsDir: async () => tmpdir(),
      readiness: async () => ({ ready: true }),
      runner: async () => ({ stdout: "[]", stderr: "" }),
    });
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client(
      { name: "insight-schema-client", version: "0.0.0" },
      { capabilities: {} }
    );
    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      const listed = await client.listTools();
      const tool = listed.tools.find((entry) => entry.name === "get_meeting_insights");
      const properties = (tool?.inputSchema as any)?.properties ?? {};

      // `limit` caps the answer, not the search, and must say so.
      expect(properties.limit?.description).toContain(
        `Every read examines the newest ${MCP_INSIGHT_SCAN_WINDOW} records regardless`
      );
      expect(properties.limit?.description).not.toMatch(/^Maximum number of results \(/);
      // `since` is a floor on a calendar date.
      expect(properties.since?.description).toContain("on or after this calendar date");
      // The tool's own policy sentence. Policy is verified on the meeting the
      // recorded path RESOLVES to, not on the recorded path itself, and a
      // released record still carries the recorded title and path. Saying
      // otherwise would tell an agent the boundary is tighter than it is.
      expect(tool?.description).toContain("resolved to a meeting in the live corpus");
      expect(tool?.description).toContain("as they were recorded");
      expect(tool?.description).not.toMatch(
        /the meeting the pipeline recorded as its source is re-read/
      );
      // The override recovers restricted sources only. "Moved" is now recovered
      // without it, so listing "moved" as unrecoverable would be false.
      expect(properties.include_restricted?.description).toContain(
        "cannot be resolved to a meeting in this corpus"
      );
      expect(properties.include_restricted?.description).not.toMatch(
        /archived, moved, or deleted/
      );
    } finally {
      await client.close();
      await mcpServer.close();
    }
  }, 60_000);

  it("keeps the scanned window at the largest limit a caller may request", () => {
    // The invariant the window's whole argument rests on. Asserting it through
    // String(MCP_INSIGHT_SCAN_WINDOW) in the argv test only proves the handler
    // read the same constant the test did; narrowing the window would ship
    // green and silently cost the tool its reach.
    expect(MCP_INSIGHT_SCAN_WINDOW).toBe(MCP_INSIGHT_RESULT_MAX);
  });

  it("binds the live implementations when nothing is injected", async () => {
    // Production registers with no deps. Nothing else asserts that those
    // defaults are the real functions, so a default rebound to a stub would
    // ship green as a tool that returns nothing or claims the CLI is missing.
    const live = resolveInsightToolDeps();
    // By reference where the binding is exported, since a name check passes
    // against any stub that sets `name`. `runMinutes` and `isCliAvailable` are
    // module-private, so those fall back to the weaker check.
    expect(live.readiness).toBe(requireAgentTrustReadiness);
    expect(live.resolveMeetingsDir).toBe(getEffectiveMeetingsDir);
    expect(live.runCli.name).toBe("runMinutes");
    expect(live.cliIsAvailable.name).toBe("isCliAvailable");
    // An override replaces exactly the one binding it names.
    const stub = async () => ({ ready: true });
    const overridden = resolveInsightToolDeps({ readiness: stub });
    expect(overridden.readiness).toBe(stub);
    expect(overridden.runCli.name).toBe("runMinutes");
  });

  it("runs the trust gate before the handler body and withholds when it fails", async () => {
    // The harness stubs readiness to stay hermetic, which would hide a
    // regression that removed the gate entirely. This asserts the gate is wired:
    // a failing bridge must withhold the records, and must do so without the
    // handler's content reaching the caller.
    const { base, root } = makeMovedCorpus();
    writeFileSync(join(root, "normal.md"), meetingFixture("Normal"));
    const mcpServer = new McpServer({ name: "minutes-insight-gate-test", version: "0.0.0" });
    let handlerRan = 0;
    registerUnavailableCompatibilityTools(mcpServer, {
      cliAvailable: async () => true,
      meetingsDir: async () => root,
      readiness: async () => {
        throw new Error("trust registry degraded");
      },
      runner: async () => {
        handlerRan += 1;
        return {
          stdout: JSON.stringify([
            {
              timestamp: "2026-07-15T10:00:00Z",
              kind: "decision",
              content: "READINESS-GATE-CANARY",
              confidence: "strong",
              participants: ["Alex"],
              source_meeting: "normal.md",
            },
          ]),
          stderr: "",
        };
      },
    });
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client(
      { name: "insight-gate-client", version: "0.0.0" },
      { capabilities: {} }
    );
    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      const result: any = await client.callTool({
        name: "get_meeting_insights",
        arguments: {},
      });
      expect(JSON.stringify(result)).not.toContain("READINESS-GATE-CANARY");
    } finally {
      await client.close();
      await mcpServer.close();
      rmSync(base, { recursive: true, force: true });
    }
    // Whether the body ran is not the contract; not releasing its content is.
    expect(handlerRan).toBeLessThanOrEqual(1);
  }, 60_000);
});

describe("restricted content policy", () => {
  it("keeps restricted exact-read stubs path-free across every MCP result field", () => {
    const parentCanary = "RESTRICTED-MCP-PARENT-CANARY";
    const fileCanary = "RESTRICTED-MCP-FILENAME-CANARY";
    const path = `/synthetic/${parentCanary}/${fileCanary}.md`;
    const meeting = parsePolicyVerifiedMeeting(
      [
        "---",
        "title: Synthetic restricted review",
        "type: meeting",
        "date: 2026-07-15T10:00:00Z",
        "sensitivity: restricted",
        "---",
        "",
        "RESTRICTED-MCP-BODY-CANARY",
      ].join("\n"),
      path
    );
    expect(meeting).not.toBeNull();
    const result = restrictedMeetingStubResult(meeting!);
    expect(Object.keys(result.structuredContent).sort()).toEqual([
      "date",
      "restricted_stub",
      "sensitivity",
      "title",
      "type",
      "view",
    ]);
    expect(Object.keys(result._meta).sort()).toEqual(["ui", "view"]);
    const serialized = JSON.stringify(result);
    expect(serialized).not.toContain(parentCanary);
    expect(serialized).not.toContain(fileCanary);
    expect(serialized).not.toContain("RESTRICTED-MCP-BODY-CANARY");
    expect(serialized).not.toContain("/synthetic/");
  });

  it("keeps the standalone logged override but recognizes native deny mode", () => {
    const records: string[] = [];
    expect(restrictedContentPolicyFromEnv(undefined)).toBe("deny");
    expect(restrictedContentPolicyFromEnv(" DENY ")).toBe("deny");
    expect(restrictedContentPolicyFromEnv("typo")).toBe("deny");
    expect(restrictedContentPolicyFromEnv("logged-override")).toBe(
      "logged-override"
    );
    expect(restrictedContentPolicyFromEnv("logged-override", "win32")).toBe(
      "logged-override"
    );
    expect(() =>
      enforceRestrictedContentPolicy(
        { include_restricted: true, query: "PRIVATE_QUERY_CANARY" },
        "search_meetings",
        "deny"
      )
    ).toThrow(/unavailable/i);
    expect(() =>
      enforceRestrictedContentPolicy(
        { include_restricted: false },
        "search_meetings",
        "deny"
      )
    ).not.toThrow();
    expect(() =>
      enforceRestrictedContentPolicy(
        { include_restricted: true, query: "PRIVATE_QUERY_CANARY" },
        "search_meetings",
        "logged-override",
        "/ignored/by/capability-bridge",
        (_path, line) => records.push(line)
      )
    ).not.toThrow();
    const audit = records.join("")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    expect(audit).toHaveLength(1);
    expect(audit[0]).toMatchObject({
      event: "sensitivity.override",
      surface: "search_meetings",
      authorization: "operator-launch-policy+tool-argument",
      scope_fields: ["query"],
    });
    expect(audit[0].scope_sha256).toMatch(/^[a-f0-9]{64}$/);
    expect(records.join("")).not.toContain("PRIVATE_QUERY_CANARY");
  });

  it("blocks both runtime registration surfaces before handlers execute", async () => {
    const previousPolicy = process.env.MINUTES_MCP_RESTRICTED_POLICY;
    process.env.MINUTES_MCP_RESTRICTED_POLICY = "deny";

    const exercise = async (kind: "tool" | "app") => {
      const mcpServer = new McpServer({
        name: `minutes-policy-${kind}`,
        version: "0.0.0",
      });
      let handlerExecutions = 0;
      const name = `policy_${kind}`;
      const handler = async () => {
        handlerExecutions += 1;
        return { content: [{ type: "text" as const, text: "handler ran" }] };
      };
      const inputSchema = {
        include_restricted: z.boolean().optional().default(false),
      };

      if (kind === "tool") {
        registerToolWithRestrictedPolicy(
          mcpServer,
          name,
          "Policy test tool",
          inputSchema,
          { readOnlyHint: true },
          handler
        );
      } else {
        registerDocsAppToolWithRestrictedPolicy(
          mcpServer,
          name,
          {
            description: "Policy test app tool",
            inputSchema,
            annotations: { readOnlyHint: true },
            _meta: { ui: { resourceUri: "ui://minutes/policy-test.html" } },
          },
          handler
        );
      }

      const [clientTransport, serverTransport] =
        InMemoryTransport.createLinkedPair();
      const client = new Client(
        { name: `policy-${kind}-client`, version: "0.0.0" },
        { capabilities: {} }
      );
      try {
        await Promise.all([
          mcpServer.connect(serverTransport),
          client.connect(clientTransport),
        ]);
        const denied = await client.callTool({
          name,
          arguments: { include_restricted: true },
        });
        expect(denied.isError).toBe(true);
        expect(JSON.stringify(denied.content)).toMatch(/unavailable/i);
        expect(handlerExecutions).toBe(0);

        const allowed = await client.callTool({
          name,
          arguments: { include_restricted: false },
        });
        expect(allowed.isError).not.toBe(true);
        expect(handlerExecutions).toBe(1);
      } finally {
        await client.close();
        await mcpServer.close();
      }
    };

    try {
      await exercise("tool");
      await exercise("app");
    } finally {
      if (previousPolicy === undefined) {
        delete process.env.MINUTES_MCP_RESTRICTED_POLICY;
      } else {
        process.env.MINUTES_MCP_RESTRICTED_POLICY = previousPolicy;
      }
    }
  });

  it("denies an override when its exact audit writer does not complete", () => {
    for (const failure of ["open", "write", "sync"] as const) {
      const auditDir = mkdtempSync(join(tmpdir(), "minutes-override-audit-io-"));
      const auditPath = join(auditDir, "audit.jsonl");
      let caught: unknown;
      try {
        enforceRestrictedContentPolicy(
          { include_restricted: true, query: "PRIVATE_AUDIT_IO_CANARY" },
          "search_meetings",
          "logged-override",
          auditPath,
          () => {
            throw new Error(`injected ${failure} error`);
          }
        );
      } catch (error) {
        caught = error;
      }
      expect(caught).toBeInstanceOf(Error);
      expect((caught as Error).message).toBe(
        "MCP error -32603: Restricted override denied because its audit record could not be written safely."
      );
      expect((caught as Error).message).not.toContain(auditPath);
      expect((caught as Error).message).not.toContain("PRIVATE_AUDIT_IO_CANARY");
      rmSync(auditDir, { recursive: true, force: true });
    }
  });

  it("bounds each audit record before invoking the native capability bridge", async () => {
    const records: string[] = [];
    const boundedWriter = (_path: string, line: string) => {
      if (Buffer.byteLength(line, "utf8") > 16 * 1024) {
        throw new Error("bounded native bridge refusal");
      }
      records.push(line);
    };
    await Promise.all(
      Array.from({ length: 16 }, (_, index) =>
        Promise.resolve().then(() =>
          enforceRestrictedContentPolicy(
            { include_restricted: true, index },
            "list_meetings",
            "logged-override",
            "/ignored/by/capability-bridge",
            boundedWriter
          )
        )
      )
    );
    const lines = records.join("").trim().split("\n");
    expect(lines).toHaveLength(16);
    expect(lines.every((line) => JSON.parse(line).event === "sensitivity.override"))
      .toBe(true);

    const oversizedField = `field_${"x".repeat(20 * 1024)}`;
    expect(() =>
      enforceRestrictedContentPolicy(
        { include_restricted: true, [oversizedField]: true },
        "list_meetings",
        "logged-override",
        "/ignored/by/capability-bridge",
        boundedWriter
      )
    ).toThrow("Restricted override denied");
  });

  it("enforces positive bounded meeting limits and an independent action cap", async () => {
    expect(normalizeMcpMeetingResultLimit(1)).toBe(1);
    expect(normalizeMcpMeetingResultLimit(MCP_MEETING_RESULT_MAX)).toBe(
      MCP_MEETING_RESULT_MAX
    );
    for (const invalid of [0, -1, 1.5, Number.NaN, MCP_MEETING_RESULT_MAX + 1]) {
      expect(() => normalizeMcpMeetingResultLimit(invalid)).toThrow(/limit must be/i);
    }

    const meetings = [
      {
        path: "/bounded/meeting.md",
        frontmatter: {
          action_items: Array.from(
            { length: MCP_ACTION_RESULT_MAX + 25 },
            (_, index) => ({
              task: `action-${index}`,
              assignee: "owner",
              status: "open",
            })
          ),
        },
      },
    ] as any;
    expect(openActionsFromMeetings(meetings)).toHaveLength(
      MCP_ACTION_RESULT_MAX
    );
    for (const invalid of [0, -1, 1.5, Number.NaN, MCP_ACTION_RESULT_MAX + 1]) {
      expect(() => openActionsFromMeetings(meetings, invalid)).toThrow(
        /open action limit must be/i
      );
    }

    for (const invalid of [
      0,
      -1,
      1.5,
      Number.NaN,
      MCP_POLICY_MEETING_RESULT_MAX + 1,
    ]) {
      await expect(
        policyListMeetings("/does-not-matter", invalid, false)
      ).rejects.toThrow(/policy meeting limit must be/i);
      await expect(
        policySearchMeetings("/does-not-matter", "query", invalid, false)
      ).rejects.toThrow(/policy search limit must be/i);
    }
  });

  it("selects the newest bounded policy corpus before downstream slicing", () => {
    const files = Array.from(
      { length: MCP_POLICY_MEETING_RESULT_MAX + 1 },
      (_, index) => {
        const day = String((index % 28) + 1).padStart(2, "0");
        const year = 2000 + Math.floor(index / 28);
        const path = `/bounded/${String(index).padStart(5, "0")}.md`;
        return {
          path,
          relativePath: `${String(index).padStart(5, "0")}.md`,
          content: `---\ntitle: Meeting ${index}\ntype: meeting\ndate: ${year}-01-${day}T00:00:00Z\nduration: 1m\n---\nbody\n`,
        };
      }
    );
    const snapshots = collectPolicyVerifiedMeetingSnapshots(
      { canonicalRoot: "/bounded", files } as any,
      false
    );

    expect(snapshots).toHaveLength(MCP_POLICY_MEETING_RESULT_MAX);
    expect(snapshots.some((entry) => entry.path.endsWith("05000.md"))).toBe(true);
    expect(snapshots.some((entry) => entry.path.endsWith("00000.md"))).toBe(false);
  });

  it("matches text and structured intents across the full bounded scan before retention", () => {
    const oldTextToken = "SYNTHETIC-OLD-ONLY-TEXT";
    const oldIntentToken = "SYNTHETIC-OLD-ONLY-DECISION";
    const commonToken = "SYNTHETIC-COMMON-SEARCH";
    const meetingFile = (
      index: number,
      options: { restricted?: boolean; oldMatch?: boolean } = {}
    ) => {
      const date = new Date(Date.UTC(2020, 0, index + 1)).toISOString();
      const path = `/bounded-search/${String(index).padStart(5, "0")}.md`;
      return {
        path,
        relativePath: `${String(index).padStart(5, "0")}.md`,
        content: [
          "---",
          `title: Synthetic meeting ${index}`,
          "type: meeting",
          `date: ${date}`,
          `sensitivity: ${options.restricted ? "restricted" : "normal"}`,
          "tags: []",
          "attendees: []",
          "people: []",
          "action_items: []",
          "decisions:",
          `  - text: ${options.oldMatch ? oldIntentToken : `unrelated-${index}`}`,
          "intents: []",
          "---",
          "",
          `${commonToken} ${options.oldMatch ? oldTextToken : "unrelated body"}`,
        ].join("\n"),
      };
    };
    const files = Array.from(
      { length: MCP_POLICY_MEETING_RESULT_MAX + 1 },
      (_, index) => meetingFile(index, { oldMatch: index === 0 })
    );
    files.push({
      ...meetingFile(MCP_POLICY_MEETING_RESULT_MAX + 1, {
        restricted: true,
        oldMatch: true,
      }),
      path: "/bounded-search/restricted-newest.md",
      relativePath: "restricted-newest.md",
    });
    const snapshot = { canonicalRoot: "/bounded-search", files } as any;

    const oldText = collectPolicyToolSearchSnapshots(snapshot, false, {
      query: oldTextToken,
      contentType: "meeting",
      since: "2019-01-01",
    });
    expect(oldText.map((entry) => entry.path)).toEqual([
      "/bounded-search/00000.md",
    ]);

    const oldIntent = collectPolicyToolSearchSnapshots(snapshot, false, {
      query: oldIntentToken,
      intentKind: "decision",
      intentsOnly: true,
      since: "2019-01-01",
    });
    expect(oldIntent.map((entry) => entry.path)).toEqual([
      "/bounded-search/00000.md",
    ]);
    expect(
      policyIntentResults(
        oldIntent.map((entry) => entry.meeting),
        oldIntentToken,
        "decision",
        undefined,
        1
      )
    ).toHaveLength(1);

    const oldOnlySnapshot = {
      canonicalRoot: "/bounded-search",
      files: [files[0]],
    } as any;
    expect(collectPolicyToolSearchSnapshots(oldOnlySnapshot, false, {
      query: oldTextToken,
      contentType: "memo",
    })).toEqual([]);
    expect(collectPolicyToolSearchSnapshots(oldOnlySnapshot, false, {
      query: oldTextToken,
      since: "2021-01-01",
    })).toEqual([]);

    const common = collectPolicyToolSearchSnapshots(snapshot, false, {
      query: commonToken,
    });
    expect(common).toHaveLength(MCP_POLICY_MEETING_RESULT_MAX);
    expect(common[0].path).toBe("/bounded-search/05000.md");
    expect(common.some((entry) => entry.path.endsWith("00000.md"))).toBe(false);
    expect(common.some((entry) => entry.path.includes("restricted"))).toBe(false);
  }, 15_000);

  it("bounds derived profile, intent, research, and relationship collections before output", () => {
    const long = "x".repeat(10_000);
    const baseMeeting = {
      path: `/bounded/${long}.md`,
      body: `Alex ${long}`,
      frontmatter: {
        title: long,
        type: "meeting",
        date: "2026-07-16T12:00:00Z",
        duration: "1m",
        tags: Array.from({ length: 75 }, (_, index) => `topic-${index}-${long}`),
        attendees: ["Alex"],
        attendees_raw: "",
        people: [],
        action_items: Array.from({ length: 75 }, (_, index) => ({
          assignee: "Alex",
          task: `task-${index}-${long}`,
          status: "open",
        })),
        decisions: Array.from({ length: 75 }, (_, index) => ({
          text: `decision-${index}-${long}`,
        })),
        intents: Array.from({ length: 75 }, (_, index) => ({
          kind: "commitment",
          what: `intent-${index}-${long}`,
          who: "Alex",
          status: "open",
        })),
      },
    } as any;
    const meetings = Array.from(
      { length: MCP_PERSON_PROFILE_MEETING_MAX + 25 },
      (_, index) => ({
        ...baseMeeting,
        path: `/bounded/meeting-${String(index).padStart(3, "0")}.md`,
      })
    );

    const profile = personProfileFromMeetings(meetings, "Alex");
    expect(profile.meetings).toHaveLength(MCP_PERSON_PROFILE_MEETING_MAX);
    expect(profile.openActions).toHaveLength(
      MCP_PERSON_PROFILE_OPEN_ACTION_MAX
    );
    expect(profile.topics).toHaveLength(MCP_PERSON_PROFILE_TOPIC_MAX);
    expect(profile.recentDecisions).toHaveLength(MCP_PERSON_PROFILE_DECISION_MAX);
    expect(profile.meetings.every((meeting) => meeting.title.length <= 2_048)).toBe(
      true
    );
    expect(profile.openActions.every((action) => action.what.length <= 2_048)).toBe(
      true
    );
    for (const [field, max] of [
      ["meetingLimit", MCP_PERSON_PROFILE_MEETING_MAX],
      ["openActionLimit", MCP_PERSON_PROFILE_OPEN_ACTION_MAX],
      ["topicLimit", MCP_PERSON_PROFILE_TOPIC_MAX],
    ] as const) {
      expect(() =>
        personProfileFromMeetings(meetings, "Alex", { [field]: max + 1 })
      ).toThrow(/person profile .* limit must be/i);
    }

    const intents = policyIntentResults(
      meetings,
      "",
      undefined,
      undefined,
      MCP_INTENT_RESULT_MAX,
      new Set(["open"])
    );
    expect(intents).toHaveLength(MCP_INTENT_RESULT_MAX);
    expect(intents.every((intent) => intent.what.length <= 2_048)).toBe(true);

    const research = researchTopicProjection(meetings, long);
    expect(research.meetings).toHaveLength(MCP_RESEARCH_MEETING_RESULT_MAX);
    expect(research.decisions).toHaveLength(MCP_RESEARCH_DECISION_RESULT_MAX);
    expect(research.openIntents).toHaveLength(MCP_INTENT_RESULT_MAX);
    expect(research.topics).toHaveLength(MCP_RESEARCH_TOPIC_RESULT_MAX);
    expect(research.text.length).toBeLessThanOrEqual(256 * 1024);
    expect(research.decisions.every((decision) => decision.length <= 2_048)).toBe(
      true
    );

  });

  it("person profiles use exact identity evidence and reject ambiguous aliases", () => {
    const makeMeeting = (name: string, body: string, task: string) => ({
      path: `/profiles/${name.toLowerCase()}.md`,
      body,
      frontmatter: {
        title: `${name} review`,
        type: "meeting",
        date: "2026-07-20T12:00:00Z",
        duration: "10m",
        tags: ["planning"],
        attendees: [name],
        attendees_raw: "",
        people: [],
        action_items: [{ assignee: name, task, status: "open" }],
        decisions: [],
        intents: [],
      },
    }) as any;
    const meetings = [
      makeMeeting("Ann", "Ordinary discussion.", "Ann task"),
      makeMeeting("Joanna", "Planning announcement mentions Ann in prose.", "Joanna task"),
    ];
    const profile = personProfileFromMeetings(meetings, "Ann");
    expect(profile.meetings.map((meeting) => meeting.title)).toEqual(["Ann review"]);
    expect(profile.openActions.map((action) => action.what)).toEqual(["Ann task"]);
    expect(() => personProfileFromMeetings(meetings, "   ")).toThrow(/empty/i);

    const mentionOnly = makeMeeting("Actual Contact", "", "Actual task") as any;
    mentionOnly.frontmatter.people = ["Discussed Person"];
    mentionOnly.frontmatter.action_items = [{
      assignee: "Discussed Person",
      task: "Owner-specific task",
      status: "open",
    }];
    mentionOnly.frontmatter.decisions = [{ text: "Participant-only decision" }];
    const mentionedProfile = personProfileFromMeetings([mentionOnly], "Discussed Person");
    expect(mentionedProfile.meetings).toEqual([]);
    expect(mentionedProfile.topics).toEqual([]);
    expect(mentionedProfile.recentDecisions).toEqual([]);
    expect(mentionedProfile.openActions.map((action) => action.what)).toEqual([
      "Owner-specific task",
    ]);

    const legacy = makeMeeting("Placeholder", "", "Unused") as any;
    legacy.frontmatter.attendees = [];
    legacy.frontmatter.attendees_raw = "Alice Smith (alice@example.com)";
    legacy.frontmatter.action_items = [
      { assignee: "Alice Smith", task: "Legacy attendee task", status: "open" },
    ];
    expect(personProfileFromMeetings([legacy], "Alice Smith").openActions[0].what)
      .toBe("Legacy attendee task");

    const aliasMeeting = makeMeeting("Avery Quinn", "", "Canonical owner task") as any;
    aliasMeeting.frontmatter.entities = {
      people: [{ slug: "avery-quinn", label: "Avery Quinn", aliases: ["AQ"] }],
    };
    expect(personProfileFromMeetings([aliasMeeting], "AQ").openActions[0].what)
      .toBe("Canonical owner task");

    const ambiguous = ["Alex North", "Alex South"].map((label) => ({
      ...makeMeeting(label, "", `${label} task`),
      frontmatter: {
        ...makeMeeting(label, "", `${label} task`).frontmatter,
        entities: {
          people: [{ slug: label.toLowerCase().replace(/\s+/g, "-"), label, aliases: ["Shared Alias"] }],
        },
      },
    })) as any;
    expect(() => personProfileFromMeetings(ambiguous, "Shared Alias")).toThrow(/ambiguous/i);
  });

  it("validates and preserves the correction-aware core person profile schema", () => {
    const profile = boundedCorePersonProfile({
      name: "Avery Quinn",
      recent_meetings: [
        { title: "Review", date: "2026-07-20T12:00:00Z", path: "/meetings/review.md" },
      ],
      open_intents: [
        {
          date: "2026-07-20T12:00:00Z",
          title: "Review",
          content_type: "meeting",
          path: "/meetings/review.md",
          kind: "commitment",
          what: "Send the plan",
          who: "Avery Quinn",
          status: "open",
          by_date: null,
        },
      ],
      recent_decisions: [
        {
          title: "Review",
          date: "2026-07-20T12:00:00Z",
          path: "/meetings/review.md",
          what: "Use the synthetic rollout",
          authority: "high",
        },
      ],
      top_topics: [{ topic: "planning", count: 3 }],
    });
    expect(profile.name).toBe("Avery Quinn");
    expect(profile.openIntents[0].what).toBe("Send the plan");
    expect(profile.topicCounts).toEqual([{ topic: "planning", count: 3 }]);
    expect(profile.recentDecisions).toEqual([
      {
        title: "Review",
        date: "2026-07-20T12:00:00Z",
        path: "/meetings/review.md",
        what: "Use the synthetic rollout",
        authority: "high",
      },
    ]);
  });

  it("preserves the historical commitment row keys and enum values", () => {
    const rows = historicalCommitmentRows([
      {
        date: "2026-07-20T12:00:00Z",
        title: "Planning",
        content_type: "meeting",
        path: "/meetings/planning.md",
        kind: "action-item",
        what: "Send the plan",
        who: "Avery Quinn",
        status: "stale",
        by_date: "2026-07-20",
      },
      {
        date: "2026-07-21T12:00:00Z",
        title: "Follow-up",
        content_type: "meeting",
        path: "/meetings/follow-up.md",
        kind: "commitment",
        what: "Review the plan",
        status: "open",
      },
    ]);
    expect(rows).toEqual([
      {
        text: "Send the plan",
        status: "stale",
        due_date: "2026-07-20",
        created_at: "2026-07-20T12:00:00Z",
        commitment_type: "action_item",
        meeting_title: "Planning",
        meeting_date: "2026-07-20T12:00:00Z",
        person_name: "Avery Quinn",
      },
      {
        text: "Review the plan",
        status: "open",
        due_date: null,
        created_at: "2026-07-21T12:00:00Z",
        commitment_type: "intent",
        meeting_title: "Follow-up",
        meeting_date: "2026-07-21T12:00:00Z",
        person_name: null,
      },
    ]);
    expect(Object.keys(rows[0])).toEqual([
      "text",
      "status",
      "due_date",
      "created_at",
      "commitment_type",
      "meeting_title",
      "meeting_date",
      "person_name",
    ]);
  });

  it("preserves the relationship map structured payload contract", () => {
    const person = {
      slug: "avery-quinn",
      name: "Avery Quinn",
      meeting_count: 3,
      last_seen: "2026-07-20T12:00:00Z",
      days_since: 1,
      open_commitments: 1,
      top_topics: ["planning"],
      score: 4.5,
      losing_touch: false,
    };
    expect(relationshipMapStructuredContent([person])).toEqual({
      people: [person],
      view: "relationship_map",
    });
    expect(Object.keys(relationshipMapStructuredContent([]))).toEqual([
      "people",
      "view",
    ]);
  });

  it("research projections omit unrelated structured facts from a matching meeting", () => {
    const meeting = {
      path: "/research/mixed-topics.md",
      body: "We mentioned pricing and then moved to hiring.",
      frontmatter: {
        title: "Mixed topic review",
        type: "meeting",
        date: "2026-07-20T12:00:00Z",
        duration: "30m",
        tags: ["pricing", "hiring"],
        attendees: [],
        attendees_raw: "",
        people: [],
        action_items: [],
        decisions: [
          { text: "Keep the current price", topic: "pricing" },
          { text: "Open a design role", topic: "hiring" },
        ],
        intents: [
          {
            kind: "commitment",
            what: "Model the pricing change",
            who: "Alex",
            status: "open",
          },
          {
            kind: "commitment",
            what: "Draft the hiring scorecard",
            who: "Case",
            status: "open",
          },
        ],
      },
    } as any;

    const research = researchTopicProjection([meeting], "pricing");
    expect(research.decisions).toHaveLength(1);
    expect(research.decisions[0]).toContain("current price");
    expect(research.openIntents.map((intent) => intent.what)).toEqual([
      "Model the pricing change",
    ]);
    expect(research.topics).toEqual([{ topic: "pricing", count: 1 }]);
    expect(research.text).not.toContain("design role");
    expect(research.text).not.toContain("hiring scorecard");
  });

  it("commitment projections deduplicate mirrors and derive overdue status", () => {
    const meeting = {
      path: "/commitments/source.md",
      body: "",
      frontmatter: {
        title: "Commitment review",
        type: "meeting",
        date: "2026-07-20T12:00:00Z",
        duration: "30m",
        tags: [],
        attendees: ["Alex"],
        attendees_raw: "",
        people: [],
        action_items: [
          {
            assignee: "Alex",
            task: "Send the pricing memo",
            due: "2026-07-19",
            status: "open",
          },
          {
            assignee: "Alex",
            task: "Review the forecast",
            due: "2026-07-30",
            status: "open",
          },
          {
            assignee: "Alex",
            task: "Finish the due-date item",
            due: "2026-07-21",
            status: "open",
          },
          {
            assignee: "Alex",
            task: "Finish the offset item",
            due: "2026-07-20T23:30:00-07:00",
            status: "open",
          },
        ],
        decisions: [],
        intents: [
          {
            kind: "commitment",
            what: "  send   the pricing memo ",
            who: "alex",
            by_date: "2026-07-19",
            status: "open",
          },
          {
            kind: "open-question",
            what: "Should hiring accelerate?",
            who: "Alex",
            status: "open",
          },
        ],
      },
    } as any;

    const commitments = policyCommitmentResults(
      [meeting],
      "Alex",
      MCP_INTENT_RESULT_MAX,
      Date.parse("2026-07-21T00:00:00Z")
    );
    expect(commitments).toHaveLength(4);
    expect(
      commitments.filter((item) =>
        item.what.toLowerCase().includes("pricing memo")
      )
    ).toHaveLength(1);
    expect(commitments[0].status).toBe("stale");
    expect(commitments.find((item) => item.what.includes("due-date"))?.status).toBe("open");
    expect(commitments.find((item) => item.what.includes("offset"))?.status).toBe("open");
    expect(commitments.some((item) => item.what.includes("hiring"))).toBe(false);
  });

  it("commitment owner selectors never substring-match another person", () => {
    const meeting = {
      path: "/commitments/overlap.md",
      body: "",
      frontmatter: {
        title: "Owner overlap",
        type: "meeting",
        date: "2026-07-20T12:00:00Z",
        duration: "10m",
        tags: [],
        attendees: ["Ann", "Joanna"],
        attendees_raw: "",
        people: [],
        decisions: [],
        intents: [],
        action_items: [
          { assignee: "Ann", task: "Send Ann item", status: "open" },
          { assignee: "Joanna", task: "Send Joanna item", status: "open" },
        ],
      },
    } as any;
    const commitments = policyCommitmentResults([meeting], "Ann");
    expect(commitments.map((item) => item.what)).toEqual(["Send Ann item"]);
  });

  it("fails closed on malformed or unknown sensitivity frontmatter", () => {
    const base = [
      "---",
      "title: Policy probe",
      "type: meeting",
      "date: 2026-07-15T10:00:00Z",
      "duration: 1m",
      "SENSITIVITY",
      "---",
      "",
      "canary",
    ].join("\n");
    expect(
      parsePolicyVerifiedMeeting(base.replace("SENSITIVITY", "sensitivity: normal"), "normal.md")
    ).not.toBeNull();
    expect(
      parsePolicyVerifiedMeeting(
        base.replace("SENSITIVITY", "sensitivity: restricted"),
        "restricted.md"
      )?.frontmatter.sensitivity
    ).toBe("restricted");
    expect(
      parsePolicyVerifiedMeeting(
        base.replace("SENSITIVITY", "sensitivity: confidential"),
        "unknown.md"
      )
    ).toBeNull();
    expect(parsePolicyVerifiedMeeting("no frontmatter canary", "bad.md")).toBeNull();
    for (const invalidRequiredField of [
      "title: Policy probe",
      "type: meeting",
      "date: 2026-07-15T10:00:00Z",
    ]) {
      expect(
        parsePolicyVerifiedMeeting(
          base.replace(`${invalidRequiredField}\n`, ""),
          "missing-required.md"
        )
      ).toBeNull();
    }
  });

  it("denies invalid UTF-8 policy bytes across exact, stable, research, and tool reads", async () => {
    const meetingsDir = mkdtempSync(join(tmpdir(), "minutes-invalid-utf8-policy-"));
    const invalidPath = join(meetingsDir, "invalid-utf8.md");
    const privateCanary = "INVALID-UTF8-MCP-PRIVATE-CANARY";
    const normalCanary = "INVALID-UTF8-MCP-NORMAL-CANARY";
    const invalidBytes = Buffer.from(
      [
        "---",
        "title: Invalid UTF-8 policy probe",
        "type: meeting",
        "date: 2026-07-15T10:00:00Z",
        "sensitivity: restricted",
        "---",
        "",
        privateCanary,
      ].join("\n")
    );
    const keyOffset = invalidBytes.indexOf(Buffer.from("sensitivity"));
    expect(keyOffset).toBeGreaterThanOrEqual(0);
    invalidBytes[keyOffset + 5] = 0xff;
    writeFileSync(invalidPath, invalidBytes);
    writeFileSync(
      join(meetingsDir, "normal.md"),
      [
        "---",
        "title: Normal policy probe",
        "type: meeting",
        "date: 2026-07-16T10:00:00Z",
        "sensitivity: normal",
        "---",
        "",
        normalCanary,
      ].join("\n")
    );

    const mcpServer = new McpServer({
      name: "minutes-invalid-utf8-policy",
      version: "0.0.0",
    });
    let researchProjectionExecutions = 0;
    registerToolWithRestrictedPolicy(
      mcpServer,
      "invalid_utf8_research",
      "Synthetic research boundary for invalid UTF-8 policy bytes",
      { query: z.string() },
      { readOnlyHint: true },
      async ({ query }) => {
        const meetings = await policyListMeetings(
          meetingsDir,
          MCP_POLICY_MEETING_RESULT_MAX,
          false
        );
        researchProjectionExecutions += 1;
        return {
          content: [
            {
              type: "text" as const,
              text: researchTopicProjection(meetings, query).text,
            },
          ],
        };
      }
    );

    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client(
      { name: "invalid-utf8-policy-client", version: "0.0.0" },
      { capabilities: {} }
    );
    try {
      expect(
        await policyVerifiedExactMeetingSnapshot(invalidPath, meetingsDir, true)
      ).toBeNull();

      const aggregateOutcomes = await Promise.allSettled([
        policyListMeetings(meetingsDir, 10, false),
        policySearchMeetings(meetingsDir, privateCanary, 10, false),
        policyListMeetings(meetingsDir, 10, true),
      ]);
      expect(
        aggregateOutcomes.every((outcome) => outcome.status === "rejected")
      ).toBe(true);
      const aggregateSerialized = JSON.stringify(aggregateOutcomes);
      expect(aggregateSerialized).not.toContain(privateCanary);
      expect(aggregateSerialized).not.toContain(normalCanary);
      expect(aggregateSerialized).not.toContain(invalidPath);

      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      const researchResult = await client.callTool({
        name: "invalid_utf8_research",
        arguments: { query: privateCanary },
      });
      expect(researchResult.isError).toBe(true);
      expect(researchProjectionExecutions).toBe(0);
      const toolSerialized = JSON.stringify(researchResult);
      expect(toolSerialized).not.toContain(privateCanary);
      expect(toolSerialized).not.toContain(normalCanary);
      expect(toolSerialized).not.toContain(invalidPath);
    } finally {
      await client.close().catch(() => {});
      await mcpServer.close().catch(() => {});
      rmSync(meetingsDir, { recursive: true, force: true });
    }
  });

  it("re-verifies installed-SDK list and search candidates from live files", async () => {
    const meetingsDir = mkdtempSync(join(tmpdir(), "minutes-installed-sdk-policy-"));
    const meeting = (title: string, sensitivity: string, body: string) =>
      [
        "---",
        `title: ${title}`,
        "type: meeting",
        "date: 2026-07-15T10:00:00Z",
        "duration: 1m",
        ...(sensitivity ? [`sensitivity: ${sensitivity}`] : []),
        "tags: []",
        "attendees: []",
        "people: []",
        "action_items: []",
        "decisions: []",
        "intents: []",
        "---",
        "",
        body,
      ].join("\n");
    writeFileSync(join(meetingsDir, "normal.md"), meeting("Normal", "", "shared canary"));
    writeFileSync(
      join(meetingsDir, "restricted.md"),
      meeting("Restricted", "restricted", "restricted shared canary")
    );
    writeFileSync(
      join(meetingsDir, "unknown.md"),
      meeting("Unknown", "confidential", "UNKNOWN_POLICY_CANARY shared canary")
    );

    try {
      expect(
        (await policyListMeetings(meetingsDir, 10, false)).map(
          (item) => item.frontmatter.title
        )
      ).toEqual(["Normal"]);
      expect(
        (await policySearchMeetings(meetingsDir, "shared canary", 10, false)).map(
          (item) => item.frontmatter.title
        )
      ).toEqual(["Normal"]);
      expect(
        (await policyListMeetings(meetingsDir, 10, true))
          .map((item) => item.frontmatter.title)
          .sort()
      ).toEqual(["Normal", "Restricted"]);
    } finally {
      rmSync(meetingsDir, { recursive: true, force: true });
    }
  });

  it("orders list, search, type-filter, and research projections by normalized date descending", async () => {
    const meetingsDir = mkdtempSync(join(tmpdir(), "minutes-policy-recency-"));
    const meeting = (
      title: string,
      type: "meeting" | "memo",
      date: string,
      body: string
    ) =>
      [
        "---",
        `title: ${title}`,
        `type: ${type}`,
        `date: ${date}`,
        "sensitivity: normal",
        "tags: []",
        "attendees: []",
        "people: []",
        "action_items: []",
        "decisions: []",
        "intents: []",
        "---",
        "",
        body,
      ].join("\n");
    writeFileSync(
      join(meetingsDir, "a-old.md"),
      meeting("Old meeting", "meeting", "2024-01-01T09:00:00-08:00", "shared research topic")
    );
    writeFileSync(
      join(meetingsDir, "b-newest-memo.md"),
      meeting("Newest memo", "memo", "2026-05-01T18:00:00Z", "shared research topic")
    );
    writeFileSync(
      join(meetingsDir, "z-new-meeting.md"),
      meeting("New meeting", "meeting", "2026-04-30T20:00:00-07:00", "shared research topic")
    );
    writeFileSync(
      join(meetingsDir, "c-tie-a.md"),
      meeting("Tie A", "memo", "2025-06-01T12:00:00Z", "shared research topic")
    );
    writeFileSync(
      join(meetingsDir, "d-tie-b.md"),
      meeting("Tie B", "memo", "2025-06-01T12:00:00Z", "shared research topic")
    );

    try {
      expect(
        (await policyListMeetings(meetingsDir, 2, false)).map(
          (item) => item.frontmatter.title
        )
      ).toEqual(["Newest memo", "New meeting"]);
      expect(
        (await policySearchMeetings(meetingsDir, "shared research", 2, false)).map(
          (item) => item.frontmatter.title
        )
      ).toEqual(["Newest memo", "New meeting"]);

      const ordered = await policyListMeetings(meetingsDir, 100, false);
      expect(ordered.slice(2, 4).map((item) => item.frontmatter.title)).toEqual([
        "Tie A",
        "Tie B",
      ]);
      expect(
        ordered
          .filter((item) => item.frontmatter.type === "meeting")
          .slice(0, 1)
          .map((item) => item.frontmatter.title)
      ).toEqual(["New meeting"]);
      expect(
        ordered
          .filter((item) => item.body.includes("shared research topic"))
          .slice(0, 2)
          .map((item) => item.frontmatter.title)
      ).toEqual(["Newest memo", "New meeting"]);
    } finally {
      rmSync(meetingsDir, { recursive: true, force: true });
    }
  });

  it("rejects inactive corpus components again at the MCP boundary", async () => {
    const meetingsDir = mkdtempSync(join(tmpdir(), "minutes-inactive-policy-"));
    const inactivePaths = [
      join(meetingsDir, "Archive", "private.md"),
      join(meetingsDir, ".git", "private.md"),
      join(meetingsDir, "nested", ".private", "private.md"),
    ];
    for (const [index, privatePath] of inactivePaths.entries()) {
      mkdirSync(join(privatePath, ".."), { recursive: true });
      writeFileSync(
        privatePath,
        [
          "---",
          `title: INACTIVE-MCP-CANARY-${index}`,
          "type: meeting",
          "date: 2026-07-15T10:00:00Z",
          "---",
          "",
          `INACTIVE-MCP-CANARY-${index}`,
        ].join("\n")
      );
    }
    try {
      for (const privatePath of inactivePaths) {
        expect(isActiveCorpusMeetingPath(privatePath, meetingsDir)).toBe(false);
      }
      expect(await policyListMeetings(meetingsDir, 100, true)).toEqual([]);
      for (const privatePath of inactivePaths) {
        expect(
          await enrichWithFrontmatter(
            [{ source_path: privatePath, snippet: "INACTIVE-MCP-CANARY" }],
            true,
            meetingsDir
          )
        ).toEqual([]);
      }
    } finally {
      rmSync(meetingsDir, { recursive: true, force: true });
    }
  });

  it("binds exact meeting reads to the active corpus root", async () => {
    const meetingsDir = mkdtempSync(join(tmpdir(), "minutes-exact-policy-"));
    const outsideDir = mkdtempSync(join(tmpdir(), "minutes-exact-outside-"));
    const meeting = (title: string, sensitivity = "") =>
      [
        "---",
        `title: ${title}`,
        "type: meeting",
        "date: 2026-07-15T10:00:00Z",
        ...(sensitivity ? [`sensitivity: ${sensitivity}`] : []),
        "---",
        "",
        `${title} body`,
      ].join("\n");
    const normalPath = join(meetingsDir, "normal.md");
    const restrictedPath = join(meetingsDir, "restricted.md");
    const inactivePath = join(meetingsDir, "Archive", "inactive.md");
    const outsidePath = join(outsideDir, "outside.md");
    writeFileSync(normalPath, meeting("Normal exact"));
    writeFileSync(restrictedPath, meeting("Restricted exact", "restricted"));
    mkdirSync(join(meetingsDir, "Archive"));
    writeFileSync(inactivePath, meeting("Inactive exact"));
    writeFileSync(outsidePath, meeting("Outside exact"));

    try {
      expect(
        (
          await policyVerifiedExactMeetingSnapshot(
            normalPath,
            meetingsDir,
            false
          )
        )?.meeting.frontmatter.title
      ).toBe("Normal exact");
      expect(
        await policyVerifiedExactMeetingSnapshot(
          restrictedPath,
          meetingsDir,
          false
        )
      ).toBeNull();
      expect(
        await policyVerifiedExactMeetingSnapshot(
          inactivePath,
          meetingsDir,
          true
        )
      ).toBeNull();
      expect(
        await policyVerifiedExactMeetingSnapshot(
          outsidePath,
          meetingsDir,
          true
        )
      ).toBeNull();
    } finally {
      rmSync(meetingsDir, { recursive: true, force: true });
      rmSync(outsideDir, { recursive: true, force: true });
    }
  });

  it("retries a persistent A-to-restricted flip without dropping stable B", async () => {
    const meetingsDir = mkdtempSync(join(tmpdir(), "minutes-post-snapshot-policy-"));
    const path = join(meetingsDir, "a.md");
    const meeting = (title: string, sensitivity: string, body: string) =>
      [
        "---",
        `title: ${title}`,
        "type: meeting",
        "date: 2026-07-15T10:00:00Z",
        ...(sensitivity ? [`sensitivity: ${sensitivity}`] : []),
        "---",
        "",
        body,
      ].join("\n");
    writeFileSync(path, meeting("A private", "", "POST-SNAPSHOT-PRIVATE-CANARY"));
    writeFileSync(
      join(meetingsDir, "b.md"),
      meeting("B stable", "", "POST-SNAPSHOT-STABLE-CANARY")
    );
    try {
      let flipped = false;
      const result = await policyListMeetings(meetingsDir, 10, false, () => {
        if (flipped) return;
        flipped = true;
        writeFileSync(
          path,
          meeting("A private", "restricted", "POST-SNAPSHOT-PRIVATE-CANARY")
        );
      });
      expect(result.map((item) => item.frontmatter.title)).toEqual(["B stable"]);
      expect(JSON.stringify(result)).not.toContain("POST-SNAPSHOT-PRIVATE-CANARY");
    } finally {
      rmSync(meetingsDir, { recursive: true, force: true });
    }
  });

  it("rejects an exact-byte ABA transition instead of trusting restored current state", async () => {
    const meetingsDir = mkdtempSync(join(tmpdir(), "minutes-policy-aba-"));
    const path = join(meetingsDir, "mutable.md");
    const normal = [
      "---",
      "title: ABA private",
      "type: meeting",
      "date: 2026-07-15T10:00:00Z",
      "---",
      "",
      "EXACT-ABA-PRIVATE-CANARY",
    ].join("\n");
    const restricted = normal.replace("date: 2026", "sensitivity: restricted\ndate: 2026");
    writeFileSync(path, normal);
    const initial = statSync(path);

    try {
      await expect(
        policySearchMeetings(meetingsDir, "EXACT-ABA", 10, false, () => {}, {
          beforeFinalManifest: () => {
            writeFileSync(path, restricted);
            writeFileSync(path, normal);
            utimesSync(path, initial.atime, initial.mtime);
          },
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
    } finally {
      rmSync(meetingsDir, { recursive: true, force: true });
    }
  });

  it("fails closed on watcher failure and sentinel timeout", async () => {
    const meetingsDir = mkdtempSync(join(tmpdir(), "minutes-policy-watcher-"));
    writeFileSync(
      join(meetingsDir, "normal.md"),
      "---\ntitle: Watcher\ntype: meeting\ndate: 2026-07-15T10:00:00Z\n---\nwatcher canary"
    );

    try {
      await expect(
        policyListMeetings(meetingsDir, 10, false, () => {}, {
          onWatcherReady: ({ controls }) => controls.failWatcher("test failure"),
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
      await expect(
        policyListMeetings(meetingsDir, 10, false, () => {}, {
          timeoutMs: 25,
          onWatcherReady: ({ controls }) => controls.suppressNextFence(),
        })
      ).rejects.toThrow("stable meeting corpus authorization failed");
    } finally {
      rmSync(meetingsDir, { recursive: true, force: true });
    }
  });
});

describe("QMD sensitivity verification", () => {
  it("drops restricted, malformed, unreadable, and out-of-root index hits", async () => {
    const meetingsDir = mkdtempSync(join(tmpdir(), "minutes-qmd-meetings-"));
    const outsideDir = mkdtempSync(join(tmpdir(), "minutes-qmd-outside-"));
    const normalPath = join(meetingsDir, "normal.md");
    const nestedDir = join(meetingsDir, "memos");
    const nestedNormalPath = join(nestedDir, "nested-normal.md");
    const restrictedPath = join(meetingsDir, "restricted.md");
    const unknownPath = join(meetingsDir, "unknown.md");
    const malformedPath = join(meetingsDir, "malformed-yaml.md");
    const outsidePath = join(outsideDir, "outside.md");
    const symlinkPath = join(meetingsDir, "outside-link.md");
    const meeting = (title: string, sensitivity?: string) =>
      [
        "---",
        `title: ${title}`,
        "type: meeting",
        "date: 2026-07-15T10:00:00Z",
        "duration: 10m",
        ...(sensitivity ? [`sensitivity: ${sensitivity}`] : []),
        "tags: []",
        "attendees: []",
        "people: []",
        "action_items: []",
        "decisions: []",
        "intents: []",
        "---",
        "",
        `## Transcript\\n\\n${title} canary`,
      ].join("\n");

    mkdirSync(nestedDir);
    writeFileSync(normalPath, meeting("Normal"));
    writeFileSync(nestedNormalPath, meeting("Nested Normal"));
    writeFileSync(restrictedPath, meeting("Restricted", "restricted"));
    writeFileSync(unknownPath, meeting("Unknown", "confidential"));
    writeFileSync(
      malformedPath,
      "---\ntitle: Broken\nsensitivity: [unterminated\n---\nMALFORMED_YAML_CANARY"
    );
    writeFileSync(outsidePath, meeting("Outside"));
    symlinkSync(outsidePath, symlinkPath);

    try {
      const canonicalMeetingsDir = realpathSync(meetingsDir);
      const hits = [
        { source_path: realpathSync(normalPath), snippet: "poisoned stale index canary" },
        {
          source_path: realpathSync(nestedNormalPath),
          snippet: "poisoned nested stale index canary",
        },
        { source_path: realpathSync(restrictedPath), snippet: "restricted canary" },
        { source_path: realpathSync(unknownPath), snippet: "unknown canary" },
        { source_path: realpathSync(malformedPath), snippet: "malformed canary" },
        { source_path: realpathSync(outsidePath), snippet: "outside canary" },
        { source_path: symlinkPath, snippet: "symlink canary" },
        {
          source_path: join(canonicalMeetingsDir, "missing.md"),
          snippet: "missing canary",
        },
      ];
      const filtered = await enrichWithFrontmatter(
        hits,
        false,
        canonicalMeetingsDir
      );
      expect(filtered).toHaveLength(2);
      expect(filtered).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            title: "Normal",
            path: realpathSync(normalPath),
          }),
          expect.objectContaining({
            title: "Nested Normal",
            path: realpathSync(nestedNormalPath),
          }),
        ])
      );
      expect(filtered.map((hit) => hit.snippet).join("\n")).toContain(
        "Nested Normal canary"
      );
      expect(filtered.map((hit) => hit.snippet).join("\n")).not.toContain(
        "poisoned"
      );
      expect(JSON.stringify(filtered)).not.toMatch(
        /restricted|unknown|malformed|outside|symlink|missing canary/i
      );

      const standaloneOverride = await enrichWithFrontmatter(
        hits,
        true,
        canonicalMeetingsDir
      );
      expect(standaloneOverride.map((hit) => hit.title).sort()).toEqual([
        "Nested Normal",
        "Normal",
        "Restricted",
      ]);
    } finally {
      rmSync(meetingsDir, { recursive: true, force: true });
      rmSync(outsideDir, { recursive: true, force: true });
    }
  });

  it("derives snippets only from the verified live body", () => {
    expect(
      liveMeetingSnippet("prefix words unique target and safe suffix", "unique target")
    ).toContain("unique target");
    expect(liveMeetingSnippet("  live\n body  ")).toBe("live body");
  });
});

describe("meeting insight contract", () => {
  it("exports only the insight kinds the pipeline emits today", () => {
    expect(MEETING_INSIGHT_KINDS).toEqual(["decision", "commitment", "question"]);
  });
});

describe("meeting shape contract", () => {
  const meeting = {
    path: "/tmp/meeting.md",
    frontmatter: {
      date: "2026-05-05T10:00:00-07:00",
      title: "Capture Health Review",
      type: "meeting",
      duration: "12m",
      recording_health: {
        capture_warnings: [
          {
            kind: "silent",
            source: "system",
            message: "System audio was silent.",
            diagnostic_confidence: "inferred",
          },
        ],
        diarization_path: "ml-bleed-degraded",
      },
    },
  };

  it("omits recording_health from list and search results", () => {
    expect(meetingListItem(meeting)).toEqual({
      date: "2026-05-05T10:00:00-07:00",
      title: "Capture Health Review",
      content_type: "meeting",
      path: "/tmp/meeting.md",
      duration: "12m",
    });
    expect(meetingSearchItem(meeting)).toEqual({
      date: "2026-05-05T10:00:00-07:00",
      title: "Capture Health Review",
      content_type: "meeting",
      path: "/tmp/meeting.md",
    });
  });

  it("bounds every list/search field before structured output", () => {
    const oversized = "x".repeat(10_000);
    const boundedList = meetingListItem({
      path: oversized,
      frontmatter: {
        date: oversized,
        title: oversized,
        type: oversized,
        duration: oversized,
      },
    });
    const boundedSearch = meetingSearchItem({
      path: oversized,
      frontmatter: { date: oversized, title: oversized, type: oversized },
    });
    for (const value of [
      ...Object.values(boundedList),
      ...Object.values(boundedSearch),
    ]) {
      expect(value?.length).toBeLessThanOrEqual(2_048);
    }
  });

  it("surfaces recording_health in detail payloads", () => {
    expect(
      meetingDetailPayload({
        path: meeting.path,
        speaker_map: [],
        recording_health: meeting.frontmatter.recording_health,
        overlay_applied: false,
      })
    ).toEqual({
      path: "/tmp/meeting.md",
      view: "detail",
      speaker_map: [],
      recording_health: meeting.frontmatter.recording_health,
      overlay_applied: false,
    });
  });

  it("surfaces the transcript body and synthesis fields in detail payloads (issue #255)", () => {
    const actionItems = [{ assignee: "Mat", task: "Ship fix", status: "open" }];
    const decisions = [{ text: "Enrich structuredContent" }];
    const intents = [{ kind: "commitment", what: "Reply to contributor", status: "open" }];

    const payload = meetingDetailPayload({
      path: meeting.path,
      speaker_map: [],
      overlay_applied: false,
      title: "Native Call",
      summary: "We agreed to fix get_meeting.",
      action_items: actionItems,
      decisions,
      intents,
      body: "## Summary\n\nWe agreed to fix get_meeting.\n\n## Transcript\n\n[00:00] Hello.",
    });

    expect(payload).toMatchObject({
      path: "/tmp/meeting.md",
      view: "detail",
      title: "Native Call",
      summary: "We agreed to fix get_meeting.",
      action_items: actionItems,
      decisions,
      intents,
    });
    expect(payload.body).toContain("## Transcript");
  });

  it("omits synthesis fields entirely when not provided", () => {
    expect(meetingDetailPayload({ path: meeting.path })).toEqual({
      path: "/tmp/meeting.md",
      view: "detail",
    });
  });

  it("accepts CLI overlays only with an exact source-bound proof", () => {
    const source = "---\ntitle: Safe\n---\nSPEAKER_0: hello\n";
    const exact = verifiedCliSpeakerOverlay(
      {
        overlay_applied: true,
        overlay_source_sha256: createHash("sha256").update(source).digest("hex"),
        raw_markdown: source,
        frontmatter: {
          speaker_map: [
            {
              speaker_label: "SPEAKER_0",
              name: "Alex",
              confidence: "high",
              source: "manual",
            },
          ],
        },
      },
      source
    );
    expect(exact?.overlay_applied).toBe(true);
    expect(exact?.speaker_map).toHaveLength(1);

    for (const stale of [
      { overlay_source_sha256: "0".repeat(64) },
      { raw_markdown: source.replace("hello", "replacement") },
      { overlay_applied: false },
    ]) {
      expect(
        verifiedCliSpeakerOverlay(
          {
            overlay_applied: true,
            overlay_source_sha256: createHash("sha256").update(source).digest("hex"),
            raw_markdown: source,
            frontmatter: { speaker_map: [{ name: "STALE-PRIVATE-CANARY" }] },
            ...stale,
          },
          source
        )
      ).toBeNull();
    }
  });
});

describe("extractMarkdownSection", () => {
  const body = [
    "## Summary",
    "",
    "First synthesized line.",
    "Second synthesized line.",
    "",
    "## Decisions",
    "",
    "- Ship the fix.",
    "",
    "## Transcript",
    "",
    "[00:00] Hello.",
  ].join("\n");

  it("returns a section's text up to the next heading", () => {
    expect(extractMarkdownSection(body, "Summary")).toBe(
      "First synthesized line.\nSecond synthesized line."
    );
  });

  it("returns undefined for an absent section", () => {
    expect(extractMarkdownSection(body, "Commitments")).toBeUndefined();
  });

  it("returns undefined for empty or missing input", () => {
    expect(extractMarkdownSection(undefined, "Summary")).toBeUndefined();
    expect(extractMarkdownSection("## Summary\n\n", "Summary")).toBeUndefined();
  });
});

describe("verified stop recording responses", () => {
  it("materializes rich output only from the authorized meeting snapshot", () => {
    const summary = verifiedStopRecordingSummary({
      path: "/safe/meetings/authorized.md",
      meeting: {
        body: "## Transcript\n\nAuthorized words only.",
        frontmatter: {
          title: "Authorized title",
          duration: "12m",
          people: ["Alex"],
          action_items: [
            { task: "Ship safely", assignee: "Avery", status: "open" },
          ],
          decisions: [{ text: "Keep the boundary fail-closed" }],
        },
      },
    });

    expect(summary).toContain("Authorized title");
    expect(summary).toContain("/safe/meetings/authorized.md");
    expect(summary).toContain("Ship safely");
    expect(summary).not.toContain("CLI-PRIVATE-CANARY");
    expect(summary).not.toContain("job-private-canary");
  });
});

describe("parseKnowledgeConfig", () => {
  it("only treats enabled=true inside the knowledge section as enabling the knowledge base", () => {
    const parsed = parseKnowledgeConfig(`
[recording]
enabled = true

[knowledge]
enabled = false
path = "~/kb"
`);

    expect(parsed).toEqual({
      enabled: false,
      path: "~/kb",
      adapter: "wiki",
      engine: "none",
    });
  });

  it("reads knowledge settings from the knowledge section", () => {
    const parsed = parseKnowledgeConfig(`
[knowledge]
enabled = true
path = "~/kb"
adapter = "para"
engine = "agent"
`);

    expect(parsed).toEqual({
      enabled: true,
      path: "~/kb",
      adapter: "para",
      engine: "agent",
    });
  });
});

describe("atomic Rust knowledge status bridge", () => {
  it("returns only the Rust-owned snapshot from one command", async () => {
    const calls: string[][] = [];
    await expect(
      readKnowledgeStatusSnapshot(async (args) => {
        calls.push(args);
        return {
          stdout: '{"enabled":true,"configured":true,"adapter":"wiki","engine":"none","people_count":2,"log_entries":3}',
          stderr: "",
        };
      })
    ).resolves.toMatchObject({ people_count: 2, log_entries: 3 });
    expect(calls).toEqual([["knowledge-status", "--json"]]);
  });

  it("fails closed on malformed, negative, or failed bridge responses", async () => {
    for (const stdout of [
      "",
      "not-json",
      '{"enabled":true}',
      '{"enabled":true,"configured":true,"adapter":"wiki","engine":"none","people_count":-1,"log_entries":0}',
      "null",
    ]) {
      await expect(
        readKnowledgeStatusSnapshot(async () => ({
          stdout,
          stderr: "PRIVATE-DERIVATIVE-CANARY",
        }))
      ).rejects.toThrow(/could not be safely read/i);
    }

    await expect(
      readKnowledgeStatusSnapshot(async () => {
        throw new Error("bridge failed");
      })
    ).rejects.toThrow("bridge failed");
  });
});

describe("agent trust readiness bridge", () => {
  it("authorizes and audits restricted input before any mutating readiness", async () => {
    const deniedOrder: string[] = [];
    expect(() =>
      runAgentToolPolicies(
        "search_meetings",
        { include_restricted: true },
        () => deniedOrder.push("handler"),
        async () => deniedOrder.push("readiness"),
        "deny",
        () => deniedOrder.push("audit")
      )
    ).toThrow("Restricted meeting content is unavailable");
    expect(deniedOrder).toEqual([]);

    const overrideOrder: string[] = [];
    await expect(
      runAgentToolPolicies(
        "search_meetings",
        { include_restricted: true, query: "PRIVATE_ORDER_CANARY" },
        () => {
          overrideOrder.push("handler");
          return "authorized";
        },
        async () => {
          overrideOrder.push("readiness");
        },
        "logged-override",
        (_path, line) => {
          expect(line).not.toContain("PRIVATE_ORDER_CANARY");
          overrideOrder.push("audit");
        }
      )
    ).resolves.toBe("authorized");
    expect(overrideOrder).toEqual(["audit", "readiness", "handler"]);
  });

  it("probes the required CLI before connecting without globally gating controls on QMD", async () => {
    const order: string[] = [];
    const result = await afterRequiredCli(
      async () => {
        order.push("connect");
        return "connected";
      },
      async () => {
        order.push("cli");
        return true;
      }
    );

    expect(result).toBe("connected");
    expect(order).toEqual(["cli", "connect"]);
  });

  it("fails path-free before connect when the required CLI is unavailable", async () => {
    let connected = false;
    const error = await afterRequiredCli(
      async () => {
        connected = true;
      },
      async () => false
    ).catch((failure: unknown) => failure);

    expect(connected).toBe(false);
    expect(error).toBeInstanceOf(Error);
    expect((error as Error).message).toBe(
      "Minutes CLI is required to establish the agent trust boundary."
    );
  });

  it("rechecks readiness for every content read after a runtime registry change", async () => {
    let ready = true;
    let readinessChecks = 0;
    let reads = 0;
    const readiness = async () => {
      readinessChecks += 1;
      if (!ready) throw new Error("registry changed after startup");
    };

    await expect(
      afterContentBearingToolReadiness(
        "search_meetings",
        async () => {
          reads += 1;
          return "first read";
        },
        readiness
      )
    ).resolves.toBe("first read");

    ready = false;
    await expect(
      afterContentBearingToolReadiness(
        "search_meetings",
        async () => {
          reads += 1;
          return "stale authorization read";
        },
        readiness
      )
    ).rejects.toThrow("registry changed after startup");

    expect(readinessChecks).toBe(2);
    expect(reads).toBe(1);
  });

  it("rechecks readiness before every content-bearing resource read", async () => {
    let ready = true;
    let readinessChecks = 0;
    let reads = 0;
    const readiness = async () => {
      readinessChecks += 1;
      if (!ready) throw new Error("resource registry changed after connection");
    };

    await expect(
      afterContentResourceReadiness("recent_meetings", async () => {
        reads += 1;
        return "first resource";
      }, readiness)
    ).resolves.toBe("first resource");

    ready = false;
    await expect(
      afterContentResourceReadiness("recent_meetings", async () => {
        reads += 1;
        return "PRIVATE-RESOURCE-CANARY";
      }, readiness)
    ).rejects.toThrow("resource registry changed after connection");
    expect(readinessChecks).toBe(2);
    expect(reads).toBe(1);
  });

  it("enumerates every registered content-bearing resource behind the per-read gate", () => {
    expect(contentBearingAgentResourceNames()).toEqual(
      [
        "live_copilot",
        "live_events",
        "live_events_since_seq",
        "meeting",
        "open_actions",
        "recent-ideas",
        "recent_meetings",
      ].sort()
    );
  });

  it("does not gate non-content mutation tools on agent-read readiness", async () => {
    let readinessChecks = 0;
    await expect(
      afterContentBearingToolReadiness(
        "add_note",
        async () => "mutated",
        async () => {
          readinessChecks += 1;
          throw new Error("must not run");
        }
      )
    ).resolves.toBe("mutated");
    expect(readinessChecks).toBe(0);
  });

  it("exposes add_note only for the active recording", () => {
    expect(Object.keys(MCP_ADD_NOTE_INPUT_SCHEMA)).toEqual(["text"]);
    expect(MCP_ADD_NOTE_INPUT_SCHEMA).not.toHaveProperty("meeting_path");
  });

  const serverModuleSource = readFileSync(
    new URL("./index.ts", import.meta.url),
    "utf8"
  );

  it("classifies every registered tool as content-bearing or explicitly not", async () => {
    // Derived from the live registry rather than restated. The previous shape
    // froze a literal list, so a newly content-bearing tool that was never
    // added to the gate still passed. Anything unclassified fails here, which
    // makes forgetting the gate a test failure instead of a silent hole.
    const mcpServer = new McpServer({
      name: "minutes-tool-classification-test",
      version: "0.0.0",
    });
    registerUnavailableCompatibilityTools(mcpServer);
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client(
      { name: "tool-classification-client", version: "0.0.0" },
      { capabilities: {} }
    );
    let registered: string[];
    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      registered = (await client.listTools()).tools.map((tool) => tool.name);
    } finally {
      await client.close();
      await mcpServer.close();
    }

    // These two are the derived-record surface this suite registers. Both must
    // be classified: insights returns meeting content and is gated;
    // annotations is an unavailable stub that returns none.
    expect(registered.sort()).toEqual(["get_agent_annotations", "get_meeting_insights"]);
    expect(contentBearingAgentToolNames()).toContain("get_meeting_insights");
    expect(contentBearingAgentToolNames()).not.toContain("get_agent_annotations");

    // The full gate set must stay free of duplicates and of names that are no
    // longer registered anywhere in the server module.
    const gated = contentBearingAgentToolNames();
    expect(new Set(gated).size).toBe(gated.length);
    for (const name of gated) {
      expect(
        serverModuleSource.includes(`"${name}"`),
        `${name} is gated but no longer registered`
      ).toBe(true);
    }
  });

  it("allows content-free inactive copilot status without QMD readiness", async () => {
    let readinessChecks = 0;
    let reads = 0;
    await expect(
      afterActiveCopilotReadiness(
        { active: false },
        async () => {
          reads += 1;
          return "Copilot is not active (Off).";
        },
        async () => {
          readinessChecks += 1;
          throw new Error("blocked QMD retirement");
        }
      )
    ).resolves.toBe("Copilot is not active (Off).");
    expect(readinessChecks).toBe(0);
    expect(reads).toBe(1);
  });

  it("blocks active copilot content before reading the observation stream", async () => {
    let reads = 0;
    await expect(
      afterActiveCopilotReadiness(
        { active: true },
        async () => {
          reads += 1;
          return "PRIVATE-COPILOT-NUDGE-CANARY";
        },
        async () => {
          throw new Error("blocked QMD retirement");
        }
      )
    ).rejects.toThrow("blocked QMD retirement");
    expect(reads).toBe(0);
  });

  it("runs terminal controls before blocked readiness and withholds their result", async () => {
    const order: string[] = [];
    const stopped = await terminalControlBeforeContentReadiness(
      async () => {
        order.push("stop");
        return "PRIVATE-STOP-RESULT-CANARY";
      },
      async () => {
        order.push("readiness");
        throw new Error("blocked");
      }
    );
    expect(order).toEqual(["stop", "readiness"]);
    expect(stopped.mayRevealContent).toBe(false);
    const response = stopped.mayRevealContent
      ? stopped.result
      : "stopped, result withheld";
    expect(response).toBe("stopped, result withheld");
    expect(response).not.toContain("PRIVATE-STOP-RESULT-CANARY");
  });

  it("admits only a clean external registry before MCP connection", async () => {
      const qmdRetirement = "ready-clean" as const;
      const calls: string[][] = [];
      await expect(
        requireAgentTrustReadiness(async (args) => {
          calls.push(args);
          return {
            stdout: JSON.stringify({
              schema: 1,
              ready: true,
              qmd_retirement: qmdRetirement,
            }),
            stderr: "",
          };
        })
      ).resolves.toMatchObject({ qmd_retirement: qmdRetirement });
      expect(calls).toEqual([["agent-readiness", "--json"]]);
  });

  it("blocks MCP readiness with the path-free remediation returned by Rust", async () => {
    await expect(
      requireAgentTrustReadiness(async () => ({
        stdout: JSON.stringify({
          schema: 1,
          ready: false,
          qmd_retirement: "blocked",
          remediation:
            "Run minutes qmd cleanup, then restart Minutes before using Recall or agent features.",
        }),
        stderr: "PRIVATE-DERIVATIVE-CANARY",
      }))
    ).rejects.toThrow(/run minutes qmd cleanup/i);
  });

  it("turns an old engine missing agent-readiness into actionable upgrade guidance", async () => {
    const calls: string[][] = [];
    const error = await readAgentTrustReadiness(async (args) => {
      calls.push(args);
      if (args[0] === "--version") {
        return { stdout: "minutes 0.24.0", stderr: "" };
      }
      const failure = new Error(
        "error: unrecognized subcommand 'agent-readiness'\nPRIVATE-PATH-CANARY"
      );
      (failure as any).stderr = failure.message;
      throw failure;
    }, "win32").catch((failure: unknown) => failure);

    expect(calls).toEqual([
      ["agent-readiness", "--json"],
      ["--version"],
    ]);
    expect(error).toBeInstanceOf(Error);
    expect((error as Error).message).toContain("found v0.24.0");
    expect((error as Error).message).toContain("needs v0.25.0 or newer");
    expect((error as Error).message).toContain("Minutes desktop app");
    expect((error as Error).message).not.toContain("PRIVATE");
  });

  it("keeps unexpected readiness failures opaque even when version probing would be possible", async () => {
    const calls: string[][] = [];
    const error = await readAgentTrustReadiness(async (args) => {
      calls.push(args);
      throw new Error("PRIVATE-CONFIG-CANARY");
    }).catch((failure: unknown) => failure);

    expect(calls).toEqual([["agent-readiness", "--json"]]);
    expect((error as Error).message).toBe(
      "Minutes agent readiness could not be verified safely."
    );
  });

  it("does not mislabel a current custom engine as old", async () => {
    const error = await readAgentTrustReadiness(async (args) => {
      if (args[0] === "--version") {
        return { stdout: "minutes 0.25.0", stderr: "" };
      }
      const failure = new Error("error: unknown subcommand 'agent-readiness'");
      (failure as any).stderr = failure.message;
      throw failure;
    }).catch((failure: unknown) => failure);

    expect((error as Error).message).toBe(
      "Minutes agent readiness could not be verified safely."
    );
  });

  it("fails closed on malformed or inconsistent readiness responses", async () => {
    for (const stdout of [
      "",
      "not-json",
      '{"schema":2,"ready":true,"qmd_retirement":"ready-clean","remediation":null}',
      '{"schema":1,"ready":false,"qmd_retirement":"ready-clean","remediation":null}',
      '{"schema":1,"ready":true,"qmd_retirement":"blocked","remediation":"retry"}',
      '{"schema":1,"ready":false,"qmd_retirement":"blocked","remediation":null}',
      '{"schema":1,"ready":true,"qmd_retirement":"ready-clean","remediation":"unexpected"}',
      '{"schema":1,"ready":true,"qmd_retirement":"ready-deferred-no-execution"}',
    ]) {
      await expect(
        readAgentTrustReadiness(async () => ({
          stdout,
          stderr: "PRIVATE-DERIVATIVE-CANARY",
        }))
      ).rejects.toThrow(/could not be verified safely/i);
    }
  });

  it("redacts rejected CLI failures and never connects the MCP transport", async () => {
    let connected = false;
    const error = await afterAgentTrustReadiness(
      async () => {
        connected = true;
      },
      async () => {
        throw new Error(
          "exec /PRIVATE/HOME/minutes failed: PRIVATE-CONFIG-CANARY"
        );
      }
    ).catch((failure: unknown) => failure);

    expect(connected).toBe(false);
    expect(error).toBeInstanceOf(Error);
    expect((error as Error).message).toBe(
      "Minutes agent readiness could not be verified safely."
    );
    expect((error as Error).message).not.toContain("PRIVATE");
  });
});

describe("strict live meeting root bridge", () => {
  it("uses an explicit environment override without invoking the CLI", async () => {
    const root = mkdtempSync(join(tmpdir(), "minutes-explicit-root-"));
    let invoked = false;
    try {
      await expect(
        getEffectiveMeetingsDir(
          async () => {
            invoked = true;
            throw new Error("must not run");
          },
          async () => {
            invoked = true;
            return true;
          },
          root
        )
      ).resolves.toBe(realpathSync(root));
      expect(invoked).toBe(false);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("strictly parses one exact schema and fails closed on bridge errors", async () => {
    expect(
      parseMeetingsRootSnapshot(
        JSON.stringify({ schema_version: 1, output_dir: "/tmp/meetings" })
      )
    ).toBe(resolve("/tmp/meetings"));
    for (const stdout of [
      "not-json PRIVATE-ROOT-CANARY",
      JSON.stringify({ output_dir: "/tmp/meetings" }),
      JSON.stringify({ schema_version: 1, output_dir: "" }),
      JSON.stringify({ schema_version: 1, output_dir: "/tmp/meetings", extra: true }),
    ]) {
      expect(() => parseMeetingsRootSnapshot(stdout)).toThrow(
        "The live meeting root could not be safely resolved."
      );
    }
    await expect(
      getEffectiveMeetingsDir(
        async () => {
          throw new Error("PRIVATE-ROOT-CANARY");
        },
        async () => true,
        undefined
      )
    ).rejects.toThrow("The live meeting root could not be safely resolved.");
  });

  it("resolves every operation anew after a runtime config-root flip", async () => {
    const roots = [
      resolve("/tmp/meetings-one"),
      resolve("/tmp/meetings-two"),
    ];
    let call = 0;
    const runner = async () => ({
      stdout: JSON.stringify({ schema_version: 1, output_dir: roots[call++] }),
      stderr: "",
    });
    await expect(getEffectiveMeetingsDir(runner, async () => true, undefined)).resolves.toBe(
      roots[0]
    );
    await expect(getEffectiveMeetingsDir(runner, async () => true, undefined)).resolves.toBe(
      roots[1]
    );
    expect(call).toBe(2);
  });
});

describe("shouldRunMainEntry", () => {
  it("accepts npm .bin shims that realpath to the module file", () => {
    const tempRoot = mkdtempSync(join(tmpdir(), "minutes-mcp-entry-"));
    const packageDir = join(tempRoot, "node_modules", "minutes-mcp", "dist");
    const binDir = join(tempRoot, "node_modules", ".bin");
    const modulePath = join(packageDir, "index.js");
    const shimPath = join(binDir, "minutes-mcp");

    mkdirSync(packageDir, { recursive: true });
    mkdirSync(binDir, { recursive: true });
    writeFileSync(modulePath, "export {};\n");
    symlinkSync(modulePath, shimPath);

    try {
      expect(shouldRunMainEntry(shimPath, modulePath)).toBe(true);
    } finally {
      rmSync(tempRoot, { recursive: true, force: true });
    }
  });

  it("accepts equivalent paths once symlinks are resolved", () => {
    expect(shouldRunMainEntry(import.meta.filename, import.meta.filename)).toBe(true);
  });

  it("rejects unrelated worker entrypoints", () => {
    expect(
      shouldRunMainEntry(
        "/Users/dev/project/node_modules/vitest/dist/workers/forks.js",
        "/Users/dev/project/crates/mcp/src/index.ts"
      )
    ).toBe(false);
  });
});

describe("copilot MCP observation contract", () => {
  const createdMs = Date.parse("2026-07-14T12:00:00.000Z");
  const firstNudge = {
    v: 1,
    id: "nudge-41-1",
    kind: "Ask",
    text: "Ask who owns the rollout date.",
    source_chip: "rollout date",
    evidence_revision: 41,
    created_ts: "2026-07-14T12:00:00.000Z",
    ttl_ms: 12000,
  };
  const secondNudge = {
    ...firstNudge,
    id: "nudge-42-2",
    kind: "Clarify",
    text: "Clarify whether Friday means launch or handoff.",
    evidence_revision: 42,
    created_ts: "2026-07-14T12:00:05.000Z",
    supersedes: "nudge-41-1",
  };

  it("parses the exact versioned CLI status without retaining raw or content fields", () => {
    expect(parseCopilotStatusOutput(JSON.stringify({
      schema_version: 1,
      active: false,
      state: "Off",
      pid: null,
      surface: null,
      evidence_cursor: 0,
      input_mode: "final_only",
      setup_needed: false,
    }))).toEqual({
      schema_version: 1,
      available: true,
      active: false,
      state: "Off",
      pid: null,
      surface: null,
      evidence_cursor: 0,
      input_mode: "final_only",
      setup_needed: false,
    });

    const active = parseCopilotStatusOutput(JSON.stringify({
      schema_version: 1,
      active: true,
      state: "Listening",
      pid: 4321,
      surface: "stdout",
      evidence_cursor: 42,
      input_mode: "realtime",
      setup_needed: false,
    }));
    expect(active).toEqual({
      schema_version: 1,
      available: true,
      active: true,
      state: "Listening",
      pid: 4321,
      surface: "stdout",
      evidence_cursor: 42,
      input_mode: "realtime",
      setup_needed: false,
    });
    expect(active).not.toHaveProperty("raw");
    expect(active).not.toHaveProperty("goal");
    expect(active).not.toHaveProperty("last_error");
  });

  it("rejects status extensions instead of leaking their content", () => {
    const canary = "PRIVATE-STATUS-CONTENT-CANARY";
    const parse = () => parseCopilotStatusOutput(JSON.stringify({
      schema_version: 1,
      active: false,
      state: "Off",
      pid: null,
      surface: null,
      evidence_cursor: 0,
      input_mode: "final_only",
      setup_needed: false,
      goal: canary,
    }));
    expect(parse).toThrow("Copilot status response was invalid.");
    try {
      parse();
    } catch (error) {
      expect(String(error)).not.toContain(canary);
    }
  });

  it("requests only the strict JSON status bridge and contains CLI failures", async () => {
    const calls: string[][] = [];
    const status = await readCopilotStatusFromCli(
      async (args) => {
        calls.push(args);
        return {
          stdout: JSON.stringify({
            schema_version: 1,
            active: false,
            state: "Off",
            pid: null,
            surface: null,
            evidence_cursor: 0,
            input_mode: "final_only",
            setup_needed: false,
          }),
          stderr: "",
        };
      },
      async () => true
    );
    expect(calls).toEqual([["copilot", "status", "--json"]]);
    expect(status).toMatchObject({ available: true, active: false });

    const canary = "/private/status/PATH-CONTENT-CANARY";
    const failed = await readCopilotStatusFromCli(
      async () => {
        throw new Error(canary);
      },
      async () => true
    );
    expect(failed).toMatchObject({
      available: false,
      error: "Unable to read copilot status safely.",
    });
    expect(JSON.stringify(failed)).not.toContain(canary);
  });

  it("returns content-free inactive status after issuing stop", async () => {
    const order: string[] = [];
    let engineActive = true;
    const stopped = await stopCopilotBeforeStatusRead(
      async () => {
        order.push("stop");
        expect(engineActive).toBe(true);
        engineActive = false;
      },
      async () => {
        order.push("status");
        expect(engineActive).toBe(false);
        return parseCopilotStatusOutput(JSON.stringify({
          schema_version: 1,
          active: false,
          state: "Off",
          pid: null,
          surface: null,
          evidence_cursor: 42,
          input_mode: "realtime",
          setup_needed: false,
        }));
      },
      async () => {
        order.push("readiness");
      }
    );

    expect(order).toEqual(["stop", "status"]);
    expect(stopped).toMatchObject({ mayRevealContent: true, status: { active: false } });
  });

  it("still stops but withholds a remaining active session when readiness is blocked", async () => {
    const order: string[] = [];
    const stopped = await stopCopilotBeforeStatusRead(
      async () => {
        order.push("stop");
      },
      async () => {
        order.push("status");
        return parseCopilotStatusOutput(JSON.stringify({
          schema_version: 1,
          active: true,
          state: "Listening",
          pid: 42,
          surface: "stdout",
          evidence_cursor: 7,
          input_mode: "realtime",
          setup_needed: false,
        }));
      },
      async () => {
        order.push("readiness");
        throw new Error("blocked");
      }
    );

    expect(order).toEqual(["stop", "status", "readiness"]);
    expect(stopped).toEqual({ mayRevealContent: false });
  });

  it("parses JSON nudges with cursor and TTL metadata", () => {
    const nudges = parseCopilotNudgeLog(
      `${JSON.stringify(firstNudge)}\n${JSON.stringify(secondNudge)}\n`,
      createdMs + 6000
    );

    expect(nudges).toHaveLength(2);
    expect(nudges[0]).toMatchObject({ cursor: 1, format: "json", expired: false });
    expect(nudges[1]).toMatchObject({
      cursor: 2,
      format: "json",
      expired: false,
      nudge: { id: "nudge-42-2", supersedes: "nudge-41-1" },
    });
  });

  it("returns lossless cursor pages and resets a cursor from a prior session", () => {
    const nudges = parseCopilotNudgeLog(
      `${JSON.stringify(firstNudge)}\n${JSON.stringify(secondNudge)}\n`,
      createdMs + 6000
    );
    const observation: CopilotNudgeObservation = {
      attached: true,
      cursor: 2,
      session: null,
      nudges,
      note: "attached",
    };

    expect(selectCopilotNudges(observation, { cursor: 0, limit: 1 })).toMatchObject({
      cursor: 2,
      next_cursor: 1,
      cursor_reset: false,
      has_more: true,
      nudges: [{ cursor: 1 }],
    });
    expect(selectCopilotNudges(observation, { cursor: 99 })).toMatchObject({
      cursor: 2,
      next_cursor: 2,
      cursor_reset: true,
      has_more: false,
      nudges: [{ cursor: 1 }, { cursor: 2 }],
    });
    expect(
      selectCopilotNudges(observation, { since: "2s" }, createdMs + 6000).nudges
    ).toMatchObject([{ cursor: 2 }]);
  });

  it("exposes latest but never current advice after TTL expiry", () => {
    const status = parseCopilotStatusOutput(JSON.stringify({
      schema_version: 1,
      active: true,
      state: "Nudge",
      pid: 4321,
      surface: "stdout",
      evidence_cursor: 42,
      input_mode: "realtime",
      setup_needed: false,
    }));
    const nudges = parseCopilotNudgeLog(JSON.stringify(firstNudge), createdMs + 13000);
    const payload = buildLiveCopilotResourcePayload(status, {
      attached: true,
      cursor: 1,
      session: null,
      nudges,
      note: "attached",
    });

    expect(payload.latest_nudge).toMatchObject({ cursor: 1, expired: true });
    expect(payload.current_nudge).toBeNull();
  });
});

describe("live event MCP resource", () => {
  it("keeps production reads and subscriptions constant when hidden events change", async () => {
    expect(LIVE_EVENTS_SUBSCRIPTIONS_ENABLED).toBe(false);
    const first = await readLiveEventsResource(new URL(LIVE_EVENTS_RESOURCE_URI));
    const second = await readLiveEventsResource(new URL(LIVE_EVENTS_RESOURCE_URI));
    expect(second).toEqual(first);
    const payload = JSON.parse(first.contents[0].text);
    expect(payload).toMatchObject({
      latest_seq: 0,
      events: [],
      reconnect: {
        cursor: 0,
        read_uri: `${LIVE_EVENTS_RESOURCE_URI}?since_seq=0`,
      },
    });
    expect(payload.unavailable).toContain("non-sensitive cursor");

    const requested = await readLiveEventsResource(
      new URL(`${LIVE_EVENTS_RESOURCE_URI}?since_seq=42&limit=7`)
    );
    expect(JSON.parse(requested.contents[0].text)).toMatchObject({
      latest_seq: 42,
      events: [],
      reconnect: {
        cursor: 42,
        read_uri: `${LIVE_EVENTS_RESOURCE_URI}?since_seq=42`,
      },
    });
  });

  it("keeps exported subscription handlers fail-closed by default", async () => {
    const mcpServer = new McpServer({ name: "minutes-safe-default-test", version: "0.0.0" });
    let sourceReads = 0;
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      latestEventSeq: async () => {
        sourceReads += 1;
        return 1;
      },
      readEventsSinceSeq: async () => {
        sourceReads += 1;
        return [{ seq: 1 }];
      },
      resourceReadiness: async () => {},
    });
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client(
      { name: "safe-default-client", version: "0.0.0" },
      { capabilities: {} }
    );

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await expect(
        client.subscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI })
      ).rejects.toThrow();
      await new Promise((resolve) => setTimeout(resolve, 25));
      expect(sourceReads).toBe(0);
      expect(controller.subscriptionCount()).toBe(0);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });

  it("parses the base resource and cursor read URIs", () => {
    expect(parseLiveEventsResourceUri("minutes://events/live")).toMatchObject({
      uri: "minutes://events/live",
      sinceSeq: null,
      limit: 20,
    });
    expect(parseLiveEventsResourceUri("minutes://events/live?since_seq=42&limit=7")).toMatchObject({
      uri: "minutes://events/live?since_seq=42&limit=7",
      sinceSeq: 42,
      limit: 7,
    });
    expect(parseLiveEventsResourceUri("minutes://events/recent")).toBeNull();
  });

  it("builds a reconnect cursor from the highest delivered sequence", () => {
    const payload = buildLiveEventsResourcePayload(
      { uri: "minutes://events/live?since_seq=10", sinceSeq: 10, limit: 100 },
      [{ seq: 11 }, { seq: 14 }],
      12
    );

    expect(payload.latest_seq).toBe(14);
    expect(payload.reconnect).toEqual({
      cursor: 14,
      read_uri: "minutes://events/live?since_seq=14",
    });
  });

  it("keeps the reconnect cursor on the delivered page boundary", () => {
    const payload = buildLiveEventsResourcePayload(
      { uri: "minutes://events/live?since_seq=10&limit=1", sinceSeq: 10, limit: 1 },
      [{ seq: 11 }],
      14
    );

    expect(payload.latest_seq).toBe(14);
    expect(payload.reconnect).toEqual({
      cursor: 11,
      read_uri: "minutes://events/live?since_seq=11",
    });
  });

  it("does not move a future reconnect cursor backward", () => {
    const payload = buildLiveEventsResourcePayload(
      { uri: "minutes://events/live?since_seq=99", sinceSeq: 99, limit: 100 },
      [],
      14
    );

    expect(payload.latest_seq).toBe(14);
    expect(payload.reconnect).toEqual({
      cursor: 99,
      read_uri: "minutes://events/live?since_seq=99",
    });
  });

  it("sends resource updated notifications over an MCP client subscription", async () => {
    const mcpServer = new McpServer({ name: "minutes-test", version: "0.0.0" });
    const updates: string[] = [];
    let readCursor = 4;
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      enableLiveEvents: true,
      latestEventSeq: async () => 4,
      readEventsSinceSeq: async (sinceSeq) => {
        if (sinceSeq >= readCursor) {
          readCursor = 9;
          return [{ seq: 9, event_type: "live.utterance.final" }];
        }
        return [];
      },
      resourceReadiness: async () => {},
    });

    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "test-client", version: "0.0.0" }, { capabilities: {} });
    client.setNotificationHandler(ResourceUpdatedNotificationSchema, (notification) => {
      updates.push(notification.params.uri);
    });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });

      await waitFor(() => updates.length > 0);
      expect(updates).toEqual([LIVE_EVENTS_RESOURCE_URI]);

      await client.unsubscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });
      expect(controller.subscriptionCount()).toBe(0);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });

  it("routes copilot updates through the same subscription handler", async () => {
    const mcpServer = new McpServer({ name: "minutes-copilot-test", version: "0.0.0" });
    const updates: string[] = [];
    let fingerprint = "off:0";
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      enableLiveEvents: false,
      enableCopilot: true,
      copilotFingerprint: async () => fingerprint,
      resourceReadiness: async () => {},
    });

    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "copilot-test-client", version: "0.0.0" }, { capabilities: {} });
    client.setNotificationHandler(ResourceUpdatedNotificationSchema, (notification) => {
      updates.push(notification.params.uri);
    });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_COPILOT_RESOURCE_URI });
      fingerprint = "listening:1";

      await waitFor(() => updates.length > 0);
      expect(updates).toEqual([LIVE_COPILOT_RESOURCE_URI]);

      await client.unsubscribeResource({ uri: LIVE_COPILOT_RESOURCE_URI });
      expect(controller.subscriptionCount()).toBe(0);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });

  it("stops live-event subscription source reads when readiness is revoked", async () => {
    const mcpServer = new McpServer({ name: "minutes-event-revocation-test", version: "0.0.0" });
    const updates: string[] = [];
    let readinessAllowed = true;
    let sourceReads = 0;
    let nextSeq = 4;
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      enableLiveEvents: true,
      latestEventSeq: async () => 4,
      readEventsSinceSeq: async (sinceSeq) => {
        sourceReads += 1;
        return nextSeq > sinceSeq ? [{ seq: nextSeq }] : [];
      },
      resourceReadiness: async () => {
        if (!readinessAllowed) throw new Error("synthetic readiness revocation");
      },
      sendResourceUpdated: async (uri) => {
        updates.push(uri);
      },
      onError: () => {},
    });

    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "event-revocation-client", version: "0.0.0" }, { capabilities: {} });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });
      readinessAllowed = false;
      nextSeq = 9;
      const readsAtRevocation = sourceReads;

      await new Promise((resolve) => setTimeout(resolve, 40));
      expect(sourceReads).toBe(readsAtRevocation);
      expect(updates).toEqual([]);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });

  it("stops Copilot subscription source reads when readiness is revoked", async () => {
    const mcpServer = new McpServer({ name: "minutes-copilot-revocation-test", version: "0.0.0" });
    const updates: string[] = [];
    let readinessAllowed = true;
    let sourceReads = 0;
    let fingerprint = "quiet:0";
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      enableLiveEvents: false,
      enableCopilot: true,
      copilotFingerprint: async () => {
        sourceReads += 1;
        return fingerprint;
      },
      resourceReadiness: async () => {
        if (!readinessAllowed) throw new Error("synthetic readiness revocation");
      },
      sendResourceUpdated: async (uri) => {
        updates.push(uri);
      },
      onError: () => {},
    });

    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "copilot-revocation-client", version: "0.0.0" }, { capabilities: {} });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_COPILOT_RESOURCE_URI });
      readinessAllowed = false;
      fingerprint = "changed:1";
      const readsAtRevocation = sourceReads;

      await new Promise((resolve) => setTimeout(resolve, 40));
      expect(sourceReads).toBe(readsAtRevocation);
      expect(updates).toEqual([]);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });

  it("does not advance or notify when live-event readiness is revoked during a read", async () => {
    const mcpServer = new McpServer({ name: "minutes-event-mid-read-test", version: "0.0.0" });
    const updates: string[] = [];
    const seenCursors: number[] = [];
    let readinessAllowed = true;
    let signalReadStarted!: () => void;
    let releaseRead!: () => void;
    const readStarted = new Promise<void>((resolve) => { signalReadStarted = resolve; });
    const readRelease = new Promise<void>((resolve) => { releaseRead = resolve; });
    let suspendNextRead = true;
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      enableLiveEvents: true,
      latestEventSeq: async () => 4,
      readEventsSinceSeq: async (sinceSeq) => {
        seenCursors.push(sinceSeq);
        if (suspendNextRead) {
          suspendNextRead = false;
          signalReadStarted();
          await readRelease;
        }
        return [{ seq: 9 }];
      },
      resourceReadiness: async () => {
        if (!readinessAllowed) throw new Error("synthetic mid-read revocation");
      },
      sendResourceUpdated: async (uri) => {
        updates.push(uri);
      },
      onError: () => {},
    });
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "event-mid-read-client", version: "0.0.0" }, { capabilities: {} });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });
      await readStarted;
      readinessAllowed = false;
      releaseRead();

      await new Promise((resolve) => setTimeout(resolve, 30));
      expect(updates).toEqual([]);
      expect(seenCursors).toEqual([4]);

      readinessAllowed = true;
      await waitFor(() => updates.length > 0);
      expect(seenCursors.slice(0, 2)).toEqual([4, 4]);
      expect(updates).toEqual([LIVE_EVENTS_RESOURCE_URI]);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });

  it("does not advance or notify when Copilot readiness is revoked during a read", async () => {
    const mcpServer = new McpServer({ name: "minutes-copilot-mid-read-test", version: "0.0.0" });
    const updates: string[] = [];
    let readinessAllowed = true;
    let fingerprintReads = 0;
    let signalReadStarted!: () => void;
    let releaseRead!: () => void;
    const readStarted = new Promise<void>((resolve) => { signalReadStarted = resolve; });
    const readRelease = new Promise<void>((resolve) => { releaseRead = resolve; });
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      enableLiveEvents: false,
      enableCopilot: true,
      copilotFingerprint: async () => {
        fingerprintReads += 1;
        if (fingerprintReads === 1) return "quiet:0";
        if (fingerprintReads === 2) {
          signalReadStarted();
          await readRelease;
        }
        return "changed:1";
      },
      resourceReadiness: async () => {
        if (!readinessAllowed) throw new Error("synthetic mid-read revocation");
      },
      sendResourceUpdated: async (uri) => {
        updates.push(uri);
      },
      onError: () => {},
    });
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "copilot-mid-read-client", version: "0.0.0" }, { capabilities: {} });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_COPILOT_RESOURCE_URI });
      await readStarted;
      readinessAllowed = false;
      releaseRead();

      await new Promise((resolve) => setTimeout(resolve, 30));
      expect(updates).toEqual([]);
      expect(fingerprintReads).toBe(2);

      readinessAllowed = true;
      await waitFor(() => updates.length > 0);
      expect(fingerprintReads).toBeGreaterThanOrEqual(3);
      expect(updates).toEqual([LIVE_COPILOT_RESOURCE_URI]);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });

  it("does not deliver an in-flight event read to a replacement subscription", async () => {
    const mcpServer = new McpServer({ name: "minutes-event-epoch-test", version: "0.0.0" });
    const updates: string[] = [];
    const seenCursors: number[] = [];
    let signalReadStarted!: () => void;
    let releaseRead!: () => void;
    const readStarted = new Promise<void>((resolve) => { signalReadStarted = resolve; });
    const readRelease = new Promise<void>((resolve) => { releaseRead = resolve; });
    let reads = 0;
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      enableLiveEvents: true,
      latestEventSeq: async () => 4,
      readEventsSinceSeq: async (sinceSeq) => {
        seenCursors.push(sinceSeq);
        reads += 1;
        if (reads === 1) {
          signalReadStarted();
          await readRelease;
          return [{ seq: 9 }];
        }
        return [];
      },
      resourceReadiness: async () => {},
      sendResourceUpdated: async (uri) => { updates.push(uri); },
      onError: () => {},
    });
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "event-epoch-client", version: "0.0.0" }, { capabilities: {} });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });
      await readStarted;
      await client.unsubscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });
      await client.subscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });
      releaseRead();

      await new Promise((resolve) => setTimeout(resolve, 40));
      expect(updates).toEqual([]);
      expect(seenCursors.length).toBeGreaterThanOrEqual(2);
      expect(seenCursors.every((cursor) => cursor === 4)).toBe(true);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });

  it("does not let an old Copilot poll seed a replacement subscription", async () => {
    const mcpServer = new McpServer({ name: "minutes-copilot-epoch-test", version: "0.0.0" });
    const updates: string[] = [];
    let signalReadStarted!: () => void;
    let releaseRead!: () => void;
    const readStarted = new Promise<void>((resolve) => { signalReadStarted = resolve; });
    const readRelease = new Promise<void>((resolve) => { releaseRead = resolve; });
    let fingerprintReads = 0;
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      enableLiveEvents: false,
      enableCopilot: true,
      copilotFingerprint: async () => {
        fingerprintReads += 1;
        if (fingerprintReads === 1) return "initial:0";
        if (fingerprintReads === 2) {
          signalReadStarted();
          await readRelease;
          return "obsolete:1";
        }
        return "replacement:0";
      },
      resourceReadiness: async () => {},
      sendResourceUpdated: async (uri) => { updates.push(uri); },
      onError: () => {},
    });
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "copilot-epoch-client", version: "0.0.0" }, { capabilities: {} });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_COPILOT_RESOURCE_URI });
      await readStarted;
      await client.unsubscribeResource({ uri: LIVE_COPILOT_RESOURCE_URI });
      await client.subscribeResource({ uri: LIVE_COPILOT_RESOURCE_URI });
      releaseRead();

      await new Promise((resolve) => setTimeout(resolve, 40));
      expect(fingerprintReads).toBeGreaterThanOrEqual(4);
      expect(updates).toEqual([]);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });

  it("reinitializes one resource after unsubscribe while the other poller remains live", async () => {
    const mcpServer = new McpServer({ name: "minutes-resource-reset-test", version: "0.0.0" });
    let latestSeqReads = 0;
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 20,
      enableLiveEvents: true,
      enableCopilot: true,
      latestEventSeq: async () => {
        latestSeqReads += 1;
        return latestSeqReads === 1 ? 4 : 10;
      },
      readEventsSinceSeq: async () => [],
      copilotFingerprint: async () => "steady:0",
      resourceReadiness: async () => {},
      sendResourceUpdated: async () => {},
      onError: () => {},
    });
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "resource-reset-client", version: "0.0.0" }, { capabilities: {} });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });
      await client.subscribeResource({ uri: LIVE_COPILOT_RESOURCE_URI });
      await client.unsubscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });
      expect(controller.subscriptionCount()).toBe(1);
      await client.subscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });
      expect(latestSeqReads).toBe(2);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });
});

async function waitFor(predicate: () => boolean): Promise<void> {
  const deadline = Date.now() + 1000;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error("timed out waiting for condition");
}

describe("missing engine guidance (#774)", () => {
  it("points macOS and Windows at the app, and Linux at the CLI", () => {
    const mac = cliMissingGuidance("darwin");
    const win = cliMissingGuidance("win32");
    const linux = cliMissingGuidance("linux");
    // The desktop app ships for macOS and Windows only. Telling a Linux user
    // to install it sends them to a download that does not exist.
    expect(mac).toContain("desktop app");
    expect(win).toContain("desktop app");
    expect(linux).not.toContain("desktop app");
    expect(linux).toContain("cargo install minutes-cli");
    for (const text of [mac, win, linux]) {
      expect(text).toContain("https://useminutes.app");
      // The likely cause, because auto-install normally handles this and the
      // reader is usually someone who never chose to have a CLI.
      expect(text).toMatch(/internet|proxy|unsupported platform/);
    }
  });

  it("lets a missing engine through the readiness gate, but nothing else", async () => {
    const missing = Object.assign(new Error("engine gone"), { cliMissing: true });
    await expect(
      readAgentTrustReadiness(async () => {
        throw missing;
      })
    ).rejects.toBe(missing);

    // An unverifiable answer must stay opaque: the caller does not learn why
    // the boundary refused.
    await expect(
      readAgentTrustReadiness(async () => {
        throw new Error("something the caller must not see");
      })
    ).rejects.toThrow("could not be verified safely");
  });
});
