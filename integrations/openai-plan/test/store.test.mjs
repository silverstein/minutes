import test from 'node:test';
import assert from 'node:assert/strict';
import { chmod, mkdtemp, readFile, realpath, rm, stat, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { Store } from '../store.mjs';

async function fixture(t) {
  // macOS temp paths contain /var -> /private/var. Use a physical test root
  // while continuing to test the store's rejection of symlink ancestors.
  const root = await mkdtemp(path.join(await realpath(tmpdir()), 'minutes-plan-store-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}

test('host identity survives restarts and credentials have owner-only permissions', async t => {
  const root = await fixture(t); const store = new Store(root);
  let host;
  await store.transaction(async (data, save) => { host = data.host_id; data.accounts = [{ client_id: 'oaiapp_sample', access_token: 'sample-secret' }]; await save(data); });
  assert.equal((await new Store(root).read()).host_id, host);
  assert.equal((await stat(path.join(root, 'accounts.json'))).mode & 0o777, 0o600);
  assert.equal((await stat(root)).mode & 0o777, 0o700);
});

test('concurrent processes cannot race a rotating refresh token', async t => {
  const root = await fixture(t); const store = new Store(root);
  await store.transaction(async () => {
    await assert.rejects(new Store(root).transaction(async () => {}), /credential lock/);
  });
  await store.transaction(async () => {});
});

test('symlink credentials and symlink ancestor directories are refused', async t => {
  const root = await fixture(t); const outside = path.join(root, 'outside');
  await writeFile(outside, 'secret', { mode: 0o600 });
  await symlink(outside, path.join(root, 'accounts.json'));
  await assert.rejects(new Store(root).read());
  assert.equal(await readFile(outside, 'utf8'), 'secret');
  await symlink(root, path.join(root, 'linked'));
  await assert.rejects(new Store(path.join(root, 'linked/store')).prepare(), /symlinks/);
});

test('insecure directory and credential permissions are refused', async t => {
  const root = await fixture(t); await chmod(root, 0o755);
  await assert.rejects(new Store(root).read(), /0700/);
  await chmod(root, 0o700);
  const store = new Store(root); await store.transaction(async () => {});
  await chmod(path.join(root, 'accounts.json'), 0o644);
  await assert.rejects(store.read(), /0600/);
});

test('malformed credentials do not expose token snippets in diagnostics', async t => {
  const root = await fixture(t);
  await writeFile(path.join(root, 'accounts.json'), '{ "access_token": sample-secret-token', { mode: 0o600 });
  await assert.rejects(new Store(root).read(), error => error.message.includes('not valid JSON') && !error.message.includes('sample-secret-token'));
});
