import { lstat, readFile, realpath } from 'node:fs/promises';
import path from 'node:path';

// Upgrade only a private installation created by this installer in the same
// parent. A similarly named marketplace or a link to another project is not
// authority to replace its registration. Existing directories are retained.
export async function ownsMinutesInstallation(root, parent) {
  try {
    const info = await lstat(root);
    if (!info.isDirectory() || info.isSymbolicLink()) return false;
    if (process.getuid && (info.uid !== process.getuid() || (info.mode & 0o077))) return false;
    const canonicalRoot = await realpath(root);
    if (path.dirname(canonicalRoot) !== await realpath(parent)) return false;
    const receiptPath = path.join(root, 'installation.json');
    if (!(await lstat(receiptPath)).isFile()) return false;
    const receipt = JSON.parse(await readFile(receiptPath, 'utf8'));
    const catalog = JSON.parse(await readFile(path.join(root, '.agents/plugins/marketplace.json'), 'utf8'));
    return receipt.plugin === 'minutes@minutes' && typeof receipt.version === 'string'
      && catalog.name === 'minutes' && catalog.plugins.length === 1 && catalog.plugins[0].name === 'minutes';
  } catch { return false; }
}
