const invoke = window.__TAURI__.core.invoke;

const elements = {
  setupView: document.querySelector("#setup-view"),
  scanningView: document.querySelector("#scanning-view"),
  resultsView: document.querySelector("#results-view"),
  indexingView: document.querySelector("#indexing-view"),
  searchView: document.querySelector("#search-view"),
  emptyLocations: document.querySelector("#empty-locations"),
  locationList: document.querySelector("#location-list"),
  addLocations: document.querySelector("#add-locations"),
  runCensus: document.querySelector("#run-census"),
  cancelCensus: document.querySelector("#cancel-census"),
  cancelIndexing: document.querySelector("#cancel-indexing"),
  startOver: document.querySelector("#start-over"),
  exportReport: document.querySelector("#export-report"),
  buildTextVault: document.querySelector("#build-text-vault"),
  backToCensus: document.querySelector("#back-to-census"),
  setupStatus: document.querySelector("#setup-status"),
  exportStatus: document.querySelector("#export-status"),
  errorBanner: document.querySelector("#error-banner"),
  errorMessage: document.querySelector("#error-message"),
  dismissError: document.querySelector("#dismiss-error"),
  resultStatus: document.querySelector("#result-status"),
  resultSummary: document.querySelector("#result-summary"),
  metricArtifacts: document.querySelector("#metric-artifacts"),
  metricLocations: document.querySelector("#metric-locations"),
  metricBytes: document.querySelector("#metric-bytes"),
  metricCloud: document.querySelector("#metric-cloud"),
  categoryList: document.querySelector("#category-list"),
  signalsList: document.querySelector("#signals-list"),
  vaultSummary: document.querySelector("#vault-summary"),
  searchForm: document.querySelector("#search-form"),
  searchQuery: document.querySelector("#search-query"),
  searchSubmit: document.querySelector("#search-submit"),
  queryInterpretation: document.querySelector("#query-interpretation"),
  queryChips: document.querySelector("#query-chips"),
  candidateCount: document.querySelector("#candidate-count"),
  searchStatus: document.querySelector("#search-status"),
  searchResults: document.querySelector("#search-results"),
};

let locations = [];
let lastReport = null;
let vaultReport = null;

const categoryLabels = {
  pdf: "PDF",
  word_processing: "Word processing",
  email: "Email containers",
  plain_text: "Plain text & HTML",
  image_or_scan: "Images & scans",
  spreadsheet: "Spreadsheets",
  presentation: "Presentations",
  archive: "Compressed archives",
  database: "Databases",
  apple_document: "Apple documents",
  icloud_placeholder: "iCloud placeholders",
  other: "Other formats",
};

function showError(error) {
  elements.errorMessage.textContent =
    typeof error === "string" ? error : "The operation could not be completed safely.";
  elements.errorBanner.hidden = false;
}

function hideError() {
  elements.errorBanner.hidden = true;
  elements.errorMessage.textContent = "";
}

function showView(name) {
  elements.setupView.hidden = name !== "setup";
  elements.scanningView.hidden = name !== "scanning";
  elements.resultsView.hidden = name !== "results";
  elements.indexingView.hidden = name !== "indexing";
  elements.searchView.hidden = name !== "search";
  const activeStep =
    name === "setup" ? "1" : name === "scanning" ? "2" : name === "results" ? "3" : "4";
  for (const step of document.querySelectorAll("[data-step]")) {
    step.classList.toggle("is-active", step.dataset.step === activeStep);
  }
  // Past step 1 the operator is working, not being introduced.
  document.body.classList.toggle("is-working", activeStep !== "1");
}

function setLocationControlsDisabled(disabled) {
  elements.addLocations.disabled = disabled;
  elements.runCensus.disabled = disabled || locations.length === 0;
  for (const button of elements.locationList.querySelectorAll("button")) {
    button.disabled = disabled;
  }
}

function renderLocations(nextLocations) {
  locations = nextLocations;
  elements.locationList.replaceChildren();
  elements.emptyLocations.hidden = locations.length > 0;

  for (const location of locations) {
    const item = document.createElement("li");
    item.className = "location-item";

    const copy = document.createElement("div");
    copy.className = "location-copy";
    const icon = document.createElement("span");
    icon.className = "location-icon";
    icon.setAttribute("aria-hidden", "true");
    icon.textContent = "⌂";
    const text = document.createElement("div");
    const title = document.createElement("strong");
    title.textContent = location.label;
    const detail = document.createElement("span");
    detail.textContent = "Read-only native authority";
    text.append(title, detail);
    copy.append(icon, text);

    const remove = document.createElement("button");
    remove.className = "remove-location";
    remove.type = "button";
    remove.setAttribute("aria-label", `Remove ${location.label}`);
    remove.textContent = "×";
    remove.addEventListener("click", () => removeLocation(location.id));
    item.append(copy, remove);
    elements.locationList.append(item);
  }

  elements.runCensus.disabled = locations.length === 0;
  elements.setupStatus.textContent =
    locations.length === 0
      ? "Choose at least one location to continue."
      : `${locations.length.toLocaleString()} location${
          locations.length === 1 ? "" : "s"
        } approved. Nothing has been scanned yet.`;
}

