#!/usr/bin/env node
// Exercise the production get_meeting handler with a source-bound CLI overlay.
// No microphone, models, personal meetings, or persistent user configuration.
import assert from 'node:assert/strict';
import { cpSync, mkdtempSync, mkdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

if (process.platform === 'win32') {
  console.log('SKIP: the fixture executable uses a Unix shebang; overlay unit coverage runs on Windows.');
  process.exit(0);
}
const packageRoot = resolve(import.meta.dirname, '..');
const profile = realpathSync(mkdtempSync(join(tmpdir(), 'minutes-evidence-overlay-')));
const runtime = join(profile, 'runtime', 'crates', 'mcp');
const corpus = join(profile, 'meetings');
const path = join(corpus, 'meeting.md');
const body = '[SPEAKER_0 00:05] Keep annual billing.\n';
const content = `---\ntitle: Synthetic correction\ntype: meeting\ndate: 2020-01-01T00:00:00Z\nspeaker_map:\n  - speaker_label: SPEAKER_0\n    name: Wrong Name\n    confidence: high\n    source: inferred\n---\n${body}`;

async function check(mode) {
  writeFileSync(path, content);
  const client = new Client({ name: 'evidence-overlay-regression', version: '1.0.0' });
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [join(runtime, 'dist', 'index.js')],
    cwd: runtime,
    env: {
      ...process.env, HOME: profile, USERPROFILE: profile,
      MINUTES_HOME: join(profile, '.minutes'), XDG_CONFIG_HOME: join(profile, '.config'),
      MEETINGS_DIR: corpus, MINUTES_MCP_AUTO_SETUP: '0',
      MINUTES_TEST_OVERLAY_MODE: mode,
    },
    stderr: 'pipe',
  });
  let stderr = '';
  transport.stderr?.on('data', data => { stderr += data.toString(); });
  try {
    await client.connect(transport);
    const result = await client.callTool({ name: 'get_meeting', arguments: { path } }, undefined, { timeout: 70000 });
    if (mode === 'changed-source') {
      assert.equal(result.isError, true);
      assert.doesNotMatch(JSON.stringify(result), /Keep annual billing|Correct Name|Wrong Name/);
      return;
    }
    assert.ok(!result.isError, JSON.stringify(result.content));
    const detail = result.structuredContent;
    const segment = detail.evidence.segments.find(item => item.speaker?.label === 'SPEAKER_0');
    assert.ok(segment, 'source speaker label is retained');
    assert.equal(detail.evidence.source_revision, createHash('sha256').update(content).digest('hex'));
    assert.equal(content.slice(segment.start, segment.end), segment.quote);
    assert.equal(segment.timestamp_seconds, 5);
    if (mode === 'verified') {
      assert.equal(detail.overlay_applied, true);
      assert.equal(detail.speaker_map[0].name, 'Correct Name');
      assert.equal(segment.speaker.name, undefined, 'raw evidence must not repeat a corrected stale name');
      assert.deepEqual(segment.speaker, { label: 'SPEAKER_0' });
    } else {
      assert.equal(detail.overlay_applied, false);
      assert.equal(detail.speaker_map[0].name, 'Wrong Name');
      assert.equal(segment.speaker.name, 'Wrong Name');
    }
    assert.doesNotMatch(stderr, /downloading it|models? downloaded/);
  } finally {
    await client.close();
  }
}

try {
  mkdirSync(corpus, { recursive: true });
  mkdirSync(runtime, { recursive: true });
  cpSync(join(packageRoot, 'dist'), join(runtime, 'dist'), { recursive: true });
  cpSync(join(packageRoot, 'dist-ui'), join(runtime, 'dist-ui'), { recursive: true });
  cpSync(join(packageRoot, 'node_modules'), join(runtime, 'node_modules'), { recursive: true });
  writeFileSync(join(runtime, 'package.json'), '{"type":"module"}\n');
  const fixtureDir = join(profile, 'runtime', 'target', 'debug');
  mkdirSync(fixtureDir, { recursive: true });
  writeFileSync(join(fixtureDir, 'minutes'), `#!${process.execPath}
const fs = require('node:fs');
const crypto = require('node:crypto');
const command = process.argv[2];
if (command === '--version') console.log('minutes 0.28.2');
else if (command === 'capabilities') console.log(JSON.stringify({version:'0.28.2',api_version:1,features:{}}));
else if (command === 'agent-readiness') console.log(JSON.stringify({schema:1,ready:true,qmd_retirement:'ready-clean'}));
else if (command === 'get') {
  const file = process.argv[3];
  const raw = fs.readFileSync(file, 'utf8');
  const mode = process.env.MINUTES_TEST_OVERLAY_MODE;
  const result = { overlay_applied:mode !== 'none', raw_markdown:raw,
    overlay_source_sha256:crypto.createHash('sha256').update(raw).digest('hex'),
    frontmatter:{speaker_map:[{speaker_label:'SPEAKER_0',name:'Correct Name',confidence:'high',source:'manual'}]} };
  if (mode === 'stale-hash') result.overlay_source_sha256 = '0'.repeat(64);
  if (mode === 'stale-bytes') result.raw_markdown += 'changed';
  if (mode === 'missing-proof') delete result.overlay_source_sha256;
  if (mode === 'changed-source') fs.writeFileSync(file, raw.replace('type: meeting', 'type: meeting\\nsensitivity: restricted'));
  console.log(JSON.stringify(result));
} else process.exit(64);
`, { mode: 0o700 });
  for (const mode of ['none', 'verified', 'stale-hash', 'stale-bytes', 'missing-proof', 'changed-source']) await check(mode);
  console.log('PASS: get_meeting correction attribution, rejected overlay proofs, exact quotes, and final source fence');
} finally {
  rmSync(profile, { recursive: true, force: true });
}
