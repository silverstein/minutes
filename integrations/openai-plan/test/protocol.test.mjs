import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { generateKeyPair, SignJWT } from 'jose';
import { ISSUER, RESOURCE, SCOPES, PLAN_SCOPE, newAttempt, authorizationUrl, validateCallback, validateIdentity, credentialRecord, jsonRequest, refresh, revoke, login, signInSuccessPage } from '../oauth.mjs';
import { demoRequest, completedText, models, runDemo } from '../inference.mjs';

const host = 'urn:uuid:00000000-0000-4000-8000-000000000000';
const callback = 'http://127.0.0.1:1455/auth/callback';
const client = 'oaiapp_minutes_test';
const account = { client_id: client, subject: 'test-subject', email: 'sample@example.invalid', access_token: 'test-access', refresh_token: 'test-refresh', scopes: [PLAN_SCOPE], ext_agent_host_id: host };
const tokens = { token_type: 'Bearer', access_token: 'test-access', refresh_token: 'test-refresh', expires_in: 3600, scope: SCOPES };
const stream = events => new Response(events.map(event => `data: ${JSON.stringify(event)}\n\n`).join(''), { headers: { 'Content-Type': 'text/event-stream' } });
const completed = { type: 'response.completed', response: { id: 'resp_test', status: 'completed', output: [{ type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'A cited sample answer.' }] }] } };

test('dynamic registration binds unique state, nonce, PKCE and the exact loopback callback', () => {
  const attempt = newAttempt(host, callback);
  const url = new URL(authorizationUrl(attempt));
  assert.equal(url.origin, ISSUER);
  assert.equal(url.searchParams.get('client_id'), 'dynamic_agent_client');
  assert.equal(url.searchParams.get('agent_name_hint'), 'Minutes');
  assert.equal(url.searchParams.get('redirect_uri'), callback);
  assert.equal(url.searchParams.get('resource'), RESOURCE);
  assert.equal(url.searchParams.get('scope'), SCOPES);
  assert.notEqual(attempt.state, newAttempt(host, callback).state);
  assert.notEqual(attempt.nonce, attempt.verifier);
});

test('returning sign-in keeps its client and account hint', () => {
  const url = new URL(authorizationUrl(newAttempt(host, callback, { ...account, id_token: 'test-hint' })));
  assert.equal(url.searchParams.get('client_id'), client);
  assert.equal(url.searchParams.get('id_token_hint'), 'test-hint');
  assert.equal(url.searchParams.has('agent_name_hint'), false);
});

test('callbacks reject state, duplicate parameters, missing registration and changed clients', () => {
  const attempt = newAttempt(host, callback);
  const url = new URL(callback);
  url.search = new URLSearchParams({ state: attempt.state, code: 'test-code', client_id: client });
  assert.deepEqual(validateCallback(url, attempt), { code: 'test-code', clientId: client });
  url.searchParams.set('state', 'wrong'); assert.throws(() => validateCallback(url, attempt), /state mismatch/);
  url.searchParams.set('state', attempt.state); url.searchParams.append('code', 'second');
  assert.throws(() => validateCallback(url, attempt), /Duplicate/);
  url.searchParams.delete('code'); url.searchParams.set('code', 'test-code'); url.searchParams.delete('client_id');
  assert.throws(() => validateCallback(url, attempt), /issued client/);
  url.searchParams.set('client_id', 'dynamic_agent_client'); assert.throws(() => validateCallback(url, attempt));
  const returning = newAttempt(host, callback, account);
  url.searchParams.set('state', returning.state); url.searchParams.set('client_id', 'oaiapp_other');
  assert.throws(() => validateCallback(url, returning), /changed/);
});

test('expiry and denied consent stop an authorization', () => {
  const attempt = newAttempt(host, callback);
  const url = new URL(`${callback}?state=${attempt.state}&error=access_denied`);
  assert.throws(() => validateCallback(url, attempt), /access_denied/);
  attempt.expiresAt = 0; assert.throws(() => validateCallback(url, attempt), /expired/);
});

