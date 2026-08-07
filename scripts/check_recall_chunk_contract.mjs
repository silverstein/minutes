#!/usr/bin/env node
/**
 * Guard the Recall chat chunk contract.
 *
 * The backend emits `recall-chat-chunk` payloads tagged with a `type`, and the
 * panel dispatches on that tag. Nothing links the two, so a provider path can
 * stream perfectly while the UI silently discards every chunk.
 *
 * That is not hypothetical. The Ollama path emitted `{"type":"text",...}` from
 * the day it landed and the panel never had a branch for it, so local-model
 * Recall chat rendered nothing at all until #650. No Rust test could see it:
 * the defect lived in the gap between the two layers.
 *
 * This asserts every emitted tag is handled. Run from the repo root.
 */
import { readFileSync } from "node:fs";

const BACKEND = "tauri/src-tauri/src/commands.rs";
const PANEL = "tauri/src/index.html";

const backend = readFileSync(BACKEND, "utf8");
const panel = readFileSync(PANEL, "utf8");

// Emitted tags: json!({"type": "<tag>" ...}) passed to a recall-chat-chunk emit.
// Scan a window after each emit so unrelated json! macros are not swept in.
const emitted = new Set();
const emitRe = /"recall-chat-chunk"/g;
let m;
while ((m = emitRe.exec(backend)) !== null) {
  const window = backend.slice(m.index, m.index + 400);
  for (const t of window.matchAll(/"type"\s*:\s*"([a-z_]+)"/g)) emitted.add(t[1]);
}

// Handled tags: chunk.type === '<tag>' in the panel's dispatch.
const handled = new Set();
for (const t of panel.matchAll(/chunk\.type\s*===\s*'([a-z_]+)'/g)) handled.add(t[1]);

if (emitted.size === 0) {
  console.error(
    "recall chunk contract: found no emitted chunk types; the emit shape in " +
      `${BACKEND} probably changed and this guard needs updating.`,
  );
  process.exit(1);
}

const unhandled = [...emitted].filter((t) => !handled.has(t));
if (unhandled.length > 0) {
  console.error(
    `recall chunk contract: ${BACKEND} emits chunk type(s) the panel never ` +
      `renders: ${unhandled.join(", ")}.\n` +
      `  handled in ${PANEL}: ${[...handled].sort().join(", ") || "(none)"}\n` +
      "  A provider streaming one of these produces an empty reply bubble.\n" +
      "  Add a branch to the recall-chat-chunk listener, or stop emitting the tag.",
  );
  process.exit(1);
}

console.log(
  `recall chunk contract: ok (${[...emitted].sort().join(", ")} all handled)`,
);