async function chooseLocations() {
  hideError();
  setLocationControlsDisabled(true);
  try {
    renderLocations(await invoke("choose_archive_locations"));
    lastReport = null;
    vaultReport = null;
  } catch (error) {
    showError(error);
  } finally {
    setLocationControlsDisabled(false);
  }
}

async function removeLocation(locationId) {
  hideError();
  setLocationControlsDisabled(true);
  try {
    renderLocations(await invoke("remove_archive_location", { locationId }));
    lastReport = null;
    vaultReport = null;
  } catch (error) {
    showError(error);
  } finally {
    setLocationControlsDisabled(false);
  }
}

function formatBytes(bytes) {
  if (bytes === 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  return `${value >= 10 || index === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`;
}

function createSignal(label, value) {
  const row = document.createElement("div");
  const term = document.createElement("dt");
  term.textContent = label;
  const description = document.createElement("dd");
  description.textContent = Number(value).toLocaleString();
  row.append(term, description);
  return row;
}

function renderReport(report) {
  lastReport = report;
  const status = report.status.replaceAll("_", " ");
  elements.resultStatus.textContent = status;
  elements.resultStatus.dataset.status = report.status;
  elements.resultSummary.textContent =
    report.status === "complete"
      ? "The approved locations were counted without reading document contents."
      : report.status === "cancelled"
        ? "The census was cancelled. Partial counts were discarded from export."
        : "The census stopped at a safety boundary. Review the aggregate signals.";

  elements.metricArtifacts.textContent = report.summary.artifacts.toLocaleString();
  elements.metricLocations.textContent = report.summary.approved_locations.toLocaleString();
  elements.metricBytes.textContent = formatBytes(report.summary.regular_file_bytes);
  elements.metricCloud.textContent = report.signals.icloud_placeholders.toLocaleString();

  elements.categoryList.replaceChildren();
  const categories = [...report.categories].sort((left, right) => right.artifacts - left.artifacts);
  const largest = Math.max(1, ...categories.map((category) => category.artifacts));
  for (const category of categories.slice(0, 10)) {
    const row = document.createElement("div");
    row.className = "category-row";
    const label = document.createElement("span");
    label.className = "category-label";
    label.textContent = categoryLabels[category.category] ?? category.category;
    const track = document.createElement("span");
    track.className = "category-track";
    const fill = document.createElement("span");
    fill.style.width = `${Math.max(2, (category.artifacts / largest) * 100)}%`;
    track.append(fill);
    const count = document.createElement("span");
    count.className = "category-count";
    count.textContent = category.artifacts.toLocaleString();
    row.append(label, track, count);
    elements.categoryList.append(row);
  }
  if (categories.length === 0) {
    const empty = document.createElement("p");
    empty.className = "status";
    empty.textContent = "No file or package artifacts were found.";
    elements.categoryList.append(empty);
  }

  elements.signalsList.replaceChildren(
    createSignal(
      "Likely OCR candidates",
      report.categories.find((item) => item.category === "image_or_scan")?.artifacts ?? 0,
    ),
    createSignal("iCloud placeholders", report.signals.icloud_placeholders),
    createSignal("Apple packages", report.summary.packages),
    createSignal("Unreadable by mode", report.signals.permission_mode_unreadable),
    createSignal("Links skipped", report.signals.symlinks_skipped),
    createSignal(
      "Metadata errors",
      report.signals.metadata_errors + report.signals.directory_errors,
    ),
  );

  const usableReport = report.status === "complete" || report.status === "partial";
  elements.exportReport.disabled = !usableReport;
  elements.buildTextVault.disabled = !usableReport;
  elements.exportStatus.textContent = "";
  showView("results");
}

async function runCensus() {
  hideError();
  vaultReport = null;
  showView("scanning");
  try {
    const report = await invoke("run_archive_census");
    if (report.status === "cancelled") {
      lastReport = null;
      showView("setup");
      elements.setupStatus.textContent = "Census cancelled. No report was retained.";
      return;
    }
    renderReport(report);
  } catch (error) {
    showView("setup");
    showError(error);
  }
}

async function cancelOperation(button) {
  button.disabled = true;
  const original = button.textContent;
  button.textContent = "Cancelling…";
  try {
    await invoke("cancel_archive_census");
  } catch (error) {
    showError(error);
  } finally {
    button.textContent = original;
    button.disabled = false;
  }
}

async function exportReport() {
  hideError();
  elements.exportReport.disabled = true;
  elements.exportStatus.textContent = "Preparing report…";
  try {
    const saved = await invoke("export_archive_census");
    elements.exportStatus.textContent = saved ? "Aggregate report saved." : "";
  } catch (error) {
    elements.exportStatus.textContent = "";
    showError(error);
  } finally {
    elements.exportReport.disabled = false;
  }
}

function renderVaultSummary(report) {
  vaultReport = report;
  // Defaulted, not asserted. A missing count must never blank the search view:
  // throwing here hides every result behind a number the reader does not need.
  const inferredBoundaryDocuments = report.inferred_boundary_documents ?? 0;
  elements.vaultSummary.textContent =
    `${report.indexed_documents.toLocaleString()} supported document${
      report.indexed_documents === 1 ? "" : "s"
    } indexed (${formatBytes(report.indexed_bytes)}). ` +
    `${report.searchable_pdf_documents.toLocaleString()} PDF, ` +
    `${report.docx_documents.toLocaleString()} DOCX. ` +
    `${report.unsupported_files_skipped.toLocaleString()} unsupported item${
      report.unsupported_files_skipped === 1 ? "" : "s"
    } skipped; ${report.ocr_required_files.toLocaleString()} PDF${
      report.ocr_required_files === 1 ? "" : "s"
    } require OCR. ` +
    // Defaulted, not asserted: a missing count must never blank the search
    // view. Throwing here would hide every result behind a field the reader
    // does not care about.
    `${inferredBoundaryDocuments.toLocaleString()} indexed document${
      inferredBoundaryDocuments === 1 ? "" : "s"
    } cannot answer same-clause questions because the file reports no section structure. ` +
    (report.semantic_retrieval_enabled
      ? `${report.semantic_provisions_indexed.toLocaleString()} provision suggestion vector${
          report.semantic_provisions_indexed === 1 ? "" : "s"
        } built with the pinned macOS model inside its network-denied worker. `
      : "Semantic suggestions are unavailable on this Mac. ") +
    describeDroppedSources(report) +
    "All indexes exist only in memory.";
}

// Documents that could not be indexed were counted and never shown. Counsel
// saw "N supported documents indexed" and had no way to learn that a
// privileged PDF had been refused by the converter, was malformed, was over
// budget, or sat behind a permission error -- a search would simply return
// nothing for a clause that is plainly in the file. Coverage gaps have to be
// visible to be acted on.
function describeDroppedSources(report) {
  const dropped = [
    [report.conversion_failures, "could not be converted"],
    [report.malformed_text_files_skipped, "were malformed"],
    [report.oversized_files_skipped, "exceeded the size budget"],
    [report.duplicate_files_skipped, "were duplicates"],
    [report.metadata_errors, "could not be read"],
    [report.directory_errors, "were in unreadable folders"],
  ].filter(([count]) => count > 0);
  if (dropped.length === 0) {
    return "No supported document was dropped. ";
  }
  const total = dropped.reduce((sum, [count]) => sum + count, 0);
  const detail = dropped
    .map(([count, reason]) => `${count.toLocaleString()} ${reason}`)
    .join(", ");
  return (
    `${total.toLocaleString()} document${total === 1 ? "" : "s"} ` +
    `${total === 1 ? "was" : "were"} not indexed and cannot be searched ` +
    `(${detail}). Review these before relying on a negative result. `
  );
}

async function buildTextVault() {
  hideError();
  showView("indexing");
  try {
    const report = await invoke("build_archive_text_vault");
    renderVaultSummary(report);
    elements.searchResults.replaceChildren();
    elements.queryInterpretation.hidden = true;
    elements.searchStatus.textContent =
      report.indexed_documents === 0
        ? "No searchable PDF, DOCX, TXT, or Markdown documents were found in the approved locations."
        : "Enter a legal retrieval question. Results are exact source excerpts, not generated answers.";
    showView("search");
    if (report.indexed_documents > 0) {
      elements.searchQuery.focus();
    }
  } catch (error) {
    if (lastReport) renderReport(lastReport);
    else showView("setup");
    showError(error);
  }
}

function humanize(value) {
  return value.replaceAll("_", " ");
}

function addQueryChip(label) {
  const chip = document.createElement("span");
  chip.className = "query-chip";
  chip.textContent = label;
  elements.queryChips.append(chip);
}

function renderQueryInterpretation(response) {
  const query = response.query;
  elements.queryChips.replaceChildren();
  addQueryChip(
    query.scope === "same_provision" ? "Same provision" : "Anywhere in one document",
  );
  for (const concept of query.required_concepts) {
    addQueryChip(`Must cover: ${humanize(concept)}`);
  }
  for (const concept of query.excluded_concepts) {
    addQueryChip(`Exclude: ${humanize(concept)}`);
  }
  if (query.exact_phrase) addQueryChip(`Exact: “${query.exact_phrase}”`);
  if (query.max_sentences) addQueryChip(`At most ${query.max_sentences} sentences`);
  if (response.semantic_query_applied) addQueryChip("On-device meaning suggestions");
  elements.candidateCount.textContent =
    `${response.lexical_candidates_considered.toLocaleString()} lexical candidate${
      response.lexical_candidates_considered === 1 ? "" : "s"
    } checked` +
    (response.semantic_query_applied
      ? ` · ${response.semantic_candidates_considered.toLocaleString()} semantic vector${
          response.semantic_candidates_considered === 1 ? "" : "s"
        } ranked`
      : "");
  elements.queryInterpretation.hidden = false;
}

function evidenceCard(card, compact = false, semantic = false) {
  if (compact) {
    const item = document.createElement("div");
    item.className = "criterion-evidence";
    const heading = document.createElement("strong");
    heading.textContent = card.provision_heading ?? card.source_anchor;
    const excerpt = document.createElement("p");
    excerpt.textContent = card.exact_excerpt;
    item.append(heading, excerpt);
    // The compact card dropped why_matched entirely, so in document-scope
    // results the disclosure that a concept matched in the heading rather
    // than in the quoted text never reached the reader. The anchor went too,
    // leaving nothing to verify the excerpt against in the source.
    if (card.why_matched) {
      const why = document.createElement("p");
      why.className = "criterion-why";
      why.textContent = `${card.source_anchor} — ${card.why_matched}`;
      item.append(why);
    }
    return item;
  }

  const article = document.createElement("article");
  article.className = "evidence-card";
  const header = document.createElement("div");
  header.className = "evidence-card-header";
  const titleBlock = document.createElement("div");
  const kicker = document.createElement("span");
  kicker.className = "evidence-kicker";
  kicker.textContent = semantic
    ? "Meaning-similar suggestion"
    : (card.provision_heading ?? "Provision");
  const title = document.createElement("strong");
  title.className = "evidence-title";
  title.textContent = card.document_title;
  titleBlock.append(kicker, title);
  const fresh = document.createElement("span");
  fresh.className = "metadata-pill";
  fresh.textContent = card.index_fresh ? "Source verified" : "Unavailable";
  header.append(titleBlock, fresh);

  const excerpt = document.createElement("blockquote");
  excerpt.className = "evidence-excerpt";
  excerpt.textContent = card.exact_excerpt;
  const meta = document.createElement("div");
  meta.className = "evidence-meta";
  const anchor = document.createElement("span");
  anchor.textContent = card.source_anchor;
  const sentences = document.createElement("span");
  sentences.textContent = `${card.sentence_count} sentence${
    card.sentence_count === 1 ? "" : "s"
  }`;
  const converter = document.createElement("span");
  converter.textContent = humanize(card.source_converter);
  meta.append(anchor, sentences, converter);
  const why = document.createElement("p");
  why.className = "evidence-why";
  why.textContent = semantic ? card.why_suggested : card.why_matched;
  article.append(header, excerpt, meta, why);
  if (semantic) article.classList.add("semantic-card");
  return article;
}

function documentCard(card) {
  const article = document.createElement("article");
  article.className = "document-card";
  const header = document.createElement("div");
  header.className = "document-card-header";
  const titleBlock = document.createElement("div");
  const kicker = document.createElement("span");
  kicker.className = "evidence-kicker";
  kicker.textContent = "Document-level conjunction";
  const title = document.createElement("strong");
  title.className = "evidence-title";
  title.textContent = card.document_title;
  titleBlock.append(kicker, title);
  const count = document.createElement("span");
  count.className = "metadata-pill";
  count.textContent = `${card.criterion_evidence.length} proof provision${
    card.criterion_evidence.length === 1 ? "" : "s"
  }`;
  header.append(titleBlock, count);
  const why = document.createElement("p");
  why.className = "evidence-why";
  why.textContent = card.why_matched;
  const criteria = document.createElement("div");
  criteria.className = "criterion-list";
  for (const evidence of card.criterion_evidence) {
    criteria.append(evidenceCard(evidence, true));
  }
  article.append(header, why, criteria);
  return article;
}

function renderSearchResponse(response) {
  renderQueryInterpretation(response);
  elements.searchResults.replaceChildren();
  for (const card of response.evidence) {
    elements.searchResults.append(evidenceCard(card));
  }
  for (const card of response.documents) {
    elements.searchResults.append(documentCard(card));
  }
  const verifiedCount = response.evidence.length + response.documents.length;
  if (response.semantic_suggestions.length > 0) {
    const heading = document.createElement("p");
    heading.className = "semantic-heading";
    heading.textContent = "Broader recall · review, not verified legal matches";
    elements.searchResults.append(heading);
    for (const card of response.semantic_suggestions) {
      elements.searchResults.append(evidenceCard(card, false, true));
    }
  }
  const suggestionCount = response.semantic_suggestions.length;
  if (verifiedCount === 0 && suggestionCount === 0) {
    const empty = document.createElement("div");
    empty.className = "empty-results";
    empty.textContent =
      response.stale_evidence_withdrawn > 0
        ? "The indexed match is no longer current. It was withdrawn; rebuild the document index before relying on it."
        : "No current source satisfied every visible constraint. Try removing one constraint or search for a different legal concept.";
    elements.searchResults.append(empty);
  }
  const resultKind = response.query.scope === "same_provision" ? "provision" : "document";
  const staleNote =
    response.stale_evidence_withdrawn > 0
      ? ` ${response.stale_evidence_withdrawn.toLocaleString()} stale source${
          response.stale_evidence_withdrawn === 1 ? " was" : "s were"
        } withdrawn.`
      : "";
  const boundaryNote =
    (response.inferred_boundary_evidence_withdrawn ?? 0) > 0
      ? ` ${(response.inferred_boundary_evidence_withdrawn ?? 0).toLocaleString()} searchable document${
          response.inferred_boundary_evidence_withdrawn === 1 ? "" : "s"
        } report no section structure, so they were excluded from same-clause questions.`
      : "";
  elements.searchStatus.textContent =
    `${verifiedCount.toLocaleString()} current ${resultKind}${
      verifiedCount === 1 ? "" : "s"
    } matched every visible constraint; ${suggestionCount.toLocaleString()} separately labeled meaning suggestion${
      suggestionCount === 1 ? "" : "s"
    }.${staleNote}${boundaryNote}`;
}

async function searchVault(event) {
  event.preventDefault();
  hideError();
  const query = elements.searchQuery.value.trim();
  if (!query) {
    elements.searchStatus.textContent = "Enter a legal retrieval question first.";
    elements.searchQuery.focus();
    return;
  }
  elements.searchSubmit.disabled = true;
  elements.searchStatus.textContent =
    "Checking exact constraints, on-device meaning candidates, and current source revisions…";
  try {
    const response = await invoke("search_archive_text_vault", { query });
    renderSearchResponse(response);
  } catch (error) {
    elements.searchStatus.textContent = "No evidence was returned.";
    showError(error);
  } finally {
    elements.searchSubmit.disabled = false;
  }
}

function startOver() {
  hideError();
  showView("setup");
  elements.setupStatus.textContent = `${locations.length.toLocaleString()} approved location${
    locations.length === 1 ? "" : "s"
  }. Change locations or run the metadata census again.`;
}

function backToCensus() {
  hideError();
  if (lastReport) renderReport(lastReport);
  else showView("setup");
}

async function bootstrap() {
  try {
    const state = await invoke("archive_bootstrap");
    renderLocations(state.locations);
    lastReport = state.report;
    vaultReport = state.textVaultReport;
    if (state.scanRunning) {
      showView("scanning");
    } else if (state.textVaultReport) {
      renderVaultSummary(state.textVaultReport);
      showView("search");
    } else if (state.report) {
      renderReport(state.report);
    } else {
      showView("setup");
    }
  } catch (error) {
    showError(error);
  }
}

elements.addLocations.addEventListener("click", chooseLocations);
elements.runCensus.addEventListener("click", runCensus);
elements.cancelCensus.addEventListener("click", () => cancelOperation(elements.cancelCensus));
elements.cancelIndexing.addEventListener("click", () => cancelOperation(elements.cancelIndexing));
elements.startOver.addEventListener("click", startOver);
elements.exportReport.addEventListener("click", exportReport);
elements.buildTextVault.addEventListener("click", buildTextVault);
elements.backToCensus.addEventListener("click", backToCensus);
elements.searchForm.addEventListener("submit", searchVault);
elements.dismissError.addEventListener("click", hideError);

bootstrap();
