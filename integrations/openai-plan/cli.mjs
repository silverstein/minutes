#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { Store } from './store.mjs';
import { login, refresh, revoke, PLAN_SCOPE } from './oauth.mjs';
import { models, runDemo, DEMO_QUESTION } from './inference.mjs';

const [command = 'help', ...args] = process.argv.slice(2);
const help = `Minutes · ChatGPT plan qualification prototype
  node cli.mjs login [--account <issued-client-id>] [--port <port>]
  node cli.mjs status
  node cli.mjs use <issued-client-id>
  node cli.mjs models
  node cli.mjs demo --model <account-model-slug> [--question <question>]
  node cli.mjs logout [--account <issued-client-id>]

Demo uses two public sample meetings. Login and demo require explicit browser
consent and use your ChatGPT allowance. Manage limits: https://chatgpt.com/settings/usage
Credentials: ~/.config/minutes/openai-plan-prototype (Unix prototype only).
Support: https://github.com/silverstein/minutes/discussions`;

function options(values) {
  const result = {};
  const allowed = { login: ['account', 'port'], demo: ['model', 'question'], logout: ['account'] }[command] ?? [];
  for (let i = 0; i < values.length; i += 2) {
    const name = values[i]?.slice(2);
    if (!values[i]?.startsWith('--') || !allowed.includes(name) || !values[i + 1] || result[name]) throw new Error('Unexpected or incomplete option. Run help.');
    result[name] = values[i + 1];
  }
  return result;
}

async function main() {
  if (command === 'help' || command === '--help') { console.log(help); return; }
  if (!['login', 'status', 'use', 'models', 'demo', 'logout'].includes(command)) throw new Error('Unknown command. Run help.');
  const opts = command === 'use' ? {} : options(args);
  if (command === 'use' && (args.length !== 1 || !args[0].startsWith('oaiapp_'))) throw new Error('use requires one issued client ID.');
  const store = new Store();
  const abort = new AbortController();
  const cancel = () => abort.abort();
  process.once('SIGINT', cancel); process.once('SIGTERM', cancel);
  try {
    await store.transaction(async (data, save) => {
      if (command === 'status') {
        console.log(JSON.stringify({ prototype: true, host_registered: true, active: data.active, accounts: data.accounts.map(account => ({ client_id: account.client_id, email: account.email, signed_in: Boolean(account.access_token), plan_enabled: account.scopes.includes(PLAN_SCOPE), expires_at: account.expires_at })) }, null, 2));
        return;
      }
      if (command === 'login') {
        const existing = opts.account ? data.accounts.find(account => account.client_id === opts.account) : null;
        if (opts.account && !existing) throw new Error('Unknown account registration.');
        const port = opts.port === undefined ? 0 : Number(opts.port);
        if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error('Invalid callback port.');
        const account = await login(data.host_id, existing, { signal: abort.signal, port, onReady: origin => {
          console.log(`Continue with ChatGPT: ${origin}/`);
          console.log('Open this local URL on the same computer, or forward this port over SSH first.');
          const viewer = spawn('hview', [`${origin}/`], { stdio: 'ignore' });
          viewer.on('error', () => {}); viewer.unref();
        } });
        data.accounts = [...data.accounts.filter(item => item.client_id !== account.client_id), account];
        data.active = account.client_id;
        await save(data);
        console.log(`Sign-in verified; ChatGPT plan ${account.scopes.includes(PLAN_SCOPE) ? 'enabled' : 'disabled'}. Run models to inspect this account's choices.`);
        return;
      }
      if (command === 'use') {
        if (!data.accounts.some(account => account.client_id === args[0] && account.access_token)) throw new Error('This registration needs sign-in before selection.');
        data.active = args[0]; await save(data); console.log('Active account updated.'); return;
      }
      let account = data.accounts.find(item => item.client_id === (opts.account ?? data.active));
      if (!account?.access_token) throw new Error('No signed-in account. Run login first.');
      if (command === 'logout') {
        let confirmed = false;
        try { confirmed = await revoke(account); } catch {}
        account.access_token = null; account.refresh_token = null; account.id_token = null;
        account.scopes = []; account.expires_at = null;
        if (data.active === account.client_id) data.active = null;
        await save(data);
        console.log(confirmed ? 'Signed out; renewable session revoked.' : 'Signed out locally; remote revocation was not confirmed. Disconnect Minutes in ChatGPT Settings.');
        return;
      }
      if (!account.scopes.includes(PLAN_SCOPE)) throw new Error('ChatGPT plan use is disabled. Authorize plan usage before inference.');
      if (Date.now() >= account.expires_at - 60_000) {
        try { account = await refresh(account, fetch, abort.signal); }
        catch (error) {
          if (['invalid_grant', 'invalid_refresh_token', 'token_expired', 'refresh_token_expired', 'refresh_token_invalidated', 'refresh_token_reused'].includes(error.code)) {
            const invalid = data.accounts.find(item => item.client_id === account.client_id);
            invalid.access_token = null; invalid.refresh_token = null; invalid.id_token = null;
            await save(data);
          }
          throw error;
        }
        data.accounts = data.accounts.map(item => item.client_id === account.client_id ? account : item);
        await save(data);
      }
      if (command === 'models') { console.log(JSON.stringify(await models(account, fetch, abort.signal), null, 2)); return; }
      if (!opts.model) throw new Error('demo requires --model from this account’s model catalog.');
      console.error('Sending two public sample meeting records to OpenAI using your ChatGPT plan.');
      const result = await runDemo(account, opts.model, opts.question ?? DEMO_QUESTION, fetch, abort.signal);
      console.log(result.text);
      console.error(JSON.stringify({ status: 'completed', model: opts.model, response_id: result.response_id, request_id: result.request_id, sources: ['2026-02-28-pricing-strategy.md', '2026-03-25-pricing-reversal.md'], usage: result.usage }));
    });
  } finally { process.removeListener('SIGINT', cancel); process.removeListener('SIGTERM', cancel); }
}

main().catch(error => { console.error(error instanceof Error ? error.message : 'Prototype failed.'); process.exitCode = 1; });
