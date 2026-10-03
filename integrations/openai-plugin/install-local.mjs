#!/usr/bin/env node
// Install the generated local plugin with a pinned runtime and absolute Node
// entrypoint, avoiding GUI-launch PATH differences. No model or meeting reads.
import { cp, mkdir, mkdtemp, readFile, writeFile, stat, chmod } from 'node:fs/promises';
import { homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { ownsMinutesInstallation } from './installation-owner.mjs';
import { prepareDesktopLayout } from './desktop-layout.mjs';

const args = process.argv.slice(2);
const options = {};
for (let i = 0; i < args.length; i += 2) {
  if (!['--codex', '--npm', '--source', '--parent'].includes(args[i]) || !args[i + 1] || options[args[i]]) throw new Error('Use --codex, --npm, --source or --parent followed by a path.');
  options[args[i]] = args[i + 1];
}
const source = path.resolve(options['--source'] ?? fileURLToPath(new URL('../..', import.meta.url)));
const codex = options['--codex'] ?? 'codex';
const npm = options['--npm'] ?? 'npm';
const parent = path.resolve(options['--parent'] ?? path.join(homedir(), '.local/share/minutes/chatgpt-plugins'));

async function run(command, argv, cwd) {
  await new Promise((resolve, reject) => {
    const child = spawn(command, argv, { cwd, shell: false, stdio: 'inherit',
      env: { ...process.env, PATH: [path.dirname(process.execPath), process.env.PATH ?? ''].join(path.delimiter) } });
    child.once('error', reject);
    child.once('exit', code => code === 0 ? resolve() : reject(new Error('Local plugin installation step failed. The owned installation directory is preserved.')));
  });
}

async function capture(command, argv, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, argv, { cwd, shell: false, stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '';
    child.stdout.on('data', chunk => { output += chunk; });
    child.stderr.on('data', () => {});
    child.once('error', reject);
    child.once('exit', code => code === 0 ? resolve(output) : reject(new Error('Could not inspect existing plugin marketplaces.')));
  });
}

const plugin = JSON.parse(await readFile(path.join(source, '.agents/plugins/minutes/plugin.json'), 'utf8'));
const manifest = JSON.parse(await readFile(path.join(source, '.agents/plugins/minutes/mcp.json'), 'utf8'));
const runtimePackage = JSON.parse(await readFile(path.join(source, 'integrations/openai-plugin/package.json'), 'utf8'));
if (manifest.mcpServers.minutes.args[1] !== `minutes-mcp@${runtimePackage.dependencies['minutes-mcp']}`) throw new Error('Generated plugin and runtime package versions differ. Recompile before installation.');
// The prototype store and API/Codex credentials are never read or copied.
await mkdir(parent, { recursive: true, mode: 0o700 });
const root = await mkdtemp(path.join(parent, `minutes-${plugin.version}-`));
await chmod(root, 0o700);
const runtime = path.join(root, 'runtime');
await mkdir(runtime, { mode: 0o700 });
await mkdir(path.join(root, '.agents/plugins'), { recursive: true });
await cp(path.join(source, '.agents/plugins/minutes'), path.join(root, '.agents/plugins/minutes'), { recursive: true, dereference: false });
await cp(path.join(source, '.agents/plugins/marketplace.json'), path.join(root, '.agents/plugins/marketplace.json'));
for (const name of ['package.json', 'package-lock.json']) await cp(path.join(source, 'integrations/openai-plugin', name), path.join(runtime, name));
await run(npm, ['ci', '--ignore-scripts'], runtime);
manifest.mcpServers.minutes.command = process.execPath;
manifest.mcpServers.minutes.args = [path.join(runtime, 'node_modules/minutes-mcp/dist/index.js')];
manifest.mcpServers.minutes.env = { MINUTES_MCP_AUTO_SETUP: '0' };
await writeFile(path.join(root, '.agents/plugins/minutes/mcp.json'), JSON.stringify(manifest, null, 2) + '\n');
// ChatGPT 26.928's bundled Codex 0.159.2 reads portable skills but does not
// load portable MCP components, even with an inline or compatibility override.
// Select the generated Codex entrypoint in this owned local copy. Preserve the
// portable manifest outside the plugin root; the source package stays portable.
await prepareDesktopLayout(root, plugin);
// Keep a private rollback snapshot before Codex modifies its configuration.
// The snapshot is not printed, put into the plugin, or sent to a provider.
const backup = path.join(root, 'local-config-backup');
await mkdir(backup, { mode: 0o700 });
for (const [from, name] of [[path.join(homedir(), '.codex/config.toml'), 'codex-config.toml'], [path.join(homedir(), '.agents/plugins/marketplace.json'), 'personal-marketplace.json']]) {
  try {
    if (!(await stat(from)).isFile()) throw new Error('Expected a regular configuration file.');
    await cp(from, path.join(backup, name));
    await chmod(path.join(backup, name), 0o600);
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
}
await writeFile(path.join(root, 'installation.json'), JSON.stringify({ plugin: 'minutes@minutes', version: plugin.version, runtime: runtimePackage.dependencies['minutes-mcp'], node: process.execPath, source, created_at: new Date().toISOString(), local_mcp_only: true, manifest_layout: 'codex-compatibility' }, null, 2) + '\n', { mode: 0o600 });
const marketplaces = JSON.parse(await capture(codex, ['plugin', 'marketplace', 'list', '--json'], homedir()));
const previous = marketplaces.marketplaces.find(item => item.name === 'minutes');
const previousRoot = previous?.marketplaceSource?.source;
if (previous && (previous.marketplaceSource?.sourceType !== 'local' || !await ownsMinutesInstallation(previousRoot, parent))) {
  throw new Error('The existing Minutes marketplace is not an owned installation under this parent. Its registration was preserved.');
}
if (previous) await run(codex, ['plugin', 'marketplace', 'remove', 'minutes', '--json'], root);
try {
  await run(codex, ['plugin', 'marketplace', 'add', root, '--json'], root);
  await run(codex, ['plugin', 'add', 'minutes@minutes', '--json'], root);
} catch (error) {
  if (previous) {
    // Restore just our marketplace registration; never overwrite the user's
    // entire config from the backup while other tools may be editing it.
    const current = JSON.parse(await capture(codex, ['plugin', 'marketplace', 'list', '--json'], homedir()))
      .marketplaces.find(item => item.name === 'minutes');
    if (current && current.marketplaceSource?.source !== root && current.marketplaceSource?.source !== previousRoot) {
      throw new Error('Minutes registration changed during installation. Preserved the concurrent registration and private rollback snapshot.');
    }
    if (current?.marketplaceSource?.source === root) await run(codex, ['plugin', 'marketplace', 'remove', 'minutes', '--json'], root);
    if (current?.marketplaceSource?.source !== previousRoot) await run(codex, ['plugin', 'marketplace', 'add', previousRoot, '--json'], root);
    await run(codex, ['plugin', 'add', 'minutes@minutes', '--json'], root);
  }
  throw error;
}
await run(codex, ['plugin', 'list', '--marketplace', 'minutes', '--json'], root);
console.log(JSON.stringify({ installed: true, root, version: plugin.version, restart_chatgpt_desktop_needed: true, native_app_changed: false, model_calls: 0, meeting_reads: 0 }));
