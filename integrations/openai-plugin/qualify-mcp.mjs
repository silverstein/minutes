// Real stdio qualification of the published package; no model calls or user corpus.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtemp, mkdir, cp, rm, readFile, realpath, writeFile } from 'node:fs/promises';
import { tmpdir, homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

const require = createRequire(import.meta.url);
const packageVersion = JSON.parse(await readFile(new URL('node_modules/minutes-mcp/package.json', import.meta.url), 'utf8')).version;
const expectedTools = JSON.parse(await readFile(new URL('../../manifest.json', import.meta.url), 'utf8')).tools;
const profile = await realpath(await mkdtemp(path.join(tmpdir(), 'minutes-openai-plugin-')));
const corpus = path.join(profile, 'sample-meetings');
const outside = path.join(profile, 'outside.md');
const client = new Client({ name: 'minutes-openai-qualification', version: '0.1.0' });
try {
  await cp(fileURLToPath(new URL('../../crates/mcp/fixtures/demo/', import.meta.url)), corpus, { recursive: true });
  await writeFile(outside, '---\ntitle: Outside sample library\ndate: 2026-09-30\n---\nPrivate violet lantern.\n');
  await mkdir(path.join(profile, 'config'), { recursive: true });
  await mkdir(path.join(profile, 'minutes-home'), { recursive: true });
  const binary = process.env.MINUTES_QUALIFICATION_BIN;
  if (binary && (process.platform !== 'linux' || !path.isAbsolute(binary))) throw new Error('MINUTES_QUALIFICATION_BIN requires Linux/bubblewrap and an absolute binary path.');
  // The published MCP resolves ~/.cargo/bin/minutes before PATH and has no
  // binary override. Overlay only the child process's filesystem; preserve
  // the installed host CLI and HOME. No shim or source-package substitution.
  const command = binary ? 'bwrap' : process.execPath;
  const args = binary ? ['--ro-bind', '/', '/', '--tmpfs', '/tmp', '--bind', profile, profile,
    '--bind', path.join(profile, 'minutes-home'), path.join(homedir(), '.minutes'),
    '--bind', path.join(profile, 'config'), path.join(homedir(), '.config'),
    '--ro-bind', binary, path.join(homedir(), '.cargo/bin/minutes'), '--proc', '/proc', '--dev', '/dev',
    '--', process.execPath, require.resolve('minutes-mcp')] : [require.resolve('minutes-mcp')];
  const transport = new StdioClientTransport({ command, args,
    env: { ...process.env, MEETINGS_DIR: corpus, MINUTES_HOME: path.join(profile, 'minutes-home'),
      MINUTES_DATA_DIR: path.join(profile, 'minutes-home'),
      XDG_CONFIG_HOME: path.join(profile, 'config'), MINUTES_MCP_AUTO_SETUP: '0' }, stderr: 'pipe' });
  // Avoid printing machine-specific or private diagnostics. The failed assertion
  // supplies a named check; inspect stderr locally if qualification fails.
  transport.stderr?.on('data', () => {});
  await client.connect(transport);
  const catalog = await client.listTools();
  const names = catalog.tools.map(tool => tool.name);
  assert.equal(new Set(names).size, names.length, 'duplicate tool names');
  assert.deepEqual([...names].sort(), expectedTools.map(tool => typeof tool === 'string' ? tool : tool.name).sort(), 'published MCP and manifest tool surfaces differ');
  const call = async (name, args = {}) => {
    const result = await client.callTool({ name, arguments: args }, undefined, { timeout: 70000 });
    assert.ok(!result.isError, `${name} failed: ${JSON.stringify(result.content)}`);
    return result;
  };
  const list = await call('list_meetings', { limit: 10 });
  assert.equal(list.structuredContent.meetings.length, 5);
  assert.ok(list.structuredContent.meetings.every(meeting => meeting.path.startsWith(`${corpus}/`)));
  const search = await call('search_meetings', { query: 'pricing' });
  assert.equal(search.structuredContent.results.length, 2);
  const reversal = path.join(corpus, '2026-03-25-pricing-reversal.md');
  const source = await call('get_meeting', { path: reversal });
  assert.match(JSON.stringify(source.content), /annual-only/);
  const report = await call('consistency_report');
  const conflict = report.structuredContent.decision_conflicts.find(item => item.topic === 'pricing');
  assert.equal(conflict.latest.path, reversal);
  assert.ok(conflict.previous.some(item => item.path.endsWith('2026-02-28-pricing-strategy.md')));
  const denied = await client.callTool({ name: 'get_meeting', arguments: { path: outside } });
  assert.ok(denied.isError);
  assert.doesNotMatch(JSON.stringify(denied), /violet lantern/);
  console.log(JSON.stringify({ status: 'passed', package: `minutes-mcp@${packageVersion}`, transport: 'stdio',
    advertised_tools: names.sort(), advertised_tool_count: names.length, tool_surface_matches_manifest: true,
    sample_meetings: 5, pricing_sources: 2, decision_reversal: true, outside_corpus_denied: true,
    model_calls: 0, real_meeting_reads: 0 }));
} finally {
  await client.close();
  await rm(profile, { recursive: true, force: true });
}