test('OIDC validates signature, audience, issuer, expiry, nonce and selected subject', async () => {
  const { publicKey, privateKey } = await generateKeyPair('RS256');
  const attempt = newAttempt(host, callback, account);
  const sign = (changes = {}) => new SignJWT({ nonce: attempt.nonce, ...changes }).setProtectedHeader({ alg: 'RS256' }).setIssuer(ISSUER).setAudience(client).setSubject(account.subject).setIssuedAt().setExpirationTime('1h').sign(privateKey);
  const token = await sign();
  assert.equal((await validateIdentity({ id_token: token }, attempt, client, publicKey)).sub, account.subject);
  await assert.rejects(validateIdentity({ id_token: await sign({ nonce: 'wrong' }) }, attempt, client, publicKey));
  await assert.rejects(validateIdentity({ id_token: token }, attempt, 'oaiapp_other', publicKey));
  await assert.rejects(validateIdentity({ id_token: token }, { ...attempt, account: { ...account, subject: 'other' } }, client, publicKey));
  const other = await generateKeyPair('RS256');
  await assert.rejects(validateIdentity({ id_token: token }, attempt, client, other.publicKey));
  const expired = await new SignJWT({ nonce: attempt.nonce }).setProtectedHeader({ alg: 'RS256' }).setIssuer(ISSUER).setAudience(client).setSubject(account.subject).setIssuedAt().setExpirationTime(1).sign(privateKey);
  await assert.rejects(validateIdentity({ id_token: expired }, attempt, client, publicKey));
});

test('identity without plan consent remains disabled and cannot reach the model endpoint', async () => {
  const record = credentialRecord({ ...tokens, scope: 'openid email' }, { sub: account.subject }, client, host);
  assert.equal(record.scopes.includes(PLAN_SCOPE), false);
  await assert.rejects(models(record, () => { throw new Error('Must not dispatch'); }), /disabled/);
});

test('an ID token with multiple audiences must bind the authorized party', async () => {
  const { publicKey, privateKey } = await generateKeyPair('RS256');
  const attempt = newAttempt(host, callback);
  const sign = azp => new SignJWT({ nonce: attempt.nonce, ...(azp ? { azp } : {}) }).setProtectedHeader({ alg: 'RS256' }).setIssuer(ISSUER).setAudience([client, 'other']).setSubject(account.subject).setIssuedAt().setExpirationTime('1h').sign(privateKey);
  await assert.rejects(validateIdentity({ id_token: await sign() }, attempt, client, publicKey), /authorized party/);
  await assert.rejects(validateIdentity({ id_token: await sign('other') }, attempt, client, publicKey), /authorized party/);
  assert.equal((await validateIdentity({ id_token: await sign(client) }, attempt, client, publicKey)).sub, account.subject);
});

test('cancellation reaches the account catalog and inference transport', async () => {
  const abort = new AbortController(); abort.abort();
  await assert.rejects(models(account, async (url, options) => { options.signal.throwIfAborted(); }, abort.signal), { name: 'AbortError' });
  const inferenceAbort = new AbortController();
  await assert.rejects(runDemo(account, 'selected', 'Question', async (url, options) => {
    if (String(url).endsWith('/models')) { inferenceAbort.abort(); return Response.json({ models: [{ slug: 'selected', display_name: 'Selected', visibility: 'list' }] }); }
    options.signal.throwIfAborted();
  }, inferenceAbort.signal), { name: 'AbortError' });
});

test('invalid token records are not persisted as successful connections', () => {
  for (const patch of [{ token_type: 'Basic' }, { access_token: '' }, { expires_in: 0 }, { scope: undefined }]) {
    assert.throws(() => credentialRecord({ ...tokens, ...patch }, { sub: account.subject }, client, host));
  }
});

test('refresh uses the issued client and rotates the complete token set', async () => {
  const renewed = await refresh(account, async (url, options) => {
    assert.equal(new URL(url).origin, ISSUER);
    assert.equal(options.body.get('client_id'), client);
    assert.equal(options.body.has('scope'), false);
    assert.equal(options.body.get('refresh_token'), account.refresh_token);
    return Response.json({ ...tokens, access_token: 'new-access', refresh_token: 'new-refresh' });
  });
  assert.equal(renewed.access_token, 'new-access'); assert.equal(renewed.refresh_token, 'new-refresh');
  assert.equal(renewed.subject, account.subject);
  await assert.rejects(refresh(account, async () => Response.json(tokens)), /rotating/);
});

