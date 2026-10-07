/* Local dictation preferences; no transcript is sent except an explicit local edit. */
(() => {
  'use strict';
  const $ = (id) => document.getElementById(id);
  const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
  const clone = (value) => JSON.parse(JSON.stringify(value));
  let preferences = null;
  let saving = Promise.resolve();
  let testing = false;
  let loading = false;
  let selectedSnippet = null;
  let selectedWord = null;
  let selectedRule = null;
  let correctionOriginal = "";
  let reviewSource = null;
  let pendingReview = null;
  let editorRevision = 0;
  let reviewReturnFocus = null;
  let destination = { ready: false, appName: null };
  function status(message, error = false) {
    const tabs = document.querySelector('.settings-tabs');
    $('dictation-save-status').style.top = `${parseFloat(getComputedStyle(tabs).top) + tabs.offsetHeight}px`;
    $('dictation-save-status').textContent = message;
    $('dictation-save-status').dataset.error = String(error);
  }
  function updateDestinationControls() {
    for (const button of document.querySelectorAll('[data-dictation-action="paste"], #dictation-edit-paste, #dictation-original-paste')) {
      button.disabled = !destination.ready || (button.id === 'dictation-original-paste' && !$('dictation-edit-original').value.trim());
      button.title = destination.ready ? `Paste into ${destination.appName || 'your destination app'}` : 'Open Recent Dictations using its shortcut from the destination app, or use Copy.';
    }
    $('dictation-destination-status').textContent = destination.ready
      ? `Destination: ${destination.appName || 'your app'}. Text is checked again before pasting.`
      : 'Copy works anywhere. To paste, open Recent Dictations with its shortcut from your destination app.';
  }
  async function refreshDestination() {
    try { destination = await invoke('cmd_dictation_history_destination'); }
    catch (_) { destination = { ready: false, appName: null }; }
    updateDestinationControls();
  }
  function pane(name, focus = false) {
    for (const button of document.querySelectorAll('[data-dictation-pane]')) {
      const active = button.dataset.dictationPane === name;
      button.setAttribute('aria-selected', String(active));
      button.tabIndex = active ? 0 : -1;
      if (active && focus) button.focus();
    }
    for (const panel of document.querySelectorAll('.dictation-pane')) {
      const active = panel.id === `dictation-pane-${name}`;
      panel.hidden = !active;
      panel.setAttribute('aria-hidden', String(!active));
    }
    if (name !== 'mic') stopMic();
    if (name !== 'writing') window.MinutesShortcutRecorder?.cancel();
  }
  for (const button of document.querySelectorAll('[data-dictation-pane]')) {
    button.addEventListener('click', () => pane(button.dataset.dictationPane));
    button.addEventListener('keydown', (event) => {
      const buttons = [...document.querySelectorAll('[data-dictation-pane]')];
      let index = buttons.indexOf(button);
      if (event.key === 'ArrowRight') index = (index + 1) % buttons.length;
      else if (event.key === 'ArrowLeft') index = (index + buttons.length - 1) % buttons.length;
      else if (event.key === 'Home') index = 0;
      else if (event.key === 'End') index = buttons.length - 1;
      else return;
      event.preventDefault(); pane(buttons[index].dataset.dictationPane, true);
    });
  }
  function action(label, run) {
    const button = document.createElement('button');
    button.type = 'button'; button.className = 'btn btn-secondary btn-sm'; button.textContent = label;
    button.addEventListener('click', run); return button;
  }
  function row(title, detail, actions) {
    const root = document.createElement('div'); root.className = 'dictation-list-row';
    const copy = document.createElement('div'); copy.className = 'dictation-list-copy';
    const name = document.createElement('strong'); name.textContent = title;
    const description = document.createElement('small'); description.textContent = detail;
    copy.append(name, description);
    const controls = document.createElement('div'); controls.className = 'dictation-actions';
    for (const button of actions) button.setAttribute('aria-label', `${button.textContent}: ${title}`);
    controls.append(...actions);
    root.append(copy, controls); return root;
  }
  function empty(root, message) {
    const note = document.createElement('p'); note.className = 'dictation-note'; note.textContent = message; root.replaceChildren(note);
  }
  function render() {
    if (!preferences) return;
    const e = preferences.experience;
    $('dictation-clipboard-delay').value = String(e.clipboard_restore_delay_ms || 500);
    $('dictation-context-enabled').checked = e.context_enabled;
    $('settings-dictation-writing-style').value = e.writing_style;
    $('dictation-mic-mode').value = e.microphone_mode;
    $('dictation-virtual-mics').checked = e.include_virtual_microphones;
    $('settings-dictation-microphone').disabled = e.microphone_mode !== 'preferred';
    $('dictation-mic-add').disabled = e.microphone_mode !== 'preferred';
    const words = $('dictation-dictionary'); words.replaceChildren();
    const entries = Object.entries(e.dictionary);
    if (!entries.length) empty(words, 'No saved spellings yet. Add a name or term you use often.');
    for (const [spoken, spelling] of entries) words.append(row(spelling, `When you say: ${spoken}`, [
      action('Edit', () => { selectedWord = spoken; formMode('word', true); $('dictation-word-spoken').value = spoken; $('dictation-word-written').value = spelling; $('dictation-word-written').focus(); }),
      action('Forget', () => save((next) => delete next.experience.dictionary[spoken])),
    ]));
    const snippets = $('dictation-snippets'); snippets.replaceChildren();
    const saved = Object.entries(preferences.snippets);
    if (!saved.length) empty(snippets, 'Save a sign-off, a repeated prompt, or a short reply.');
    for (const [name, text] of saved) snippets.append(row(name, text, [
      action('Edit', () => { selectedSnippet = name; formMode('snippet', true); $('dictation-snippet-name').value = name; $('dictation-snippet-text').value = text; $('dictation-snippet-text').focus(); }),
      action('Copy', () => copy(text)),
      action('Remove', () => save((next) => delete next.snippets[name])),
    ]));
    const rules = $('dictation-rules'); rules.replaceChildren();
    if (!e.target_rules.length) empty(rules, 'Uses ordinary app formatting until you add a preference.');
    e.target_rules.forEach((rule, index) => rules.append(row(rule.target.replace(/^app:|^site:/, ''), `${rule.mode.replaceAll('_', ' ')} · ${rule.style}`, [
      action('Edit', () => { selectedRule = rule.target; formMode('rule', true); $('dictation-rule-kind').value = rule.target.split(':')[0]; updateRuleKind(); $('dictation-rule-target').value = rule.target.slice(rule.target.indexOf(':') + 1); $('dictation-rule-mode').value = rule.mode; $('dictation-rule-style').value = rule.style; $('dictation-rule-target').focus(); }),
      action('Remove', () => save((next) => next.experience.target_rules.splice(index, 1))),
    ])));
    const microphones = $('dictation-microphones'); microphones.replaceChildren();
    if (!e.microphones.length) empty(microphones, 'Add preferred inputs in the order you want to use them.');
    e.microphones.forEach((name, index) => {
      const up = action('Move up', () => save((next) => { const items = next.experience.microphones; [items[index - 1], items[index]] = [items[index], items[index - 1]]; })); up.disabled = index === 0;
      const down = action('Move down', () => save((next) => { const items = next.experience.microphones; [items[index + 1], items[index]] = [items[index], items[index + 1]]; })); down.disabled = index === e.microphones.length - 1;
      microphones.append(row(name, index === 0 ? 'First choice when available' : 'Fallback when higher choices are unavailable', [up, down, action('Remove', () => save((next) => next.experience.microphones.splice(index, 1)))]));
    });
  }
  function formMode(kind, editing) {
    const form = $(`dictation-${kind}-form`);
    const labels = { word: ['Remember spelling', 'Update spelling'], rule: ['Add preference', 'Update preference'], snippet: ['Save snippet', 'Update snippet'] };
    form.querySelector('[type="submit"]').textContent = labels[kind][Number(editing)];
    form.querySelector('[data-dictation-cancel]').hidden = !editing;
  }
  function resetForm(kind) {
    if (kind === 'word') selectedWord = null;
    if (kind === 'rule') selectedRule = null;
    if (kind === 'snippet') selectedSnippet = null;
    $(`dictation-${kind}-form`).reset();
    $(`dictation-${kind}-error`).textContent = '';
    formMode(kind, false);
    if (kind === 'rule') updateRuleKind();
  }
  async function submitForm(kind, mutate) {
    const form = $(`dictation-${kind}-form`);
    const error = $(`dictation-${kind}-error`);
    const controls = [...form.querySelectorAll('input, textarea, select, button')];
    error.textContent = '';
    if (kind === 'snippet' && new TextEncoder().encode($('dictation-snippet-text').value).length > 8192) { error.textContent = 'This snippet is too long. Try a shorter version.'; $('dictation-snippet-text').focus(); return false; }
    for (const control of controls) control.disabled = true;
    const ok = await save(mutate);
    for (const control of controls) control.disabled = false;
    if (ok) resetForm(kind);
    else { error.textContent = $('dictation-save-status').textContent; form.querySelector('input').focus(); }
    return ok;
  }
  for (const button of document.querySelectorAll('[data-dictation-cancel]')) button.addEventListener('click', () => resetForm(button.dataset.dictationCancel));
  async function save(mutate) {
    saving = saving.catch(() => {}).then(async () => {
      if (!preferences) { status('Preferences are still loading. Try again.', true); return false; }
      const next = clone(preferences); mutate(next); status('Saving…');
      try {
        await invoke('cmd_save_dictation_preferences', { preferences: next });
        preferences = next; render(); status('Saved on this device.'); return true;
      } catch (error) { render(); status(String(error), true); return false; }
    });
    return saving;
  }
  async function devices() {
    try {
      const data = await invoke('cmd_dictation_devices');
      const picker = $('settings-dictation-microphone'); const previous = picker.value; picker.replaceChildren();
      for (const entry of data.entries || []) {
        if (!preferences?.experience.include_virtual_microphones && /blackhole|loopback|soundflower|aggregate|virtual|vb-cable/i.test(entry.name)) continue;
        const option = document.createElement('option'); option.value = entry.name; option.textContent = entry.name;
        if (data.lidClosed && /built-in|macbook/i.test(entry.name)) { option.textContent += ' — laptop lid is closed'; option.disabled = true; }
        picker.append(option);
      }
      if ([...picker.options].some((option) => option.value === previous && !option.disabled)) picker.value = previous;
      if (!picker.options.length) { const option = document.createElement('option'); option.textContent = 'No physical microphone found'; option.value = ''; picker.append(option); }
      $('dictation-lid-warning').hidden = !data.lidClosed;
    } catch (error) { status(`Could not refresh microphones: ${error}`, true); }
  }
  async function load() {
    if (loading) return;
    loading = true;
    try { preferences = await invoke('cmd_dictation_preferences'); render(); window.MinutesShortcuts?.refreshRecovery(); window.MinutesDictationReadiness?.refresh(); await refreshDestination(); await devices(); }
    catch (error) { status(`Could not load dictation preferences: ${error}`, true); }
    finally { loading = false; }
  }
  async function copy(text) {
    try { await invoke('cmd_copy_dictation_text', { text }); status('Copied.'); }
    catch (error) { status(String(error), true); }
  }
  async function stopMic() {
    testing = false; $('dictation-mic-test').textContent = 'Test microphone';
    $('dictation-mic-level').value = 0;
    try { await invoke('cmd_stop_dictation_mic_test'); } catch (_) { /* teardown is best effort */ }
  }
  $('dictation-clipboard-delay').addEventListener('change', (event) => { const value = Number(event.target.value); save((next) => next.experience.clipboard_restore_delay_ms = value); });
  $('dictation-context-enabled').addEventListener('change', (event) => { const value = event.target.checked; save((next) => next.experience.context_enabled = value); });
  $('settings-dictation-writing-style').addEventListener('change', (event) => { const value = event.target.value; save((next) => next.experience.writing_style = value); });
  $('dictation-mic-mode').addEventListener('change', (event) => { const value = event.target.value; stopMic(); save((next) => next.experience.microphone_mode = value); });
  $('dictation-virtual-mics').addEventListener('change', async (event) => { const value = event.target.checked; await save((next) => next.experience.include_virtual_microphones = value); devices(); });
  $('settings-dictation-microphone').addEventListener('focus', devices);
  $('dictation-mic-add').addEventListener('click', () => { const value = $('settings-dictation-microphone').value; if (value) save((next) => { if (!next.experience.microphones.includes(value)) next.experience.microphones.push(value); }); });
  $('dictation-word-form').addEventListener('submit', async (event) => {
    event.preventDefault(); const spoken = $('dictation-word-spoken').value.trim(); const written = $('dictation-word-written').value.trim(); const previousWord = selectedWord;
    await submitForm('word', (next) => { if (previousWord && previousWord !== spoken) delete next.experience.dictionary[previousWord]; next.experience.dictionary[spoken] = written; });
  });
  function updateRuleKind() {
    const website = $('dictation-rule-kind').value === 'site';
    $('dictation-rule-target-label').firstChild.textContent = website ? 'Website hostname' : 'App name';
    $('dictation-rule-target').placeholder = website ? 'mail.google.com' : 'Ghostty';
  }
  $('dictation-rule-kind').addEventListener('change', updateRuleKind);
  $('dictation-rule-form').addEventListener('submit' , async (event) => {
    event.preventDefault(); const target = `${$('dictation-rule-kind').value}:${$('dictation-rule-target').value.trim()}`; const mode = $('dictation-rule-mode').value; const style = $('dictation-rule-style').value; const previousTarget = selectedRule;
    await submitForm('rule', (next) => {
      if (previousTarget && previousTarget.toLowerCase() !== target.toLowerCase()) next.experience.target_rules = next.experience.target_rules.filter((rule) => rule.target.toLowerCase() !== previousTarget.toLowerCase());
      const index = next.experience.target_rules.findIndex((r) => r.target.toLowerCase() === target.toLowerCase()); const rule = { target, mode, style };
      if (index < 0) next.experience.target_rules.push(rule); else next.experience.target_rules[index] = rule;
    });
  });
  $('dictation-snippet-form').addEventListener('submit', async (event) => {
    event.preventDefault(); const name = $('dictation-snippet-name').value.trim(); const text = $('dictation-snippet-text').value; const previousName = selectedSnippet;
    await submitForm('snippet', (next) => { if (previousName && name !== previousName) delete next.snippets[previousName]; next.snippets[name] = text; });
  });
  $('dictation-mic-test').addEventListener('click', async () => {
    if (testing) { await stopMic(); return; }
    testing = true; $('dictation-mic-test').textContent = 'Stop test'; $('dictation-mic-status').textContent = 'Opening microphone…';
    try { await invoke('cmd_test_dictation_microphone'); }
    catch (error) { await stopMic(); $('dictation-mic-status').textContent = String(error); }
  });
  $('dictation-undo').addEventListener('click', async () => { try { status(await invoke('cmd_undo_last_dictation')); } catch (error) { status(String(error), true); } });
  $('dictation-refresh').addEventListener('click', () => window.loadRecentDictations?.());
  $('dictation-original-copy').addEventListener('click', () => copy($('dictation-edit-original').value));
  $('dictation-correction').addEventListener('click', async () => {
    try {
      const candidate = await invoke('cmd_dictation_correction', { original: correctionOriginal, corrected: $('dictation-edit-original').value });
      if (!candidate) { $('dictation-edit-status').textContent = 'Change one spelling in a recent dictation, then try again. For other phrases, add a dictionary entry in Words & snippets.'; return; }
      pane('words'); resetForm('word'); $('dictation-word-spoken').value = candidate[0]; $('dictation-word-written').value = candidate[1]; $('dictation-word-written').focus();
      status('Check both phrases, then choose Remember spelling. Nothing has been learned yet.');
    } catch (error) { $('dictation-edit-status').textContent = String(error); }
  });
  $('dictation-rewrite').addEventListener('click', async () => {
    const button = $('dictation-rewrite'); const text = $('dictation-edit-original').value; const instruction = $('dictation-edit-instruction').value; const revision = editorRevision;
    if (new TextEncoder().encode(text).length > 8192 || new TextEncoder().encode(instruction).length > 500) { status('This text or instruction is too long for a local edit. Try a shorter version.', true); return; }
    rewriteBusy = true; button.disabled = true; $('dictation-edit-status').textContent = 'Editing with your local model…'; status('Editing with your local model…');
    try {
      const result = await invoke('cmd_dictation_rewrite', { text, instruction });
      if (editorRevision !== revision || $('dictation-edit-original').value !== text) { $('dictation-edit-status').textContent = 'The original changed while editing. Preview again with the current text.'; status($('dictation-edit-status').textContent, true); return; }
      $('dictation-edit-result').value = result.text; $('dictation-edit-preview').hidden = false; $('dictation-edit-status').textContent = 'Preview ready. The original is unchanged.'; status($('dictation-edit-status').textContent);
      updateDestinationControls(); $('dictation-edit-result').focus(); $('dictation-edit-preview').scrollIntoView({ block: 'nearest' });
    } catch (error) { $('dictation-edit-status').textContent = String(error); status(String(error), true); }
    finally { rewriteBusy = false; updateEditorControls(); }
  });
  $('dictation-edit-copy').addEventListener('click', () => copy($('dictation-edit-result').value));
  $('dictation-edit-paste').addEventListener('click', async () => {
    try { const result = await invoke('cmd_paste_dictation_from_history', { text: $('dictation-edit-result').value }); $('dictation-edit-status').textContent = result.message; }
    catch (error) { $('dictation-edit-status').textContent = String(error); }
  });
  $('dictation-edit-discard').addEventListener('click', () => { $('dictation-edit-result').value = ''; $('dictation-edit-preview').hidden = true; $('dictation-edit-status').textContent = 'Edit discarded. The original is unchanged.'; });
  function updateEditorControls() {
    const hasText = Boolean($('dictation-edit-original').value.trim());
    $('dictation-original-copy').disabled = !hasText;
    $('dictation-correction').disabled = !hasText || $('dictation-edit-original').value === correctionOriginal;
    $('dictation-original-reset').disabled = $('dictation-edit-original').value === correctionOriginal;
    $('dictation-rewrite').disabled = !hasText || !$('dictation-edit-instruction').value.trim() || rewriteBusy;
    updateDestinationControls();
  }
  let rewriteBusy = false;
  function changedEditor() {
    editorRevision++;
    $('dictation-edit-preview').hidden = true;
    $('dictation-edit-result').value = '';
    $('dictation-edit-status').textContent = '';
    updateEditorControls();
  }
  $('dictation-edit-original').addEventListener('input', changedEditor);
  $('dictation-edit-instruction').addEventListener('input', changedEditor);
  for (const button of document.querySelectorAll('[data-dictation-instruction]')) button.addEventListener('click', () => {
    $('dictation-edit-instruction').value = button.dataset.dictationInstruction;
    changedEditor(); $('dictation-edit-instruction').focus();
  });
  $('dictation-original-reset').addEventListener('click', () => { $('dictation-edit-original').value = correctionOriginal; changedEditor(); });
  $('dictation-original-paste').addEventListener('click', async () => {
    const button = $('dictation-original-paste'); button.disabled = true;
    try { const result = await invoke('cmd_paste_dictation_from_history', { text: $('dictation-edit-original').value }); status(result.message); }
    catch (error) { status(String(error), true); }
    finally { updateEditorControls(); }
  });
  function openReview(text, record = null, force = false) {
    if (!force && $('dictation-edit-original').value !== correctionOriginal) {
      pendingReview = { text, record }; $('dictation-review').hidden = false; $('dictation-review-switch').hidden = false;
      $('dictation-review-keep').focus(); $('dictation-review').scrollIntoView({ block: 'start' }); return false;
    }
    correctionOriginal = text || ''; reviewSource = record;
    reviewReturnFocus = record
      ? document.querySelector(`[data-dictation-action="edit"][data-dictation-id="${CSS.escape(record.id)}"]`)
      : $('dictation-history-search');
    pendingReview = null; $('dictation-review-switch').hidden = true;
    pane('recent'); $('dictation-review').hidden = false;
    $('dictation-review-source').textContent = record ? window.dictationRecentMeta(record) : 'Text selected in your destination app';
    $('dictation-source-text').textContent = record?.originalTranscript ? (record.originalTranscript.cleanedText || 'No transcript was available before recovery.') : correctionOriginal;
    $('dictation-source-raw').textContent = record?.originalTranscript?.rawText || record?.rawText || '';
    $('dictation-source-raw-section').hidden = !$('dictation-source-raw').textContent || $('dictation-source-raw').textContent === $('dictation-source-text').textContent;
    $('dictation-edit-original').value = correctionOriginal;
    $('dictation-edit-instruction').value = '';
    $('dictation-local-edit').open = false;
    changedEditor(); $('dictation-edit-original').setSelectionRange(0, 0); $('dictation-edit-original').focus(); $('dictation-review').scrollIntoView({ block: 'start' });
    document.dispatchEvent(new CustomEvent('minutes:dictation-review-source', { detail: record })); return true;
  }
  $('dictation-review-keep').addEventListener('click', () => { pendingReview = null; $('dictation-review-switch').hidden = true; $('dictation-edit-original').focus(); });
  $('dictation-review-load').addEventListener('click', () => { if (pendingReview) openReview(pendingReview.text, pendingReview.record, true); });
  $('dictation-review-close').addEventListener('click', () => {
    // Closing keeps the draft; reopening the same source resumes it.
    $('dictation-review').hidden = true; editorRevision++; $('dictation-edit-preview').hidden = true;
    if (reviewReturnFocus?.isConnected) reviewReturnFocus.focus();
    else $('dictation-history-search').focus();
  });
  $('dictation-practice-clear').addEventListener('click', () => { $('dictation-practice-text').value = ''; $('dictation-practice-text').focus(); });
  $('dictation-history-search').addEventListener('input', () => { $('settings-dictation-recents')._dictationVisible = 25; window.renderRecentDictations?.(); });
  $('dictation-history-recovery').addEventListener('change', () => { $('settings-dictation-recents')._dictationVisible = 25; window.renderRecentDictations?.(); });
  $('dictation-history-search').addEventListener('keydown', (event) => {
    if (event.key === 'Enter') { event.preventDefault(); $('settings-dictation-recents').querySelector('[data-dictation-action="edit"]')?.click(); }
  });
  window.MinutesDictation = {
    load, stopMic, notify: status, updateDestinationControls, show: pane,
    testMic() { pane('mic'); if (!testing) $('dictation-mic-test').click(); $('dictation-mic-test').focus(); },
    edit(text, record) {
      if (record && reviewSource?.id === record.id && $('dictation-edit-original').value !== correctionOriginal) { pane('recent'); $('dictation-review').hidden = false; $('dictation-edit-original').focus(); return true; }
      return openReview(text, record);
    },
    hasReviewDraft() { return $('dictation-edit-original').value !== correctionOriginal; },
    refreshReview(text, record) { return openReview(text, record, true); },
    snippet(text) { pane('words'); resetForm('snippet'); $('dictation-snippet-text').value = text || ''; $('dictation-snippet-name').focus(); },
  };
  if (window.__TAURI__?.event) {
    window.__TAURI__.event.listen('dictation:mic-test', (event) => {
      const data = event.payload;
      if (data.state === 'level') $('dictation-mic-level').value = data.level;
      if (data.state === 'listening') $('dictation-mic-status').textContent = `Listening to ${data.device}. Speak to check the level.`;
      if (data.state === 'stopped' || data.state === 'error') { testing = false; $('dictation-mic-test').textContent = 'Test microphone'; $('dictation-mic-level').value = 0; $('dictation-mic-status').textContent = data.message || 'Test finished. Nothing was saved.'; }
    });
    window.__TAURI__.event.listen('minutes://show-dictation-history', async () => {
      window.openSettings?.({ keepDictationTarget: true }); $('tab-dictation').click(); pane('recent'); await refreshDestination();
      const selected = await invoke('cmd_dictation_selection'); if (selected) window.MinutesDictation.edit(selected);
    });
  }
  const toggleLabels = { 'shortcut-toggle-dictation': 'Dictation shortcut', 'settings-dictation-voice-commands': 'Exact voice-edit commands', 'settings-dictation-daily-note': 'Log dictations to daily note' };
  function describeToggles() {
    for (const [id, name] of Object.entries(toggleLabels)) {
      const button = $(id); const on = button.textContent.trim() === 'On';
      button.setAttribute('aria-label', `${name}: ${on ? 'on' : 'off'}`);
      button.setAttribute('aria-pressed', String(on));
    }
  }
  const toggleObserver = new MutationObserver(describeToggles);
  for (const id of Object.keys(toggleLabels)) toggleObserver.observe($(id), { childList: true, subtree: true, characterData: true });
  describeToggles();
  // Native capture uses these labels too; make existing selects discoverable by name.
  for (const select of document.querySelectorAll('.dictation-page select')) {
    if (!select.labels?.length && !select.hasAttribute('aria-label')) select.setAttribute('aria-label', select.closest('.about-controls-group')?.querySelector('.about-controls-copy')?.textContent || 'Dictation preference');
  }
  document.addEventListener('keydown', (event) => { if (event.key === 'Escape' && testing) stopMic(); });
  const observer = new MutationObserver(() => { if (!$('settings-overlay').classList.contains('active') || !$('panel-dictation').classList.contains('is-active')) { if (testing) stopMic(); } });
  observer.observe($('settings-overlay'), { attributes: true, attributeFilter: ['class'] });
  observer.observe($('panel-dictation'), { attributes: true, attributeFilter: ['class'] });
})();
