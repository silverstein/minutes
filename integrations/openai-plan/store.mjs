import { constants } from 'node:fs';
import { lstat, mkdir, open, rename, unlink } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { homedir } from 'node:os';
import path from 'node:path';

export const DEFAULT_STORE = path.join(homedir(), '.config/minutes/openai-plan-prototype');

// Separate prototype storage; never read Codex credentials or activate a provider.
export class Store {
  constructor(root = DEFAULT_STORE) { this.root = path.resolve(root); }

  async prepare() {
    if (process.platform === 'win32') throw new Error('Prototype credential storage is qualified on Unix only.');
    // Reject symlink ancestors as well as symlink credential files.
    let current = path.parse(this.root).root;
    for (const part of this.root.slice(current.length).split(path.sep)) {
      current = path.join(current, part);
      try { await mkdir(current, { mode: 0o700 }); }
      catch (error) { if (error.code !== 'EEXIST') throw error; }
      const info = await lstat(current);
      if (!info.isDirectory() || info.isSymbolicLink()) throw new Error('Credential directory must not contain symlinks.');
    }
    const info = await lstat(this.root);
    if (info.uid !== process.getuid() || (info.mode & 0o077)) {
      throw new Error('Credential directory must be owned by this user with mode 0700.');
    }
  }

  async read() {
    await this.prepare();
    const file = path.join(this.root, 'accounts.json');
    let handle;
    try {
      handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW);
      const info = await handle.stat();
      if (!info.isFile() || info.uid !== process.getuid() || (info.mode & 0o077)) {
        throw new Error('Credential file must be owned by this user with mode 0600.');
      }
      if (info.size > 1024 * 1024) throw new Error('Credential file exceeds the prototype limit.');
      let data;
      try { data = JSON.parse(await handle.readFile('utf8')); }
      catch { throw new Error('Credential record is not valid JSON. Inspect it locally without printing tokens.'); }
      if (data.version !== 1 || !Array.isArray(data.accounts) || typeof data.host_id !== 'string') {
        throw new Error('Unsupported credential record.');
      }
      return data;
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
      return { version: 1, host_id: `urn:uuid:${randomUUID()}`, active: null, accounts: [] };
    } finally { await handle?.close(); }
  }

  async write(data) {
    const temporary = path.join(this.root, `.accounts-${randomUUID()}.tmp`);
    const handle = await open(temporary, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL, 0o600);
    try {
      await handle.writeFile(`${JSON.stringify(data)}\n`);
      await handle.sync();
      await handle.close();
      await rename(temporary, path.join(this.root, 'accounts.json'));
    } finally {
      await handle.close();
      await unlink(temporary).catch(error => { if (error.code !== 'ENOENT') throw error; });
    }
  }

  async transaction(fn) {
    await this.prepare();
    const lock = path.join(this.root, 'session.lock');
    let handle;
    try { handle = await open(lock, 'wx', 0o600); }
    catch (error) {
      if (error.code === 'EEXIST') throw new Error('Another prototype session holds the credential lock. Inspect it before removing a stale lock.');
      throw error;
    }
    try {
      const data = await this.read();
      // Persist the stable host identity before the first browser authorization.
      await this.write(data);
      return await fn(data, updated => this.write(updated));
    } finally { await handle.close(); await unlink(lock); }
  }
}
