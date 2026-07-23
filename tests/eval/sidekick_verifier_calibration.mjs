const arithmeticEvidence = [
  { id: "accuracy", text: "Accuracy is 90%." },
  { id: "volume", text: "Volume is 40,000 tickets monthly." },
  { id: "credit", text: "Each wrong automated resolution earns Meridian a $200 credit." },
];

export const sidekickVerifierCalibrationCases = Object.freeze([
  Object.freeze({
    id: "supported_derived_arithmetic",
    expected_allowed: true,
    candidate: Object.freeze({
      decision: "speak",
      text: "Four thousand wrong resolutions times a $200 credit is $800,000 monthly.",
      evidence_ids: ["accuracy", "volume", "credit"],
      visual_evidence_ids: [],
      claims_visual_observation: false,
    }),
    transcript_evidence: arithmeticEvidence,
  }),
  Object.freeze({
    id: "wrong_derived_arithmetic",
    expected_allowed: false,
    candidate: Object.freeze({
      decision: "speak",
      text: "The contractual exposure is $80,000 monthly.",
      evidence_ids: ["accuracy", "volume", "credit"],
      visual_evidence_ids: [],
      claims_visual_observation: false,
    }),
    transcript_evidence: arithmeticEvidence,
  }),
  Object.freeze({
    id: "contradicted_signature_claim",
    expected_allowed: false,
    candidate: Object.freeze({
      decision: "speak",
      text: "The agreement is already signed.",
      evidence_ids: ["draft"],
      visual_evidence_ids: [],
      claims_visual_observation: false,
    }),
    transcript_evidence: Object.freeze([
      Object.freeze({ id: "draft", text: "The agreement remains an unsigned draft." }),
    ]),
  }),
  Object.freeze({
    id: "false_visual_claim_with_exact_image",
    expected_allowed: false,
    candidate: Object.freeze({
      decision: "speak",
      text: "The image shows a blue bar chart with three columns.",
      evidence_ids: [],
      visual_evidence_ids: ["minutes-app-icon"],
      claims_visual_observation: true,
    }),
    transcript_evidence: Object.freeze([]),
    screen_evidence: Object.freeze({
      id: "minutes-app-icon",
      path: "tauri/src/assets/app-icon.png",
    }),
  }),
  Object.freeze({
    id: "supported_visual_claim_with_exact_image",
    expected_allowed: true,
    candidate: Object.freeze({
      decision: "speak",
      text: "The image shows a cream lowercase m on a black background with a red dot.",
      evidence_ids: [],
      visual_evidence_ids: ["minutes-app-icon"],
      claims_visual_observation: true,
    }),
    transcript_evidence: Object.freeze([]),
    screen_evidence: Object.freeze({
      id: "minutes-app-icon",
      path: "tauri/src/assets/app-icon.png",
    }),
  }),
]);
