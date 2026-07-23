import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const SIDEKICK_SOTA_SCHEMA_VERSION = 1;

export const SIDEKICK_SOTA_EXECUTION_STATUSES = Object.freeze([
  "executable",
  "executable_projection",
  "contract_only",
]);

const TOP_LEVEL_KEYS = new Set([
  "schema_version",
  "id",
  "content_origin",
  "description",
  "domain",
  "execution",
  "privacy",
  "capture",
  "prepared_context",
  "context_evidence",
  "transcript",
  "turns",
  "global_forbidden_behaviors",
]);
const EXECUTION_KEYS = new Set(["status", "reason", "required_capabilities"]);
const PRIVACY_KEYS = new Set([
  "generation_method",
  "source_material",
  "approved_role_tokens",
]);
const CAPTURE_KEYS = new Set(["mode", "screen_context", "expected_speakers"]);
const PREPARED_CONTEXT_KEYS = new Set([
  "user_role",
  "posture",
  "goal",
]);
const CONTEXT_EVIDENCE_KEYS = new Set([
  "id",
  "kind",
  "sensitivity",
  "disclosed",
  "text",
]);
const TRANSCRIPT_KEYS = new Set(["id", "sequence", "speaker", "text"]);
const TURN_KEYS = new Set([
  "id",
  "mode",
  "typed_prompt",
  "transcript_through_id",
  "expected_decision",
  "required_evidence_ids",
  "forbidden_evidence_ids",
  "rubric",
  "forbidden_behaviors",
  "max_words",
]);
const RUBRIC_KEYS = new Set(["id", "description", "critical"]);
const DOMAINS = new Set([
  "board",
  "customer_discovery",
  "engineering",
  "hiring",
  "incident",
  "negotiation",
  "operations",
  "procurement",
  "product",
  "sales",
]);
const CAPTURE_MODES = new Set(["live", "recording"]);
const SCREEN_CONTEXTS = new Set([
  "unavailable",
  "available_not_material",
  "material_fixture",
]);
const CONTEXT_KINDS = new Set([
  "meeting_artifact",
  "repository_result",
  "user_statement",
]);
const SENSITIVITIES = new Set(["unrestricted", "restricted"]);
const TURN_MODES = new Set(["background", "foreground"]);
const DECISIONS = new Set(["silent", "speak"]);
const IDENTIFIER = /^[a-z][a-z0-9_]*$/;

function isNonemptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function isStringArray(value, { nonempty = false } = {}) {
  return Array.isArray(value) &&
    (!nonempty || value.length > 0) &&
    value.every(isNonemptyString);
}

function expectExactKeys(value, expected, location, findings) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    findings.push(`${location}: object_required`);
    return false;
  }
  const actual = new Set(Object.keys(value));
  for (const key of expected) {
    if (!actual.has(key)) findings.push(`${location}.${key}: required_key_missing`);
  }
  for (const key of actual) {
    if (!expected.has(key)) findings.push(`${location}.${key}: unknown_key`);
  }
  return actual.size === expected.size && [...expected].every((key) => actual.has(key));
}

function expectUniqueStrings(value, location, findings, { nonempty = false } = {}) {
  if (!isStringArray(value, { nonempty })) {
    findings.push(`${location}: ${nonempty ? "nonempty_" : ""}string_array_required`);
    return;
  }
  if (new Set(value).size !== value.length) {
    findings.push(`${location}: duplicate_value`);
  }
}

function validateIdentifier(value, location, findings) {
  if (!isNonemptyString(value) || !IDENTIFIER.test(value)) {
    findings.push(`${location}: snake_case_identifier_required`);
  }
}

