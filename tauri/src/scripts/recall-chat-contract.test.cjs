'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const {
  SCHEMA_VERSION,
  EVENT_KINDS,
  VISIBLE_LIFECYCLE_STATES,
  createTurnRecord,
  getTurnSnapshot,
  acceptEnvelope,
} = require('./recall-chat-contract.js');
const RUST_ENVELOPE_GOLDEN = require('./fixtures/recall-envelope-v2.json');

const BASE_PROVIDER = Object.freeze({
  bindingId: 'provider-binding-1',
  generation: 3,
  attestationId: 'provider-attestation-1',
  profile: 'agent_controlled_text',
});

const BASE_EXPECTED = Object.freeze({
  schemaVersion: SCHEMA_VERSION,
  clientRequestId: 'client-request-1',
  processEpoch: 'process-epoch-1',
  sourceBindingId: 'source-binding-1',
  assistanceSessionId: 'assistance-session-1',
  foregroundTurnId: 'foreground-turn-1',
  invocation: Object.freeze({
    sequence: 11,
    sourcePolicyGeneration: 5,
    userGeneration: 2,
  }),
  focusGeneration: 7,
  provider: BASE_PROVIDER,
});

function authority(overrides = {}) {
  return {
    schemaVersion: SCHEMA_VERSION,
    processEpoch: BASE_EXPECTED.processEpoch,
    sourceBindingId: BASE_EXPECTED.sourceBindingId,
    assistanceSessionId: BASE_EXPECTED.assistanceSessionId,
    foregroundTurnId: BASE_EXPECTED.foregroundTurnId,
    invocation: {
      ...BASE_EXPECTED.invocation,
      ...(overrides.invocation || {}),
    },
    focusGeneration: BASE_EXPECTED.focusGeneration,
    provider: {
      ...BASE_PROVIDER,
      ...(overrides.provider || {}),
    },
    clientRequestId: BASE_EXPECTED.clientRequestId,
    ...Object.fromEntries(
      Object.entries(overrides).filter(([key]) => !['invocation', 'provider'].includes(key)),
    ),
  };
}

function envelope(eventSequence, eventKind, payload, authorityOverrides = {}) {
  return {
    authority: authority(authorityOverrides),
    eventSequence,
    eventKind,
    payload,
  };
}

function record(expectedOverrides = {}) {
  const expected = authority(expectedOverrides);
  const handle = createTurnRecord(expected);
  assert.ok(handle, 'test fixture must create a valid record');
  return handle;
}

test('exports the isolated v2 event and visible-lifecycle vocabulary', () => {
  assert.equal(SCHEMA_VERSION, 2);
  assert.deepEqual(EVENT_KINDS, [
    'status', 'text', 'error', 'done', 'cancelled', 'retracted',
  ]);
  assert.deepEqual(VISIBLE_LIFECYCLE_STATES, [
    'queued', 'attaching', 'ready', 'preparing', 'grounding', 'answering',
    'cancelling', 'meeting_ended', 'processing', 'finalized', 'restarting',
    'degraded', 'completed', 'cancelled', 'retracted', 'failed',
  ]);
});

test('accepts the exact full envelope golden serialized by Rust', () => {
  const handle = createTurnRecord(RUST_ENVELOPE_GOLDEN.authority);
  assert.ok(handle);
  const result = acceptEnvelope(handle, RUST_ENVELOPE_GOLDEN);
  assert.equal(result.lifecycleState, 'retracted');
  assert.equal(result.terminalReason, 'focus_changed');
  assert.equal(result.lastEventSequence, 1);
});

test('models attachment, readiness, restart, and recoverable degradation explicitly', () => {
  const handle = record();
  const states = [
    'attaching',
    'ready',
    'preparing',
    'degraded',
    'restarting',
    'attaching',
    'ready',
    'grounding',
  ];
  states.forEach((state, index) => {
    const result = acceptEnvelope(handle, envelope(index + 1, 'status', { state }));
    assert.equal(result.lifecycleState, state);
    assert.equal(result.terminal, false);
  });
  assert.equal(acceptEnvelope(handle, envelope(9, 'text', 'Recovered answer')).lifecycleState, 'answering');
  assert.equal(
    acceptEnvelope(handle, envelope(10, 'done', { reason: 'completed' })).lifecycleState,
    'completed',
  );
});

