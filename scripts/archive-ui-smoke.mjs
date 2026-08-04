#!/usr/bin/env node

import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const chrome =
  process.env.ARCHIVE_TEST_CHROME ??
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const profile = await mkdtemp(join(tmpdir(), "minutes-archive-ui-"));
const screenshot = join(profile, "search-proof.png");
const frontend = pathToFileURL(resolve("archive/src/index.html")).href;

const census = {
  schema: "minutes.archive-census.v1",
  status: "complete",
  privacy: {
    document_content_read: false,
    filenames_emitted: false,
    paths_emitted: false,
    symlinks_followed: false,
    hashes_computed: false,
  },
  summary: {
    approved_locations: 1,
    artifacts: 4,
    regular_files: 4,
    packages: 0,
    regular_file_bytes: 4096,
    directories_scanned: 1,
  },
  formats: [
    {
      extension: ".txt",
      category: "plain_text",
      files: 3,
      packages: 0,
      regular_file_bytes: 3072,
    },
    {
      extension: ".pdf",
      category: "pdf",
      files: 1,
      packages: 0,
      regular_file_bytes: 1024,
    },
  ],
  categories: [
    { category: "plain_text", artifacts: 3, regular_file_bytes: 3072 },
    { category: "pdf", artifacts: 1, regular_file_bytes: 1024 },
  ],
  age_buckets: {},
  size_buckets: {},
  signals: {
    symlinks_skipped: 0,
    hidden_artifacts: 0,
    icloud_placeholders: 0,
    zero_byte_files: 0,
    permission_mode_unreadable: 0,
    special_files_skipped: 0,
    metadata_errors: 0,
    directory_errors: 0,
    max_depth: 1,
  },
};

const vaultReport = {
  schema: "minutes.archive-document-vault.v1",
  vault_id: "local-private-vault",
  approved_locations: 1,
  indexed_documents: 3,
  inferred_boundary_documents: 0,
  indexed_bytes: 3072,
  unsupported_files_skipped: 1,
  oversized_files_skipped: 0,
  malformed_text_files_skipped: 0,
  conversion_failures: 0,
  ocr_required_files: 0,
  searchable_pdf_documents: 1,
  docx_documents: 1,
  duplicate_files_skipped: 0,
  symlinks_skipped: 0,
  metadata_errors: 0,
  directory_errors: 0,
  source_content_persisted: false,
  retrieval_index_persisted: false,
  converter_sandbox_verified: true,
  semantic_worker_sandbox_verified: true,
  semantic_retrieval_enabled: true,
  semantic_model: {
    model_id: "apple-nl-sentence-en-r1",
    revision: 1,
    dimension: 512,
    built_in_os_asset: true,
    model_download_requested: false,
  },
  semantic_provisions_indexed: 4,
  semantic_provisions_skipped: 0,
  semantic_derivatives_persisted: false,
  semantic_model_download_requested: false,
  supported_formats: [".docx", ".md", ".pdf", ".text", ".txt"],
};

const evidence = {
  query: {
    raw: "Find confidentiality provisions under three sentences covering affiliates.",
    scope: "same_provision",
    required_concepts: ["confidentiality", "affiliates"],
    excluded_concepts: [],
    exact_phrase: null,
    max_sentences: 3,
    limit: 20,
  },
  evidence: [
    {
      vault_id: "local-private-vault",
      document_id: "document-0000000000000001",
      document_title: "Synthetic Agreement",
      provision_heading: "7. CONFIDENTIALITY",
      source_anchor: "section:0001",
      exact_excerpt:
        "Confidential Information includes information of Recipient and its affiliates.",
      sentence_count: 1,
      source_revision: { sha256: "00", byte_len: 76 },
      source_converter: "pdf-extract-0.12.0-v1",
      matched_concepts: ["confidentiality", "affiliates"],
      why_matched:
        "Matched confidentiality, affiliates, sentence limit in the same provision; 1 sentence.",
      lexical_rank: -2.1,
      index_fresh: true,
    },
  ],
  documents: [],
  semantic_suggestions: [
    {
      vault_id: "local-private-vault",
      document_id: "document-0000000000000002",
      document_title: "Meaning Similar Agreement",
      provision_heading: "8. NONDISCLOSURE",
      source_anchor: "paragraph:000021/section:0001",
      exact_excerpt:
        "The recipient must protect all nonpublic business material from disclosure.",
      sentence_count: 1,
      source_revision: { sha256: "11", byte_len: 82 },
      source_converter: "docx-xml-0.41.0-v1",
      semantic_similarity: 0.31,
      why_suggested:
        "Meaning-similar suggestion from a revision-pinned on-device model; review the exact excerpt. This is not a determination of legal sufficiency.",
      index_fresh: true,
    },
  ],
  lexical_candidates_considered: 1,
  semantic_candidates_considered: 4,
  semantic_query_applied: true,
  semantic_model: {
    model_id: "apple-nl-sentence-en-r1",
    revision: 1,
    dimension: 512,
    built_in_os_asset: true,
    model_download_requested: false,
  },
  stale_evidence_withdrawn: 0,
  inferred_boundary_evidence_withdrawn: 0,
};

const mockScript = `
  window.__TAURI__ = {
    core: {
      invoke: async (command) => {
        const responses = ${JSON.stringify({
          archive_bootstrap: {
            locations: [],
            scanRunning: false,
            report: null,
            textVaultReport: null,
          },
          choose_archive_locations: [{ id: 1, label: "Approved location 1" }],
          remove_archive_location: [],
          run_archive_census: census,
          cancel_archive_census: true,
          export_archive_census: false,
          build_archive_text_vault: vaultReport,
          search_archive_text_vault: evidence,
        })};
        return structuredClone(responses[command]);
      }
    }
  };
`;

