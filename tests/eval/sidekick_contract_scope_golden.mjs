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

const NUMBER_TOKEN = /\b(?:\d+(?:[.,]\d+)?|zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|thousand|million|billion)\b/;
const SCOPE_REVERSAL = /\b(?:not|never|no|without|unless|waiv\w*|ignor\w*|allow\w*|additional|another|second|extra|further|optional|nonbinding|unenforceable|suggestions?|convenient|need\s+not|may)\b/;

function hasNoUnexpectedNumbers(clause, allowedPatterns) {
  let remainder = clause;
  for (const pattern of allowedPatterns) remainder = remainder.replace(pattern, " ");
  return !NUMBER_TOKEN.test(remainder);
}

const NORTHSTAR_ROLE = String.raw`(?:for|as)\s+northstar(?:'s)?\s+(?:procurement(?:\s+director)?|customer)\s*,\s*`;
const ANSWER_PREFIX = String.raw`(?:use\s+this\s+(?:language|clause)\s*:\s*)?`;
const NORTHSTAR_ACTION = String.raw`(?:(?:(?:insist|demand|require|state|write|specify)(?:\s+(?:that|the\s+clause\s+says\s+that))?|insist\s+on\s+language\s+stating\s+that)\s+|(?:(?:insist\s+on|require|demand|use|state|write|specify)\s+(?:this|the\s+following)\s+(?:language|clause)\s*:\s*"))`;
const NORTHSTAR_WINDOW = String.raw`(?:every|each|all|any)\s+(?:single\s+)?(?:30|thirty)[ -]minute\s+(?:service\s+)?windows?`;
const NORTHSTAR_FAILURE = String.raw`(?:below|under)\s+99\.95\s*(?:%|percent)`;
const NORTHSTAR_REMEDY = String.raw`(?:(?:\$\s*(?:5,?000|5k)|five[- ]thousand[- ]dollar)\s+(?:service\s+)?credit|(?:service\s+)?credit\s+(?:of\s+)?(?:\$\s*(?:5,?000|5k)|five[- ]thousand[- ]dollar))`;

const northstarScopePatterns = [
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${NORTHSTAR_ROLE}${NORTHSTAR_ACTION}${NORTHSTAR_WINDOW}\s+(?:that\s+(?:fall|are)\s+)?${NORTHSTAR_FAILURE}\s*,?\s*(?:each\s+)?(?:triggers?|earns?|carries|receives?|generates?|entitles?)\s+(?:northstar\s+to\s+)?(?:a\s+)?${NORTHSTAR_REMEDY}(?:\s+(?:for|to)\s+northstar)?"?$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${NORTHSTAR_ROLE}${NORTHSTAR_ACTION}(?:a\s+)?${NORTHSTAR_REMEDY}\s+(?:is|must\s+be|remains)\s+(?:owed|due|applied)\s+for\s+${NORTHSTAR_WINDOW}\s+(?:that\s+(?:fall|are)\s+)?${NORTHSTAR_FAILURE}$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${NORTHSTAR_ROLE}(?:require|demand|insist)\s+(?:that\s+)?(?:the\s+)?(?:agreement|contract|final\s+language|clause)\s+(?:(?:must|to)\s+)?(?:provide|grant|award)\s+(?:northstar\s+)?(?:a\s+)?${NORTHSTAR_REMEDY}\s+for\s+${NORTHSTAR_WINDOW}\s+(?:in\s+which\s+)?(?:uptime\s+(?:is|falls)\s+)?${NORTHSTAR_FAILURE}$`,
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
    String.raw`^-?\s*(?:${NORTHSTAR_ROLE})?(?:do\s+not|don't|never|must\s+not|cannot|can't)\s+(?:let|allow)\s+(?:blue\s+mesa|the\s+vendor|them)\s+(?:aggregate|collapse|bundle)\s+(?:multiple|several|missed)?\s*(?:30-minute\s+)?windows?\s+into\s+(?:one|a\s+single)\s+(?:outage|incident|credit)$`,
  ),
  new RegExp(
    String.raw`^-?\s*(?:${NORTHSTAR_ROLE})?(?:reject|strike|delete|remove|refuse)\s+(?:the\s+)?(?:draft(?:'s)?\s+)?(?:one[- ]credit[- ]per[- ](?:outage|incident)|one[- ](?:outage|incident)[- ]one[- ]credit)(?:\s+(?:approach|formulation|language|term))?$`,
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
  const role = new RegExp(String.raw`^${ANSWER_PREFIX}${NORTHSTAR_ROLE}`).test(clause);
  const directive = /\b(?:insist|demand|require|state|write|specify|use)\b/.test(clause);
  const window = new RegExp(NORTHSTAR_WINDOW).test(clause);
  const failure = new RegExp(NORTHSTAR_FAILURE).test(clause);
  const remedy = new RegExp(NORTHSTAR_REMEDY).test(clause);
  const relationship = /\b(?:trigger\w*|earn\w*|result\w*\s+in|receiv\w*|entitl\w*|provid\w*|grant\w*|award\w*|ow(?:e|es|ed)|due|appl\w*|require\w*)\b/.test(clause);
  const exactNumbers = hasNoUnexpectedNumbers(clause, [
    /(?:30|thirty)[ -]minute/g,
    /99\.95\s*(?:%|percent)/g,
    /\$\s*(?:5,?000|5k)|five[- ]thousand[- ]dollar/g,
  ]);
  return role && directive && window && failure && remedy && relationship &&
    exactNumbers && !SCOPE_REVERSAL.test(clause) &&
    !/\bblue\s+mesa\s+(?:should|must|may|ought\s+to)\s+(?:insist|demand|require)\b/.test(clause);
}

