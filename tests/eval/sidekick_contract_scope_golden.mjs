function check(name, passed, detail) {
  return { name, passed: Boolean(passed), detail };
}

function normalized(text) {
  return String(text ?? "")
    .replace(/[’‘]/g, "'")
    .replace(/[–—]/g, "-")
    .replace(/[*_`#]/g, "")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

function clauses(text) {
  return String(text ?? "")
    .replace(/[“”]/g, '"')
    .replace(/(\d)\.(\d)/g, "$1<decimal>$2")
    .replace(/(^|\n)\s*\d+[.)]\s+/g, "$1")
    .split(/[.;!?\n]+/)
    .map((clause) => clause.replaceAll("<decimal>", ".").replace(/^\s*[-•]\s*/, ""))
    .map((clause) => normalized(clause).replace(/^"+\s*|\s*"+$/g, ""))
    .filter(Boolean);
}

function matchesAnyWholeClause(clause, patterns) {
  return patterns.some((pattern) => pattern.test(clause));
}

const NORTHSTAR_ROLE = String.raw`(?:for|as)\s+northstar(?:\s+health)?(?:'s)?(?:\s+(?:procurement(?:\s+director)?|customer))?\s*,\s*`;
const ANSWER_PREFIX = String.raw`(?:use\s+this\s+(?:language|clause)\s*:\s*)?`;
const NORTHSTAR_ACTION = String.raw`(?:(?:(?:insist|demand|require|state|write|specify)(?:\s+(?:that|the\s+clause\s+says\s+that))?|insist\s+on\s+language\s+stating\s+that)\s+|(?:(?:insist\s+on|require|demand|use|state|write|specify)\s+(?:this|the\s+following)\s+(?:language|clause)\s*:\s*"))`;
const NORTHSTAR_WINDOW = String.raw`(?:every|each|any)\s+(?:single\s+)?(?:30|thirty)[ -]minute\s+(?:service\s+)?windows?`;
const NORTHSTAR_ALL_WINDOWS = String.raw`all\s+(?:30|thirty)[ -]minute\s+(?:service\s+)?windows`;
const NORTHSTAR_FAILURE = String.raw`(?:below|under)\s+99\.95\s*(?:%|percent)`;
const NORTHSTAR_REMEDY = String.raw`(?:(?:\$\s*(?:5,?000|5k)|five[- ]thousand[- ]dollar)\s+(?:service\s+)?credit|(?:service\s+)?credit\s+(?:of\s+)?(?:\$\s*(?:5,?000|5k)|five[- ]thousand[- ]dollar))`;

const northstarScopePatterns = [
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${NORTHSTAR_ROLE}${NORTHSTAR_ACTION}${NORTHSTAR_WINDOW}\s+(?:(?:that\s+(?:fall|are)|where\s+uptime\s+(?:falls|is))\s+)?${NORTHSTAR_FAILURE}\s*,?\s*(?:to\s+)?(?:(?:triggers?|carries|generates?|results?\s+in)\s+(?:a\s+)?${NORTHSTAR_REMEDY}(?:\s+(?:for|to)\s+northstar)?|(?:earns?|gives?)\s+(?:northstar\s+)?(?:a\s+)?${NORTHSTAR_REMEDY}|entitles?\s+northstar\s+to\s+(?:a\s+)?${NORTHSTAR_REMEDY})"?$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${NORTHSTAR_ROLE}${NORTHSTAR_ACTION}${NORTHSTAR_ALL_WINDOWS}\s+(?:(?:that\s+(?:fall|are)|where\s+uptime\s+(?:falls|is))\s+)?${NORTHSTAR_FAILURE}\s*,?\s+each\s+(?:triggers?|earns?|carries|generates?|results?\s+in)\s+(?:a\s+)?${NORTHSTAR_REMEDY}(?:\s+(?:for|to)\s+northstar)?"?$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${NORTHSTAR_ROLE}${NORTHSTAR_ACTION}(?:a\s+)?${NORTHSTAR_REMEDY}\s+(?:is|must\s+be|remains)\s+(?:owed|due|applied)\s+for\s+${NORTHSTAR_WINDOW}\s+(?:that\s+(?:fall|are)\s+)?${NORTHSTAR_FAILURE}$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${NORTHSTAR_ROLE}(?:require|demand|insist)\s+(?:that\s+)?(?:the\s+)?(?:agreement|contract|final\s+language|clause)\s+(?:(?:must|to)\s+)?(?:provide|grant|award)\s+(?:northstar\s+)?(?:a\s+)?${NORTHSTAR_REMEDY}\s+for\s+${NORTHSTAR_WINDOW}\s+(?:in\s+which\s+)?(?:uptime\s+(?:is|falls)\s+)?${NORTHSTAR_FAILURE}$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${NORTHSTAR_ROLE}(?:require|demand|insist)(?:\s+that)?\s+northstar\s+(?:receives?|earns?|is\s+owed|is\s+due)\s+(?:a\s+)?${NORTHSTAR_REMEDY}\s+for\s+${NORTHSTAR_WINDOW}\s+(?:that\s+(?:fall|are)\s+)?${NORTHSTAR_FAILURE}$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${NORTHSTAR_ROLE}(?:require|demand|insist)(?:\s+that)?\s+(?:a\s+)?${NORTHSTAR_REMEDY}\s+for\s+${NORTHSTAR_WINDOW}\s+(?:(?:that\s+(?:fall|are)|in\s+which\s+uptime\s+(?:falls|is))\s+)?${NORTHSTAR_FAILURE}$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${NORTHSTAR_ROLE}(?:require|demand|insist)(?:\s+that)?\s+(?:a\s+)?${NORTHSTAR_REMEDY}\s+each\s+time\s+uptime\s+(?:falls|is)\s+${NORTHSTAR_FAILURE}\s+in\s+(?:a|each|every)\s+(?:30|thirty)[ -]minute\s+(?:service\s+)?window$`,
  ),
];

const northstarRejectionPatterns = [
  new RegExp(
    String.raw`^-?\s*(?:${NORTHSTAR_ROLE})?(?:reject|strike|delete|remove|refuse)\s+(?:the\s+)?(?:draft(?:'s)?\s+)?(?:one|a\s+single|single)\s+(?:service\s+)?credit\s+(?:per|for\s+(?:the\s+)?(?:whole|entire))\s+(?:outage|incident)(?:\s+(?:or|and)\s+(?:outage|incident))?$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${NORTHSTAR_ROLE})?(?:do\s+not|don't|never|must\s+not|cannot|can't)\s+(?:accept|allow|use|limit|cap)\s+(?:the\s+)?(?:draft(?:'s)?\s+)?(?:one|a\s+single|single)\s+(?:service\s+)?credit\s+(?:per|for\s+(?:the\s+)?(?:whole|entire))\s+(?:outage|incident)(?:\s+(?:or|and)\s+(?:outage|incident))?$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${NORTHSTAR_ROLE})?(?:do\s+not|don't|never|must\s+not|cannot|can't)\s+(?:let|allow)\s+(?:blue\s+mesa|the\s+vendor|them)\s+(?:aggregate|collapse|bundle|convert)\s+(?:(?:multiple|several)\s+)?(?:missed\s+)?(?:30-minute\s+)?windows?\s+into\s+(?:one|a\s+single)\s+(?:outage|incident|credit)$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${NORTHSTAR_ROLE})?(?:reject|strike|delete|remove|refuse)\s+(?:the\s+)?(?:draft(?:'s)?\s+)?(?:one[- ]credit[- ]per[- ](?:outage|incident)|one[- ](?:outage|incident)[- ]one[- ]credit)(?:\s+(?:approach|formulation|language|term))?$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${NORTHSTAR_ROLE})?(?:reject|strike|delete|remove|refuse)\s+(?:the\s+)?(?:draft(?:'s)?\s+)?(?:outage|incident)[- ]level\s+(?:service\s+)?(?:credit|remedy|approach|formulation|language|term)$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${NORTHSTAR_ROLE})?(?:reject|strike|delete|remove|refuse)\s+treating\s+(?:a\s+)?continuous\s+outage\s+as\s+(?:one|a\s+single)\s+incident\s+for\s+(?:service\s+)?credit\s+purposes$`,
  ),
];

const northstarExplanationPatterns = [
  /^(?:that|this)\s+(?:would|does)\s+(?:improperly\s+)?(?:aggregate|collapse|bundle)\s+(?:multiple|several|many)\s+missed\s+windows$/,
  /^(?:one|a\s+single)\s+incident\s+may\s+contain\s+(?:multiple|several|many)\s+missed\s+windows$/,
  /^(?:that|this)\s+(?:would|does)\s+(?:change|narrow|undercut)\s+(?:the\s+)?(?:economics|remedy|protection)$/,
];

const northstarAuditPatterns = [
  /^(?:require|demand|add|preserve)\s+(?:independent\s+)?(?:uptime\s+)?(?:measurement|monitoring|reporting|reports?|logs?|audit\s+rights?)(?:\s+and\s+(?:independent\s+)?(?:measurement|monitoring|reporting|reports?|logs?|audit\s+rights?))*$/,
];

const northstarQuestionPatterns = [
  /^(?:will\s+(?:blue\s+mesa|the\s+vendor)\s+accept|can\s+(?:blue\s+mesa|the\s+vendor)\s+agree\s+to)\s+(?:that|this)\s+(?:wording|language|clause)$/,
];

function isSafeNorthstarScope(clause) {
  return matchesAnyWholeClause(clause, northstarScopePatterns);
}

function isSafeNorthstarRejection(clause) {
  return matchesAnyWholeClause(clause, northstarRejectionPatterns);
}

function isSafeNorthstarMaterialClause(clause) {
  return isSafeNorthstarScope(clause) ||
    isSafeNorthstarRejection(clause) ||
    matchesAnyWholeClause(clause, northstarExplanationPatterns) ||
    matchesAnyWholeClause(clause, northstarAuditPatterns) ||
    matchesAnyWholeClause(clause, northstarQuestionPatterns);
}

export function scorePerWindowRemedy(text) {
  const answerClauses = clauses(text);
  const scopeClauses = answerClauses.filter(isSafeNorthstarScope);
  const rejectionClauses = answerClauses.filter(isSafeNorthstarRejection);
  const allClausesSafe = answerClauses.length > 0 && answerClauses.every(isSafeNorthstarMaterialClause);

  const checks = [
    check(
      "northstar_customer_role",
      scopeClauses.length === 1,
      "The exact scope clause must begin with a concrete directive for Northstar procurement.",
    ),
    check(
      "exact_per_window_scope",
      scopeClauses.length === 1,
      "Exactly one whole clause must bind every failed 30-minute window below 99.95% to the $5,000 service credit.",
    ),
    check(
      "reject_per_incident_substitution",
      rejectionClauses.length >= 1,
      "A whole clause must explicitly reject, never accept, one credit per outage or incident.",
    ),
    check(
      "no_scope_or_amount_invention",
      allClausesSafe,
      "Every nonempty clause must be consumed completely by the allowed scope, rejection, explanation, or non-numeric audit grammar.",
    ),
  ];
  return {
    passed: checks.every((item) => item.passed),
    score: { numerator: checks.filter((item) => item.passed).length, denominator: checks.length },
    checks,
  };
}

const HARBOR_ROLE = String.raw`(?:for|as)\s+harbor(?:\s+foods)?(?:'s)?(?:\s+(?:procurement(?:\s+lead)?|customer))?\s*,\s*`;
const HARBOR_ACTION = String.raw`(?:(?:(?:require|demand|insist)(?:\s+that)?|demand\s+contract\s+language\s+under\s+which)\s+|(?:(?:insist\s+on|require|demand|use|state|write|specify)\s+(?:this|the\s+following)\s+(?:language|clause)\s*:\s*"))`;
const HARBOR_TRIGGER_TAIL = String.raw`(?:exceeds|exceeding|is\s+above|rises\s+above|goes\s+over|above|over)\s+(?:2|two)\s*(?:%|percent)`;
const HARBOR_TRIGGER = String.raw`(?:aggregate\s+quarterly|quarterly\s+aggregate)\s+spoilage\s+${HARBOR_TRIGGER_TAIL}`;
const HARBOR_REBATE_TERM = String.raw`(?:one|a\s+single)\s+(?:quarterly\s+)?rebate\s+(?:equal\s+to|of|worth)\s+(?:8|eight)\s*(?:%|percent)\s+(?:of\s+)?(?:that\s+quarter's\s+fees|the\s+quarter's\s+fees|quarterly\s+fees)`;
const HARBOR_REMEDY = String.raw`(?:(?:polar\s+route\s+(?:owes?|pays?|provides?|must\s+(?:owe|pay|provide))\s+${HARBOR_REBATE_TERM})|(?:(?:results?\s+in|triggers?|yields?)\s+${HARBOR_REBATE_TERM}(?:\s+for\s+harbor)?)|(?:entitles?\s+harbor\s+to\s+${HARBOR_REBATE_TERM})|(?:harbor\s+receives?\s+${HARBOR_REBATE_TERM}))`;

const harborScopePatterns = [
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${HARBOR_ROLE}${HARBOR_ACTION}(?:if\s+)?${HARBOR_TRIGGER}\s*,?\s*${HARBOR_REMEDY}"?$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${HARBOR_ROLE}(?:require|demand|insist)(?:\s+that)?\s+(?:an?\s+|one\s+|a\s+single\s+)?(?:8|eight)\s*(?:%|percent)\s+rebate\s+(?:of|equal\s+to)\s+(?:quarterly\s+fees|(?:that|the)\s+quarter's\s+fees)(?:\s+for\s+harbor)?\s+(?:whenever|when|if)\s+aggregate\s+spoilage\s+(?:for|in)\s+(?:that|the)\s+quarter\s+${HARBOR_TRIGGER_TAIL}$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${HARBOR_ROLE}(?:require|demand|insist)(?:\s+that)?\s+polar\s+route\s+(?:must\s+)?(?:pay|owe|provide)\s+${HARBOR_REBATE_TERM}\s+(?:whenever|when|if)\s+${HARBOR_TRIGGER}$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${HARBOR_ROLE}(?:require|demand|insist)(?:\s+that)?\s+${HARBOR_REBATE_TERM}(?:\s+for\s+harbor)?\s+(?:whenever|when|if)\s+${HARBOR_TRIGGER}$`,
  ),
];

const SAFE_PER_SHIPMENT_TAIL = String.raw`(?:,\s*(?:and\s+)?(?:state\s+that\s+)?(?:this|it|the\s+remedy)\s+(?:is|remains)\s+not\s+(?:a\s+)?per[- ]shipment\s+(?:credit|rebate|penalty|remedy)|,\s*with\s+no\s+per[- ]shipment\s+(?:credit|rebate|penalty|remedy))?`;
const harborCapPatterns = [
  new RegExp(
    String.raw`^-?\s*(?:${HARBOR_ROLE})?(?:cap|limit)\s+(?:it|the\s+rebate|the\s+remedy)\s+(?:at|to)\s+(?:one|a\s+single|single|1)(?:\s+(?:such\s+)?rebate)?\s+per\s+(?:calendar\s+)?quarter${SAFE_PER_SHIPMENT_TAIL}$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${HARBOR_ROLE})?limit\s+recovery\s+to\s+(?:one|1)\s+rebate\s+(?:per|each)\s+(?:calendar\s+)?quarter${SAFE_PER_SHIPMENT_TAIL}$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${HARBOR_ROLE})?limit\s+recovery\s+to\s+(?:one|a\s+single|single|1)\s+rebate\s+in\s+(?:that|the|each)\s+(?:calendar\s+)?quarter${SAFE_PER_SHIPMENT_TAIL}$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${HARBOR_ROLE})?(?:set|state|require)\s+(?:a\s+)?maximum\s+of\s+(?:one|1)\s+(?:such\s+)?rebate\s+per\s+(?:calendar\s+)?quarter${SAFE_PER_SHIPMENT_TAIL}$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${HARBOR_ROLE})?(?:at\s+most|no\s+more\s+than|only)\s+(?:one|1)\s+(?:such\s+)?rebate\s+per\s+(?:calendar\s+)?quarter${SAFE_PER_SHIPMENT_TAIL}$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${HARBOR_ROLE})?(?:a\s+)?single\s+quarterly\s+rebate\s+(?:is|remains)\s+the\s+maximum${SAFE_PER_SHIPMENT_TAIL}$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${HARBOR_ROLE})?limit\s+harbor\s+to\s+(?:one|a\s+single|single|1)\s+rebate\s+per\s+(?:calendar\s+)?quarter${SAFE_PER_SHIPMENT_TAIL}$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${HARBOR_ROLE})?no\s+more\s+than\s+(?:one|a\s+single|single|1)\s+rebate\s+may\s+be\s+paid\s+in\s+(?:any|a|each|the)\s+(?:calendar\s+)?quarter${SAFE_PER_SHIPMENT_TAIL}$`,
  ),
];

const harborNoPerShipmentPatterns = [
  /^(?:state|specify|require)?\s*(?:that\s+)?(?:this|it|the\s+remedy)\s+(?:is|remains)\s+not\s+(?:a\s+)?per[- ]shipment\s+(?:credit|rebate|penalty|remedy)$/,
  /^no\s+per[- ]shipment\s+(?:credit|rebate|penalty|remedy)$/,
  /^(?:do\s+not|don't|never|must\s+not|cannot|can't|reject)\s+(?:add|create|grant|apply|attach|require)\s+(?:a\s+)?per[- ]shipment\s+(?:credit|rebate|penalty|remedy)$/,
  /^(?:explicitly\s+)?exclude\s+shipment[- ]level\s+(?:credits?|rebates?|penalties|remedies)$/,
  /^(?:each|every|any)\s+(?:spoiled\s+)?(?:shipment|load)\s+(?:does|must|should|will|can)\s+not\s+(?:trigger|earn|receive|incur|carry|get)\s+(?:a\s+)?(?:credit|rebate|penalty|remedy)$/,
  /^no\s+(?:individual\s+)?(?:shipment|delivery|load)\s+(?:triggers?|earns?|receives?|carries|gets)\s+(?:a\s+)?(?:credit|rebate|penalty|remedy)$/,
  /^no\s+(?:credit|rebate|penalty|remedy)\s+accrues\s+on\s+(?:a\s+)?shipment[- ]by[- ]shipment\s+basis$/,
];

const harborAuditPatterns = [
  /^(?:require|demand|add|preserve)\s+(?:independent\s+)?(?:spoilage\s+|quarterly\s+)?(?:measurement|monitoring|reporting|reports?|logs?|audit\s+rights?)(?:\s+and\s+(?:independent\s+)?(?:measurement|monitoring|reporting|reports?|logs?|audit\s+rights?))*$/,
  /^(?:require|demand)\s+auditable\s+quarterly\s+measurement\s+and\s+access\s+to\s+(?:source|underlying)\s+records$/,
];

const harborQuestionPatterns = [
  /^(?:will\s+(?:polar\s+route|the\s+supplier|the\s+vendor)\s+accept|can\s+(?:polar\s+route|the\s+supplier|the\s+vendor)\s+agree\s+to)\s+(?:that|this)\s+(?:wording|language|clause)$/,
];

function isSafeHarborScope(clause) {
  return matchesAnyWholeClause(clause, harborScopePatterns);
}

function isSafeHarborCap(clause) {
  return matchesAnyWholeClause(clause, harborCapPatterns) ||
    (isSafeHarborScope(clause) && /\ba\s+single\s+quarterly\s+rebate\b/.test(clause));
}

function isSafeHarborMaterialClause(clause) {
  return isSafeHarborScope(clause) ||
    isSafeHarborCap(clause) ||
    matchesAnyWholeClause(clause, harborNoPerShipmentPatterns) ||
    matchesAnyWholeClause(clause, harborAuditPatterns) ||
    matchesAnyWholeClause(clause, harborQuestionPatterns);
}

export function scoreAggregateCappedRemedy(text) {
  const answerClauses = clauses(text);
  const scopeClauses = answerClauses.filter(isSafeHarborScope);
  const capClauses = answerClauses.filter(isSafeHarborCap);
  const allClausesSafe = answerClauses.length > 0 && answerClauses.every(isSafeHarborMaterialClause);
  const shipmentClauses = answerClauses.filter((clause) =>
    /\b(?:per[- ]shipment|(?:each|every|any|no)\s+(?:spoiled\s+)?(?:shipment|delivery|load))\b/.test(clause),
  );
  const shipmentClausesAreNegative = shipmentClauses.every((clause) =>
    matchesAnyWholeClause(clause, harborNoPerShipmentPatterns) || isSafeHarborCap(clause),
  );

  const checks = [
    check(
      "harbor_customer_role",
      scopeClauses.length === 1,
      "The exact scope clause must begin with a concrete directive for Harbor procurement.",
    ),
    check(
      "exact_aggregate_quarterly_scope",
      scopeClauses.length === 1,
      "Exactly one whole clause must bind aggregate quarterly spoilage above 2% to one 8%-of-fees rebate.",
    ),
    check(
      "preserve_one_rebate_quarterly_cap",
      capClauses.length >= 1,
      "A whole clause must state an unconditional upper bound of one rebate per quarter.",
    ),
    check(
      "no_per_shipment_remedy_invention",
      allClausesSafe && shipmentClausesAreNegative,
      "Every nonempty clause must be consumed completely; shipment-level clauses may only deny a per-shipment remedy.",
    ),
  ];
  return {
    passed: checks.every((item) => item.passed),
    score: { numerator: checks.filter((item) => item.passed).length, denominator: checks.length },
    checks,
  };
}
