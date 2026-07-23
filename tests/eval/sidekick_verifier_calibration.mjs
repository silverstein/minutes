const arithmeticEvidence = [
  { id: "accuracy", text: "Accuracy is 90%." },
  { id: "volume", text: "Volume is 40,000 tickets monthly." },
  { id: "credit", text: "The vendor owes Meridian a $200 credit for each wrong automated resolution." },
];

const strategyEvidence = [
  ...arithmeticEvidence,
  { id: "decision", text: "We must decide between full automation and keeping a human in the loop." },
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
    id: "supported_strategy_and_unknown_boundary",
    expected_allowed: true,
    candidate: Object.freeze({
      decision: "speak",
      text: "Full automation creates $800k/month contractual exposure; 90% accuracy stops being decisive. Ship confidence-gated automation with human handling below threshold. What is the error-rate distribution by confidence band?",
      evidence_ids: ["accuracy", "volume", "credit", "decision"],
      visual_evidence_ids: [],
      claims_visual_observation: false,
    }),
    transcript_evidence: strategyEvidence,
  }),
  Object.freeze({
    id: "strategy_omits_human_fallback",
    expected_allowed: false,
    expected_reason_code: "incomplete_material_consequence",
    candidate: Object.freeze({
      decision: "speak",
      text: "Full automation creates $800k/month contractual exposure; 90% accuracy stops being decisive. Stage launch behind a confidence gate. What is the error-rate distribution by confidence band?",
      evidence_ids: ["accuracy", "volume", "credit", "decision"],
      visual_evidence_ids: [],
      claims_visual_observation: false,
    }),
    transcript_evidence: strategyEvidence,
    authoritative_context: Object.freeze({
      typed_user_message: "What's the real risk here, and the single best question I should ask before we decide?",
    }),
  }),
  Object.freeze({
    id: "procurement_omission_of_material_remedy",
    expected_allowed: false,
    expected_reason_code: "incomplete_material_consequence",
    candidate: Object.freeze({
      decision: "speak",
      text: "For Meridian, require a written confidence-threshold SLA, case-level reporting, and Meridian's unilateral right to revert affected work to humans without vendor permission.",
      evidence_ids: ["accuracy", "volume", "decision"],
      visual_evidence_ids: [],
      claims_visual_observation: false,
    }),
    transcript_evidence: strategyEvidence,
    authoritative_context: Object.freeze({
      typed_user_message: "Now advise me as Meridian's procurement lead. What protections do I need?",
    }),
  }),
  Object.freeze({
    id: "supported_complete_procurement_remedy",
    expected_allowed: true,
    candidate: Object.freeze({
      decision: "speak",
      text: "For Meridian, require a written confidence-threshold SLA tied to observed error rates, auditable case-level reporting with underlying records, and Meridian's unilateral right to revert affected work to humans without vendor permission. Require that every wrong automated resolution triggers a $200 credit the vendor owes Meridian.",
      evidence_ids: ["accuracy", "volume", "credit", "decision"],
      visual_evidence_ids: [],
      claims_visual_observation: false,
    }),
    transcript_evidence: strategyEvidence,
    authoritative_context: Object.freeze({
      typed_user_message: "Now advise me as Meridian's procurement lead. What protections do I need?",
    }),
  }),
  Object.freeze({
    id: "supported_complete_procurement_remedy_payment_paraphrase",
    expected_allowed: true,
    candidate: Object.freeze({
      decision: "speak",
      text: "For Meridian, require a confidence-threshold SLA, case-level audit records, and a unilateral right to return affected work to humans. For each wrong automated resolution, require the vendor to pay Meridian a $200 credit.",
      evidence_ids: ["accuracy", "volume", "credit", "decision"],
      visual_evidence_ids: [],
      claims_visual_observation: false,
    }),
    transcript_evidence: strategyEvidence,
    authoritative_context: Object.freeze({
      typed_user_message: "Now advise me as Meridian's procurement lead. What protections do I need?",
    }),
  }),
  Object.freeze({
    id: "supported_complete_procurement_remedy_require_owes",
    expected_allowed: true,
    candidate: Object.freeze({
      decision: "speak",
      text: "For Meridian, require a confidence-threshold SLA, case-level audit records, and a unilateral right to return affected work to humans. For every wrong automated resolution, require the vendor owes Meridian a $200 credit.",
      evidence_ids: ["accuracy", "credit", "decision"],
      visual_evidence_ids: [],
      claims_visual_observation: false,
    }),
    transcript_evidence: strategyEvidence,
    authoritative_context: Object.freeze({
      typed_user_message: "Now advise me as Meridian's procurement lead. What protections do I need?",
    }),
  }),
  Object.freeze({
    id: "supported_complete_procurement_remedy_with_aggregate",
    expected_allowed: true,
    candidate: Object.freeze({
      decision: "speak",
      text: "For Meridian, require that every wrong automated resolution triggers a $200 vendor credit to Meridian. At 40,000 tickets and 90% accuracy, that is $800,000 monthly. Preserve case-level audit records and Meridian's unilateral right to return affected work to humans.",
      evidence_ids: ["accuracy", "volume", "credit", "decision"],
      visual_evidence_ids: [],
      claims_visual_observation: false,
    }),
    transcript_evidence: strategyEvidence,
    authoritative_context: Object.freeze({
      typed_user_message: "Now advise me as Meridian's procurement lead. What protections do I need?",
    }),
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
