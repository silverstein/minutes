/* Readiness is a local snapshot, never proof of delivery into another app. */
(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
  let revision = 0;
  let last = null;
  let installing = null;
  let downloadText = '';
  let snapshotError = '';
  let checking = false;
  let expanded = true;
  try { expanded = localStorage.getItem('minutes.dictationSetupExpanded') !== 'false'; } catch (_) {}
  const visible = () => $('settings-overlay').classList.contains('active') && $('panel-dictation').classList.contains('is-active');
  const deadline = (promise, label) => new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(label + ' is taking longer than expected. Try Check again.')), 6000);
    promise.then(value => { clearTimeout(timer); resolve(value); }, error => { clearTimeout(timer); reject(error); });
  });
  function button(label, run) {
    const control = document.createElement('button'); control.type = 'button';
    control.className = 'btn btn-secondary btn-sm'; control.textContent = label;
    control.addEventListener('click', run); return control;
  }
  function line(label, state, detail, controls = []) {
    const root = document.createElement('div'); root.className = 'dictation-ready-row'; root.dataset.state = state;
    const copy = document.createElement('div'); copy.className = 'dictation-ready-copy';
    const name = document.createElement('strong'); name.textContent = label;
    const text = document.createElement('p'); text.className = 'dictation-note'; text.textContent = detail;
    copy.append(name, text); root.append(copy);
    const actions = document.createElement('div'); actions.className = 'dictation-actions';
    for (const control of controls) control.setAttribute('aria-label', control.textContent + ': ' + label);
    actions.append(...controls); root.append(actions); return root;
  }
  function permissionHelp() { window.openDictationPermissionHelp?.(); }
  function focusShortcut() {
    window.MinutesDictation?.show('writing'); $('shortcut-recorder-dictation').focus();
    $('shortcut-recorder-dictation').scrollIntoView({ block: 'center' });
  }
  function modelSettings() { $('tab-transcription').click(); }
  async function installModel(name) {
    if (installing) return;
    installing = name; downloadText = 'Downloading ' + name + '…'; snapshotError = ''; render();
    try { await invoke('cmd_download_model', { model: name }); }
    catch (error) { snapshotError = 'Could not install ' + name + '. ' + String(error); }
    finally { installing = null; downloadText = ''; await refresh(); }
  }
  function render() {
    if (!last || checking) return;
    const rows = $('dictation-ready-rows'); rows.replaceChildren();
    const { prefs, shortcut, devices, permissions, errors } = last;
    const issues = [];
    const recording = shortcut && shortcut.enabled && !shortcut.pending && !shortcut.needsPermission;
    const keyLabel = $('shortcut-recorder-dictation').textContent.trim();
    rows.append(line('Shortcut', recording ? 'ready' : 'attention', shortcut
      ? recording ? keyLabel + ' is active. Hold to speak, or tap for hands-free.'
        : shortcut.needsPermission ? 'This shortcut needs Input Monitoring. You can also choose a standard key combination.'
        : shortcut.pending ? 'Your shortcut is still starting.' : 'Enable your shortcut below, or click its keys to change it.'
      : 'Could not check your shortcut. Use Check again.',
      [button(shortcut?.needsPermission ? 'Allow shortcut' : 'Shortcut settings', shortcut?.needsPermission ? permissionHelp : focusShortcut)]));
    if (!recording) issues.push('shortcut');
    const micPermission = permissions?.find(row => row.kind === 'microphone');
    const micAllowed = micPermission?.runtimeUsable === true;
    const candidate = devices?.candidates?.[0];
    const available = !!devices?.entries?.length && Array.isArray(devices?.candidates) && devices.candidates.length > 0
      && (candidate == null || devices.entries.some(entry => entry.name.toLowerCase() === candidate.toLowerCase()));
    const microphone = candidate || (prefs?.experience.microphone_mode === 'shared' ? 'Recording microphone' : 'System default microphone');
    let micText = available ? microphone + ' is selected. Test it to check your input level.' : devices ? 'No usable microphone is connected. Choose a connected input.' : 'Could not check connected microphones.';
    if (!micAllowed) micText += micPermission?.status === 'denied' ? ' Allow microphone access in System Settings.' : ' Microphone access needs to be checked before you dictate.';
    rows.append(line('Microphone', available && micAllowed ? 'ready' : 'attention', micText,
      [button('Test microphone', () => window.MinutesDictation?.testMic()), ...(!micAllowed ? [button('Microphone access', permissionHelp)] : [])]));
    if (!available || !micAllowed) issues.push('microphone');
    const model = prefs?.model;
    const installable = model && ['tiny', 'base', 'small', 'medium', 'large-v3'].includes(model.name);
    let modelText = model ? model.name + (model.ready ? ' is installed on this device.' : ' is missing or incomplete. Install it before dictating.') : 'Could not check the selected dictation model.';
    if (installing) modelText = downloadText;
    const modelControl = button(installing ? 'Installing…' : model?.ready ? 'Model settings' : installable ? 'Install ' + model.name : 'Model settings', model?.ready || !installable ? modelSettings : () => installModel(model.name));
    modelControl.disabled = !!installing;
    rows.append(line('Speech model', model?.ready ? 'ready' : 'attention', modelText, [modelControl]));
    if (!model?.ready) issues.push('model');
    const copyOnly = $('settings-dictation-destination').value === 'clipboard';
    const automation = permissions?.find(row => row.kind === 'automation');
    const direct = !copyOnly && automation?.runtimeUsable === true;
    rows.append(line('Text delivery', 'info', copyOnly
      ? 'Words are copied. Paste them into the practice field or any app yourself.'
      : direct ? 'Minutes will attempt insertion at your cursor. The actual result is checked; clipboard recovery stays available.'
      : 'Minutes will attempt insertion. macOS may ask to allow pasting on first use. If it cannot paste, your words stay on the clipboard.',
      copyOnly ? [] : [button('Paste permissions', permissionHelp)]));
    rows.setAttribute('aria-busy', 'false');
    const showChecks = issues.length > 0 || expanded;
    $('dictation-ready-body').hidden = !showChecks;
    $('dictation-ready-toggle').hidden = issues.length > 0;
    $('dictation-ready-toggle').textContent = showChecks ? 'Hide checks' : 'Show checks';
    $('dictation-ready-toggle').setAttribute('aria-expanded', String(showChecks));
    $('dictation-ready-practice').disabled = issues.length > 0 || !!installing;
    $('dictation-ready-title').textContent = issues.length ? 'Get ready to dictate' : 'Ready to dictate';
    $('dictation-ready-summary').textContent = snapshotError || (errors.length ? 'Some checks could not finish. Use Check again, or fix the item below.'
      : issues.length ? 'Check the items below, then try your shortcut in the practice field.'
      : 'Try your shortcut here, then use it in the app where you want to write.');
    $('dictation-ready-summary').dataset.error = String(!!snapshotError || errors.length > 0);
  }
  async function refresh() {
    const current = ++revision;
    checking = true;
    $('dictation-ready-toggle').disabled = true;
    for (const control of $('dictation-ready-rows').querySelectorAll('button')) control.disabled = true;
    $('dictation-ready-refresh').disabled = true;
    $('dictation-ready-rows').setAttribute('aria-busy', 'true');
    $('dictation-ready-practice').disabled = true;
    $('dictation-ready-title').textContent = 'Checking dictation…';
    const results = await Promise.allSettled([
      deadline(invoke('cmd_dictation_preferences'), 'Model check'),
      deadline(invoke('cmd_shortcut_status', { slot: 'dictation' }), 'Shortcut check'),
      deadline(invoke('cmd_dictation_devices'), 'Microphone check'),
      deadline(invoke('cmd_macos_permission_rows'), 'Permission check'),
    ]);
    if (revision !== current) return;
    const value = index => results[index].status === 'fulfilled' ? results[index].value : null;
    last = { prefs: value(0), shortcut: value(1), devices: value(2), permissions: Array.isArray(value(3)) ? value(3) : null, errors: results.filter(result => result.status === 'rejected') };
    checking = false; render(); $('dictation-ready-refresh').disabled = false; $('dictation-ready-toggle').disabled = false;
  }
  function setExpanded(value) {
    expanded = value;
    try { localStorage.setItem('minutes.dictationSetupExpanded', String(value)); } catch (_) {}
    render();
  }
  $('dictation-ready-toggle').addEventListener('click', () => { if (!checking) setExpanded(!expanded); });
  $('dictation-ready-refresh').addEventListener('click', () => { snapshotError = ''; refresh(); });
  $('dictation-ready-practice').addEventListener('click', () => {
    setExpanded(false);
    window.MinutesDictation?.show('writing'); $('dictation-practice').open = true;
    $('dictation-practice-text').focus(); $('dictation-practice').scrollIntoView({ block: 'center' });
  });
  window.addEventListener('focus', () => { if (visible()) refresh(); });
  window.addEventListener('minutes:shortcut-changed', event => { if (event.detail.slot === 'dictation' && visible()) refresh(); });
  window.MinutesDictationReadiness = { refresh };
  if (window.__TAURI__?.event) {
    window.__TAURI__.event.listen('dictation:preferences-changed', () => { if (visible()) refresh(); });
    window.__TAURI__.event.listen('dictation:mic-test', event => { if (event.payload.state !== 'level' && visible()) refresh(); });
    window.__TAURI__.event.listen('download-model', event => {
      const progress = event.payload;
      if (progress.model !== installing || !installing) return;
      const total = progress.totalBytes;
      downloadText = total ? 'Downloading ' + installing + ' · ' + Math.min(100, Math.round(progress.downloadedBytes / total * 100)) + '%' : 'Downloading ' + installing + '…';
      render();
    });
  }
  $('tab-dictation').addEventListener('click', refresh);
})();
