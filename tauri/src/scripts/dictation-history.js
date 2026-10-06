/* Local recovery review. Loading/previewing never pastes or replaces history. */
(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
  let selected = null, revision = 0, candidate = null, busy = false, loadedAudio = false;
  const player = $('dictation-audio-player');
  function message(text, error = false) {
    $('dictation-audio-status').textContent = text;
    $('dictation-audio-status').dataset.error = String(error);
  }
  function controls() {
    for (const id of ['dictation-audio-listen', 'dictation-audio-retranscribe', 'dictation-audio-delete', 'dictation-audio-keep', 'dictation-recovery-discard']) $(id).disabled = busy;
    $('dictation-recovery-accept').disabled = busy || !candidate || window.MinutesDictation?.hasReviewDraft();
  }
  function stopAudio() {
    player.pause(); player.removeAttribute('src'); player.load(); player.hidden = true; loadedAudio = false;
  }
  function invalidate() {
    revision++; busy = false; candidate = null; selected = null;
    $('dictation-edit-original').readOnly = false;
    stopAudio(); $('dictation-recovery-preview').hidden = true; $('dictation-recovery-text').value = '';
    $('dictation-audio-delete-confirm').hidden = true; controls();
  }
  function select(record) {
    invalidate(); selected = record;
    $('dictation-audio-review').hidden = !record?.recoveryAudioPath;
    $('dictation-audio-title').textContent = 'Saved recovery audio';
    message(''); controls();
  }
  function current(version) { return version === revision && Boolean(selected); }
  async function listen() {
    if (!selected || busy) return;
    const version = revision; busy = true; controls(); message('Loading saved audio…');
    try {
      if (!loadedAudio) {
        const source = await invoke('cmd_dictation_audio', { id: selected.id });
        if (!current(version)) return;
        if (typeof source !== 'string' || !source.startsWith('data:audio/wav;base64,')) throw new Error('Minutes could not load this recovery recording.');
        player.src = source; player.hidden = false; loadedAudio = true;
      }
      if (player.ended) player.currentTime = 0;
      try { await player.play(); if (current(version)) message('Playing saved recovery audio.'); }
      catch (_) { if (current(version)) message('Audio is ready. Press Play to listen.'); }
    } catch (error) { if (current(version)) message(String(error), true); }
    finally { if (current(version)) { busy = false; controls(); } }
  }
  async function retranscribe() {
    if (!selected || busy) return;
    const version = revision; busy = true; candidate = null; controls(); stopAudio();
    $('dictation-recovery-preview').hidden = true; message('Transcribing saved audio on this computer…');
    try {
      const result = await invoke('cmd_reprocess_dictation', { id: selected.id, previewOnly: true });
      if (!current(version)) return;
      if (!result?.candidateId || typeof result.text !== 'string') throw new Error('The recovery preview is unavailable. Your saved transcript was kept.');
      candidate = result; $('dictation-recovery-saved').textContent = selected.cleanedText || selected.rawText || 'No transcript is saved yet.'; $('dictation-recovery-text').value = result.text; $('dictation-recovery-preview').hidden = false;
      message(window.MinutesDictation?.hasReviewDraft() ? 'Your review has edits. Restore its saved text before accepting a new transcript.' : result.message);
      $('dictation-recovery-text').focus();
    } catch (error) { if (current(version)) message(String(error), true); }
    finally { if (current(version)) { busy = false; controls(); } }
  }
  async function accept() {
    if (!selected || !candidate || busy || window.MinutesDictation?.hasReviewDraft()) return;
    const version = revision, id = selected.id, token = candidate.candidateId;
    busy = true; controls(); $('dictation-edit-original').readOnly = true; message('Saving recovered transcript…');
    try {
      const result = await invoke('cmd_accept_dictation_recovery', { candidateId: token });
      // A closed/switched review never receives this operation's result.
      if (!current(version)) return;
      candidate = null; $('dictation-recovery-preview').hidden = true; message(result);
      await window.loadRecentDictations?.();
      if (!current(version)) return;
      const record = ($('settings-dictation-recents')._dictationRecords || []).find(item => item.id === id);
      if (record) { window.MinutesDictation?.refreshReview(record.cleanedText || '', record); message(result); }
      else message('The transcript was saved, but this history entry could not be refreshed. Refresh Recent to review it.');
    } catch (error) { if (current(version)) message(String(error), true); }
    finally {
      if (current(version)) { $('dictation-edit-original').readOnly = false; busy = false; controls(); }
    }
  }
  function askDelete() {
    if (!selected || busy) return;
    $('dictation-audio-delete-confirm').hidden = false; $('dictation-audio-keep').focus();
  }
  async function deleteAudio() {
    if (!selected || busy) return;
    const version = revision, id = selected.id; busy = true; controls(); stopAudio(); message('Deleting saved audio…');
    try {
      const result = await invoke('cmd_delete_dictation_audio', { id });
      if (!current(version)) return;
      candidate = null; $('dictation-recovery-preview').hidden = true; $('dictation-audio-delete-confirm').hidden = true;
      await window.loadRecentDictations?.();
      if (!current(version)) return;
      $('dictation-audio-listen').hidden = true; $('dictation-audio-retranscribe').hidden = true;
      $('dictation-audio-title').textContent = 'Recovery audio deleted';
      message(result);
    } catch (error) { if (current(version)) message(String(error), true); }
    finally { if (current(version)) { busy = false; controls(); } }
  }
  $('dictation-audio-listen').addEventListener('click', listen);
  $('dictation-audio-retranscribe').addEventListener('click', retranscribe);
  $('dictation-recovery-accept').addEventListener('click', accept);
  $('dictation-recovery-discard').addEventListener('click', () => { candidate = null; $('dictation-recovery-preview').hidden = true; $('dictation-recovery-text').value = ''; message('Saved transcript kept.'); controls(); });
  $('dictation-audio-delete').addEventListener('click', deleteAudio);
  $('dictation-audio-keep').addEventListener('click', () => { $('dictation-audio-delete-confirm').hidden = true; message('Saved audio kept.'); });
  $('dictation-review-close').addEventListener('click', invalidate);
  $('dictation-edit-original').addEventListener('input', controls);
  $('dictation-original-reset').addEventListener('click', controls);
  player.addEventListener('ended', () => message('Finished playing saved audio.'));
  player.addEventListener('error', () => { if (loadedAudio) message('Could not play this audio. The recording is still saved for recovery.', true); });
  document.addEventListener('minutes:dictation-review-source', event => {
    select(event.detail); $('dictation-audio-listen').hidden = false; $('dictation-audio-retranscribe').hidden = false;
  });
  const observer = new MutationObserver(() => {
    if (!$('settings-overlay').classList.contains('active') || !$('panel-dictation').classList.contains('is-active') || $('dictation-pane-recent').hidden) invalidate();
  });
  for (const id of ['settings-overlay', 'panel-dictation', 'dictation-pane-recent']) observer.observe($(id), { attributes: true, attributeFilter: ['class', 'hidden'] });
  function needsRecovery(record) {
    return Boolean(record.recoveryAudioPath) || ['failed', 'blocked', 'recovered'].includes(record.insertion?.outcome);
  }
  function deliveryLabel(record) {
    const outcome = record.insertion?.outcome;
    if (['typed', 'pasted'].includes(outcome)) return record.insertion.verified ? (outcome === 'typed' ? 'Inserted in destination' : 'Pasted in destination') : 'Insertion unverified';
    if (outcome === 'copied') return 'Copied only';
    if (outcome === 'recovered') return 'Recovered · not pasted';
    if (['failed', 'blocked'].includes(outcome)) return 'Needs recovery';
    if (record.recoveryAudioPath && !(record.cleanedText || '').trim()) return 'Audio saved · needs transcription';
    return 'Saved locally';
  }
  window.MinutesDictationHistory = {
    needsRecovery, deliveryLabel,
    async act(action, record) {
      if (window.MinutesDictation?.edit(record.cleanedText || record.rawText || '', record) === false) return;
      if (selected?.id !== record.id) select(record);
      if (action === 'listen') return listen();
      if (action === 'reprocess') return retranscribe();
      if (action === 'delete-audio') askDelete();
    },
  };
})();
