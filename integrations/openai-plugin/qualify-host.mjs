// Verify discovery through the actual Codex/ChatGPT runtime, not a direct
// connection to the MCP entrypoint. No model turn or meeting tool is invoked.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';

const args = process.argv.slice(2);
if (args.length && (args.length !== 2 || args[0] !== '--codex')) throw new Error('Use --codex /path/to/codex or omit it.');
const expected = JSON.parse(await readFile(new URL('../../manifest.json', import.meta.url), 'utf8')).tools;
const expectedNames = expected.map(tool => typeof tool === 'string' ? tool : tool.name).sort();
const child = spawn(args[1] ?? 'codex', ['app-server'], { stdio: ['pipe', 'pipe', 'pipe'] });
child.stderr.on('data', () => {});
const lines = createInterface({ input: child.stdout });
const pending = new Map();
let sequence = 0;
lines.on('line', line => {
  let message;
  try { message = JSON.parse(line); } catch { return; }
  const waiting = pending.get(message.id);
  if (waiting) {
    pending.delete(message.id);
    message.error ? waiting.reject(new Error(`${waiting.method}: ${message.error.message}`)) : waiting.resolve(message.result);
  }
});
const failPending = error => {
  for (const waiting of pending.values()) waiting.reject(error);
  pending.clear();
};
child.on('error', failPending);
child.on('exit', () => failPending(new Error('Desktop runtime exited before qualification completed.')));
function request(method, params) {
  return new Promise((resolve, reject) => {
    const id = ++sequence;
    pending.set(id, { method, resolve, reject });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  });
}
const deadline = setTimeout(() => {
  failPending(new Error('Desktop MCP discovery did not finish within 45 seconds.'));
  child.kill();
}, 45000);
try {
  await request('initialize', { clientInfo: { name: 'minutes-host-qualification', version: '0.1.0' }, capabilities: { experimentalApi: true } });
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'initialized' }) + '\n');
  const servers = [];
  let cursor;
  do {
    const page = await request('mcpServerStatus/list', { limit: 100, ...(cursor ? { cursor } : {}) });
    servers.push(...page.data);
    cursor = page.nextCursor;
  } while (cursor);
  const minutes = servers.filter(server => server.pluginId === 'minutes@minutes' && server.name === 'minutes');
  assert.equal(minutes.length, 1, 'Desktop runtime must discover exactly one Minutes plugin MCP server.');
  const names = Object.values(minutes[0].tools ?? {}).map(tool => tool.name).sort();
  assert.deepEqual(names, expectedNames, 'Desktop runtime must expose the complete Minutes MCP tool surface.');
  console.log(JSON.stringify({ status: 'passed', plugin: 'minutes@minutes', runtime_discovered_server: true,
    tool_count: names.length, tool_surface_matches_manifest: true, model_turns: 0, meeting_reads: 0,
    in_app_execution: 'requires a separate ChatGPT conversation test' }));
} finally {
  clearTimeout(deadline);
  child.kill();
  lines.close();
}
