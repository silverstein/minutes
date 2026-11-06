import { randomBytes, createHash, timingSafeEqual } from 'node:crypto';
import { createServer } from 'node:http';
import { createRemoteJWKSet, jwtVerify } from 'jose';

export const ISSUER = 'https://auth.openai.com';
export const RESOURCE = 'https://api.openai.com/v1';
export const PLAN_SCOPE = 'chatgpt.tokens.use.direct';
export const SCOPES = 'openid profile email offline_access resource.invoke chatgpt.tokens.use.direct';
const TOKEN_ENDPOINT = `${ISSUER}/api/accounts/oauth/token`;
const keys = createRemoteJWKSet(new URL(`${ISSUER}/.well-known/jwks.json`));
const random = () => randomBytes(32).toString('base64url');
const safeCode = value => typeof value === 'string' && /^[a-zA-Z0-9_.-]{1,100}$/.test(value) ? value : 'unknown';
const CLEAN_CALLBACK_SCRIPT = "history.replaceState(null, '', '/auth/connected');";

export function signInSuccessPage() {
  const hash = createHash('sha256').update(CLEAN_CALLBACK_SCRIPT).digest('base64');
  return {
    contentType: 'text/html; charset=utf-8',
    csp: `default-src 'none'; script-src 'sha256-${hash}'; base-uri 'none'; frame-ancestors 'none'`,
    html: `<!doctype html><html lang="en"><meta charset="utf-8"><title>Minutes connected</title><script>${CLEAN_CALLBACK_SCRIPT}</script><h1>Minutes connected</h1><p>Sign-in verified. Return to the Minutes prototype terminal.</p></html>`,
  };
}

export class ProviderError extends Error {
  constructor(stage, status, code, requestId = null, shape = 'unknown') {
    super(`${stage}: HTTP ${status}; code=${safeCode(code)}; body=${shape}${requestId ? `; request_id=${safeCode(requestId)}` : ''}`);
    this.status = status;
    this.code = safeCode(code);
  }
}

export async function jsonRequest(url, options = {}, fetcher = fetch) {
  const signal = options.signal ? AbortSignal.any([options.signal, AbortSignal.timeout(30_000)]) : AbortSignal.timeout(30_000);
  const response = await fetcher(url, { ...options, redirect: 'error', signal });
  let data;
  try { data = await response.json(); } catch { throw new ProviderError('Provider response', response.status, 'non_json_response'); }
  if (!response.ok) throw new ProviderError('Provider request', response.status, data.error?.code ?? (typeof data.error === 'string' ? data.error : null), response.headers.get('x-request-id'), data.detail ? 'detail' : data.error ? 'error' : 'other');
  return data;
}

export function newAttempt(hostId, redirectUri, account = null) {
  const verifier = random();
  return { hostId, redirectUri, account, verifier, state: random(), nonce: random(), expiresAt: Date.now() + 10 * 60_000 };
}

export function authorizationUrl(attempt) {
  const params = new URLSearchParams({
    client_id: attempt.account?.client_id ?? 'dynamic_agent_client',
    ext_agent_host_id: attempt.hostId,
    response_type: 'code', redirect_uri: attempt.redirectUri, scope: SCOPES, resource: RESOURCE,
    state: attempt.state, nonce: attempt.nonce, code_challenge_method: 'S256',
    code_challenge: createHash('sha256').update(attempt.verifier).digest('base64url'),
  });
  if (!attempt.account) params.set('agent_name_hint', 'Minutes');
  // The hint stays in the redirect, never in terminal output or diagnostics.
  if (attempt.account?.id_token) params.set('id_token_hint', attempt.account.id_token);
  return `${ISSUER}/api/accounts/authorize?${params}`;
}

