(() => {
  const { core, event: events } = window.__TAURI__;
  const { invoke } = core;
  const find = id => document.getElementById('settings-voice-' + id);
  const section = find('section');
  const entry = document.getElementById('btn-voice');
  const meet = find('meet');
  const meetEnable = find('meet-enable');
  const toggle = find('enabled');
  const consent = find('consent');
  const consentAccept = find('consent-accept');
  const status = find('status');
  const voice = find('name');
  const key = find('key');
  const keyMask = find('key-mask');
  const keyEditor = find('key-editor');
  const replace = find('key-replace');
  const cancel = find('key-cancel');
  const save = find('key-save');
  const clear = find('key-clear');
  const keyStatus = find('key-status');
  const test = find('test');
  let settings;
  let secret;
  let replacing = false;
  let voiceActive = false;
  let shortcutEnabled = false;
  let busy = false;
  let revision = 0;

  function statusText(message, isError = false) {
    status.textContent = message;
    status.classList.toggle('error', isError);
  }

  function showEntry() {
    entry.hidden = !(settings?.available && settings.enabled);
    entry.setAttribute('aria-pressed', String(voiceActive));
    entry.title = voiceActive ? 'End Voice' : 'Start Voice';
  }

  function showMeet() {
    const label = window.MinutesShortcuts?.voiceLabel();
    find('meet-shortcut').textContent = label ? (shortcutEnabled ? 'Press ' + label + ' to start.' : 'Shortcut ' + label) : '';
    meetEnable.hidden = shortcutEnabled;
    meetEnable.disabled = busy;
  }

  function renderKey() {
    const edit = secret?.supported && (replacing || !secret.keySet);
    keyEditor.hidden = !edit;
    keyMask.hidden = Boolean(edit);
    keyStatus.textContent = secret?.storedKeySet ? 'Saved in Keychain ••••' : secret?.message || '';
    const canReplace = Boolean(secret?.supported && secret.keySet);
    replace.hidden = !canReplace;
    clear.hidden = !secret?.storedKeySet;
    cancel.hidden = !canReplace;
    key.disabled = save.disabled = !edit || busy;
    replace.disabled = clear.disabled = cancel.disabled = busy;
  }

  function render() {
    if (!settings) return;
    section.hidden = !settings.available;
    toggle.textContent = settings.enabled ? 'ON' : 'OFF';
    toggle.setAttribute('aria-checked', String(settings.enabled));
    showEntry();
    showMeet();
    renderKey();

    if (!Array.from(voice.options).some(option => option.value === settings.voiceName)) {
      voice.add(new Option(settings.voiceName, settings.voiceName));
    }
    voice.value = settings.voiceName;
    toggle.disabled = voice.disabled = busy;
    test.disabled = busy || !settings.enabled || !secret?.keySet;
  }

  async function refresh() {
    if (busy) return;
    const request = ++revision;
    try {
      const next = await invoke('cmd_voice_settings');
      const nextSecret = next.available ? await invoke('cmd_voice_secret_status') : null;
      if (request !== revision) return;
      settings = next;
      secret = nextSecret;
      render();
    } catch (error) {
      if (request === revision) statusText(String(error), true);
    }
  }

  async function run(action) {
    if (busy) return;
    busy = true;
    revision++;
    render();
    statusText('');
    try {
      await action();
    } catch (error) {
      statusText(String(error), true);
    } finally {
      busy = false;
      render();
    }
  }

  async function setEnabled(enabled, accepted) {
    settings = await invoke('cmd_set_voice_enabled', { enabled, consent: accepted });
    if (!enabled) {
      meet.hidden = true;
    } else if (accepted) {
      meet.hidden = false;
      showMeet();
      find('meet-title').focus();
    }
  }

  toggle.addEventListener('click', () => {
    if (!settings || busy) return;
    if (!settings.enabled && settings.needsConsent && settings.provider === 'Google') {
      find('provider').textContent = settings.provider;
      consent.showModal();
      return;
    }
    run(() => setEnabled(!settings.enabled, false));
  });

  consentAccept.addEventListener('click', () => {
    consent.close();
    run(() => setEnabled(true, true));
  });

  replace.addEventListener('click', () => {
    replacing = true;
    renderKey();
    key.focus();
  });
  cancel.addEventListener('click', () => {
    key.value = '';
    replacing = false;
    renderKey();
    replace.focus();
  });

  save.addEventListener('click', () => {
    const apiKey = key.value.trim();
    key.value = '';
    run(async () => {
      secret = await invoke('cmd_set_voice_api_key', { apiKey });
      replacing = false;
    });
  });
  clear.addEventListener('click', () => run(async () => {
    secret = await invoke('cmd_clear_voice_api_key');
    key.value = '';
    replacing = false;
  }));

  voice.addEventListener('change', () => {
    const voiceName = voice.value;
    run(async () => {
      await invoke('cmd_set_voice_name', { voiceName });
      settings.voiceName = voiceName;
      statusText('Applies to the next Voice session.');
    });
  });

  test.addEventListener('click', () => run(async () => {
    const Audio = window.AudioContext || window.webkitAudioContext;
    if (!Audio) throw new Error('Audio playback is unavailable.');
    const context = new Audio();
    try {
      await context.resume();
      statusText('Requesting a sample from Google…');
      const encoded = await invoke('cmd_test_voice');
      const bytes = Uint8Array.from(atob(encoded), character => character.charCodeAt(0));
      const pcm = new DataView(bytes.buffer);
      const sample = context.createBuffer(1, bytes.length / 2, 24000);
      const channel = sample.getChannelData(0);
      for (let i = 0; i < channel.length; i++) {
        channel[i] = pcm.getInt16(i * 2, true) / 32768;
      }
      const source = context.createBufferSource();
      source.buffer = sample;
      source.connect(context.destination);
      statusText('Playing voice sample…');
      await new Promise(resolve => {
        source.onended = resolve;
        source.start();
      });
      statusText('Voice test complete.');
    } finally {
      await context.close();
    }
  }));

  window.addEventListener('minutes:shortcut-changed', ({ detail }) => {
    if (detail?.slot !== 'voice') return;
    shortcutEnabled = detail.enabled;
    showMeet();
  });
  meetEnable.addEventListener('click', () => run(async () => {
    const enabled = await window.MinutesShortcuts.enableVoice();
    if (!enabled) statusText('Could not enable the Voice shortcut. Check the shortcut status below.', true);
  }));
  find('meet-dismiss').addEventListener('click', () => {
    meet.hidden = true;
    toggle.focus();
  });

  entry.addEventListener('click', async () => {
    entry.disabled = true;
    try {
      await invoke('cmd_toggle_desktop_voice');
      voiceActive = (await invoke('cmd_voice_status')).active;
      showEntry();
    } catch (error) {
      window.dispatchEvent(new CustomEvent('minutes:voice-error', { detail: String(error) }));
    } finally {
      entry.disabled = false;
    }
  });

  events.listen('voice:event', ({ payload }) => {
    if (payload.type === 'state' || payload.type === 'closed') {
      voiceActive = payload.type !== 'closed' && payload.state !== 'closed';
      showEntry();
    }
  });
  // Discoverability only. Keychain status is read when Capture Settings opens.
  invoke('cmd_voice_settings').then(initial => {
    if (!settings && !revision) settings = initial;
    showEntry();
  }).catch(() => {});

  window.MinutesVoiceSettings = { refresh };
})();
