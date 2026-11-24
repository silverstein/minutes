(() => {
  const { invoke } = window.__TAURI__.core;
  const find = id => document.getElementById('settings-voice-' + id);
  const section = find('section');
  const toggle = find('enabled');
  const consent = find('consent');
  const consentAccept = find('consent-accept');
  const status = find('status');
  const voice = find('name');
  const key = find('key');
  const save = find('key-save');
  const clear = find('key-clear');
  const keyStatus = find('key-status');
  const test = find('test');
  let settings;
  let secret;
  let busy = false;
  let revision = 0;

  function statusText(message, isError = false) {
    status.textContent = message;
    status.classList.toggle('error', isError);
  }

  function render() {
    if (!settings) return;
    section.hidden = !settings.available;
    toggle.textContent = settings.enabled ? 'On' : 'Off';
    toggle.setAttribute('aria-checked', String(settings.enabled));
    toggle.classList.toggle('btn-primary', settings.enabled);
    toggle.classList.toggle('btn-secondary', !settings.enabled);

    if (!Array.from(voice.options).some(option => option.value === settings.voiceName)) {
      voice.add(new Option(settings.voiceName, settings.voiceName));
    }
    voice.value = settings.voiceName;
    keyStatus.textContent = secret?.message || '';
    toggle.disabled = voice.disabled = busy;
    key.disabled = save.disabled = busy || !secret?.supported;
    clear.disabled = busy || !secret?.supported || !secret.keySet;
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

  save.addEventListener('click', () => {
    const apiKey = key.value.trim();
    key.value = '';
    run(async () => {
      secret = await invoke('cmd_set_voice_api_key', { apiKey });
    });
  });
  clear.addEventListener('click', () => run(async () => {
    secret = await invoke('cmd_clear_voice_api_key');
    key.value = '';
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

  window.MinutesVoiceSettings = { refresh };
})();