test('errors preserve status/code/body shape without exposing provider text or tokens', async () => {
  await assert.rejects(jsonRequest(`${RESOURCE}/models`, {}, async () => Response.json({ error: { code: 'subscription_sharing_user_not_eligible', message: 'private test-access' } }, { status: 403 })), error => error.status === 403 && error.code === 'subscription_sharing_user_not_eligible' && !error.message.includes('private'));
  await assert.rejects(jsonRequest(`${RESOURCE}/models`, {}, async () => Response.json({ detail: 'private test-refresh' }, { status: 503 })), error => error.message.includes('body=detail') && !error.message.includes('test-refresh'));
});

test('revocation uses discovery and refuses token delivery to another origin', async () => {
  let called = false;
  assert.equal(await revoke(account, async (url, options) => {
    if (String(url).endsWith('openid-configuration')) return Response.json({ issuer: ISSUER, revocation_endpoint: `${ISSUER}/api/accounts/oauth/revoke` });
    called = true; assert.equal(options.body.get('token'), account.refresh_token); return new Response(null, { status: 200 });
  }), true);
  assert.equal(called, true);
  await assert.rejects(revoke(account, async () => Response.json({ issuer: ISSUER, revocation_endpoint: 'https://other.example/revoke' })), /Unexpected/);
});

test('catalog choices remain account-specific', async () => {
  const result = await models(account, async () => Response.json({ models: [{ slug: 'visible', display_name: 'Visible', visibility: 'list' }, { slug: 'hidden', display_name: 'Hidden', visibility: 'hide' }] }));
  assert.deepEqual(result, [{ slug: 'visible', display_name: 'Visible' }]);
});

test('demo sends supported parameters and labelled evidence rather than hosted tools', () => {
  const body = demoRequest('selected', 'sample evidence');
  assert.equal(body.store, false); assert.equal(body.stream, true);
  assert.equal(body.tools, undefined); assert.equal(body.previous_response_id, undefined);
  assert.match(body.input[0].content, /public synthetic demo/);
});

test('completed stream returns the terminal answer, even when deltas differ', async () => {
  const result = await completedText(stream([{ type: 'response.output_text.delta', delta: 'unfinished' }, completed]));
  assert.equal(result.text, 'A cited sample answer.'); assert.equal(result.response_id, 'resp_test');
});

const finishedMessage = (index, text) => ({ type: 'response.output_item.done', output_index: index,
  item: { id: `msg_${index}`, type: 'message', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text }] } });
const emptyCompleted = { ...completed, response: { ...completed.response, output: [] } };

test('empty terminal output uses finished assistant messages in output order', async () => {
  const result = await completedText(stream([finishedMessage(1, 'Second.'), finishedMessage(0, 'First.'), emptyCompleted]));
  assert.equal(result.text, 'First.\nSecond.');
  assert.equal(result.response_id, 'resp_test');
  assert.equal((await completedText(stream([finishedMessage(0, 'Earlier.'), completed]))).text, 'A cited sample answer.');
});

test('finished messages do not establish response completion', async () => {
  await assert.rejects(completedText(stream([finishedMessage(0, 'Finished item only.')])), /without response.completed/);
  for (const type of ['response.failed', 'response.incomplete']) {
    await assert.rejects(completedText(stream([finishedMessage(0, 'Discard me.'), { type, response: { error: { code: 'test_failure' } } }, emptyCompleted])), /test_failure/);
  }
  await assert.rejects(completedText(stream([{ type: 'response.output_text.delta', delta: 'Partial.' }, emptyCompleted])), /contained no answer/);
  await assert.rejects(completedText(stream([{ ...emptyCompleted, response: { ...emptyCompleted.response, status: 'incomplete' } }])), /did not confirm completion/);
});

test('finished-message fallback excludes other roles and rejects ambiguous or unfinished items', async () => {
  const message = finishedMessage(0, 'Answer.');
  await assert.rejects(completedText(stream([{ ...message, item: { ...message.item, role: 'user' } }, emptyCompleted])), /contained no answer/);
  for (const invalid of [
    { ...message, output_index: -1 },
    { ...message, item: { ...message.item, status: 'in_progress' } },
    { ...message, item: { ...message.item, id: '' } },
  ]) await assert.rejects(completedText(stream([invalid, emptyCompleted])), /Invalid or duplicate/);
  await assert.rejects(completedText(stream([message, message, emptyCompleted])), /Invalid or duplicate/);
  await assert.rejects(completedText(stream([message, { ...message, output_index: 1 }, emptyCompleted])), /Invalid or duplicate/);
  const toolOnly = { ...completed, response: { ...completed.response, output: [{ type: 'function_call' }] } };
  await assert.rejects(completedText(stream([message, toolOnly])), /contained no answer/);
});