test('provider failure is terminal failed, distinct from recoverable degraded', () => {
  const handle = record();
  assert.equal(
    acceptEnvelope(handle, envelope(1, 'status', { state: 'degraded' })).lifecycleState,
    'degraded',
  );
  const failed = acceptEnvelope(handle, envelope(2, 'error', { reason: 'provider_failed' }));
  assert.equal(failed.lifecycleState, 'failed');
  assert.equal(failed.terminal, true);
  assert.equal(failed.terminalReason, 'provider_failed');
  assert.equal(acceptEnvelope(handle, envelope(3, 'status', { state: 'restarting' })), null);
});

test('accepts a contiguous foreground lifecycle and exactly one completion', () => {
  const handle = record();
  assert.equal(getTurnSnapshot(handle).lifecycleState, 'queued');
  assert.equal(acceptEnvelope(handle, envelope(1, 'status', { state: 'preparing' })).lifecycleState, 'preparing');
  assert.equal(acceptEnvelope(handle, envelope(2, 'status', { state: 'grounding' })).lifecycleState, 'grounding');
  assert.equal(acceptEnvelope(handle, envelope(3, 'text', 'Answer')).lifecycleState, 'answering');
  const completed = acceptEnvelope(handle, envelope(4, 'done', { reason: 'completed' }));
  assert.equal(completed.lifecycleState, 'completed');
  assert.equal(completed.terminal, true);
  assert.equal(completed.terminalKind, 'done');
  assert.equal(completed.terminalReason, 'completed');
  assert.equal(acceptEnvelope(handle, envelope(5, 'done', { reason: 'completed' })), null);
  assert.equal(getTurnSnapshot(handle).lastEventSequence, 4);
});

test('malformed authority and malformed records fail closed without mutation', () => {
  const invalidRecords = [
    null,
    authority({ clientRequestId: 'short' }),
    authority({ processEpoch: 'has whitespace' }),
    authority({ foregroundTurnId: '' }),
    authority({ invocation: { sequence: 0 } }),
    authority({ focusGeneration: 0 }),
    authority({ provider: { profile: 'unavailable' } }),
  ];
  for (const value of invalidRecords) assert.equal(createTurnRecord(value), null);

  const malformedAuthorities = [
    { schemaVersion: 1 },
    authority({ schemaVersion: 3 }),
    authority({ processEpoch: '' }),
    authority({ foregroundTurnId: '' }),
    authority({ invocation: { sequence: 0 } }),
    authority({ invocation: { sourcePolicyGeneration: -1 } }),
    authority({ invocation: { userGeneration: Number.MAX_SAFE_INTEGER + 1 } }),
    authority({ provider: { generation: 0 } }),
    authority({ provider: { profile: 'unavailable' } }),
  ];
  for (const malformed of malformedAuthorities) {
    const handle = record();
    const before = getTurnSnapshot(handle);
    assert.equal(acceptEnvelope(handle, {
      authority: malformed,
      eventSequence: 1,
      eventKind: 'status',
      payload: { state: 'preparing' },
    }), null);
    assert.deepEqual(getTurnSnapshot(handle), before);
  }
});

