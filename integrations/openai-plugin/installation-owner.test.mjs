import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm, symlink, readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { tmpdir } from 'node:os';
import { ownsMinutesInstallation } from './installation-owner.mjs';
import { prepareDesktopLayout } from './desktop-layout.mjs';

test('desktop install removes portable shadowing and retains the patched MCP file', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'minutes-desktop-layout-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const directory = path.join(root, '.agents/plugins/minutes');
  await mkdir(path.join(directory, '.codex-plugin'), { recursive: true });
  const plugin = { name: 'minutes', version: '0.2.2' };
  const portable = JSON.stringify(plugin);
  const compatibilityPath = path.join(directory, '.codex-plugin/plugin.json');
  await writeFile(path.join(directory, 'plugin.json'), portable);
  await writeFile(path.join(directory, 'mcp.json'), 'patched absolute runtime');
  await writeFile(compatibilityPath, JSON.stringify({ ...plugin, mcpServers: './wrong.json' }));
  await assert.rejects(prepareDesktopLayout(root, plugin), /does not match/);
  assert.equal(await readFile(path.join(directory, 'plugin.json'), 'utf8'), portable);
  await writeFile(compatibilityPath, JSON.stringify({ ...plugin, mcpServers: './mcp.json' }));
  await prepareDesktopLayout(root, plugin);
  await assert.rejects(stat(path.join(directory, 'plugin.json')), { code: 'ENOENT' });
  assert.equal(await readFile(path.join(root, 'portable-plugin.json'), 'utf8'), portable);
  assert.equal(await readFile(path.join(directory, 'mcp.json'), 'utf8'), 'patched absolute runtime');
});

test('upgrades distinguish owned installations from unrelated or linked sources', async t => {
  const parent = await mkdtemp(path.join(tmpdir(), 'minutes-install-owner-'));
  t.after(() => rm(parent, { recursive: true, force: true }));
  const root = path.join(parent, 'minutes-previous');
  await mkdir(path.join(root, '.agents/plugins'), { recursive: true, mode: 0o700 });
  // Recursive mkdir uses the supplied mode for the newly created ancestors.
  await writeFile(path.join(root, 'installation.json'), JSON.stringify({ plugin: 'minutes@minutes', version: '0.2.0' }));
  await writeFile(path.join(root, '.agents/plugins/marketplace.json'), JSON.stringify({ name: 'minutes', plugins: [{ name: 'minutes' }] }));
  assert.equal(await ownsMinutesInstallation(root, parent), true);
  assert.equal(await ownsMinutesInstallation(root, root), false, 'wrong installation parent');
  await symlink(root, path.join(parent, 'linked'));
  assert.equal(await ownsMinutesInstallation(path.join(parent, 'linked'), parent), false, 'linked directory');
  await writeFile(path.join(root, 'installation.json'), JSON.stringify({ plugin: 'other@minutes', version: '0.2.0' }));
  assert.equal(await ownsMinutesInstallation(root, parent), false, 'different plugin identity');
  await writeFile(path.join(root, 'installation.json'), 'not JSON');
  assert.equal(await ownsMinutesInstallation(root, parent), false, 'invalid ownership receipt');
});