export function validateSidekickSotaFixture(
  fixture,
  { filename = null } = {},
) {
  const findings = [];
  if (!fixture || typeof fixture !== "object" || Array.isArray(fixture)) {
    return ["$: object_required"];
  }
  expectExactKeys(fixture, TOP_LEVEL_KEYS, "$", findings);
  if (fixture.schema_version !== SIDEKICK_SOTA_SCHEMA_VERSION) {
    findings.push("$.schema_version: unsupported_schema_version");
  }
  if (!isNonemptyString(fixture.id) || !/^synthetic-[a-z0-9-]+$/.test(fixture.id)) {
    findings.push("$.id: synthetic_kebab_identifier_required");
  }
  if (
    filename &&
    isNonemptyString(fixture.id) &&
    path.basename(filename, path.extname(filename)).replaceAll("_", "-") !== fixture.id
  ) {
    findings.push("$.id: id_must_match_filename");
  }
  if (fixture.content_origin !== "synthetic") {
    findings.push("$.content_origin: synthetic_origin_required");
  }
  if (!isNonemptyString(fixture.description)) {
    findings.push("$.description: nonempty_string_required");
  }
  if (!DOMAINS.has(fixture.domain)) {
    findings.push("$.domain: unsupported_value");
  }

  expectExactKeys(fixture.execution, EXECUTION_KEYS, "$.execution", findings);
  if (fixture.execution && typeof fixture.execution === "object") {
    if (!SIDEKICK_SOTA_EXECUTION_STATUSES.includes(fixture.execution.status)) {
      findings.push("$.execution.status: unsupported_value");
    }
    expectUniqueStrings(
      fixture.execution.required_capabilities,
      "$.execution.required_capabilities",
      findings,
      { nonempty: true },
    );
    if (
      fixture.execution.status === "executable" &&
      fixture.execution.reason !== null
    ) {
      findings.push("$.execution.reason: executable_reason_must_be_null");
    }
    if (
      fixture.execution.status !== "executable" &&
      !isNonemptyString(fixture.execution.reason)
    ) {
      findings.push("$.execution.reason: deferred_reason_required");
    }
  }

  expectExactKeys(fixture.privacy, PRIVACY_KEYS, "$.privacy", findings);
  if (fixture.privacy && typeof fixture.privacy === "object") {
    if (fixture.privacy.generation_method !== "behavior_first_from_scratch") {
      findings.push("$.privacy.generation_method: unsupported_value");
    }
    if (fixture.privacy.source_material !== "none") {
      findings.push("$.privacy.source_material: none_required");
    }
    expectUniqueStrings(
      fixture.privacy.approved_role_tokens,
      "$.privacy.approved_role_tokens",
      findings,
      { nonempty: true },
    );
  }

  expectExactKeys(fixture.capture, CAPTURE_KEYS, "$.capture", findings);
  if (fixture.capture && typeof fixture.capture === "object") {
    if (!CAPTURE_MODES.has(fixture.capture.mode)) {
      findings.push("$.capture.mode: unsupported_value");
    }
    if (!SCREEN_CONTEXTS.has(fixture.capture.screen_context)) {
      findings.push("$.capture.screen_context: unsupported_value");
    }
    if (
      !Number.isInteger(fixture.capture.expected_speakers) ||
      fixture.capture.expected_speakers < 1 ||
      fixture.capture.expected_speakers > 12
    ) {
      findings.push("$.capture.expected_speakers: integer_1_to_12_required");
    }
  }

  expectExactKeys(
    fixture.prepared_context,
    PREPARED_CONTEXT_KEYS,
    "$.prepared_context",
    findings,
  );
  if (fixture.prepared_context && typeof fixture.prepared_context === "object") {
    for (const key of ["user_role", "posture", "goal"]) {
      if (!isNonemptyString(fixture.prepared_context[key])) {
        findings.push(`$.prepared_context.${key}: nonempty_string_required`);
      }
    }
  }

  const evidenceIds = new Set();
  const disclosedEvidenceIds = new Set();
  if (!Array.isArray(fixture.context_evidence)) {
    findings.push("$.context_evidence: array_required");
  } else {
    fixture.context_evidence.forEach((item, index) => {
      const location = `$.context_evidence[${index}]`;
      expectExactKeys(item, CONTEXT_EVIDENCE_KEYS, location, findings);
      if (!item || typeof item !== "object") return;
      validateIdentifier(item.id, `${location}.id`, findings);
      if (evidenceIds.has(item.id)) findings.push(`${location}.id: duplicate_evidence_id`);
      evidenceIds.add(item.id);
      if (!CONTEXT_KINDS.has(item.kind)) findings.push(`${location}.kind: unsupported_value`);
      if (!SENSITIVITIES.has(item.sensitivity)) {
        findings.push(`${location}.sensitivity: unsupported_value`);
      }
      if (typeof item.disclosed !== "boolean") {
        findings.push(`${location}.disclosed: boolean_required`);
      } else if (item.disclosed) {
        disclosedEvidenceIds.add(item.id);
      }
      if (item.sensitivity === "restricted" && item.disclosed === true) {
        findings.push(`${location}: restricted_context_must_not_be_disclosed`);
      }
      if (!isNonemptyString(item.text)) {
        findings.push(`${location}.text: nonempty_string_required`);
      }
    });
  }

  const approvedRoles = new Set(fixture.privacy?.approved_role_tokens ?? []);
  const transcriptSequences = new Map();
  if (!Array.isArray(fixture.transcript) || fixture.transcript.length === 0) {
    findings.push("$.transcript: nonempty_array_required");
  } else {
    let previousSequence = 0;
    fixture.transcript.forEach((item, index) => {
      const location = `$.transcript[${index}]`;
      expectExactKeys(item, TRANSCRIPT_KEYS, location, findings);
      if (!item || typeof item !== "object") return;
      validateIdentifier(item.id, `${location}.id`, findings);
      if (evidenceIds.has(item.id)) findings.push(`${location}.id: duplicate_evidence_id`);
      evidenceIds.add(item.id);
      disclosedEvidenceIds.add(item.id);
      transcriptSequences.set(item.id, item.sequence);
      if (!Number.isInteger(item.sequence) || item.sequence <= previousSequence) {
        findings.push(`${location}.sequence: strictly_increasing_positive_integer_required`);
      } else {
        previousSequence = item.sequence;
      }
      if (!approvedRoles.has(item.speaker)) {
        findings.push(`${location}.speaker: unapproved_role_token`);
      }
      if (!isNonemptyString(item.text)) {
        findings.push(`${location}.text: nonempty_string_required`);
      }
    });
  }

  const turnIds = new Set();
  let previousTranscriptSequence = 0;
  if (!Array.isArray(fixture.turns) || fixture.turns.length === 0) {
    findings.push("$.turns: nonempty_array_required");
  } else {
    fixture.turns.forEach((turn, turnIndex) => {
      const location = `$.turns[${turnIndex}]`;
      expectExactKeys(turn, TURN_KEYS, location, findings);
      if (!turn || typeof turn !== "object") return;
      validateIdentifier(turn.id, `${location}.id`, findings);
      if (turnIds.has(turn.id)) findings.push(`${location}.id: duplicate_turn_id`);
      turnIds.add(turn.id);
      if (!TURN_MODES.has(turn.mode)) findings.push(`${location}.mode: unsupported_value`);
      if (turn.mode === "background" && turn.typed_prompt !== null) {
        findings.push(`${location}.typed_prompt: background_prompt_must_be_null`);
      }
      if (turn.mode === "foreground" && !isNonemptyString(turn.typed_prompt)) {
        findings.push(`${location}.typed_prompt: foreground_prompt_required`);
      }
      if (!transcriptSequences.has(turn.transcript_through_id)) {
        findings.push(`${location}.transcript_through_id: unknown_transcript_id`);
      } else if (
        transcriptSequences.get(turn.transcript_through_id) <
        previousTranscriptSequence
      ) {
        findings.push(
          `${location}.transcript_through_id: turn_transcript_must_not_move_backward`,
        );
      } else {
        previousTranscriptSequence = transcriptSequences.get(
          turn.transcript_through_id,
        );
      }
      if (!DECISIONS.has(turn.expected_decision)) {
        findings.push(`${location}.expected_decision: unsupported_value`);
      }
      for (const key of ["required_evidence_ids", "forbidden_evidence_ids"]) {
        expectUniqueStrings(turn[key], `${location}.${key}`, findings);
        for (const [index, evidenceId] of (turn[key] ?? []).entries()) {
          if (!evidenceIds.has(evidenceId)) {
            findings.push(`${location}.${key}[${index}]: unknown_evidence_id`);
          }
          if (key === "required_evidence_ids" && !disclosedEvidenceIds.has(evidenceId)) {
            findings.push(`${location}.${key}[${index}]: evidence_not_disclosed`);
          }
        }
      }
      for (const evidenceId of turn.required_evidence_ids ?? []) {
        if (turn.forbidden_evidence_ids?.includes(evidenceId)) {
          findings.push(`${location}: evidence_cannot_be_required_and_forbidden`);
        }
      }
      if (
        turn.expected_decision === "speak" &&
        (!Array.isArray(turn.required_evidence_ids) ||
          turn.required_evidence_ids.length === 0)
      ) {
        findings.push(`${location}.required_evidence_ids: speaking_turn_requires_evidence`);
      }
      if (!Array.isArray(turn.rubric) || turn.rubric.length < 2) {
        findings.push(`${location}.rubric: at_least_two_criteria_required`);
      } else {
        const rubricIds = new Set();
        turn.rubric.forEach((criterion, criterionIndex) => {
          const criterionLocation = `${location}.rubric[${criterionIndex}]`;
          expectExactKeys(criterion, RUBRIC_KEYS, criterionLocation, findings);
          if (!criterion || typeof criterion !== "object") return;
          validateIdentifier(criterion.id, `${criterionLocation}.id`, findings);
          if (rubricIds.has(criterion.id)) {
            findings.push(`${criterionLocation}.id: duplicate_criterion_id`);
          }
          rubricIds.add(criterion.id);
          if (!isNonemptyString(criterion.description)) {
            findings.push(`${criterionLocation}.description: nonempty_string_required`);
          }
          if (typeof criterion.critical !== "boolean") {
            findings.push(`${criterionLocation}.critical: boolean_required`);
          }
        });
      }
      expectUniqueStrings(
        turn.forbidden_behaviors,
        `${location}.forbidden_behaviors`,
        findings,
      );
      if (
        !Number.isInteger(turn.max_words) ||
        turn.max_words < 1 ||
        turn.max_words > 120
      ) {
        findings.push(`${location}.max_words: integer_1_to_120_required`);
      }
    });
  }

  expectUniqueStrings(
    fixture.global_forbidden_behaviors,
    "$.global_forbidden_behaviors",
    findings,
    { nonempty: true },
  );
  return findings;
}