test('accessor-backed and extended records cannot change truth between validation and commit', () => {
  let getterCalls = 0;
  const changingTerminal = {};
  Object.defineProperty(changingTerminal, 'reason', {
    enumerable: true,
    get() {
      getterCalls += 1;
      return getterCalls === 1 ? 'completed' : 'invented';
    },
  });
  const terminalHandle = record();
  assert.equal(acceptEnvelope(terminalHandle, envelope(1, 'done', changingTerminal)), null);
  assert.equal(getterCalls, 0);
  assert.equal(getTurnSnapshot(terminalHandle).terminal, false);

  const changingAuthority = authority();
  Object.defineProperty(changingAuthority, 'processEpoch', {
    enumerable: true,
    get() {
      getterCalls += 1;
      return BASE_EXPECTED.processEpoch;
    },
  });
  assert.equal(createTurnRecord(changingAuthority), null);
  assert.equal(getterCalls, 0);

  const extendedRecords = [
    { ...envelope(1, 'status', { state: 'preparing' }), extra: true },
    envelope(1, 'status', { state: 'preparing', extra: true }),
    envelope(1, 'done', { reason: 'completed', extra: true }),
    envelope(1, 'status', { state: 'preparing' }, { extra: true }),
    envelope(1, 'status', { state: 'preparing' }, {
      invocation: { extra: true },
    }),
    envelope(1, 'status', { state: 'preparing' }, {
      provider: { extra: true },
    }),
  ];
  for (const value of extendedRecords) {
    const handle = record();
    assert.equal(acceptEnvelope(handle, value), null);
    assert.equal(getTurnSnapshot(handle).lastEventSequence, 0);
  }

  assert.equal(createTurnRecord(authority({ processEpoch: 'process\u200bepoch' })), null);
});

test('wrong process, source, focus, session, turn, or client request fails on the first event', () => {
  const cases = [
    { processEpoch: 'wrong-process' },
    { sourceBindingId: 'wrong-source' },
    { focusGeneration: 8 },
    { assistanceSessionId: 'wrong-session' },
    { foregroundTurnId: 'wrong-turn' },
    { invocation: { sequence: 12 } },
    { clientRequestId: 'client-request-2' },
  ];
  for (const changed of cases) {
    const handle = record();
    assert.equal(
      acceptEnvelope(handle, envelope(1, 'status', { state: 'preparing' }, changed)),
      null,
    );
    assert.equal(getTurnSnapshot(handle).lastEventSequence, 0);
  }
});

test('every provider binding and attestation field is immutable authority', () => {
  const cases = [
    { bindingId: 'provider-binding-2' },
    { generation: 4 },
    { attestationId: 'provider-attestation-2' },
    { profile: 'verified_loopback_text' },
  ];
  for (const provider of cases) {
    const handle = record();
    assert.equal(
      acceptEnvelope(handle, envelope(1, 'status', { state: 'preparing' }, { provider })),
      null,
    );
  }
});

test('every event remains bound to the exact reducer invocation identity', () => {
  const handle = record();
  assert.ok(acceptEnvelope(handle, envelope(1, 'status', { state: 'preparing' })));
  const wrongAuthorities = [
    { foregroundTurnId: 'foreground-turn-2' },
    { invocation: { sequence: 12 } },
    { invocation: { sourcePolicyGeneration: 6 } },
    { invocation: { userGeneration: 3 } },
  ];
  for (const changed of wrongAuthorities) {
    assert.equal(
      acceptEnvelope(handle, envelope(2, 'text', 'stale', changed)),
      null,
    );
    assert.equal(getTurnSnapshot(handle).lastEventSequence, 1);
  }
  assert.ok(acceptEnvelope(handle, envelope(2, 'text', 'current')));
});

