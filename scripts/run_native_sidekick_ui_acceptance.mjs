#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import { randomBytes, createHash } from "node:crypto";
import { constants as fsConstants, statSync } from "node:fs";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { scoreMeridianResponses } from "../tests/eval/sidekick_rehearsal_golden.mjs";
import { CODEX_REALTIME_MODEL, CODEX_VERIFIER_MODEL } from "./lib/sidekick_provider.mjs";
import {
  runAndLoadSidekickHybridQualityArtifact,
  sidekickHybridQualityReceiptPasses,
} from "./lib/sidekick_hybrid_quality_gate.mjs";
import {
  runSidekickExactSemanticGate,
  sidekickExactSemanticReceiptPasses,
} from "./lib/sidekick_exact_semantic_gate.mjs";
import { currentSidekickQualitySourceBinding } from "./lib/sidekick_quality_source_binding.mjs";
import {
  attestSidekickProviderExecutable,
  sidekickProviderAttestationMatches,
} from "./lib/sidekick_provider_attestation.mjs";
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

export async function canonicalExistingPath(filePath) {
  return fs.realpath(filePath);
}

function check(name, passed, detail) {
  return { name, passed: Boolean(passed), detail };
}

const canonicalFixtureBytes = await fs.readFile(CANONICAL_FIXTURE_PATH);
const canonicalFixture = JSON.parse(canonicalFixtureBytes.toString("utf8"));
if (!Array.isArray(canonicalFixture.turns) || canonicalFixture.turns.length !== 2) {
  throw new Error("canonical Meridian UI fixture must contain exactly two turns");
}

function canonicalEvidenceId(id) {
  const sequence = String(id).match(/-(\d+)$/)?.[1];
  return sequence ? `utterance-${sequence}` : String(id);
}

function semanticResponsesFromUiPayload(payload) {
  const turns = Array.isArray(payload?.turns) ? payload.turns : [];
  const evidenceIds = (index) => Array.isArray(
    turns[index]?.candidate_evidence?.transcriptEvidenceIds,
  ) ? turns[index].candidate_evidence.transcriptEvidenceIds.map(canonicalEvidenceId) : [];
  return {
    turn_1: { text: String(turns[0]?.response ?? ""), evidence_ids: evidenceIds(0) },
    turn_2: { text: String(turns[1]?.response ?? ""), evidence_ids: evidenceIds(1) },
  };
}

