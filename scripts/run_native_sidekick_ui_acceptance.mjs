#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import { randomBytes, createHash } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { scoreMeridianResponses } from "../tests/eval/sidekick_rehearsal_golden.mjs";
import {
  bundleManifestSha256,
  canonicalInstalledAppPath,
  currentGeneratedBuildHelperPaths,
  validateCanonicalInstalledApp,
} from "./run_native_sidekick_contract_scope_acceptance.mjs";

const MAX_DOM_PAINT_MS = 5_000;
const HARD_TIMEOUT_MS = 230_000;
const FORCED_CLEANUP_GRACE_MS = 75_000;
const MAX_OUTPUT_BYTES = 1024 * 1024;
const CANONICAL_FIXTURE_PATH = fileURLToPath(
  new URL("../tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json", import.meta.url),
);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function check(name, passed, detail) {
  return { name, passed: Boolean(passed), detail };
}

const canonicalFixtureBytes = await fs.readFile(CANONICAL_FIXTURE_PATH);
const canonicalFixture = JSON.parse(canonicalFixtureBytes.toString("utf8"));
if (!Array.isArray(canonicalFixture.turns) || canonicalFixture.turns.length !== 2) {
  throw new Error("canonical Meridian UI fixture must contain exactly two turns");
}

export function evaluateNativeSidekickUiAcceptance(payload, runtime) {
  const turns = Array.isArray(payload?.turns) ? payload.turns : [];
  const turnOneCandidateEvidence = Array.isArray(turns[0]?.candidate_evidence?.transcriptEvidenceIds)
    ? turns[0].candidate_evidence.transcriptEvidenceIds
    : [];
  const quality = scoreMeridianResponses({
    turn_1: turns[0]?.response ?? "",
    turn_2: turns[1]?.response ?? "",
    turn_1_evidence_ids: turnOneCandidateEvidence.map((id) => {
      const sequence = String(id).match(/-(\d+)$/)?.[1];
      return sequence ? `utterance-${sequence}` : String(id);
    }),
  });
  const expectedPrompts = canonicalFixture.turns.map(({ id, typed_prompt }) => ({
    id,
    prompt: typed_prompt,
  }));
  const approvedTranscriptIds = Array.isArray(payload?.transcript?.approved_evidence_ids)
    ? payload.transcript.approved_evidence_ids
    : [];
  const approvedVisualPrefix = payload?.screen?.provider_marker_evidence_prefix;
  const sourceChecks = [
    check(
      "installed_app_exit_zero",
      runtime.exit_code === 0,
      `The signed app exited ${runtime.exit_code}.`,
    ),
    check(
      "canonical_executable_matches_signed_build",
      /^[a-f0-9]{64}$/.test(runtime.executable_sha256 ?? "") &&
        runtime.executable_sha256 === runtime.expected_executable_sha256,
      "The installed executable must byte-match the freshly signed build.",
    ),
    check(
      "canonical_bundle_matches_signed_build",
      /^[a-f0-9]{64}$/.test(runtime.bundle_sha256 ?? "") &&
        runtime.bundle_sha256 === runtime.expected_bundle_sha256,
      "The complete installed bundle must match the freshly signed build.",
    ),
    check(
      "current_embedded_commit",
      payload?.build_commit === runtime.expected_build_commit,
      "The installed executable must report the committed checkout used for this run.",
    ),
    check(
      "real_dev_product_path",
      payload?.mode === "diagnose-native-sidekick-ui" &&
        payload?.passed_product_path === true &&
        payload?.bundle_identifier === "com.useminutes.desktop.dev",
      "The check must traverse the real Tauri dev app and native Sidekick window.",
    ),
    check(
      "approved_embedded_fixture",
      payload?.fixture_id === canonicalFixture.id &&
        payload?.fixture_sha256 === sha256(canonicalFixtureBytes),
      "The visible UI run must use the same approved Meridian bytes as the deterministic golden.",
    ),
    check(
      "real_microphone_signal_smoke",
      payload?.audio?.growing === true &&
        payload?.audio?.intent === "room" &&
        Number.isFinite(payload?.audio?.size_before) &&
        payload.audio.size_before > 44 &&
        payload?.audio?.size_after > payload.audio.size_before &&
        payload?.audio?.samples_inspected >= 1_000 &&
        payload?.audio?.peak_amplitude >= 8 &&
        payload?.audio?.rms_amplitude >= 1 &&
        payload?.audio?.nonzero_ratio >= 0.01 &&
        payload?.audio?.scope === "microphone_signal_smoke_only" &&
        payload?.audio?.speech_or_asr_claimed === false,
      "The installed app must produce sustained real room-mic signal; this is explicitly not speech, ASR, or diarization proof.",
    ),
    check(
      "bounded_cold_start_and_total_runtime",
      payload?.startup_latency?.recording_ready_ms <= 25_000 &&
        payload?.startup_latency?.screen_ready_ms <= 45_000 &&
        payload?.startup_latency?.sidekick_ready_ms <= 75_000 &&
        Number.isFinite(runtime.wall_ms) && runtime.wall_ms <= 150_000,
      "Recording, exact-session screen, Sidekick readiness, and total diagnostic runtime must stay bounded.",
    ),
    check(
      "pinned_fixture_excludes_ambient_transcript",
      payload?.transcript?.source === "acceptance_pinned_fixture" &&
      payload?.transcript?.adapter === "verified_bytes_live_transcript_jsonl_delta" &&
        /^[a-f0-9]{64}$/.test(payload?.transcript?.fixture_jsonl_sha256 ?? "") &&
        payload?.transcript?.fixture_jsonl_sha256 === payload?.transcript?.final_jsonl_sha256 &&
        /^[a-f0-9]{64}$/.test(payload?.transcript?.initial_jsonl_sha256 ?? "") &&
        payload?.transcript?.initial_jsonl_sha256 !== payload?.transcript?.final_jsonl_sha256 &&
        payload?.transcript?.items === canonicalFixture.transcript.length &&
        payload?.transcript?.initial_items === 4 &&
        payload?.transcript?.delta_items === canonicalFixture.transcript.length - 4 &&
        payload?.transcript?.delta_turn_id === expectedPrompts[1].id &&
        payload?.transcript?.ambient_live_transcript_allowed === false &&
        approvedTranscriptIds.length === canonicalFixture.transcript.length &&
        new Set(approvedTranscriptIds).size === canonicalFixture.transcript.length &&
        approvedTranscriptIds.every((id) => /^acceptance-transcript-[a-f0-9]{16}-\d+$/.test(id)),
      "Cloud reasoning must receive only the approved fixture evidence IDs; ambient room transcript is forbidden.",
    ),
    check(
      "real_screen_contains_verified_safe_marker",
      Number.isFinite(payload?.screen?.permission_capture_bytes) &&
        payload.screen.permission_capture_bytes > 8 &&
        /^[a-f0-9]{64}$/.test(payload?.screen?.permission_capture_sha256 ?? "") &&
        /^[a-f0-9]{64}$/.test(payload?.screen?.provider_marker_sha256 ?? "") &&
        payload?.screen?.marker_nonce_verified_from_pixels === true &&
        payload?.screen?.provider_marker_is_generated_nonce_only === true &&
        payload?.screen?.adapter === "context_store_exact_session" &&
        /^acceptance-screen-[a-f0-9]{16}$/.test(approvedVisualPrefix ?? "") &&
        payload?.screen?.capture_session_id === payload?.context_session_id,
      "The real screen worker must capture and locally decode the run nonce; that exact safe image must stay pinned to the recording.",
    ),
    check(
      "same_recording_session_visible_in_sidekick",
      typeof payload?.context_session_id === "string" &&
        payload.context_session_id.length > 0 &&
        payload?.context_session_type === "recording" &&
        payload?.sidekick?.ready_session_id === payload.context_session_id &&
        payload?.sidekick?.screen_available === true &&
        payload?.sidekick?.launch_surface === "main_sidekick_button_cloud_consent" &&
        payload?.sidekick?.main_launch_completed === true &&
        [
          "main_sidekick_button",
          "cloud_consent_confirm",
          ...expectedPrompts.flatMap(({ id }) => [
            `${id}:sidekick_input`,
            `${id}:sidekick_send`,
          ]),
        ].every((key) => payload?.sidekick?.interactable_targets?.[key] === true),
      "The visible Sidekick window must attach to the exact active recording and its screen context.",
    ),
    check(
      "one_persistent_reasoning_session",
      payload?.sidekick?.reasoning_sessions_started === 1 &&
        /^[a-f0-9]{64}$/.test(payload?.sidekick?.reasoning_session_correlation ?? ""),
      "Both visible turns must use one persistent provider-neutral reasoning session.",
    ),
    check(
      "trusted_host_provider_copy_unchanged_pre_post",
      /^[a-f0-9]{64}$/.test(payload?.sidekick?.provider_executable_sha256 ?? "") &&
        payload?.sidekick?.provider_executable_sha256 === runtime.expected_provider_sha256 &&
        payload?.sidekick?.provider_executable_path === runtime.expected_provider_path &&
        payload?.sidekick?.provider_version === runtime.expected_provider_version &&
        payload?.sidekick?.provider_executable_attestation_scope === "trusted_host_path_pre_post" &&
        runtime.provider_copy_is_private === true &&
        runtime.provider_copy_post_sha256 === runtime.expected_provider_sha256,
      "On the declared trusted single-user host, the selected private Codex path must match before launch, inside Minutes, and after app exit; this is not live-process code-identity attestation.",
    ),
    check(
      "provider_request_and_exercised_capabilities_are_distinct",
      payload?.sidekick?.provider_requested_contract?.provider === "codex-app-server" &&
        payload?.sidekick?.provider_requested_contract?.model === "codex-fast" &&
        payload?.sidekick?.provider_requested_contract?.privacy === "cloud" &&
        payload?.sidekick?.provider_requested_contract?.persistent === true &&
        payload?.sidekick?.provider_requested_contract?.steerable === true &&
        payload?.sidekick?.provider_requested_contract?.streaming === true &&
        payload?.sidekick?.provider_requested_contract?.image_input === true &&
        payload?.sidekick?.provider_capabilities_exercised?.persistent_sequential_turns === true &&
        payload?.sidekick?.provider_capabilities_exercised?.streaming_delta_observed === true &&
        payload?.sidekick?.provider_capabilities_exercised?.steering === false &&
        payload?.sidekick?.provider_capabilities_exercised?.interruption === false,
      "The report must distinguish requested Codex/Fast capabilities from the persistent and streamed behavior exercised by this run.",
    ),
    check(
      "acceptance_scope_is_machine_readable_and_bounded",
      payload?.acceptance_scope?.kind === "bounded_native_ui_provider_integration" &&
        [
          "live_speech_recognition",
          "two_speaker_diarization",
          "semantic_desktop_screen_understanding",
          "compositor_or_occlusion_proof",
          "provider_steering_and_interruption",
          "normal_installed_app_cold_start",
          "hostile_same_user_filesystem_or_process_tampering",
          "provider_live_process_code_identity_attestation",
          "escaped_session_descendant_detection",
        ].every((item) => payload?.acceptance_scope?.excludes?.includes(item)) &&
        payload?.acceptance_scope?.host_threat_model === "trusted_single_user_no_concurrent_hostile_same_uid_process",
      "The run must stay bounded to product integration on a trusted single-user host, not claim hostile-local-process isolation or live process identity attestation.",
    ),
    check(
      "two_exact_ui_turns",
      turns.length === expectedPrompts.length &&
        new Set(turns.map((turn) => turn?.evidence_receipt?.turnId)).size === expectedPrompts.length &&
        turns.every((turn, index) => {
          const expectedTranscriptIds = index === 0
            ? approvedTranscriptIds.slice(0, 4)
            : approvedTranscriptIds;
          const expectedVisualId = `${approvedVisualPrefix}-${turn?.id}`;
          const candidateTranscriptIds = turn?.candidate_evidence?.transcriptEvidenceIds;
          const candidateVisualIds = turn?.candidate_evidence?.visualEvidenceIds;
          const expectedHash = index === 0
            ? payload?.transcript?.initial_jsonl_sha256
            : payload?.transcript?.final_jsonl_sha256;
          const expectedNewItems = index === 0 ? 0 : canonicalFixture.transcript.length - 4;
          const hasTurnSpecificGrounding = index === 0
            ? [approvedTranscriptIds[0], approvedTranscriptIds[2], approvedTranscriptIds[3]]
              .every((id) => candidateTranscriptIds?.includes(id))
            : candidateTranscriptIds?.includes(approvedTranscriptIds[2]) &&
              [approvedTranscriptIds[4], approvedTranscriptIds[5]]
                .some((id) => candidateTranscriptIds?.includes(id));
          return (
          turn?.id === expectedPrompts[index].id &&
          turn?.prompt === expectedPrompts[index].prompt &&
          typeof turn?.response === "string" &&
          turn.response.trim().length > 0 &&
          turn?.evidence_receipt?.captureSessionId === payload.context_session_id &&
          /^foreground-\d+$/.test(turn?.evidence_receipt?.turnId ?? "") &&
          JSON.stringify(turn?.evidence_receipt?.transcriptEvidenceIds) === JSON.stringify(expectedTranscriptIds) &&
          JSON.stringify(turn?.evidence_receipt?.visualEvidenceIds) === JSON.stringify([expectedVisualId]) &&
          turn?.adapter_receipt?.transcriptAdapter === "live_transcript_jsonl_delta" &&
          turn?.adapter_receipt?.transcriptCursor === expectedTranscriptIds.length &&
          turn?.adapter_receipt?.transcriptSha256 === expectedHash &&
          turn?.adapter_receipt?.transcriptNewItems === expectedNewItems &&
          turn?.adapter_receipt?.screenAdapter === "context_store_exact_session" &&
          turn?.adapter_receipt?.screenCaptureSha256 === payload?.screen?.permission_capture_sha256 &&
          turn?.adapter_receipt?.providerImageEvidenceId === expectedVisualId &&
          typeof turn?.adapter_receipt?.providerImagePath === "string" &&
          turn.adapter_receipt.providerImagePath.endsWith(`/provider-${turn.id}.png`) &&
          turn?.adapter_receipt?.providerImageSha256 === payload?.screen?.provider_marker_sha256 &&
          turn?.adapter_receipt?.providerImageTransport === "inline_data_url" &&
          turn?.adapter_receipt?.providerImageDispatchedSha256 === payload?.screen?.provider_marker_sha256 &&
          turn?.adapter_receipt?.captureSessionId === payload.context_session_id &&
          turn?.adapter_receipt?.perTurnRefreshCompleted === true &&
          Array.isArray(candidateTranscriptIds) &&
          candidateTranscriptIds.length > 0 &&
          candidateTranscriptIds.every((id) => expectedTranscriptIds.includes(id)) &&
          hasTurnSpecificGrounding &&
          Array.isArray(candidateVisualIds) && candidateVisualIds.length === 0 &&
          turn?.candidate_evidence?.claimsVisualObservation === false &&
          Number.isFinite(turn?.candidate_evidence?.firstTokenMs)
          );
        }),
      "Both UI turns must prove their exact staged transcript bytes, exact inline image bytes, transcript-only material grounding, and distinct streamed foreground turns.",
    ),
    check(
      "clean_teardown",
      payload?.teardown?.sidekick_stopped === true &&
        payload?.teardown?.sidekick_control_cleared === true &&
        payload?.teardown?.recording_stop_requested === true &&
        payload?.teardown?.recording_stopped === true &&
        payload?.teardown?.recording_pid_removed === true &&
        payload?.teardown?.recording_metadata_cleared === true &&
        payload?.teardown?.disposable_wav_removed === true &&
        payload?.teardown?.processing_idle === true &&
        payload?.teardown?.context_discarded_and_screen_stopped === true &&
        payload?.teardown?.sensitive_paths_removed === true &&
        runtime.temporary_root_removed === true &&
        runtime.process_group_empty === true &&
        runtime.provider_process_cleanup_scope === "original_app_process_group" &&
        payload?.teardown?.cleanup_complete === true,
      "The diagnostic must prove complete recording, screen-worker, context, scratch-file, and original app process-group teardown before exit.",
    ),
  ];
  const paintChecks = turns.map((turn, index) => check(
    `turn_${index + 1}_request_to_dom_paint_under_${MAX_DOM_PAINT_MS}ms`,
    turn?.dom_layout?.turnId === turn?.id &&
      turn?.dom_layout?.animationFrames === 2 &&
      turn?.dom_layout?.windowVisible === true &&
      turn?.dom_layout?.onScreen === true &&
      Number.isFinite(turn?.dom_layout?.width) && turn.dom_layout.width > 0 &&
      Number.isFinite(turn?.dom_layout?.height) && turn.dom_layout.height > 0 &&
      /^[a-f0-9]{64}$/.test(turn?.dom_layout?.responseSha256 ?? "") &&
      turn.dom_layout.responseSha256 === sha256(turn.response ?? "") &&
      Number.isFinite(turn?.dom_layout?.typedToPaintMs) &&
      turn.dom_layout.typedToPaintMs <= MAX_DOM_PAINT_MS,
    `Observed ${turn?.dom_layout?.typedToPaintMs ?? "null"}ms from UI request through two DOM-layout frames in a visible on-screen window.`,
  ));
  const checks = [...sourceChecks, ...quality.checks, ...paintChecks];
  return {
    schema_version: 1,
    fixture_id: canonicalFixture.id,
    passed: checks.every((item) => item.passed),
    score: {
      numerator: checks.filter((item) => item.passed).length,
      denominator: checks.length,
    },
    quality_score: quality.score,
    source_checks: sourceChecks,
    quality_checks: quality.checks,
    paint_checks: paintChecks,
    runtime,
    product_path: payload,
  };
}