test('duplicate, gap, reordered, unsafe, and zero event sequences fail atomically', () => {
  const handle = record();
  assert.ok(acceptEnvelope(handle, envelope(1, 'status', { state: 'preparing' })));
  for (const sequence of [1, 3, 0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
    const before = getTurnSnapshot(handle);
    assert.equal(acceptEnvelope(handle, envelope(sequence, 'text', 'not accepted')), null);
    assert.deepEqual(getTurnSnapshot(handle), before);
  }
  assert.ok(acceptEnvelope(handle, envelope(2, 'text', 'accepted')));
  assert.equal(acceptEnvelope(handle, envelope(1, 'text', 'reordered')), null);
  assert.equal(getTurnSnapshot(handle).lastEventSequence, 2);
});

test('unknown kinds, missing payloads, empty text, and invalid status transitions fail closed', () => {
  const handle = record();
  assert.equal(acceptEnvelope(handle, {
    authority: authority(), eventSequence: 1, eventKind: 'chunk', payload: 'x',
  }), null);
  assert.equal(acceptEnvelope(handle, {
    authority: authority(), eventSequence: 1, eventKind: 'status',
  }), null);
  assert.equal(acceptEnvelope(handle, envelope(1, 'text', '')), null);
  assert.equal(acceptEnvelope(handle, envelope(1, 'status', { state: 'invented' })), null);
  assert.ok(acceptEnvelope(handle, envelope(1, 'status', { state: 'cancelling' })));
  assert.equal(acceptEnvelope(handle, envelope(2, 'text', 'late provider text')), null);
  assert.equal(getTurnSnapshot(handle).lifecycleState, 'cancelling');
});

test('terminal event kind and reason combinations are exact', () => {
  const invalid = [
    ['done', 'user_cancelled'],
    ['cancelled', 'completed'],
    ['retracted', 'provider_failed'],
    ['error', 'focus_changed'],
    ['done', 'invented'],
  ];
  for (const [kind, reason] of invalid) {
    const handle = record();
    assert.equal(acceptEnvelope(handle, envelope(1, kind, { reason })), null);
    assert.equal(getTurnSnapshot(handle).terminal, false);
  }
});

test('cancel acknowledgement is terminal and rejects late provider output or completion', () => {
  const handle = record();
  assert.ok(acceptEnvelope(handle, envelope(1, 'status', { state: 'cancelling' })));
  const cancelled = acceptEnvelope(handle, envelope(2, 'cancelled', { reason: 'user_cancelled' }));
  assert.equal(cancelled.lifecycleState, 'cancelled');
  assert.equal(acceptEnvelope(handle, envelope(3, 'text', 'late text')), null);
  assert.equal(acceptEnvelope(handle, envelope(3, 'done', { reason: 'completed' })), null);
  assert.equal(getTurnSnapshot(handle).lastEventSequence, 2);
});

test('retraction reasons model source, focus, provider, supersession, and lifecycle invalidation', () => {
  const reasons = [
    'superseded',
    'provider_changed',
    'focus_changed',
    'source_policy_changed',
    'meeting_ended',
    'lifecycle_changed',
  ];
  for (const reason of reasons) {
    const handle = record();
    const result = acceptEnvelope(handle, envelope(1, 'retracted', { reason }));
    assert.equal(result.lifecycleState, 'retracted');
    assert.equal(result.terminalReason, reason);
  }
});

test('stale old-turn cancel and completion cannot affect a newly bound turn', () => {
  const currentAuthority = {
    foregroundTurnId: 'foreground-turn-2',
    invocation: { sequence: 12, userGeneration: 3 },
  };
  const current = record(currentAuthority);
  assert.ok(acceptEnvelope(current, envelope(1, 'status', { state: 'preparing' }, {
    ...currentAuthority,
  })));

  assert.equal(
    acceptEnvelope(current, envelope(2, 'cancelled', { reason: 'user_cancelled' })),
    null,
  );
  assert.equal(
    acceptEnvelope(current, envelope(2, 'done', { reason: 'completed' })),
    null,
  );
  assert.equal(getTurnSnapshot(current).lastEventSequence, 1);
  assert.ok(acceptEnvelope(current, envelope(2, 'text', 'new turn', {
    ...currentAuthority,
  })));

  const differentClient = record({ clientRequestId: 'client-request-2' });
  assert.equal(
    acceptEnvelope(differentClient, envelope(1, 'done', { reason: 'completed' })),
    null,
  );
});

test('accepted authority is copied so later envelope mutation cannot rewrite record truth', () => {
  const handle = record();
  const first = envelope(1, 'status', { state: 'preparing' });
  const accepted = acceptEnvelope(handle, first);
  first.authority.processEpoch = 'mutated-process';
  first.authority.provider.bindingId = 'mutated-provider';
  assert.equal(accepted.authority.processEpoch, BASE_EXPECTED.processEpoch);
  assert.equal(accepted.authority.provider.bindingId, BASE_PROVIDER.bindingId);
  assert.equal(getTurnSnapshot(handle).authority.processEpoch, BASE_EXPECTED.processEpoch);
});