export function assertValidSidekickSotaFixture(fixture, options = {}) {
  const findings = validateSidekickSotaFixture(fixture, options);
  if (findings.length > 0) {
    throw new Error(`Invalid Sidekick SOTA fixture:\n${findings.join("\n")}`);
  }
  return fixture;
}

export async function loadSidekickSotaFixtures(directory) {
  const directoryPath =
    directory instanceof URL ? fileURLToPath(directory) : directory;
  const entries = await fs.readdir(directoryPath, { withFileTypes: true });
  const paths = entries
    .filter((entry) => entry.isFile() && entry.name.endsWith(".json"))
    .map((entry) => path.join(directoryPath, entry.name))
    .sort();
  if (paths.length === 0) throw new Error("Sidekick SOTA fixture directory is empty");
  const fixtures = [];
  const ids = new Set();
  for (const fixturePath of paths) {
    const fixture = JSON.parse(await fs.readFile(fixturePath, "utf8"));
    assertValidSidekickSotaFixture(fixture, { filename: fixturePath });
    if (ids.has(fixture.id)) throw new Error(`Duplicate Sidekick SOTA fixture id ${fixture.id}`);
    ids.add(fixture.id);
    fixtures.push({ path: fixturePath, fixture });
  }
  return fixtures;
}

export function disclosedSidekickContext(fixture) {
  assertValidSidekickSotaFixture(fixture);
  return {
    user_role: fixture.prepared_context.user_role,
    posture: fixture.prepared_context.posture,
    goal: fixture.prepared_context.goal,
    evidence: fixture.context_evidence
      .filter((item) => item.disclosed && item.sensitivity === "unrestricted")
      .map(({ id, kind, text }) => ({ id, kind, text })),
  };
}

