/* Recall tab chrome. Metadata from native hook files is the sole status source. */
(function (global) {
  'use strict';
  let snapshot = { tabs: [], active: null };
  const labels = { working: 'Working', waiting: 'Waiting for you', done: 'Done', error: 'Error' };
  function glyph(tab) {
    const node = document.createElement('span');
    node.className = `recall-tab-glyph ${tab.status || 'done'}${tab.live ? ' live' : ''}`;
    node.setAttribute('role', 'img');
    node.setAttribute('aria-label', `${tab.live ? 'Live meeting, ' : ''}${labels[tab.status] || labels.done}`);
    node.title = node.getAttribute('aria-label');
    return node;
  }
  function renderMeetingStatus(node, path) {
    const tab = snapshot.tabs.find(tab => tab.meeting_path === path);
    node.replaceChildren();
    node.hidden = !tab;
    if (tab) node.append(glyph(tab));
  }
  function shortcut(event, mac) {
    const modifier = mac ? event.metaKey : event.ctrlKey;
    if (event.ctrlKey && event.key === 'Tab') return { type: 'cycle', direction: event.shiftKey ? -1 : 1 };
    if (!modifier || event.altKey) return null;
    if (/^[1-4]$/.test(event.key)) return { type: 'select', index: Number(event.key) - 1 };
    if (event.key.toLowerCase() === 't') return { type: 'create' };
    if (event.key.toLowerCase() === 'w') return { type: 'close' };
    return null;
  }
  function create({ strip, prompt, invoke, select, create: createTab, close, resume, isRecording }) {
    let model = snapshot;
    const mac = /Mac|iPhone|iPad/.test(navigator.platform);
    const keyHint = index => mac ? `⌘${index + 1}` : `Ctrl+${index + 1}`;
    let chooser = null, pendingLive = null, returnFocus = null;
    function dismiss() { chooser = null; returnFocus = null; prompt.replaceChildren(); prompt.hidden = true; markStrip(); }
    // Cancel puts the keyboard back where the person was typing: the element
    // focused before the prompt opened, else the visible tab's terminal, else
    // the selected tab. Without this, focus falls to the page and keys vanish.
    function cancelPrompt() {
      const target = returnFocus;
      dismiss();
      // A rejected open hides the terminal first, so its opener is often <body>.
      const usable = node => node?.isConnected && node !== document.body && !prompt.contains(node) && node.getClientRects().length > 0;
      const terminal = document.getElementById(`recall-terminal-${model.active}`)?.querySelector('textarea');
      [target, terminal, strip.querySelector('[aria-selected="true"]')].find(usable)?.focus();
    }
    // Tabs that may be closed to make room: finished, never the live meeting.
    const closable = tab => tab.status === 'done' && !tab.live;
    // The part of a meeting title before its subtitle, for a compact button.
    const shortTitle = tab => String(tab.title || '').split(' — ')[0];
    // The suggestion depends on which tab is on screen, not only on the tabs.
    const chooserSignature = () => JSON.stringify({ active: model.active, tabs: model.tabs });
    function markStrip() {
      for (const wrapper of strip.querySelectorAll('.recall-tab')) {
        const tab = model.tabs.find(item => item.id === wrapper.dataset.tabId);
        wrapper.classList.toggle('recall-tab-closable', !!(chooser && tab && closable(tab)));
      }
    }
    function show(text, actions, hint = '') {
      // A refresh of an open prompt keeps the focus target from when it opened.
      const opener = prompt.hidden ? document.activeElement : returnFocus;
      dismiss(); prompt.hidden = false; returnFocus = opener;
      const description = document.createElement('p'); description.textContent = text;
      if (hint) { const extra = document.createElement('span'); extra.className = 'recall-tab-prompt-hint'; extra.textContent = ` ${hint}`; description.append(extra); }
      prompt.append(description);
      for (const [label, action, accessibleName] of actions) {
        const button = document.createElement('button'); button.type = 'button'; button.className = 'btn btn-secondary btn-sm';
        button.textContent = label; button.title = label;
        if (accessibleName) button.setAttribute('aria-label', accessibleName);
        button.addEventListener('click', () => { dismiss(); action(); }); prompt.append(button);
      }
      const cancel = document.createElement('button'); cancel.type = 'button'; cancel.className = 'btn btn-ghost btn-sm';
      cancel.textContent = 'Cancel'; cancel.addEventListener('click', cancelPrompt); prompt.append(cancel);
      cancel.focus();
    }
    // At the cap, offer one finished tab (an inactive one first, so the
    // conversation on screen stays put) and let the strip's highlighted ×
    // buttons pick any other. Closing through the strip continues the open.
    function chooseFinished(action, liveId = null) {
      const finished = model.tabs.filter(closable);
      const suggestion = finished.find(tab => tab.id !== model.active) || finished[0];
      const lead = liveId ? 'Recording. The live meeting needs a tab, and all four are in use.' : 'All four tabs are in use.';
      if (suggestion) {
        const label = `Close ${shortTitle(suggestion)}`;
        show(lead, [[label, () => action(suggestion.id), `${label} and continue`]], finished.length > 1 ? 'Or close another highlighted tab.' : '');
      } else {
        show(liveId ? 'Recording. The live meeting needs a tab, and all four are busy.' : 'All four tabs are busy.', [], 'Close one with its × to continue; it asks first.');
      }
      chooser = { action, liveId, signature: chooserSignature() };
      markStrip();
    }
    prompt.addEventListener('keydown', event => { if (event.key === 'Escape') { event.preventDefault(); cancelPrompt(); } });
    function update(next) {
      snapshot = model = next;
      const focus = strip.contains(document.activeElement) ? document.activeElement?.dataset?.tabId : null;
      strip.replaceChildren();
      strip.hidden = false;
      for (const [index, tab] of model.tabs.entries()) {
        const wrapper = document.createElement('div'); wrapper.className = 'recall-tab'; wrapper.dataset.tabId = tab.id;
        const button = document.createElement('button'); button.type = 'button'; button.setAttribute('role', 'tab');
        button.id = `recall-tab-${tab.id}`; button.dataset.tabId = tab.id;
        button.setAttribute('aria-selected', String(tab.id === model.active)); button.tabIndex = tab.id === model.active ? 0 : -1;
        button.setAttribute('aria-controls', `recall-terminal-${tab.id}`);
        const title = document.createElement('span'); title.className = 'recall-tab-title'; title.textContent = tab.title;
        button.append(glyph(tab), title); button.title = `${tab.title} — ${labels[tab.status] || labels.done} (${keyHint(index)})`;
        button.setAttribute('aria-keyshortcuts', `${mac ? 'Meta' : 'Control'}+${index + 1}`);
        button.addEventListener('click', () => select(tab.id));
        button.addEventListener('keydown', event => {
          if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
          event.preventDefault();
          const index = model.tabs.findIndex(t => t.id === tab.id);
          const target = event.key === 'Home' ? 0 : event.key === 'End' ? model.tabs.length - 1 : (index + (event.key === 'ArrowLeft' ? -1 : 1) + model.tabs.length) % model.tabs.length;
          select(model.tabs[target].id);
        });
        const end = document.createElement('button'); end.type = 'button'; end.className = 'recall-tab-close'; end.textContent = '×';
        end.setAttribute('aria-label', `Close ${tab.title}`);
        end.addEventListener('click', () => {
          // While the cap chooser waits, closing a tab here continues the open it blocked.
          const pending = chooser;
          if (pending && !tab.live) { dismiss(); pending.action(tab.id); } else close(tab.id);
        });
        wrapper.append(button, end); strip.append(wrapper);
      }
      const add = document.createElement('button'); add.type = 'button'; add.className = 'recall-tab-add'; add.textContent = '+';
      add.setAttribute('aria-label', 'New Recall chat'); add.addEventListener('click', createTab); strip.append(add);
      const active = model.tabs.find(tab => tab.id === model.active);
      let resumeButton = strip.parentElement.querySelector('#recall-resume');
      if (!resumeButton && active?.shell && active.claude_session_id && !active.claude_active) {
        resumeButton = document.createElement('button'); resumeButton.id = 'recall-resume';
        resumeButton.type = 'button'; resumeButton.className = 'btn btn-secondary btn-sm';
        resumeButton.textContent = 'Resume previous conversation';
        strip.after(resumeButton);
      }
      if (resumeButton) {
        resumeButton.hidden = !(active?.shell && active.claude_session_id && !active.claude_active);
        resumeButton.onclick = () => resume?.(model.active);
      }
      for (const node of document.querySelectorAll('[data-recall-meeting]')) renderMeetingStatus(node, node.dataset.recallMeeting);
      if (focus) strip.querySelector(`[role="tab"][data-tab-id="${focus}"]`)?.focus();
      strip.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest', inline: 'nearest', behavior: 'instant' });
      if (chooser && chooser.signature !== chooserSignature()) chooseFinished(chooser.action, chooser.liveId);
      markStrip();

    }
    document.addEventListener('keydown', event => {
      const panel = document.getElementById('recall-panel');
      if (!panel?.classList.contains('expanded')) return;
      const action = shortcut(event, /Mac|iPhone|iPad/.test(navigator.platform));
      if (!action || (!model.tabs.length && action.type !== 'create')) return;
      event.preventDefault(); event.stopImmediatePropagation();
      if (action.type === 'select' && model.tabs[action.index]) select(model.tabs[action.index].id);
      if (action.type === 'cycle') {
        const index = model.tabs.findIndex(t => t.id === model.active);
        select(model.tabs[(index + action.direction + model.tabs.length) % model.tabs.length].id);
      }
      if (action.type === 'create') createTab();
      if (action.type === 'close' && model.active) close(model.active);
    }, true);
    return { update,
      confirmClose: (id, action = () => close(id, true)) => show('Close this conversation? Its process will stop and its saved conversation will leave Recall. Your agent keeps its transcript history.', [['Close conversation', action]]),
      chooseFinished,
      syncPendingLive: (id, action) => {
        if (!id) {
          pendingLive = null;
          if (chooser?.liveId) dismiss();
        } else if (id !== pendingLive) {
          pendingLive = id;
          chooseFinished(action, id);
        }
      },
      dismiss,
    };
  }
  global.MinutesRecallTabs = { create, renderMeetingStatus, shortcut };
})(typeof window === 'undefined' ? globalThis : window);