async function writeIsolatedConfig(homeDirectory) {
  const configDirectory = path.join(homeDirectory, ".config", "minutes");
  const outputDirectory = path.join(homeDirectory, "meetings");
  await fs.mkdir(configDirectory, { recursive: true, mode: 0o700 });
  await fs.mkdir(outputDirectory, { recursive: true, mode: 0o700 });
  const tomlString = (value) => JSON.stringify(String(value));
  await fs.writeFile(path.join(configDirectory, "config.toml"), [
    `output_dir = ${tomlString(outputDirectory)}`,
    "",
    "[screen_context]",
    "enabled = true",
    "interval_secs = 300",
    "keep_after_summary = false",
    "",
    "[consent]",
    'mode = "off"',
    "",
    "[privacy]",
    "hide_from_screen_share = false",
    "",
    "[calendar]",
    "enabled = false",
    "",
  ].join("\n"), { mode: 0o600 });
}

async function runInstalledUi(executable, runtime) {
  const egressOverrides = [
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "SSL_CERT_FILE",
  ].filter((key) => Boolean(process.env[key]));
  if (egressOverrides.length > 0) {
    throw new Error(
      `native Sidekick UI acceptance requires direct provider egress; unset ${egressOverrides.join(", ")}`,
    );
  }
  const temporaryRoot = await fs.mkdtemp(path.join(os.tmpdir(), "minutes-sidekick-ui-"));
  let child = null;
  let timeout = null;
  let forceKill = null;
  let outcome = null;
  let processGroupEmpty = false;
  let providerCopyPostSha256 = null;
  let providerCopyPath = null;
  const processGroupExists = () => {
    if (!child?.pid) return false;
    try {
      process.kill(-child.pid, 0);
      return true;
    } catch (error) {
      return error?.code === "EPERM";
    }
  };
  const terminateProcessGroup = async () => {
    if (!processGroupExists()) return true;
    try { process.kill(-child.pid, "SIGTERM"); } catch {}
    for (let attempt = 0; attempt < 30 && processGroupExists(); attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    if (processGroupExists()) {
      try { process.kill(-child.pid, "SIGKILL"); } catch {}
      for (let attempt = 0; attempt < 20 && processGroupExists(); attempt += 1) {
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    }
    return !processGroupExists();
  };
  try {
    const isolatedHome = path.join(temporaryRoot, "home");
    const isolatedTmp = path.join(temporaryRoot, "tmp");
    const minutesDirectory = path.join(isolatedHome, ".minutes");
    const reportPath = path.join(minutesDirectory, "native-sidekick-ui-acceptance.json");
    const providerDirectory = path.join(temporaryRoot, "provider");
    providerCopyPath = path.join(providerDirectory, "codex");
    const nonce = randomBytes(32).toString("hex");
    await fs.mkdir(minutesDirectory, { recursive: true, mode: 0o700 });
    await fs.mkdir(isolatedTmp, { recursive: true, mode: 0o700 });
    await fs.mkdir(providerDirectory, { mode: 0o700 });
    await fs.copyFile(runtime.source_provider_path, providerCopyPath);
    await fs.chmod(providerCopyPath, 0o500);
    const providerCopySha256 = await fs.readFile(providerCopyPath).then(sha256);
    const providerCopyVersion = execFileSync(providerCopyPath, ["--version"], {
      encoding: "utf8",
    }).trim();
    if (providerCopySha256 !== runtime.source_provider_sha256 ||
        providerCopyVersion !== runtime.source_provider_version) {
      throw new Error("the private provider copy did not match its source preflight");
    }
    runtime = {
      ...runtime,
      expected_provider_path: providerCopyPath,
      expected_provider_sha256: providerCopySha256,
      expected_provider_version: providerCopyVersion,
      provider_copy_is_private: true,
    };
    await fs.writeFile(
      path.join(minutesDirectory, "native-sidekick-ui-acceptance.challenge"),
      nonce,
      { mode: 0o600 },
    );
    await writeIsolatedConfig(isolatedHome);
    const realHome = os.homedir();
    child = spawn(executable, [
      "--diagnose-native-sidekick-ui",
      "--consent-cloud",
      "--consent-microphone",
      "--consent-screen",
      "--acceptance-nonce",
      nonce,
      "--acceptance-parent-fd",
      "3",
      "--acceptance-real-home",
      realHome,
    ], {
      env: {
        ...process.env,
        HOME: isolatedHome,
        TMPDIR: isolatedTmp,
        XDG_CONFIG_HOME: path.join(isolatedHome, ".config"),
        CODEX_HOME: process.env.CODEX_HOME || path.join(realHome, ".codex"),
        PATH: `${providerDirectory}:${process.env.PATH || "/usr/bin:/bin"}`,
      },
      stdio: ["ignore", "pipe", "pipe", "pipe"],
      detached: true,
    });
    // fd 3 is a parent-owned lease. The child blocks on its peer; if this
    // runner crashes or disconnects, the app-side watchdog sees EOF and stops
    // mic, screen, Sidekick, and the provider process itself.
    const parentLease = child.stdio[3];
    parentLease.on("error", () => {});
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    const startedAt = performance.now();
    const killProcessGroup = (signal) => {
      if (!child?.pid) return;
      try {
        process.kill(-child.pid, signal);
      } catch {
        child.kill(signal);
      }
    };
    timeout = setTimeout(() => {
      timedOut = true;
      // EOF asks the app-owned watchdog to perform its full teardown. Give it
      // longer than the Sidekick + recording teardown budget before the last-
      // resort process-group kill, so a stuck provider cannot be orphaned.
      parentLease.destroy();
      forceKill = setTimeout(() => killProcessGroup("SIGKILL"), FORCED_CLEANUP_GRACE_MS);
    }, HARD_TIMEOUT_MS);
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      if (Buffer.byteLength(stdout) > MAX_OUTPUT_BYTES) parentLease.destroy();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
      if (Buffer.byteLength(stderr) > MAX_OUTPUT_BYTES) parentLease.destroy();
    });
    const exitCode = await new Promise((resolve, reject) => {
      child.once("error", reject);
      child.once("close", (code, signal) => resolve(code ?? (signal ? 128 : 1)));
    });
    clearTimeout(timeout);
    timeout = null;
    if (forceKill) clearTimeout(forceKill);
    forceKill = null;
    processGroupEmpty = await terminateProcessGroup();
    if (!processGroupEmpty) {
      throw new Error("native Sidekick UI acceptance left a detached process in its process group");
    }
    providerCopyPostSha256 = await fs.readFile(providerCopyPath).then(sha256);
    const wallMs = Math.round(performance.now() - startedAt);
    if (timedOut) throw new Error(`native Sidekick UI acceptance exceeded ${HARD_TIMEOUT_MS}ms`);
    const payload = JSON.parse(await fs.readFile(reportPath, "utf8"));
    outcome = {
      payload,
      runtime: {
        ...runtime,
        exit_code: exitCode,
        wall_ms: wallMs,
        stderr: stderr.trim().slice(0, 4_000),
        stdout: stdout.trim().slice(0, 1_000),
      },
    };
  } finally {
    if (timeout) clearTimeout(timeout);
    if (forceKill) clearTimeout(forceKill);
    if (child && child.exitCode === null) {
      child.stdio[3]?.destroy();
      try {
        process.kill(-child.pid, "SIGTERM");
      } catch {
        child.kill("SIGTERM");
      }
    }
    if (child) processGroupEmpty = await terminateProcessGroup();
    if (providerCopyPath) {
      providerCopyPostSha256 = await fs.readFile(providerCopyPath).then(sha256).catch(() => null);
    }
    await fs.rm(temporaryRoot, { recursive: true, force: true });
  }
  outcome.runtime.temporary_root_removed = true;
  outcome.runtime.process_group_empty = processGroupEmpty;
  outcome.runtime.provider_process_cleanup_scope = "original_app_process_group";
  outcome.runtime.provider_copy_post_sha256 = providerCopyPostSha256;
  return outcome;
}