export function evaluateNativeSidekickUiAcceptance(payload, runtime) {
  const turns = Array.isArray(payload?.turns) ? payload.turns : [];
  const turnOneCandidateEvidence = Array.isArray(turns[0]?.candidate_evidence?.transcriptEvidenceIds)
    ? turns[0].candidate_evidence.transcriptEvidenceIds
    : [];
  const turnTwoCandidateEvidence = Array.isArray(turns[1]?.candidate_evidence?.transcriptEvidenceIds)
    ? turns[1].candidate_evidence.transcriptEvidenceIds
    : [];
  const semanticResponses = semanticResponsesFromUiPayload(payload);
  const quality = scoreMeridianResponses({
    turn_1: turns[0]?.response ?? "",
    turn_2: turns[1]?.response ?? "",
    turn_1_evidence_ids: turnOneCandidateEvidence.map(canonicalEvidenceId),
    turn_2_evidence_ids: turnTwoCandidateEvidence.map(canonicalEvidenceId),
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
      "calibrated_hybrid_quality_artifact",
      sidekickHybridQualityReceiptPasses(
        runtime.hybrid_quality_gate,
        runtime.quality_source_binding,
        runtime.quality_provider_executable,
      ) && runtime.hybrid_quality_gate?.producer_attested === true,
      "Semantic quality requires a fresh three-run evaluator witnessed by this acceptance process; this UI boundary also owns exact source, mechanical, provenance, and painted-product checks.",
    ),
    check(
      "exact_ui_responses_pass_semantic_judge",
      sidekickExactSemanticReceiptPasses(
        runtime.exact_semantic_quality_gate,
        semanticResponses,
        runtime.quality_source_binding,
        runtime.quality_provider_executable,
      ),
      "The calibrated semantic judge must grade these exact painted candidate bytes, not unrelated prior responses.",
    ),
    check(
      "quality_source_matches_current_build",
      runtime.quality_source_binding?.git_commit === runtime.expected_build_commit,
      "Quality prompts, evaluator, fixture, and engine must be bound to the installed build commit.",
    ),
    check(
      "one_attested_provider_for_product_and_quality_gates",
      payload?.sidekick?.provider_executable_path === runtime.quality_provider_executable?.path &&
        payload?.sidekick?.provider_executable_sha256 ===
          runtime.quality_provider_executable?.sha256 &&
        payload?.sidekick?.provider_version === runtime.quality_provider_executable?.version &&
        sidekickProviderAttestationMatches(
          runtime.hybrid_quality_gate?.provider_executable,
          runtime.quality_provider_executable,
        ) &&
        sidekickProviderAttestationMatches(
          runtime.exact_semantic_quality_gate?.provider_executable,
          runtime.quality_provider_executable,
        ),
      "The signed app, fresh evaluator, verifier, and exact semantic judge must use the same canonical private provider executable bytes.",
    ),
    check(
      "installed_app_exit_zero",
      runtime.launch_services_exit_code === 0 &&
        runtime.app_exit_receipt_verified === true &&
        runtime.app_exit_code === 0,
      `LaunchServices exited ${runtime.launch_services_exit_code}; the verified app exit was ${runtime.app_exit_code}.`,
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
        payload?.bundle_identifier === "com.useminutes.desktop.dev" &&
        runtime.launch_method === "macos_launch_services",
      "The check must launch the real Tauri dev app through macOS LaunchServices and traverse the native Sidekick window.",
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
      "fresh_independent_verifier_receipt_per_visible_turn",
      payload?.sidekick?.verifier_sessions_started === turns.length &&
        payload?.sidekick?.verifier_requested_contract?.provider === "codex-app-server" &&
        payload?.sidekick?.verifier_requested_contract?.model === CODEX_VERIFIER_MODEL &&
        payload?.sidekick?.verifier_requested_contract?.privacy === "cloud" &&
        turns.length === 2 &&
        turns.every((turn) =>
          /^[a-f0-9]{64}$/.test(turn?.candidate_evidence?.candidateSha256 ?? "") &&
          turn?.candidate_evidence?.candidateDigestVerified === true &&
          turn?.candidate_evidence?.verificationVerdict?.decision === "allow" &&
          turn?.candidate_evidence?.verificationVerdict?.reason_code === "supported" &&
          /^[a-f0-9]{64}$/.test(
            turn?.candidate_evidence?.verifierSessionCorrelation ?? "",
          )) &&
        new Set(turns.map((turn) =>
          turn.candidate_evidence.verifierSessionCorrelation)).size === turns.length,
      "Every painted candidate must be digest-bound to an allow/supported verdict from its own one-time verifier session, without synchronously prewarming an unrelated future slot.",
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
        payload?.sidekick?.provider_requested_contract?.model === CODEX_REALTIME_MODEL &&
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
              candidateTranscriptIds?.includes(approvedTranscriptIds[5]);
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
        runtime.provider_process_cleanup_scope === "app_teardown_launchservices_wait_exact_executable_scan" &&
        Array.isArray(runtime.app_processes_remaining) && runtime.app_processes_remaining.length === 0 &&
        Array.isArray(runtime.provider_processes_remaining) && runtime.provider_processes_remaining.length === 0 &&
        Array.isArray(runtime.forced_process_signals) && runtime.forced_process_signals.length === 0 &&
        payload?.teardown?.cleanup_complete === true,
      "The diagnostic must prove complete recording, screen-worker, context, scratch-file, app, and provider teardown after the LaunchServices app exits.",
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
  const checks = [...sourceChecks, ...quality.mechanical_checks, ...paintChecks];
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
    quality_checks: quality.mechanical_checks,
    semantic_diagnostics: quality.checks.filter((item) =>
      !quality.mechanical_checks.some((mechanical) => mechanical.name === item.name)),
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

export function nativeSidekickLaunchServicesArgs({
  app,
  appStdoutPath,
  appStderrPath,
  parentLeasePath,
  isolatedHome,
  isolatedTmp,
  codeHome,
  providerDirectory,
  inheritedPath,
  nonce,
  realHome,
}) {
  const environment = {
    HOME: isolatedHome,
    TMPDIR: isolatedTmp,
    XDG_CONFIG_HOME: path.join(isolatedHome, ".config"),
    CODEX_HOME: codeHome,
    PATH: `${providerDirectory}:${inheritedPath || "/usr/bin:/bin"}`,
  };
  return [
    "-n",
    "-W",
    "-i",
    parentLeasePath,
    "-o",
    appStdoutPath,
    "--stderr",
    appStderrPath,
    ...Object.entries(environment).flatMap(([key, value]) => ["--env", `${key}=${value}`]),
    app,
    "--args",
    "--diagnose-native-sidekick-ui",
    "--consent-cloud",
    "--consent-microphone",
    "--consent-screen",
    "--acceptance-nonce",
    nonce,
    "--acceptance-parent-fd",
    "0",
    "--acceptance-real-home",
    realHome,
  ];
}

export function nativeSidekickTemporaryParent(platform = process.platform, defaultTmp = os.tmpdir()) {
  return platform === "darwin" ? "/tmp" : defaultTmp;
}

export function nativeSidekickFailureWithLogs(error, streams) {
  const diagnostics = [
    ["LaunchServices stderr", streams.launchServicesStderr, 4_000],
    ["LaunchServices stdout", streams.launchServicesStdout, 1_000],
    ["Minutes stderr", streams.appStderr, 4_000],
    ["Minutes stdout", streams.appStdout, 1_000],
  ].flatMap(([label, value, limit]) => {
    const text = String(value || "").trim();
    return text ? [`${label}:\n${text.slice(0, limit)}`] : [];
  });
  if (diagnostics.length === 0) return error;
  return new Error(`${error.message}\n\n${diagnostics.join("\n\n")}`, { cause: error });
}

export function appendBoundedOutput(current, chunk, limit) {
  const input = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
  const remaining = Math.max(0, limit - current.length);
  return {
    bytes: remaining > 0
      ? Buffer.concat([current, input.subarray(0, remaining)], current.length + Math.min(input.length, remaining))
      : current,
    overflowed: input.length > remaining,
  };
}

export async function readBoundedOutputFile(filePath, limit) {
  const file = await fs.open(filePath, "r");
  try {
    const buffer = Buffer.alloc(limit + 1);
    let bytesRead = 0;
    while (bytesRead < buffer.length) {
      const result = await file.read(buffer, bytesRead, buffer.length - bytesRead, bytesRead);
      if (result.bytesRead === 0) break;
      bytesRead += result.bytesRead;
    }
    return {
      text: buffer.subarray(0, Math.min(bytesRead, limit)).toString("utf8"),
      overflowed: bytesRead > limit,
    };
  } finally {
    await file.close();
  }
}

export function singleFlightAsync(action) {
  let promise = null;
  return () => {
    promise ||= Promise.resolve().then(action);
    return promise;
  };
}

export function parseLsofTextIdentities(output) {
  const identities = [];
  let record = null;
  const flush = () => {
    if (!record) return;
    if (record.device === undefined || record.inode === undefined) {
      throw new Error(
        `lsof text record ${record.descriptor || "unknown"} is missing device or inode identity`,
      );
    }
    identities.push(record);
  };
  for (const line of output.split("\n")) {
    if (line.startsWith("f")) {
      flush();
      record = { descriptor: line.slice(1) };
    } else if (record && line.startsWith("D")) {
      record.device = BigInt(line.slice(1)).toString();
    } else if (record && line.startsWith("i")) {
      record.inode = BigInt(line.slice(1)).toString();
    } else if (record && line.startsWith("n")) {
      record.path = line.slice(1);
    }
  }
  flush();
  return identities;
}

function exactExecutablePids(executable) {
  const expected = statSync(executable, { bigint: true });
  const processName = path.basename(executable);
  let candidates = "";
  try {
    candidates = execFileSync("/usr/bin/pgrep", ["-x", processName], { encoding: "utf8" });
  } catch (error) {
    if (error?.status === 1) return [];
    throw error;
  }
  const matches = [];
  for (const candidate of candidates.split("\n").filter(Boolean)) {
    if (!/^\d+$/.test(candidate)) continue;
    let textFiles = "";
    try {
      textFiles = execFileSync("/usr/sbin/lsof", ["-a", "-p", candidate, "-d", "txt", "-F", "pDfni"], {
        encoding: "utf8",
      });
    } catch (error) {
      try {
        process.kill(Number(candidate), 0);
      } catch (probeError) {
        if (probeError?.code === "ESRCH") continue;
      }
      throw new Error(`could not inspect live ${processName} candidate PID ${candidate}: ${error.message}`);
    }
    const identities = parseLsofTextIdentities(textFiles);
    if (identities.length === 0) {
      throw new Error(`could not identify live ${processName} candidate PID ${candidate}: lsof returned no device/inode identity`);
    }
    const matched = identities.some((identity) =>
      identity.device === expected.dev.toString() && identity.inode === expected.ino.toString());
    if (matched) matches.push(Number(candidate));
  }
  return matches;
}

const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

export async function terminateNewExactProcesses({
  executable,
  baselinePids = [],
  scan = exactExecutablePids,
  signal = (pid, name) => process.kill(pid, name),
  pause = wait,
}) {
  const baseline = new Set(baselinePids);
  const signals = [];
  const remaining = () => scan(executable).filter((pid) => !baseline.has(pid));
  const waitUntilEmpty = async (attempts) => {
    for (let attempt = 0; attempt < attempts; attempt += 1) {
      const pids = remaining();
      if (pids.length === 0) return [];
      await pause(250);
    }
    return remaining();
  };
  let pids = await waitUntilEmpty(20);
  for (const pid of pids) {
    try {
      signal(pid, "SIGTERM");
      signals.push({ pid, signal: "SIGTERM" });
    } catch (error) {
      if (error?.code !== "ESRCH") throw error;
    }
  }
  pids = await waitUntilEmpty(12);
  for (const pid of pids) {
    try {
      signal(pid, "SIGKILL");
      signals.push({ pid, signal: "SIGKILL" });
    } catch (error) {
      if (error?.code !== "ESRCH") throw error;
    }
  }
  return { remaining: await waitUntilEmpty(8), signals };
}

export async function cleanupNativeSidekickProcessLanes({
  appExecutable,
  appBaselinePids = [],
  providerExecutable,
  providerBaselinePids = [],
  terminate = terminateNewExactProcesses,
}) {
  const result = {
    app: { remaining: [], signals: [] },
    provider: { remaining: [], signals: [] },
    errors: [],
    retainTemporaryRoot: false,
  };
  for (const lane of [
    { name: "app", executable: appExecutable, baselinePids: appBaselinePids },
    { name: "provider", executable: providerExecutable, baselinePids: providerBaselinePids },
  ]) {
    try {
      const cleanup = await terminate({
        executable: lane.executable,
        baselinePids: lane.baselinePids,
      });
      result[lane.name] = cleanup;
      if (cleanup.remaining.length > 0) {
        result.retainTemporaryRoot = true;
        result.errors.push(new Error(
          `exact ${lane.name} processes remained active: ${cleanup.remaining.join(",")}`,
        ));
      }
    } catch (error) {
      result.retainTemporaryRoot = true;
      result.errors.push(new Error(`exact ${lane.name} cleanup failed: ${error.message}`));
    }
  }
  return result;
}

async function runInstalledUi(app, runtime) {
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
  // The app-server capture relay uses a Unix-domain socket. macOS limits
  // sockaddr_un paths to 104 bytes, while os.tmpdir() normally expands to a
  // long /var/folders/... path. A private mkdtemp directly under /tmp keeps
  // the exact-session socket below that hard platform limit.
  const temporaryParent = nativeSidekickTemporaryParent();
  const temporaryRoot = await fs.mkdtemp(path.join(temporaryParent, "minutes-sidekick-ui-"));
  await fs.chmod(temporaryRoot, 0o700);
  let child = null;
  let timeout = null;
  let forceKill = null;
  let outcome = null;
  let processGroupEmpty = true;
  let providerCopyPostSha256 = null;
  let providerCopyPath = null;
  let parentLease = null;
  let parentLeaseCloseOperation = null;
  let parentLeaseCloseError = null;
  let processScanReady = false;
  let launchStarted = false;
  let appBaselinePids = [];
  let providerBaselinePids = [];
  let appProcessesRemaining = [];
  let providerProcessesRemaining = [];
  let forcedProcessSignals = [];
  let temporaryRootRemoved = false;
  let primaryError = null;
  let appStdoutPath = null;
  let appStderrPath = null;
  let launchServicesStdout = Buffer.alloc(0);
  let launchServicesStderr = Buffer.alloc(0);
  let launchServicesStdoutOverflowed = false;
  let launchServicesStderrOverflowed = false;
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
  const closeParentLease = () => {
    return parentLeaseCloseOperation ? parentLeaseCloseOperation() : Promise.resolve();
  };
  const requestParentLeaseClose = () => {
    void closeParentLease().catch((error) => {
      parentLeaseCloseError ||= error;
    });
  };
  try {
    const isolatedHome = path.join(temporaryRoot, "home");
    const isolatedTmp = path.join(temporaryRoot, "tmp");
    const minutesDirectory = path.join(isolatedHome, ".minutes");
    const reportPath = path.join(minutesDirectory, "native-sidekick-ui-acceptance.json");
    const exitReceiptPath = path.join(minutesDirectory, "native-sidekick-ui-acceptance.exit.json");
    appStdoutPath = path.join(temporaryRoot, "app.stdout.log");
    appStderrPath = path.join(temporaryRoot, "app.stderr.log");
    const parentLeasePath = path.join(temporaryRoot, "parent-lease.fifo");
    const providerDirectory = runtime.reuse_source_provider_copy
      ? path.dirname(runtime.source_provider_path)
      : path.join(temporaryRoot, "provider");
    providerCopyPath = runtime.reuse_source_provider_copy
      ? runtime.source_provider_path
      : path.join(providerDirectory, "codex");
    const nonce = randomBytes(32).toString("hex");
    await fs.mkdir(minutesDirectory, { recursive: true, mode: 0o700 });
    await fs.mkdir(isolatedTmp, { recursive: true, mode: 0o700 });
    if (!runtime.reuse_source_provider_copy) {
      await fs.mkdir(providerDirectory, { mode: 0o700 });
    }
    await fs.writeFile(appStdoutPath, "", { mode: 0o600 });
    await fs.writeFile(appStderrPath, "", { mode: 0o600 });
    execFileSync("/usr/bin/mkfifo", [parentLeasePath]);
    await fs.chmod(parentLeasePath, 0o600);
    parentLease = await fs.open(
      parentLeasePath,
      fsConstants.O_RDWR | fsConstants.O_NONBLOCK,
    );
    parentLeaseCloseOperation = singleFlightAsync(async () => {
      const lease = parentLease;
      parentLease = null;
      if (!lease) return;
      try {
        await lease.close();
      } catch (error) {
        if (error?.code !== "EBADF") throw error;
      }
    });
    if (!runtime.reuse_source_provider_copy) {
      await fs.copyFile(runtime.source_provider_path, providerCopyPath);
      await fs.chmod(providerCopyPath, 0o500);
    }
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
      // macOS exposes the same temporary directory through both /tmp and
      // /private/tmp. Compare the provider's own canonical path report with a
      // canonical host path so the attestation fails only for a real identity
      // mismatch, not an OS path alias.
      expected_provider_path: await canonicalExistingPath(providerCopyPath),
      expected_provider_sha256: providerCopySha256,
      expected_provider_version: providerCopyVersion,
      provider_copy_is_private: true,
    };
    appBaselinePids = exactExecutablePids(runtime.executable_path);
    providerBaselinePids = exactExecutablePids(providerCopyPath);
    processScanReady = true;
    await fs.writeFile(
      path.join(minutesDirectory, "native-sidekick-ui-acceptance.challenge"),
      nonce,
      { mode: 0o600 },
    );
    await writeIsolatedConfig(isolatedHome);
    const realHome = os.homedir();
    const launchArgs = nativeSidekickLaunchServicesArgs({
      app,
      appStdoutPath,
      appStderrPath,
      parentLeasePath,
      isolatedHome,
      isolatedTmp,
      codeHome: process.env.CODEX_HOME || path.join(realHome, ".codex"),
      providerDirectory,
      inheritedPath: process.env.PATH,
      nonce,
      realHome,
    });
    child = spawn("/usr/bin/open", launchArgs, {
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
      detached: true,
    });
    launchStarted = true;
    // LaunchServices makes Minutes Dev—not Terminal or Node—the TCC-responsible
    // process. A private named pipe is required because macOS 26 LaunchServices
    // rejects /dev/fd aliases with -10810. The parent keeps the only writer
    // open; if it crashes or closes the lease, the app watchdog sees stdin EOF
    // and stops mic, screen, Sidekick, and the provider process itself.
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
      requestParentLeaseClose();
      forceKill = setTimeout(() => killProcessGroup("SIGKILL"), FORCED_CLEANUP_GRACE_MS);
    }, HARD_TIMEOUT_MS);
    child.stdout.on("data", (chunk) => {
      const output = appendBoundedOutput(launchServicesStdout, chunk, MAX_OUTPUT_BYTES);
      launchServicesStdout = output.bytes;
      launchServicesStdoutOverflowed ||= output.overflowed;
      if (launchServicesStdoutOverflowed) requestParentLeaseClose();
    });
    child.stderr.on("data", (chunk) => {
      const output = appendBoundedOutput(launchServicesStderr, chunk, MAX_OUTPUT_BYTES);
      launchServicesStderr = output.bytes;
      launchServicesStderrOverflowed ||= output.overflowed;
      if (launchServicesStderrOverflowed) requestParentLeaseClose();
    });
    const exitCode = await new Promise((resolve, reject) => {
      child.once("error", reject);
      child.once("close", (code, signal) => resolve(code ?? (signal ? 128 : 1)));
    });
    clearTimeout(timeout);
    timeout = null;
    if (forceKill) clearTimeout(forceKill);
    forceKill = null;
    processGroupEmpty = processGroupEmpty && await terminateProcessGroup();
    if (!processGroupEmpty) {
      throw new Error("native Sidekick UI acceptance left its LaunchServices wrapper running");
    }
    providerCopyPostSha256 = await fs.readFile(providerCopyPath).then(sha256);
    const wallMs = Math.round(performance.now() - startedAt);
    if (timedOut) throw new Error(`native Sidekick UI acceptance exceeded ${HARD_TIMEOUT_MS}ms`);
    const [appStdoutOutput, appStderrOutput] = await Promise.all([
      readBoundedOutputFile(appStdoutPath, MAX_OUTPUT_BYTES),
      readBoundedOutputFile(appStderrPath, MAX_OUTPUT_BYTES),
    ]);
    if (appStdoutOutput.overflowed || appStderrOutput.overflowed ||
        launchServicesStdoutOverflowed || launchServicesStderrOverflowed) {
      throw new Error("native Sidekick UI acceptance app output exceeded its bounded limit");
    }
    const appStdout = appStdoutOutput.text;
    const appStderr = appStderrOutput.text;
    const reportBytes = await fs.readFile(reportPath);
    const payload = JSON.parse(reportBytes.toString("utf8"));
    const receipt = JSON.parse(await fs.readFile(exitReceiptPath, "utf8"));
    const appExitReceiptVerified = receipt?.schema_version === 1 &&
      receipt?.mode === "diagnose-native-sidekick-ui-exit" &&
      receipt?.nonce_sha256 === sha256(nonce) &&
      receipt?.build_commit === runtime.expected_build_commit &&
      Number.isInteger(receipt?.pid) && receipt.pid > 0 &&
      receipt.pid === payload?.process_id &&
      Number.isInteger(receipt?.exit_code) &&
      receipt?.report_sha256 === sha256(reportBytes);
    if (!appExitReceiptVerified) {
      throw new Error("native Sidekick UI acceptance exit receipt did not bind the app, report, build, and nonce");
    }
    outcome = {
      payload,
      runtime: {
        ...runtime,
        exit_code: receipt.exit_code,
        app_exit_code: receipt.exit_code,
        app_exit_receipt_verified: true,
        wall_ms: wallMs,
        launch_method: "macos_launch_services",
        launch_services_exit_code: exitCode,
        stderr: [appStderr, launchServicesStderr.toString("utf8")].filter(Boolean).join("\n").trim().slice(0, 4_000),
        stdout: [appStdout, launchServicesStdout.toString("utf8")].filter(Boolean).join("\n").trim().slice(0, 1_000),
      },
    };
  } catch (error) {
    const [appStdout, appStderr] = await Promise.all([
      appStdoutPath
        ? readBoundedOutputFile(appStdoutPath, MAX_OUTPUT_BYTES).catch(() => ({ text: "", overflowed: false }))
        : { text: "", overflowed: false },
      appStderrPath
        ? readBoundedOutputFile(appStderrPath, MAX_OUTPUT_BYTES).catch(() => ({ text: "", overflowed: false }))
        : { text: "", overflowed: false },
    ]);
    primaryError = nativeSidekickFailureWithLogs(error, {
      launchServicesStdout: launchServicesStdout.toString("utf8"),
      launchServicesStderr: launchServicesStderr.toString("utf8"),
      appStdout: `${appStdout.text}${appStdout.overflowed ? "\n[output truncated at bounded limit]" : ""}`,
      appStderr: `${appStderr.text}${appStderr.overflowed ? "\n[output truncated at bounded limit]" : ""}`,
    });
  } finally {
    const cleanupErrors = [];
    let retainTemporaryRoot = false;
    if (timeout) clearTimeout(timeout);
    if (forceKill) clearTimeout(forceKill);
    try {
      await closeParentLease();
    } catch (error) {
      parentLeaseCloseError ||= error;
    }
    if (parentLeaseCloseError) {
      cleanupErrors.push(new Error(
        `acceptance parent lease cleanup failed: ${parentLeaseCloseError.message}`,
      ));
    }
    if (child && child.exitCode === null) {
      try {
        process.kill(-child.pid, "SIGTERM");
      } catch (error) {
        try {
          child.kill("SIGTERM");
        } catch (fallbackError) {
          cleanupErrors.push(new Error(
            `could not request LaunchServices wrapper shutdown: ${error.message}; fallback: ${fallbackError.message}`,
          ));
        }
      }
    }
    if (child) {
      try {
        const wrapperGroupEmpty = await terminateProcessGroup();
        processGroupEmpty = processGroupEmpty && wrapperGroupEmpty;
        if (!wrapperGroupEmpty) {
          retainTemporaryRoot = true;
          cleanupErrors.push(new Error("LaunchServices wrapper process group remained active"));
        }
      } catch (error) {
        processGroupEmpty = false;
        retainTemporaryRoot = true;
        cleanupErrors.push(new Error(`LaunchServices wrapper cleanup failed: ${error.message}`));
      }
    }
    if (launchStarted && processScanReady) {
      const exactCleanup = await cleanupNativeSidekickProcessLanes({
        appExecutable: runtime.executable_path,
        appBaselinePids,
        providerExecutable: providerCopyPath,
        providerBaselinePids,
      });
      appProcessesRemaining = exactCleanup.app.remaining;
      providerProcessesRemaining = exactCleanup.provider.remaining;
      forcedProcessSignals = [
        ...exactCleanup.app.signals.map((item) => ({ scope: "app", ...item })),
        ...exactCleanup.provider.signals.map((item) => ({ scope: "provider", ...item })),
      ];
      if (exactCleanup.errors.length > 0) {
        processGroupEmpty = false;
        cleanupErrors.push(...exactCleanup.errors);
      }
      retainTemporaryRoot = retainTemporaryRoot || exactCleanup.retainTemporaryRoot;
      processGroupEmpty = processGroupEmpty &&
        appProcessesRemaining.length === 0 && providerProcessesRemaining.length === 0;
    }
    if (providerCopyPath) {
      providerCopyPostSha256 = await fs.readFile(providerCopyPath).then(sha256).catch(() => null);
    }
    if (!retainTemporaryRoot) {
      try {
        await fs.rm(temporaryRoot, { recursive: true, force: true });
        temporaryRootRemoved = true;
      } catch (error) {
        cleanupErrors.push(new Error(`private acceptance root cleanup failed: ${error.message}`));
      }
    }
    if (cleanupErrors.length > 0) {
      const visibleErrors = [primaryError, ...cleanupErrors]
        .filter(Boolean)
        .map((error) => error.message)
        .join("\n\n");
      throw new AggregateError(
        primaryError ? [primaryError, ...cleanupErrors] : cleanupErrors,
        `native Sidekick UI acceptance cleanup was incomplete:\n\n${visibleErrors}`,
        primaryError ? { cause: primaryError } : undefined,
      );
    }
  }
  if (primaryError) throw primaryError;
  outcome.runtime.temporary_root_removed = temporaryRootRemoved;
  outcome.runtime.process_group_empty = processGroupEmpty;
  outcome.runtime.app_processes_remaining = appProcessesRemaining;
  outcome.runtime.provider_processes_remaining = providerProcessesRemaining;
  outcome.runtime.forced_process_signals = forcedProcessSignals;
  outcome.runtime.provider_process_cleanup_scope = "app_teardown_launchservices_wait_exact_executable_scan";
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
  const providerSourcePath = await fs.realpath(execFileSync("/usr/bin/which", ["codex"], {
    encoding: "utf8",
  }).trim());
  const providerSource = await attestSidekickProviderExecutable(providerSourcePath);
  let qualityProviderRoot = null;
  let qualityProviderExecutable;
  let executableSha256;
  let bundleSha256;
  let expectedExecutableSha256;
  let expectedBundleSha256;
  let outcome;
  let qualitySourceBinding;
  let hybridQualityGate;
  let exactSemanticQualityGate;
  try {
    qualityProviderRoot = await fs.mkdtemp(path.join(
      nativeSidekickTemporaryParent(),
      "minutes-sidekick-quality-provider-",
    ));
    await fs.chmod(qualityProviderRoot, 0o700);
    const qualityProviderPath = path.join(qualityProviderRoot, "codex");
    await fs.copyFile(providerSource.path, qualityProviderPath);
    await fs.chmod(qualityProviderPath, 0o500);
    qualityProviderExecutable = await attestSidekickProviderExecutable(qualityProviderPath);
    if (
      qualityProviderExecutable.sha256 !== providerSource.sha256 ||
      qualityProviderExecutable.version !== providerSource.version
    ) {
      throw new Error("private Sidekick quality provider copy failed source attestation");
    }
    execFileSync("codesign", ["--verify", "--deep", "--strict", signedBuildApp]);
    execFileSync("codesign", ["--verify", "--deep", "--strict", canonicalApp]);
    [
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
    outcome = await runInstalledUi(canonicalApp, {
      executable_path: executable,
      executable_sha256: executableSha256,
      bundle_sha256: bundleSha256,
      expected_executable_sha256: expectedExecutableSha256,
      expected_bundle_sha256: expectedBundleSha256,
      expected_build_commit: expectedBuildCommit,
      source_provider_path: qualityProviderExecutable.path,
      source_provider_sha256: qualityProviderExecutable.sha256,
      source_provider_version: qualityProviderExecutable.version,
      reuse_source_provider_copy: true,
    });
    qualitySourceBinding = await currentSidekickQualitySourceBinding(repositoryRoot);
    hybridQualityGate = await runAndLoadSidekickHybridQualityArtifact({
      codexPath: qualityProviderExecutable.path,
    });
    exactSemanticQualityGate = await runSidekickExactSemanticGate({
      fixture: canonicalFixture,
      responses: semanticResponsesFromUiPayload(outcome.payload),
      sourceBinding: qualitySourceBinding,
      codex: qualityProviderExecutable.path,
    });
    const qualityProviderAfter = await attestSidekickProviderExecutable(
      qualityProviderExecutable.path,
    );
    if (!sidekickProviderAttestationMatches(
      qualityProviderAfter,
      qualityProviderExecutable,
    )) {
      throw new Error("private Sidekick provider changed across product and quality gates");
    }
  } finally {
    if (qualityProviderRoot) {
      await fs.rm(qualityProviderRoot, { recursive: true, force: true });
    }
  }
  const { payload, runtime } = outcome;
  runtime.quality_source_binding = qualitySourceBinding;
  runtime.quality_provider_executable = qualityProviderExecutable;
  runtime.quality_provider_copy_removed = true;
  runtime.hybrid_quality_gate = hybridQualityGate;
  runtime.exact_semantic_quality_gate = exactSemanticQualityGate;
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
