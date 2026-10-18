#!/usr/bin/env node
/** Guard the generated wire contract, explicit Rust emissions, and exhaustive
 * frontend dispatch. Claude's dynamic JSON passthrough is validated at runtime;
 * this check does not pretend to infer arbitrary Rust/JSON payload types. */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
process.argv.push('--check');
await import('./generate_recall_contract.mjs');
const read = path => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const schema = JSON.parse(read('tauri/src/contracts/recall-chat.json'));
const backend = read('tauri/src-tauri/src/commands.rs');
const controller = read('tauri/src/scripts/recall-chat.js');
const panel = read('tauri/src/index.html');
const dispatched = new Set([...controller.matchAll(/case '([a-z_]+)':/g)].map(match => match[1]));
assert.deepEqual([...dispatched].sort(), Object.keys(schema.variants).sort(), 'Recall decoder must handle every contracted tag exactly once');
for (const script of ['recall-contract.js', 'recall-chat.js']) assert.ok(panel.includes(`src="scripts/${script}"`), `shell must load ${script}`);
assert.ok(panel.includes('window.MinutesRecallChat.create('), 'shell must use the tested controller');

let emitted = 0, passthrough = 0;
for (const match of backend.matchAll(/\.emit_to\(\s*"main",\s*"recall-chat-chunk",\s*([\s\S]*?)\)\s*\.ok\(\)/g)) {
  const payload = match[1];
  if (/^json\s*$/.test(payload)) { passthrough++; continue; }
  const type = /"type"\s*:\s*"([a-z_]+)"/.exec(payload)?.[1];
  assert.ok(type && schema.variants[type], 'Unknown explicit Recall emission; extend the contract and decoder');
  for (const field of Object.keys(schema.variants[type]).filter(name => !name.endsWith('?'))) {
    assert.ok(new RegExp(`"${field}"\\s*:`).test(payload), `Rust ${type} emission is missing ${field}`);
  }
  emitted++;
}
assert.equal(passthrough, 1, 'Claude passthrough changed; inspect the runtime validation boundary');
assert.ok(emitted >= 3, 'Expected explicit local-provider emissions; scanner must be updated if Rust shape changed');
console.log(`Recall contract: ${emitted} explicit Rust emissions, ${passthrough} runtime-validated passthrough; exhaustive dispatch`);
