/* Local Work host. Source/model text is always rendered with textContent. */
(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  let current = null, updating = false, held = false, latestSpoken = '', artifact = null, lastId = '';
  let lastReviews = '', lastMemory = '', lastTasks = '';
  const call = async request => {
    if (!window.__TAURI__?.core?.invoke) throw new Error('Open this page in Minutes Dev; no desktop connection is available.');
    return window.__TAURI__.core.invoke('cmd_workbench', { request });
  };
  const notice = (text, error = false) => { $('notice').textContent = String(text); $('notice').classList.toggle('error', error); };
  const node = (tag, text, className) => { const item = document.createElement(tag); if (text !== undefined) item.textContent = String(text); if (className) item.className = className; return item; };
  const button = (text, click, className) => { const item = node('button', text, className); item.type = 'button'; item.addEventListener('click', () => action(click)); return item; };
  async function action(fn) { try { await fn(); await refresh(); } catch (error) { notice(error, true); } }
  const resetConsent = () => { $('consent').checked = false; };
  function render(view) {
    if (!view?.checkpoint) return;
    current = view;
    const data = view.checkpoint;
    if (data.id !== lastId) { $('goal').value = data.goal; $('next-step').value = data.next_step || ''; lastId = data.id; resetConsent(); }
    $('revision').textContent = `revision ${data.revision}`;
    $('selection').textContent = data.focus?.selection || 'Choose the specific text you want to work on.';
    $('source').textContent = data.focus ? `${data.focus.source.locator} · snapshot ${data.focus.source.version.slice(0,12)}` : 'No selection attached';
    $('shortcut').disabled = !view.native_selection;
    $('shortcut').textContent = view.shortcut_enabled ? `Disable ${view.shortcut}` : `Enable ${view.shortcut}`;
    if (!view.native_selection) $('capture-help').textContent = 'Native selection capture is not available on this platform. Paste selected text below; the rest of the work flow is portable.';
    $('start').disabled = view.voice_active || view.starting;
    $('stop').disabled = !view.voice_active && !view.starting;
    for (const id of ['talk','send','cancel']) $(id).disabled = !view.voice_active;
    if (!view.voice_active && !view.starting) { $('state').textContent = 'Local only'; releaseTalk(); }
    if (view.starting) $('state').textContent = 'Connecting';
    const memoryKey = JSON.stringify(data.memory);
    if (memoryKey !== lastMemory) {
      lastMemory = memoryKey; $('memory').replaceChildren();
      const labels = { model_inference:'Proposal / unverified', user_interpretation:'Your interpretation', user_confirmed_decision:'You confirmed this decision', source_record:'Source record' };
      for (const entry of [...data.memory].reverse()) {
        const box = node('div', undefined, 'entry');
        box.append(node('span', labels[entry.attribution] || entry.attribution, 'badge'), node('p', entry.text));
        if (entry.attribution === 'model_inference' && !entry.text.startsWith('Unverified conversation transcript')) {
          box.append(button('Review as my correction', () => { $('note').value = entry.text; $('decision').checked = false; $('note').focus(); }));
        }
        $('memory').append(box);
      }
    }
    const reviewKey = JSON.stringify(view.reviews);
    if (reviewKey !== lastReviews) {
      lastReviews = reviewKey; $('reviews').replaceChildren();
      $('approval-section').hidden = !view.reviews.length;
      for (const review of view.reviews) {
        const box = node('div', undefined, 'review');
        box.append(node('h3', review.verb), node('p', review.target), node('pre', JSON.stringify(review.payload,null,2)));
        const row = node('div',undefined,'row');
        row.append(button('Approve this exact action', async () => { await call({action:'approve',review}); notice('Approval submitted. Completion will be reported separately.'); }, 'approve'), button('Reject', async () => { await call({action:'reject',id:review.id}); notice('Rejected.'); }));
        box.append(row); $('reviews').append(box);
      }
    }
    const taskKey = JSON.stringify(data.tasks);
    if (taskKey !== lastTasks) {
      lastTasks = taskKey; $('tasks').replaceChildren();
      if (!data.tasks.length) $('tasks').append(node('p','No delegated tasks yet. Full output stays local until separately shared.','small'));
      for (const task of data.tasks) {
        const box = node('div',undefined,'entry');
        box.append(node('span',task.state,'badge'),node('p',task.instruction));
        if (task.spoken_summary) box.append(node('p',task.spoken_summary,'small'));
        if (task.artifact) box.append(button('Review local result', async () => {
          artifact = await call({action:'artifact',name:task.artifact.locator});
          $('artifact-content').textContent = artifact.content;
          $('share-artifact').disabled = !artifact.shareable || !current.voice_active;
          $('artifact-dialog').showModal();
        }));
        $('tasks').append(box);
      }
    }
  }
  async function refresh() {
    if (updating) return;
    updating = true;
    try { render(await call({action:'view'})); } finally { updating = false; }
  }
  async function listSaved() {
    const files = await call({action:'list'}); $('saved').replaceChildren();
    if (!files.length) $('saved').append(node('p','No saved work yet.','small'));
    for (const file of files) $('saved').append(button(`${file.goal} · revision ${file.revision}`, async () => {
      await call({action:'resume',name:file.name}); resetConsent(); $('transcript').replaceChildren(); notice('Checkpoint restored locally. Workers and approvals were not resumed.');
    }));
  }
  $('goal-form').addEventListener('submit', event => { event.preventDefault(); action(async () => { await call({action:'new',goal:$('goal').value}); resetConsent(); $('transcript').replaceChildren(); notice('New local work session.'); }); });
  $('selection-form').addEventListener('submit', event => { event.preventDefault(); action(async () => { await call({action:'select',text:$('selection-input').value}); resetConsent(); notice('Selected text is attached locally. Review it before sharing.'); }); });
  $('note-form').addEventListener('submit', event => { event.preventDefault(); action(async () => {
    if (!current) return;
    await call({action:'note',text:$('note').value,decision:$('decision').checked,revision:current.checkpoint.revision,work_id:current.checkpoint.id});
    $('note').value=''; $('decision').checked=false; notice('Saved with your attribution. Old approvals were revoked.');
  }); });
  $('park-form').addEventListener('submit', event => { event.preventDefault(); action(async () => { const result=await call({action:'park',next_step:$('next-step').value || null}); resetConsent(); render(result.view); notice(`Saved locally: ${result.saved.name}`); await listSaved(); }); });
  $('say-form').addEventListener('submit', event => { event.preventDefault(); action(async () => { await call({action:'say',text:$('say').value}); $('say').value=''; }); });
  $('start').addEventListener('click', () => action(async () => {
    if (!$('consent').checked) throw new Error('Review the sharing notice and check the consent box before starting voice.');
    $('start').disabled=true; $('state').textContent='Connecting';
    await call({action:'start',model:$('model').value,allow_cloud:true}); notice('Voice started. Hold Talk to share microphone audio; release to end your turn.');
  }));
  $('stop').addEventListener('click', () => action(async () => { await call({action:'stop'}); resetConsent(); notice('Voice stopped. Work remains local; use Park to save it.'); }));
  $('cancel').addEventListener('click', () => action(async () => { await call({action:'cancel'}); notice('Cancellation requested. Operations already completed cannot be undone.'); }));
  $('shortcut').addEventListener('click', () => action(async () => { await call({action:'shortcut',enabled:!current?.shortcut_enabled}); notice('Select text in another app, then use the shortcut. Capture happens before Minutes takes focus.'); }));
  $('refresh-saved').addEventListener('click', () => action(listSaved));
  $('use-transcript').addEventListener('click', () => { if (!latestSpoken) { notice('No completed spoken turn yet.'); return; } $('note').value=latestSpoken; $('decision').checked=false; $('note').focus(); notice('Review the transcription before saving it with your attribution.'); });
  async function pressTalk(event) {
    if (held || $('talk').disabled) return;
    if (event?.pointerId !== undefined) $('talk').setPointerCapture(event.pointerId);
    held=true; $('talk').classList.add('held'); $('talk').textContent='Listening — release to send';
    try { await call({action:'ptt',down:true}); } catch (error) { held=false; notice(error,true); }
  }
  function releaseTalk() {
    if (!held) return;
    held=false; $('talk').classList.remove('held'); $('talk').textContent='Hold to talk';
    call({action:'ptt',down:false}).catch(error => notice(error,true));
  }
  $('talk').addEventListener('pointerdown', pressTalk);
  for (const event of ['pointerup','pointercancel','lostpointercapture']) $('talk').addEventListener(event,releaseTalk);
  $('talk').addEventListener('keydown', event => { if ((event.code==='Space'||event.code==='Enter')&&!event.repeat) { event.preventDefault(); pressTalk(); } });
  $('talk').addEventListener('keyup', event => { if (event.code==='Space'||event.code==='Enter') { event.preventDefault(); releaseTalk(); } });
  window.addEventListener('blur',releaseTalk);
  document.addEventListener('visibilitychange', () => { if (document.hidden) releaseTalk(); });
  $('close-artifact').addEventListener('click', () => $('artifact-dialog').close());
  $('share-artifact').addEventListener('click', () => action(async () => { if (!artifact) return; await call({action:'share_artifact',name:artifact.name,version:artifact.version}); $('artifact-dialog').close(); notice('The reviewed artifact was shared with the active voice session.'); }));
  if (window.__TAURI__?.event?.listen) window.__TAURI__.event.listen('work:voice', ({payload:event}) => {
    if (event.type==='state') $('state').textContent=event.state;
    if (event.type==='status') notice(event.text);
    if (event.type==='closed') { resetConsent(); notice(event.reason); }
    if ((event.type==='user_transcript'||event.type==='assistant_transcript')&&!event.partial) {
      if (event.type==='user_transcript') latestSpoken=event.text;
      const line=node('p'); line.append(node('strong',event.type==='user_transcript'?'You':'Minutes'),document.createTextNode(event.text)); $('transcript').append(line);
      while ($('transcript').children.length>60) $('transcript').firstChild.remove();
      $('transcript').scrollTop=$('transcript').scrollHeight;
    }
    if (['approval_required','closed','tool_result'].includes(event.type)) refresh().catch(error => notice(error,true));
  }).catch(error=>notice(error,true));
  refresh().catch(error => notice(error,true));
  const poll=setInterval(() => { if (!document.hidden) refresh().catch(error=>notice(error,true)); },1500);
  window.addEventListener('beforeunload', () => { clearInterval(poll); releaseTalk(); });
})();