/**
 * Synthetic evaluator-only content used to detect semantic leakage.
 *
 * This must never be included in the candidate Sidekick input. SOTA fixtures
 * are required to be synthetic so an independent evaluator can inspect this
 * oracle without exposing any real restricted meeting content.
 */
export function evaluatorOnlyForbiddenContext(fixture) {
  assertValidSidekickSotaFixture(fixture);
  return fixture.context_evidence
    .filter((item) => !item.disclosed || item.sensitivity === "restricted")
    .map(({ id, kind, sensitivity, text }) => ({
      id,
      kind,
      sensitivity,
      text,
    }));
}

function wordCount(text) {
  return String(text ?? "").trim().match(/\S+/gu)?.length ?? 0;
}

export function scoreSidekickSotaResponses({ fixture, responses }) {
  assertValidSidekickSotaFixture(fixture);
  const contextEvidence = fixture.context_evidence
    .filter((item) => item.disclosed && item.sensitivity === "unrestricted")
    .map((item) => item.id);
  const results = fixture.turns.map((turn) => {
    const cutoff = fixture.transcript.findIndex(
      (item) => item.id === turn.transcript_through_id,
    );
    const availableEvidence = new Set([
      ...fixture.transcript.slice(0, cutoff + 1).map((item) => item.id),
      ...contextEvidence,
    ]);
    const response = responses?.[turn.id];
    const checks = [];
    const check = (name, passed, detail) => {
      checks.push({ name, passed: Boolean(passed), detail });
    };
    check("response_present", Boolean(response), "A response receipt is required.");
    if (!response) {
      return { turn_id: turn.id, passed: false, checks };
    }
    check(
      "decision_matches",
      response.decision === turn.expected_decision,
      `Expected ${turn.expected_decision}.`,
    );
    const evidenceIds = Array.isArray(response.evidence_ids) ? response.evidence_ids : [];
    check(
      "evidence_shape",
      Array.isArray(response.evidence_ids) &&
        evidenceIds.every((id) => typeof id === "string"),
      "Evidence IDs must be a string array.",
    );
    check(
      "evidence_available",
      evidenceIds.every((id) => availableEvidence.has(id)),
      "Every cited source must be disclosed to this turn.",
    );
    check(
      "required_evidence",
      turn.required_evidence_ids.every((id) => evidenceIds.includes(id)),
      "Every required evidence item must be cited.",
    );
    check(
      "forbidden_evidence",
      turn.forbidden_evidence_ids.every((id) => !evidenceIds.includes(id)),
      "Forbidden or restricted evidence must not be cited.",
    );
    check(
      "word_budget",
      wordCount(response.text) <= turn.max_words,
      `Visible text must be at most ${turn.max_words} words.`,
    );
    check(
      "speak_text_shape",
      turn.expected_decision !== "speak" || isNonemptyString(response.text),
      "A speaking decision must include visible text.",
    );
    check(
      "silence_shape",
      turn.expected_decision !== "silent" || !String(response.text ?? "").trim(),
      "A silent decision must not carry visible text.",
    );
    check(
      "visual_claim_shape",
      fixture.capture.screen_context === "material_fixture" ||
        (response.claims_visual_observation === false &&
          (response.visual_evidence_ids?.length ?? 0) === 0),
      "A fixture without material pixels cannot support a visual claim.",
    );
    return {
      turn_id: turn.id,
      passed: checks.every((item) => item.passed),
      checks,
    };
  });
  return {
    schema_version: 1,
    fixture_id: fixture.id,
    passed: results.every((item) => item.passed),
    turns: results,
  };
}