export function validateCallback(url, attempt) {
  if (Date.now() >= attempt.expiresAt) throw new Error('Authorization expired. Start sign-in again.');
  for (const key of ['state', 'code', 'client_id', 'error']) {
    if (url.searchParams.getAll(key).length > 1) throw new Error('Duplicate authorization callback parameter.');
  }
  const received = Buffer.from(url.searchParams.get('state') ?? '');
  const expected = Buffer.from(attempt.state);
  if (received.length !== expected.length || !timingSafeEqual(received, expected)) throw new Error('Authorization state mismatch.');
  if (url.searchParams.has('error')) throw new ProviderError('Authorization', 400, url.searchParams.get('error'));
  const clientId = url.searchParams.get('client_id') ?? attempt.account?.client_id;
  if (!clientId || clientId === 'dynamic_agent_client' || !/^oaiapp_[A-Za-z0-9_-]+$/.test(clientId)) throw new Error('Registration did not return an issued client ID.');
  if (attempt.account && clientId !== attempt.account.client_id) throw new Error('Authorization changed the selected account registration.');
  const code = url.searchParams.get('code');
  if (!code || code.length > 8192) throw new Error('Missing or invalid authorization code.');
  return { clientId, code };
}

export async function validateIdentity(tokens, attempt, clientId, keySet = keys) {
  const { payload } = await jwtVerify(tokens.id_token, keySet, { issuer: ISSUER, audience: clientId, algorithms: ['RS256'], requiredClaims: ['exp', 'iat', 'sub', 'nonce'] });
  validateAuthorizedParty(payload, clientId);
  if (payload.nonce !== attempt.nonce || typeof payload.sub !== 'string' || !payload.sub) throw new Error('Identity nonce or subject mismatch.');
  if (attempt.account && payload.sub !== attempt.account.subject) throw new Error('Sign-in returned a different account.');
  return payload;
}

function validateAuthorizedParty(payload, clientId) {
  if ((Array.isArray(payload.aud) && payload.aud.length > 1 && !payload.azp) ||
      (payload.azp !== undefined && payload.azp !== clientId)) throw new Error('Identity authorized party mismatch.');
}

export function credentialRecord(tokens, identity, clientId, hostId, previous = null) {
  if (tokens.token_type?.toLowerCase() !== 'bearer' || typeof tokens.access_token !== 'string' || !tokens.access_token || !Number.isFinite(tokens.expires_in) || tokens.expires_in <= 0) throw new Error('Incomplete token response.');
  const scopes = typeof tokens.scope === 'string' ? tokens.scope.split(/\s+/).filter(Boolean) : previous?.scopes;
  if (!Array.isArray(scopes)) throw new Error('Token response omitted granted scopes.');
  if (previous && (!tokens.refresh_token || tokens.refresh_token === previous.refresh_token)) throw new Error('Refresh did not return a rotating refresh token.');
  return { client_id: clientId, ext_agent_host_id: hostId, issuer: ISSUER, subject: identity.sub,
    email: typeof identity.email === 'string' ? identity.email : null,
    access_token: tokens.access_token, refresh_token: tokens.refresh_token ?? null,
    id_token: tokens.id_token ?? previous?.id_token ?? null, scopes,
    expires_at: Date.now() + tokens.expires_in * 1000, earliest_refresh_at: tokens.earliest_refresh_at ?? null };
}

export async function exchange(callback, attempt, fetcher = fetch, signal) {
  const tokens = await jsonRequest(TOKEN_ENDPOINT, { method: 'POST', signal, headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({ grant_type: 'authorization_code', client_id: callback.clientId, code: callback.code, code_verifier: attempt.verifier, redirect_uri: attempt.redirectUri, resource: RESOURCE }) }, fetcher);
  const identity = await validateIdentity(tokens, attempt, callback.clientId);
  return credentialRecord(tokens, identity, callback.clientId, attempt.hostId);
}

