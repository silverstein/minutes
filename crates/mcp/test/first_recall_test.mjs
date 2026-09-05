#!/usr/bin/env node

// Exercise the documented sample setup through a real MCP stdio connection.
// A separate default library catches accidental fallback to the wrong corpus.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

const packageRoot = resolve(import.meta.dirname, '..');
const serverPath = join(packageRoot, 'dist', 'index.js');
const profile = realpathSync(mkdtempSync(join(tmpdir(), 'minutes-first-recall-')));
const defaultCorpus = join(profile, 'meetings');
const defaultMeeting = join(defaultCorpus, '2026-04-01-unrelated.md');
const env = {
  ...process.env,
  HOME: profile,
  USERPROFILE: profile,
  XDG_CONFIG_HOME: join(profile, '.config'),
  MINUTES_HOME: join(profile, '.minutes'),
  MINUTES_MCP_AUTO_SETUP: '0',
};
delete env.MEETINGS_DIR;

async function withHost(overrides, operation) {
  const client = new Client({ name: 'first-recall-test', version: '1.0.0' });
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [serverPath],
    cwd: packageRoot,
    env: { ...env, ...overrides },
    stderr: 'pipe',
  });
  let stderr = '';
  transport.stderr?.on('data', data => { stderr += data.toString(); });
  try {
    await client.connect(transport);
    const call = async (name, args) => {
      const result = await client.callTool({ name, arguments: args }, undefined, { timeout: 70000 });
      assert.ok(!result.isError, `${name}: ${JSON.stringify(result.content)}`);
      return result;
    };
    await operation(call, client);
    assert.doesNotMatch(stderr, /downloading it|model downloaded|models downloaded/);
  } finally {
    await client.close();
  }
}

try {
  mkdirSync(defaultCorpus, { recursive: true });
  writeFileSync(defaultMeeting, '---\ntitle: Unrelated default library\ntype: meeting\ndate: 2026-04-01T10:00:00Z\n---\n\nOnly the default library contains the violet lantern.\n');
  const output = execFileSync(process.execPath, [serverPath, '--demo'], { env, encoding: 'utf8' });
  const config = JSON.parse(output.slice(output.indexOf('{'), output.lastIndexOf('}') + 1)).mcpServers['minutes-demo'];
  assert.equal(config.env.MINUTES_MCP_AUTO_SETUP, '0');
  const demoCorpus = config.env.MEETINGS_DIR;
  const reversalPath = join(demoCorpus, '2026-03-25-pricing-reversal.md');

  await withHost(config.env, async (call, client) => {
    const list = await call('list_meetings', { limit: 10 });
    assert.equal(list.structuredContent.meetings.length, 5);
    assert.ok(list.structuredContent.meetings.every(meeting => meeting.path.startsWith(demoCorpus)));
    const search = await call('search_meetings', { query: 'pricing' });
    assert.equal(search.structuredContent.results.length, 2);
    const reversal = await call('get_meeting', { path: reversalPath });
    assert.match(JSON.stringify(reversal.content), /annual-only/);
    assert.match(JSON.stringify(reversal.content), /2026-02-28/);
    const missing = await call('search_meetings', { query: 'contract' });
    assert.deepEqual(missing.structuredContent.results, []);
    const report = await call('consistency_report', {});
    const conflict = report.structuredContent.decision_conflicts.find(item => item.topic === 'pricing');
    assert.ok(conflict, 'the sample pricing decisions must appear in the report');
    assert.match(conflict.latest.what, /annual-only/);
    assert.equal(conflict.latest.path, reversalPath);
    assert.ok(conflict.previous.some(item => item.path === join(demoCorpus, '2026-02-28-pricing-strategy.md')));
    assert.doesNotMatch(JSON.stringify(report), /violet lantern|Unrelated default library/);
    const outside = await client.callTool({ name: 'get_meeting', arguments: { path: defaultMeeting } });
    assert.ok(outside.isError);
    assert.doesNotMatch(JSON.stringify(outside), /violet lantern/);
  });

  await withHost({}, async (call, client) => {
    const list = await call('list_meetings', { limit: 10 });
    assert.equal(list.structuredContent.meetings.length, 1);
    assert.equal(list.structuredContent.meetings[0].path, defaultMeeting);
    const search = await call('search_meetings', { query: 'pricing' });
    assert.deepEqual(search.structuredContent.results, []);
    const outside = await client.callTool({ name: 'get_meeting', arguments: { path: reversalPath } });
    assert.ok(outside.isError);
    assert.doesNotMatch(JSON.stringify(outside), /annual-only/);
  });
  assert.equal(readFileSync(defaultMeeting, 'utf8').includes('violet lantern'), true);
  console.log('PASS: sample setup, sourced reversal, missing search results, and corpus isolation');
} finally {
  rmSync(profile, { recursive: true, force: true });
}