class Cdp {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.nextId = 1;
    this.pending = new Map();
    this.socket.onmessage = (event) => {
      const message = JSON.parse(event.data);
      if (!message.id) return;
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message));
      else pending.resolve(message.result);
    };
  }

  async ready() {
    if (this.socket.readyState === WebSocket.OPEN) return;
    await new Promise((resolveOpen, rejectOpen) => {
      this.socket.onopen = resolveOpen;
      this.socket.onerror = rejectOpen;
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolveResult, rejectResult) => {
      this.pending.set(id, { resolve: resolveResult, reject: rejectResult });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    this.socket.close();
  }
}

const browser = spawn(
  chrome,
  [
    "--headless=new",
    "--disable-gpu",
    "--hide-scrollbars",
    "--allow-file-access-from-files",
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    "--window-size=1080,900",
    "about:blank",
  ],
  { stdio: "ignore" },
);

async function waitForFile(path, attempts = 100) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      return await readFile(path, "utf8");
    } catch {
      await new Promise((resolveWait) => setTimeout(resolveWait, 50));
    }
  }
  throw new Error(`Timed out waiting for ${path}`);
}

try {
  const [port] = (await waitForFile(join(profile, "DevToolsActivePort"))).split("\n");
  const pages = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) =>
    response.json(),
  );
  const page = pages.find((candidate) => candidate.type === "page");
  if (!page) throw new Error("Chrome did not expose a test page");
  const cdp = new Cdp(page.webSocketDebuggerUrl);
  await cdp.ready();
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Page.addScriptToEvaluateOnNewDocument", { source: mockScript });
  await cdp.send("Page.navigate", { url: frontend });
  await new Promise((resolveWait) => setTimeout(resolveWait, 250));

  const smoke = await cdp.send("Runtime.evaluate", {
    awaitPromise: true,
    returnByValue: true,
    expression: `
      (async () => {
        const waitFor = async (predicate, label) => {
          for (let attempt = 0; attempt < 100; attempt += 1) {
            if (predicate()) return;
            await new Promise((resolve) => setTimeout(resolve, 20));
          }
          throw new Error("Timed out waiting for " + label);
        };
        await waitFor(() => !document.querySelector("#setup-view").hidden, "setup");
        const setupButtonBounds = document
          .querySelector("#add-locations")
          .getBoundingClientRect()
          .toJSON();
        document.querySelector("#add-locations").click();
        await waitFor(() => document.querySelectorAll("#location-list li").length === 1, "location");
        document.querySelector("#run-census").click();
        await waitFor(() => !document.querySelector("#results-view").hidden, "census result");
        if (!document.querySelector("#result-summary").textContent.includes("without reading")) {
          throw new Error("Census privacy copy is missing");
        }
        document.querySelector("#build-text-vault").click();
        await waitFor(() => !document.querySelector("#search-view").hidden, "search view");
        const query = document.querySelector("#search-query");
        query.value = "Find confidentiality provisions under three sentences covering affiliates.";
        document.querySelector("#search-form").dispatchEvent(
          new Event("submit", { bubbles: true, cancelable: true })
        );
        await waitFor(() => document.querySelectorAll(".evidence-card").length === 2, "evidence");
        const body = document.body.innerText;
        if (
          !body.includes("Synthetic Agreement") ||
          !body.includes("Source verified") ||
          !body.toLowerCase().includes("review, not verified legal matches") ||
          !body.includes("Closing the window ends the session and discards the index")
        ) {
          throw new Error("Evidence provenance or session-disposal notice did not render");
        }
        if (body.includes("/Users/") || body.includes("SYNTHETIC_CONTENT_CANARY")) {
          throw new Error("A path or source canary crossed the UI boundary");
        }
        return {
          locations: document.querySelectorAll("#location-list li").length,
          evidenceCards: document.querySelectorAll(".evidence-card").length,
          searchVisible: !document.querySelector("#search-view").hidden,
          setupButtonBounds,
        };
      })()
    `,
  });
  if (smoke.exceptionDetails) {
    throw new Error(smoke.exceptionDetails.exception?.description ?? "UI smoke failed");
  }
  const image = await cdp.send("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: false,
  });
  await writeFile(screenshot, Buffer.from(image.data, "base64"));
  cdp.close();
  process.stdout.write(
    `${JSON.stringify(
      {
        ...smoke.result.value,
        ...(process.env.ARCHIVE_KEEP_UI_SMOKE ? { screenshot } : {}),
      },
      null,
      2,
    )}\n`,
  );
} finally {
  if (browser.exitCode === null) {
    const exited = new Promise((resolveExit) => browser.once("exit", resolveExit));
    browser.kill("SIGTERM");
    await Promise.race([
      exited,
      new Promise((resolveWait) => setTimeout(resolveWait, 1000)),
    ]);
  }
  if (!process.env.ARCHIVE_KEEP_UI_SMOKE) {
    for (let attempt = 0; attempt < 10; attempt += 1) {
      try {
        await rm(profile, { recursive: true, force: true });
        break;
      } catch (error) {
        if (attempt === 9) throw error;
        await new Promise((resolveWait) => setTimeout(resolveWait, 50));
      }
    }
  }
}
