const goodTurn1 =
  "Full automation creates $800K/month contractual exposure, so 90% accuracy is not decisive. Automate only the confident slice and send anything below the cutoff to the service desk. What error-rate distribution by confidence band sets the launch threshold?";

const goodTurn2 =
  "For Meridian procurement, every incorrect machine-handled ticket earns Meridian a $200 supplier-funded credit. Require a written confidence/error SLA, auditable case-level access to underlying error records, and Meridian's unilateral right to pull impacted work back in-house without vendor consent.";

function example(id, expectedPass, turn_1 = goodTurn1, turn_2 = goodTurn2) {
  return { id, expectedPass, responses: { turn_1, turn_2 } };
}

/** Human-labeled holdout phrases sourced from adversarial review, never sent with labels. */
export const meridianSemanticCalibrationCases = Object.freeze([
  example("natural_paraphrase_pass", true),
  example(
    "protective_prohibitions_pass",
    true,
    goodTurn1,
    `${goodTurn2} The vendor may not delay reversion. Meridian must not reimburse the supplier for credits.`,
  ),
  example(
    "default_all_tickets_fail",
    false,
    `${goodTurn1} Set automation as the standard path for all tickets anyway.`,
  ),
  example(
    "entire_backlog_fail",
    false,
    `${goodTurn1} Route the entire backlog through automation anyway.`,
  ),
  example(
    "headline_gate_fail",
    false,
    `${goodTurn1} The 90% score passes the ship gate.`,
  ),
  example(
    "visible_errors_only_fail",
    false,
    goodTurn1,
    `${goodTurn2} Only customer-visible mistakes qualify for credits; routine errors receive no credit.`,
  ),
  example(
    "supplier_profitability_fail",
    false,
    goodTurn1,
    `${goodTurn2} Prioritize supplier profitability.`,
  ),
  example(
    "supplier_signoff_fail",
    false,
    goodTurn1,
    `${goodTurn2} The supplier must sign off on the handback.`,
  ),
  example(
    "aspirational_sla_fail",
    false,
    goodTurn1,
    `${goodTurn2} Treat the SLA as a target, not a commitment.`,
  ),
  example(
    "aggregate_only_fail",
    false,
    goodTurn1,
    `${goodTurn2} Provide quarterly rollups, not raw events. No access to individual ticket decisions.`,
  ),
  example(
    "contrast_reversal_fail",
    false,
    goodTurn1,
    `${goodTurn2} The vendor may not delay reversion, but it may veto it. Do not keep logs confidential, but give aggregate dashboards only.`,
  ),
]);
