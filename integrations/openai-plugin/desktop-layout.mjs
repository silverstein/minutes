import { readFile, rename } from 'node:fs/promises';
import path from 'node:path';

// Called only on the installer's new owned copy, before registration. Preserve
// the portable manifest outside the plugin root so older desktop runtimes use
// the explicit Codex MCP declaration instead of shadowing it.
export async function prepareDesktopLayout(root, plugin) {
  const directory = path.join(root, '.agents/plugins/minutes');
  const compatibility = JSON.parse(await readFile(path.join(directory, '.codex-plugin/plugin.json'), 'utf8'));
  if (compatibility.name !== plugin.name || compatibility.version !== plugin.version || compatibility.mcpServers !== './mcp.json') {
    throw new Error('The desktop compatibility manifest does not match the portable package.');
  }
  await rename(path.join(directory, 'plugin.json'), path.join(root, 'portable-plugin.json'));
}