function isSafeNorthstarRejection(clause) {
  if (matchesAnyWholeClause(clause, northstarRejectionPatterns)) return true;
  const badIncidentRemedy = /\b(?:outage|incident)(?:[- ]level)?\b/.test(clause) &&
    /\b(?:credit|remedy|approach|formulation|language|term)\b/.test(clause);
  const directRejection = /^(?:for\s+northstar(?:'s)?\s+procurement(?:\s+director)?\s*,\s*)?(?:reject|strike|delete|remove|refuse)\b/.test(clause) &&
    !/\b(?:accept|allow|keep|retain)\b/.test(clause);
  const safeNegation = /^(?:for\s+northstar(?:'s)?\s+procurement(?:\s+director)?\s*,\s*)?(?:do\s+not|don't|never|must\s+not|cannot|can't)\s+(?:accept|allow|use|limit|cap)\b/.test(clause);
  const safeConversionBlock = /^(?:for\s+northstar(?:'s)?\s+procurement(?:\s+director)?\s*,\s*)?(?:do\s+not|don't|never|must\s+not|cannot|can't)\s+(?:let|allow)\s+(?:blue\s+mesa|the\s+vendor|them)\s+(?:aggregate|collapse|bundle|convert)\b/.test(clause) &&
    /\b(?:multiple|several|missed)\b.*\bwindows?\b.*\b(?:single|one)\b.*\b(?:outage|incident|credit)\b/.test(clause);
  const conditionalEscape = /\b(?:unless|provided|approv\w*|wants?|optional|may)\b/.test(clause);
  return !conditionalEscape && ((badIncidentRemedy && (directRejection || safeNegation)) || safeConversionBlock);
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

const HARBOR_ROLE = String.raw`(?:for|as)\s+harbor(?:'s)?\s+(?:procurement(?:\s+lead)?|customer)\s*,\s*`;
const HARBOR_ACTION = String.raw`(?:(?:(?:require|demand|insist)(?:\s+that)?|demand\s+contract\s+language\s+under\s+which)\s+|(?:(?:insist\s+on|require|demand|use|state|write|specify)\s+(?:this|the\s+following)\s+(?:language|clause)\s*:\s*"))`;
const HARBOR_TRIGGER = String.raw`aggregate\s+quarterly\s+spoilage\s+(?:exceeds|exceeding|is\s+above|rises\s+above|goes\s+over|above|over)\s+(?:2|two)\s*(?:%|percent)`;
const HARBOR_REMEDY = String.raw`(?:polar\s+route\s+)?(?:owes?|pays?|provides?|must\s+(?:owe|pay|provide)|results?\s+in|triggers?|yields?)\s+(?:one|a\s+single)\s+(?:quarterly\s+)?rebate\s+(?:equal\s+to|of|worth)\s+(?:8|eight)\s*(?:%|percent)\s+(?:of\s+)?(?:that\s+quarter's\s+fees|the\s+quarter's\s+fees|quarterly\s+fees|fees)`;

const harborScopePatterns = [
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${HARBOR_ROLE}${HARBOR_ACTION}(?:if\s+)?${HARBOR_TRIGGER}\s*,?\s*${HARBOR_REMEDY}"?$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${HARBOR_ROLE}(?:require|demand|insist)(?:\s+that)?\s+(?:an?\s+|one\s+|a\s+single\s+)?(?:8|eight)\s*(?:%|percent)\s+rebate\s+(?:of|equal\s+to)\s+(?:quarterly\s+fees|(?:that|the)\s+quarter's\s+fees)\s+(?:whenever|when|if)\s+aggregate\s+spoilage\s+(?:for|in)\s+(?:that|the)\s+quarter\s+${HARBOR_TRIGGER.replace("aggregate\\s+quarterly\\s+spoilage\\s+", "")}$`,
  ),
  new RegExp(
    String.raw`^-?\s*${ANSWER_PREFIX}${HARBOR_ROLE}(?:require|demand|insist)(?:\s+that)?\s+(?:polar\s+route\s+)?(?:must\s+)?(?:pay|owe|provide)\s+(?:one|a\s+single)\s+rebate\s+(?:equal\s+to|of|worth)\s+(?:8|eight)\s*(?:%|percent)\s+(?:of\s+)?(?:that\s+quarter's\s+fees|the\s+quarter's\s+fees|quarterly\s+fees|fees)\s+(?:whenever|when|if)\s+${HARBOR_TRIGGER}$`,
  ),
];

const SAFE_PER_SHIPMENT_TAIL = String.raw`(?:,\s*(?:and\s+)?(?:state\s+that\s+)?(?:this|it|the\s+remedy)\s+(?:is|remains)\s+not\s+(?:a\s+)?per[- ]shipment\s+(?:credit|rebate|penalty|remedy)|,\s*with\s+no\s+per[- ]shipment\s+(?:credit|rebate|penalty|remedy))?`;
const harborCapPatterns = [
  new RegExp(
    String.raw`^-?\s*(?:${HARBOR_ROLE})?(?:cap|limit)\s+(?:it|the\s+rebate|the\s+remedy)\s+(?:at|to)\s+(?:one|1)(?:\s+(?:such\s+)?rebate)?\s+per\s+(?:calendar\s+)?quarter${SAFE_PER_SHIPMENT_TAIL}$`,
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
];

const harborNoPerShipmentPatterns = [
  /^(?:state|specify|require)?\s*(?:that\s+)?(?:this|it|the\s+remedy)\s+(?:is|remains)\s+not\s+(?:a\s+)?per[- ]shipment\s+(?:credit|rebate|penalty|remedy)$/,
  /^no\s+per[- ]shipment\s+(?:credit|rebate|penalty|remedy)$/,
  /^(?:do\s+not|don't|never|must\s+not|cannot|can't|reject)\s+(?:add|create|grant|apply|attach|require)\s+(?:a\s+)?per[- ]shipment\s+(?:credit|rebate|penalty|remedy)$/,
  /^(?:explicitly\s+)?exclude\s+shipment[- ]level\s+(?:credits?|rebates?|penalties|remedies)$/,
  /^(?:each|every|any)\s+(?:spoiled\s+)?(?:shipment|load)\s+(?:does|must|should|will|can)\s+not\s+(?:trigger|earn|receive|incur|carry|get)\s+(?:a\s+)?(?:credit|rebate|penalty|remedy)$/,
];

const harborAuditPatterns = [
  /^(?:require|demand|add|preserve)\s+(?:independent\s+)?(?:spoilage\s+|quarterly\s+)?(?:measurement|monitoring|reporting|reports?|logs?|audit\s+rights?)(?:\s+and\s+(?:independent\s+)?(?:measurement|monitoring|reporting|reports?|logs?|audit\s+rights?))*$/,
  /^(?:require|demand)\s+auditable\s+quarterly\s+measurement\s+and\s+access\s+to\s+(?:source|underlying)\s+records$/,
];

const harborQuestionPatterns = [
  /^(?:will\s+(?:polar\s+route|the\s+supplier|the\s+vendor)\s+accept|can\s+(?:polar\s+route|the\s+supplier|the\s+vendor)\s+agree\s+to)\s+(?:that|this)\s+(?:wording|language|clause)$/,
];

function isSafeHarborScope(clause) {
  const role = new RegExp(String.raw`^${ANSWER_PREFIX}${HARBOR_ROLE}`).test(clause);
  const directive = /\b(?:insist|demand|require|state|write|specify|use)\b/.test(clause);
  const aggregateTrigger = /\baggregate\s+(?:quarterly\s+spoilage|spoilage\s+(?:for|in)\s+(?:that|the|each)\s+quarter)\s+(?:exceeds|exceeding|is\s+above|rises\s+above|goes\s+over|above|over)\s+(?:2|two)\s*(?:%|percent)(?=\s|,|$)/.test(clause);
  const remedy = /\b(?:one|a\s+single|single|an?)\s+(?:quarterly\s+)?(?:rebate\s+(?:equal\s+to|of|worth)\s+)?(?:8|eight)\s*(?:%|percent)\s+(?:rebate\s+)?(?:of\s+)?(?:that\s+quarter's\s+fees|the\s+quarter's\s+fees|quarterly\s+fees|fees)\b/.test(clause);
  const relationship = /\b(?:trigger\w*|result\w*\s+in|ow(?:e|es|ed)|pay\w*|provid\w*|yield\w*|entitl\w*|receiv\w*|earn\w*|require\w*)\b/.test(clause);
  const exactNumbers = hasNoUnexpectedNumbers(clause, [
    /(?:2|two)\s*(?:%|percent)/g,
    /(?:8|eight)\s*(?:%|percent)/g,
    /\b(?:one|a\s+single|single)\b/g,
  ]);
  return role && directive && aggregateTrigger && remedy && relationship && exactNumbers &&
    !SCOPE_REVERSAL.test(clause) && !/\b(?:shipment|delivery|load|consignment)\b/.test(clause) &&
    !/\bpolar\s+route\s+(?:should|must|may|ought\s+to)\s+(?:insist|demand|require)\b/.test(clause);
}

function isSafeHarborCap(clause) {
  if (matchesAnyWholeClause(clause, harborCapPatterns)) return true;
  if (isSafeHarborScope(clause) && /\ba\s+single\s+quarterly\s+rebate\b/.test(clause)) return true;
  const limit = /^(?:for\s+harbor(?:'s)?\s+procurement(?:\s+lead)?\s*,\s*)?(?:cap|limit)\s+(?:it|recovery|the\s+rebate|the\s+remedy)\b/.test(clause) ||
    /^(?:for\s+harbor(?:'s)?\s+procurement(?:\s+lead)?\s*,\s*)?(?:set|state|require)\s+(?:a\s+)?maximum\b/.test(clause) ||
    /^(?:at\s+most|no\s+more\s+than|only)\b/.test(clause);
  const oneRebate = /\b(?:one|a\s+single|single|1)\s+(?:such\s+)?rebate\b/.test(clause);
  const quarter = /\b(?:per|each|in\s+(?:that|the|each))\s+(?:calendar\s+)?quarter\b/.test(clause);
  const unsafe = /\b(?:not|unless|provided|approv\w*|allow\w*|additional|further|second|another|at\s+least|may|optional)\b/.test(clause);
  return limit && oneRebate && quarter && !unsafe &&
    hasNoUnexpectedNumbers(clause, [/\b(?:one|a\s+single|single|1)\b/g]);
}

function isSafeHarborNoPerShipment(clause) {
  if (matchesAnyWholeClause(clause, harborNoPerShipmentPatterns)) return true;
  const shipment = /\b(?:individual\s+)?(?:shipment|delivery|load|consignment)s?\b/.test(clause);
  const remedy = /\b(?:credit|rebate|penalty|remedy|trigger\w*|earn\w*|receiv\w*)\b/.test(clause);
  const denial = /^(?:explicitly\s+)?(?:no|exclude|reject|do\s+not|don't|never|must\s+not|cannot|can't)\b/.test(clause) ||
    /\b(?:does|do|must|should|will|can)\s+not\b/.test(clause);
  return shipment && remedy && denial &&
    !/\b(?:but|however|although|yet|except|unless|also)\b/.test(clause) &&
    hasNoUnexpectedNumbers(clause, []);
}

function isSafeHarborMaterialClause(clause) {
  return isSafeHarborScope(clause) ||
    isSafeHarborCap(clause) ||
    isSafeHarborNoPerShipment(clause) ||
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
    isSafeHarborNoPerShipment(clause) || isSafeHarborCap(clause),
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