async function main(argv) {
  const canonicalApp = canonicalInstalledAppPath();
  if (argv.length > 2) {
    throw new Error(`installed native Sidekick UI acceptance only runs against ${canonicalApp}`);
  }
  await validateCanonicalInstalledApp(canonicalApp, canonicalApp);
  const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
  const expectedBuildCommit = execFileSync("git", ["rev-parse", "--verify", "HEAD"], {
    cwd: repositoryRoot,
    encoding: "utf8",
  }).trim().toLowerCase();
  const checkoutStatus = execFileSync(
    "git",
    ["status", "--porcelain=v1", "--untracked-files=all"],
    { cwd: repositoryRoot, encoding: "utf8" },
  ).trim();
  const allowedGeneratedPaths = currentGeneratedBuildHelperPaths(repositoryRoot);
  const relevantDirtyLines = checkoutStatus.split("\n").filter(Boolean).filter((line) =>
    !allowedGeneratedPaths.some((generatedPath) => line.slice(3) === generatedPath),
  );
  if (relevantDirtyLines.length > 0) {
    throw new Error(`UI acceptance requires committed application and harness source; dirty paths:\n${relevantDirtyLines.join("\n")}`);
  }
  const signedBuildApp = path.join(
    repositoryRoot,
    "target",
    "release",
    "bundle",
    "macos",
    "Minutes Dev.app",
  );
  const executable = path.join(canonicalApp, "Contents", "MacOS", "minutes-app");
  const signedBuildExecutable = path.join(signedBuildApp, "Contents", "MacOS", "minutes-app");
  const providerPath = await fs.realpath(execFileSync("/usr/bin/which", ["codex"], {
    encoding: "utf8",
  }).trim());
  const providerVersion = execFileSync(providerPath, ["--version"], {
    encoding: "utf8",
  }).trim();
  execFileSync("codesign", ["--verify", "--deep", "--strict", signedBuildApp]);
  execFileSync("codesign", ["--verify", "--deep", "--strict", canonicalApp]);
  const [
    executableSha256,
    bundleSha256,
    expectedExecutableSha256,
    expectedBundleSha256,
  ] = await Promise.all([
    fs.readFile(executable).then(sha256),
    bundleManifestSha256(canonicalApp),
    fs.readFile(signedBuildExecutable).then(sha256),
    bundleManifestSha256(signedBuildApp),
  ]);
  const { payload, runtime } = await runInstalledUi(executable, {
    executable_sha256: executableSha256,
    bundle_sha256: bundleSha256,
    expected_executable_sha256: expectedExecutableSha256,
    expected_bundle_sha256: expectedBundleSha256,
    expected_build_commit: expectedBuildCommit,
    source_provider_path: providerPath,
    source_provider_sha256: await fs.readFile(providerPath).then(sha256),
    source_provider_version: providerVersion,
  });
  const report = evaluateNativeSidekickUiAcceptance(payload, runtime);
  report.tested_app_path = canonicalApp;
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  return report.passed ? 0 : 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = await main(process.argv);
  } catch (error) {
    process.stderr.write(`${error.stack || error}\n`);
    process.exitCode = 1;
  }
}