test('a missing MIME header still requires valid SSE and confirmed completion', async () => {
  const noHeader = events => new Response(new ReadableStream({ start(controller) {
    controller.enqueue(new TextEncoder().encode(events.map(event => `data: ${JSON.stringify(event)}\n\n`).join('')));
    controller.close();
  } }));
  assert.equal(noHeader([completed]).headers.get('content-type'), null);
  assert.equal((await completedText(noHeader([completed]))).response_id, 'resp_test');
  await assert.rejects(completedText(noHeader([{ type: 'response.output_text.delta', delta: 'unfinished' }])), /without response.completed/);
  await assert.rejects(completedText(noHeader([{ type: 'response.failed', response: { error: { code: 'subscription_sharing_usage_limit_exceeded' } } }])), /subscription_sharing_usage_limit_exceeded/);
  const wrongHeader = stream([completed]); wrongHeader.headers.set('content-type', 'application/json');
  await assert.rejects(completedText(wrongHeader), /Expected a Responses event stream/);
  const json = new Response(new TextEncoder().encode(JSON.stringify(completed)));
  await assert.rejects(completedText(json), /without response.completed/);
});

test('success page removes callback parameters using only its CSP-authorized script', () => {
  const page = signInSuccessPage();
  const script = page.html.match(/<script>(.*?)<\/script>/)[1];
  assert.equal(script, "history.replaceState(null, '', '/auth/connected');");
  assert.ok(page.csp.includes(`'sha256-${createHash('sha256').update(script).digest('base64')}'`));
  assert.equal(page.contentType, 'text/html; charset=utf-8');
  assert.ok(!page.html.includes('access_token'));
});

test('quota failures after output begins are failures, not completed answers', async () => {
  await assert.rejects(completedText(stream([{ type: 'response.output_text.delta', delta: 'unfinished' }, { type: 'response.failed', response: { error: { code: 'subscription_sharing_usage_limit_exceeded' } } }])), /subscription_sharing_usage_limit_exceeded/);
  await assert.rejects(completedText(stream([{ type: 'response.output_text.delta', delta: 'unfinished' }])), /without response.completed/);
  await assert.rejects(completedText(stream([{ type: 'response.incomplete', response: {} }])), /response.incomplete/);
});

test('SSE parser handles split UTF-8 and CRLF boundaries', async () => {
  const bytes = new TextEncoder().encode(`data: ${JSON.stringify({ ...completed, response: { ...completed.response, output: [{ type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'Décision ✓' }] }] } })}\r\n\r\n`);
  const response = new Response(new ReadableStream({ start(controller) { for (const byte of bytes) controller.enqueue(new Uint8Array([byte])); controller.close(); } }), { headers: { 'Content-Type': 'text/event-stream' } });
  assert.equal((await completedText(response)).text, 'Décision ✓');
});

test('unavailable models are refused before any inference request', async () => {
  let calls = 0;
  await assert.rejects(runDemo(account, 'unavailable', 'Question', async () => { calls++; return Response.json({ models: [] }); }), /not available/);
  assert.equal(calls, 1);
});

test('loopback login binds locally, serves a token-free launch page, and cancels', async () => {
  const abort = new AbortController();
  await assert.rejects(login(host, null, { signal: abort.signal, onReady: async origin => {
    assert.equal(new URL(origin).hostname, '127.0.0.1');
    const response = await fetch(origin);
    assert.match(await response.text(), /Continue with ChatGPT/);
    assert.equal(response.headers.get('referrer-policy'), 'no-referrer');
    const bad = await fetch(`${origin}/auth/callback?state=wrong&code=wrong`);
    assert.equal(bad.status, 400);
    const start = await fetch(`${origin}/start`, { redirect: 'manual' });
    assert.equal(start.status, 302);
    assert.equal(new URL(start.headers.get('location')).origin, ISSUER);
    abort.abort();
  } }), /cancelled/);
});