export async function refresh(account, fetcher = fetch, signal) {
  if (!account.refresh_token) throw new Error('Sign in again to renew this session.');
  const tokens = await jsonRequest(TOKEN_ENDPOINT, { method: 'POST', signal, headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({ grant_type: 'refresh_token', client_id: account.client_id, refresh_token: account.refresh_token, resource: RESOURCE }) }, fetcher);
  if (tokens.id_token) {
    const { payload } = await jwtVerify(tokens.id_token, keys, { issuer: ISSUER, audience: account.client_id, algorithms: ['RS256'], requiredClaims: ['exp', 'sub'] });
    validateAuthorizedParty(payload, account.client_id);
    if (payload.sub !== account.subject) throw new Error('Refresh changed account identity.');
  }
  return credentialRecord(tokens, { sub: account.subject, email: account.email }, account.client_id, account.ext_agent_host_id, account);
}

export async function revoke(account, fetcher = fetch) {
  if (!account.refresh_token) return false;
  const discovery = await jsonRequest(`${ISSUER}/.well-known/openid-configuration`, {}, fetcher);
  const endpoint = new URL(discovery.revocation_endpoint);
  if (discovery.issuer !== ISSUER || endpoint.origin !== ISSUER || endpoint.username || endpoint.password) throw new Error('Unexpected revocation endpoint.');
  const response = await fetcher(endpoint, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(30_000), headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({ token: account.refresh_token, token_type_hint: 'refresh_token', client_id: account.client_id }) });
  return response.status === 200;
}

export async function login(hostId, account, { onReady = () => {}, signal, port = 0 } = {}) {
  let attempt, consumed = false, finish;
  const result = new Promise((resolve, reject) => { finish = { resolve, reject }; });
  // Attach immediately: cancellation can happen before the caller awaits result.
  result.catch(() => {});
  const server = createServer(async (request, response) => {
    response.setHeader('Cache-Control', 'no-store');
    response.setHeader('Referrer-Policy', 'no-referrer');
    response.setHeader('Content-Security-Policy', "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'");
    if (request.method !== 'GET' || request.headers.host !== new URL(attempt.redirectUri).host) { response.writeHead(400).end('Invalid request.'); return; }
    const url = new URL(request.url, attempt.redirectUri);
    if (url.pathname === '/') {
      response.setHeader('Content-Type', 'text/html; charset=utf-8');
      response.end('<!doctype html><title>Minutes · ChatGPT plan prototype</title><h1>Minutes</h1><p>Use your ChatGPT plan for a sample conversation-memory demo. This connection is stored separately from your Minutes settings. Demo requests use public sample records.</p><p><a href="/start">Continue with ChatGPT</a></p>');
      return;
    }
    if (url.pathname === '/start' && !consumed) { response.writeHead(302, { Location: authorizationUrl(attempt) }).end(); return; }
    if (url.pathname !== '/auth/callback' || consumed) { response.writeHead(404).end('Not found.'); return; }
    let callback;
    try { callback = validateCallback(url, attempt); }
    catch (error) {
      response.writeHead(400).end('Sign-in was declined or the callback was invalid.');
      // Wrong-state requests cannot cancel a valid pending attempt.
      if (error instanceof ProviderError) { consumed = true; finish.reject(error); }
      return;
    }
    consumed = true;
    try {
      const record = await exchange(callback, attempt, fetch, signal);
      const page = signInSuccessPage();
      response.setHeader('Content-Type', page.contentType);
      response.setHeader('Content-Security-Policy', page.csp);
      response.end(page.html);
      finish.resolve(record);
    } catch (error) { response.writeHead(400).end('Sign-in could not be verified. Return to the terminal.'); finish.reject(error); }
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(port, '127.0.0.1', resolve); });
  const origin = `http://127.0.0.1:${server.address().port}`;
  attempt = newAttempt(hostId, `${origin}/auth/callback`, account);
  const cancel = () => finish.reject(new Error('Sign-in cancelled.'));
  signal?.addEventListener('abort', cancel, { once: true });
  const timer = setTimeout(() => finish.reject(new Error('Sign-in expired after ten minutes.')), 10 * 60_000);
  try {
    if (signal?.aborted) cancel();
    await onReady(origin);
    return await result;
  } finally {
    clearTimeout(timer); signal?.removeEventListener('abort', cancel);
    server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  }
}
