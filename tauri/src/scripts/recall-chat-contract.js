(function exposeRecallChatContract(root, factory) {
  const contract = factory();
  if (typeof module === 'object' && module.exports) {
    module.exports = contract;
  } else {
    root.MinutesRecallChatContractV2 = contract;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this, function buildRecallChatContractV2() {
  'use strict';

  const SCHEMA_VERSION = 2;
  const EVENT_KINDS = Object.freeze([
    'status',
    'text',
    'error',
    'done',
    'cancelled',
    'retracted',
  ]);
  const TERMINAL_KINDS = new Set(['error', 'done', 'cancelled', 'retracted']);
  const PROVIDER_PROFILES = new Set([
    'verified_loopback_text',
    'agent_controlled_text',
    'agent_controlled_exact_session_screen',
  ]);
  const TERMINAL_REASONS_BY_KIND = Object.freeze({
    done: new Set(['completed']),
    cancelled: new Set(['user_cancelled']),
    retracted: new Set([
      'superseded',
      'provider_changed',
      'focus_changed',
      'source_policy_changed',
      'meeting_ended',
      'lifecycle_changed',
    ]),
    error: new Set(['provider_failed', 'internal_failure']),
  });
  const VISIBLE_LIFECYCLE_STATES = Object.freeze([
    'queued',
    'attaching',
    'ready',
    'preparing',
    'grounding',
    'answering',
    'cancelling',
    'meeting_ended',
    'processing',
    'finalized',
    'restarting',
    'degraded',
    'completed',
    'cancelled',
    'retracted',
    'failed',
  ]);
  const STATUS_STATES = new Set(VISIBLE_LIFECYCLE_STATES.slice(1, 12));
  const STATUS_TRANSITIONS = Object.freeze({
    queued: new Set([
      'attaching', 'ready', 'preparing', 'grounding', 'answering', 'cancelling',
      'meeting_ended', 'processing', 'finalized', 'restarting', 'degraded',
    ]),
    attaching: new Set([
      'attaching', 'ready', 'preparing', 'grounding', 'cancelling',
      'meeting_ended', 'processing', 'finalized', 'restarting', 'degraded',
    ]),
    ready: new Set([
      'ready', 'preparing', 'grounding', 'answering', 'cancelling',
      'meeting_ended', 'processing', 'finalized', 'restarting', 'degraded',
    ]),
    preparing: new Set([
      'preparing', 'grounding', 'answering', 'cancelling',
      'meeting_ended', 'processing', 'finalized', 'restarting', 'degraded',
    ]),
    grounding: new Set([
      'grounding', 'answering', 'cancelling',
      'meeting_ended', 'processing', 'finalized', 'restarting', 'degraded',
    ]),
    answering: new Set([
      'answering', 'cancelling', 'meeting_ended', 'processing', 'finalized',
      'restarting', 'degraded',
    ]),
    cancelling: new Set(['cancelling']),
    meeting_ended: new Set([
      'meeting_ended', 'processing', 'finalized', 'cancelling', 'restarting', 'degraded',
    ]),
    processing: new Set(['processing', 'finalized', 'cancelling', 'restarting', 'degraded']),
    // A finalized-source status can precede grounding for a historical turn.
    finalized: new Set([
      'finalized', 'attaching', 'ready', 'grounding', 'answering',
      'cancelling', 'restarting', 'degraded',
    ]),
    restarting: new Set([
      'restarting', 'attaching', 'ready', 'preparing', 'cancelling', 'degraded',
    ]),
    degraded: new Set([
      'degraded', 'restarting', 'attaching', 'ready', 'preparing',
      'grounding', 'answering', 'cancelling', 'meeting_ended', 'processing', 'finalized',
    ]),
  });

  const turnStates = new WeakMap();

  function isObject(value) {
    return value !== null && typeof value === 'object' && !Array.isArray(value);
  }

  function exactDataValues(value, expectedKeys) {
    try {
      if (!isObject(value)) return null;
      const prototype = Object.getPrototypeOf(value);
      if (prototype !== Object.prototype && prototype !== null) return null;
      const ownKeys = Reflect.ownKeys(value);
      if (ownKeys.length !== expectedKeys.length
        || ownKeys.some((key) => typeof key !== 'string' || !expectedKeys.includes(key))) {
        return null;
      }
      const copy = Object.create(null);
      for (const key of expectedKeys) {
        const descriptor = Object.getOwnPropertyDescriptor(value, key);
        if (!descriptor || !descriptor.enumerable || !hasOwnDataValue(descriptor)) return null;
        copy[key] = descriptor.value;
      }
      return copy;
    } catch (_) {
      return null;
    }
  }

  function hasOwnDataValue(descriptor) {
    return Object.prototype.hasOwnProperty.call(descriptor, 'value');
  }

  function isOpaqueId(value) {
    return typeof value === 'string'
      && /^[\x21-\x7e]{1,256}$/u.test(value);
  }

  function isClientRequestId(value) {
    return typeof value === 'string'
      && value.length >= 8
      && value.length <= 128
      && /^[A-Za-z0-9]+(?:-[A-Za-z0-9]+)*$/u.test(value);
  }

  function isCounter(value, minimum) {
    return Number.isSafeInteger(value) && value >= minimum;
  }

  function freezeProvider(provider) {
    const values = exactDataValues(provider, [
      'bindingId', 'generation', 'attestationId', 'profile',
    ]);
    if (!values
      || !isOpaqueId(values.bindingId)
      || !isCounter(values.generation, 1)
      || !isOpaqueId(values.attestationId)
      || !PROVIDER_PROFILES.has(values.profile)) {
      return null;
    }
    return Object.freeze({
      bindingId: values.bindingId,
      generation: values.generation,
      attestationId: values.attestationId,
      profile: values.profile,
    });
  }

  function sameProvider(left, right) {
    return left.bindingId === right.bindingId
      && left.generation === right.generation
      && left.attestationId === right.attestationId
      && left.profile === right.profile;
  }

  function normalizeAuthority(value) {
    const values = exactDataValues(value, [
      'schemaVersion', 'processEpoch', 'sourceBindingId', 'assistanceSessionId',
      'foregroundTurnId', 'invocation', 'focusGeneration', 'provider', 'clientRequestId',
    ]);
    if (!values || values.schemaVersion !== SCHEMA_VERSION) return null;
    const provider = freezeProvider(values.provider);
    const invocation = exactDataValues(values.invocation, [
      'sequence', 'sourcePolicyGeneration', 'userGeneration',
    ]);
    if (!isClientRequestId(values.clientRequestId)
      || !isOpaqueId(values.processEpoch)
      || !isOpaqueId(values.sourceBindingId)
      || !isOpaqueId(values.assistanceSessionId)
      || !isOpaqueId(values.foregroundTurnId)
      || !isCounter(values.focusGeneration, 1)
      || !provider
      || !invocation
      || !isCounter(invocation.sequence, 1)
      || !isCounter(invocation.sourcePolicyGeneration, 0)
      || !isCounter(invocation.userGeneration, 0)) {
      return null;
    }
    return Object.freeze({
      schemaVersion: SCHEMA_VERSION,
      processEpoch: values.processEpoch,
      sourceBindingId: values.sourceBindingId,
      assistanceSessionId: values.assistanceSessionId,
      foregroundTurnId: values.foregroundTurnId,
      invocation: Object.freeze({
        sequence: invocation.sequence,
        sourcePolicyGeneration: invocation.sourcePolicyGeneration,
        userGeneration: invocation.userGeneration,
      }),
      focusGeneration: values.focusGeneration,
      provider,
      clientRequestId: values.clientRequestId,
    });
  }

  function sameAuthority(left, right) {
    return left.schemaVersion === right.schemaVersion
      && left.processEpoch === right.processEpoch
      && left.sourceBindingId === right.sourceBindingId
      && left.assistanceSessionId === right.assistanceSessionId
      && left.foregroundTurnId === right.foregroundTurnId
      && left.invocation.sequence === right.invocation.sequence
      && left.invocation.sourcePolicyGeneration === right.invocation.sourcePolicyGeneration
      && left.invocation.userGeneration === right.invocation.userGeneration
      && left.focusGeneration === right.focusGeneration
      && sameProvider(left.provider, right.provider)
      && left.clientRequestId === right.clientRequestId;
  }

  function terminalReason(eventKind, payload) {
    if (!TERMINAL_KINDS.has(eventKind)) return null;
    const values = exactDataValues(payload, ['reason']);
    if (!values || typeof values.reason !== 'string') return null;
    const allowed = TERMINAL_REASONS_BY_KIND[eventKind];
    return allowed.has(values.reason) ? values.reason : null;
  }

  function normalizeEnvelope(value) {
    const values = exactDataValues(value, [
      'authority', 'eventSequence', 'eventKind', 'payload',
    ]);
    if (!values) return null;
    const authority = normalizeAuthority(values.authority);
    if (!authority
      || !isCounter(values.eventSequence, 1)
      || !EVENT_KINDS.includes(values.eventKind)) {
      return null;
    }

    let payload;
    if (values.eventKind === 'status') {
      const status = exactDataValues(values.payload, ['state']);
      if (!status || !STATUS_STATES.has(status.state)) return null;
      payload = Object.freeze({ state: status.state });
    } else if (values.eventKind === 'text') {
      if (typeof values.payload !== 'string' || values.payload.length === 0) return null;
      payload = values.payload;
    } else {
      const reason = terminalReason(values.eventKind, values.payload);
      if (!reason) return null;
      payload = Object.freeze({ reason });
    }

    return Object.freeze({
      authority,
      eventSequence: values.eventSequence,
      eventKind: values.eventKind,
      payload,
    });
  }

  function nextLifecycleState(current, envelope) {
    if (envelope.eventKind === 'status') {
      return STATUS_TRANSITIONS[current]?.has(envelope.payload.state)
        ? envelope.payload.state
        : null;
    }
    if (envelope.eventKind === 'text') {
      return STATUS_TRANSITIONS[current]?.has('answering') ? 'answering' : null;
    }
    if (envelope.eventKind === 'done') return 'completed';
    if (envelope.eventKind === 'cancelled') return 'cancelled';
    if (envelope.eventKind === 'retracted') return 'retracted';
    return 'failed';
  }

  function snapshotFor(state) {
    return Object.freeze({
      clientRequestId: state.authority.clientRequestId,
      authority: state.authority,
      lastEventSequence: state.lastEventSequence,
      lifecycleState: state.lifecycleState,
      terminal: state.terminal,
      terminalKind: state.terminalKind,
      terminalReason: state.terminalReason,
    });
  }

  /**
   * Create an opaque frontend turn handle from a reducer-issued invocation.
   * The future command adapter must return this authority before it emits stream
   * events, preventing the first event from choosing its own turn identity.
   * This models state only; it does not imply that Native Recall's UI is wired.
   */
  function createTurnRecord(expectedAuthority) {
    const authority = normalizeAuthority(expectedAuthority);
    if (!authority) return null;
    const handle = Object.freeze({});
    turnStates.set(handle, {
      authority,
      lastEventSequence: 0,
      lifecycleState: 'queued',
      terminal: false,
      terminalKind: null,
      terminalReason: null,
    });
    return handle;
  }

  function getTurnSnapshot(handle) {
    const state = turnStates.get(handle);
    return state ? snapshotFor(state) : null;
  }

  /**
   * Accept one reducer-authorized event. Rejection is atomic and returns null.
   */
  function acceptEnvelope(handle, envelope) {
    const state = turnStates.get(handle);
    if (!state || state.terminal) return null;
    const normalized = normalizeEnvelope(envelope);
    if (!normalized
      || !sameAuthority(state.authority, normalized.authority)
      || normalized.eventSequence !== state.lastEventSequence + 1) {
      return null;
    }

    const lifecycleState = nextLifecycleState(state.lifecycleState, normalized);
    if (!lifecycleState) return null;

    state.lastEventSequence = normalized.eventSequence;
    state.lifecycleState = lifecycleState;
    if (TERMINAL_KINDS.has(normalized.eventKind)) {
      state.terminal = true;
      state.terminalKind = normalized.eventKind;
      state.terminalReason = normalized.payload.reason;
    }
    return snapshotFor(state);
  }

  return Object.freeze({
    SCHEMA_VERSION,
    EVENT_KINDS,
    VISIBLE_LIFECYCLE_STATES,
    createTurnRecord,
    getTurnSnapshot,
    acceptEnvelope,
  });
});
