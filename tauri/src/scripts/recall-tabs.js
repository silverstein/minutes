/* Recall tab chrome. Metadata from native hook files is the sole status source. */
(function (global) {
  'use strict';
  let snapshot = { tabs: [], active: null };
  const labels = { working: 'Working', waiting: 'Waiting for you', done: 'Done', error: 'Error' };
  function glyph(tab) {
    const node = document.createElement('span');
    node.className = `recall-tab-glyph ${tab.status || 'done'}${tab.live ? ' live' : ''}`;
    node.setAttribute('role', 'img');
    node.setAttribute('aria-label', `${tab.herdr_terminal ? 'In Herdr' : `${tab.live ? 'Live meeting, ' : ''}${labels[tab.status] || labels.done}`}`);
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
  function create({ strip, prompt, invoke, select, create: createTab, close, endButton, resume, isRecording, openInHerdr, takeBack, copyAddress }) {
    let model = snapshot;
    let herdrAvailable = false;
    const handoff = document.createElement('div');
    handoff.id = 'recall-herdr-state'; handoff.className = 'recall-tab-prompt'; handoff.hidden = true;
    handoff.setAttribute('role', 'status'); prompt.after(handoff);
    const mac = /Mac|iPhone|iPad/.test(navigator.platform);
    const keyHint = index => mac ? `⌘${index + 1}` : `Ctrl+${index + 1}`;
    let chooser = null, pendingLive = null, returnFocus = null, pendingClose = null;
    let recallEndArmTimer = null, closing = false;
    const closeDescription = document.createElement('span');
    closeDescription.id = 'recall-close-description';
    closeDescription.textContent = 'Its process stops and its saved conversation leaves Recall. Your agent keeps the transcript.';
    closeDescription.hidden = true;
    prompt.after(closeDescription);
    const tabCloseButton = id => document.getElementById(`recall-close-${id}`);
    function revealActiveTab() {
      if (strip.hidden || !strip.clientWidth) return;
      const armedButton = pendingClose?.button;
      const selected = strip.querySelector('[aria-selected="true"]');
      const control = strip.contains(armedButton) ? armedButton : selected;
      const tab = control?.closest('.recall-tab');
      const target = tab?.offsetWidth > strip.clientWidth ? control : tab;
      target?.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'instant' });
    }
    if (typeof ResizeObserver !== 'undefined') new ResizeObserver(revealActiveTab).observe(strip);

    function showCloseConfirmation(attempt) {
      const { button, id } = attempt;
      const tab = model.tabs.find(tab => tab.id === id);
      attempt.label = button.textContent;
      attempt.name = button.getAttribute('aria-label');
      attempt.title = button.getAttribute('title');
      const isEnd = button === endButton;
      button.textContent = isEnd ? 'End Session?' : 'Close?';
      button.setAttribute('aria-label', `Confirm ${isEnd ? 'End Session' : 'close'} for ${tab.title}`);
      button.setAttribute('aria-describedby', closeDescription.id);
      const shortcutHint = tab.live ? '' : ` Press ${mac ? '⌘W' : 'Ctrl+W'} again to close the selected tab.`;
      button.title = closeDescription.textContent + (id === model.active ? shortcutHint : '');
      button.dataset.closeArmed = '';
    }

    function cancelClose(restoreFocus = false) {
      const attempt = pendingClose;
      pendingClose = null;
      clearTimeout(recallEndArmTimer);
      recallEndArmTimer = null;
      if (!attempt) return;
      const { button, opener, label, name, title } = attempt;
      if (button?.hasAttribute('data-close-armed')) {
        button.textContent = label;
        for (const [attribute, value] of [['aria-label', name], ['title', title]]) {
          if (value === null) button.removeAttribute(attribute);
          else button.setAttribute(attribute, value);
        }
        button.removeAttribute('aria-describedby');
        delete button.dataset.closeArmed;
      }
      if (restoreFocus) (opener?.isConnected ? opener : button)?.focus();
    }

    async function finishClose(attempt, confirmed) {
      if (confirmed) {
        cancelClose();
        if (attempt.chooser && chooser === attempt.chooser) dismiss();
      }
      closing = true;
      let result;
      try {
        result = await close(attempt.id, confirmed);
      } finally {
        closing = false;
      }
      if (result === 'confirm' && pendingClose === attempt && attempt.button?.isConnected) {
        showCloseConfirmation(attempt);
        attempt.button.focus({ preventScroll: true });
        revealActiveTab();
        if (attempt.button === endButton) recallEndArmTimer = setTimeout(cancelClose, 4000);
        return;
      }
      if (pendingClose === attempt) cancelClose();
      if (result === true && attempt.chooser) {
        if (chooser === attempt.chooser) dismiss();
        await attempt.chooser.afterClose(attempt.id);
      }
    }

    function requestClose(id, button, cap = null, opener = button) {
      if (closing) return;
      if (pendingClose?.id === id && pendingClose.button === button) {
        if (button?.hasAttribute('data-close-armed')) return finishClose(pendingClose, true);
        return;
      }
      cancelClose();
      if (!id || !button) return;
      const attempt = { id, button, opener, chooser: cap };
      pendingClose = attempt;
      return finishClose(attempt, false);
    }

    function closeFromShortcut() {
      if (closing) return;
      const active = model.tabs.find(tab => tab.id === model.active);
      if (!active) return;
      if (pendingClose?.id === active.id) {
        if (!active.live && pendingClose.button?.hasAttribute('data-close-armed')) return finishClose(pendingClose, true);
        return;
      }
      return requestClose(active.id, tabCloseButton(active.id), null, document.activeElement);
    }

    endButton?.addEventListener('click', () => requestClose(model.active, endButton));
    document.addEventListener('pointerdown', event => {
      if (pendingClose && !pendingClose.button?.contains(event.target)) cancelClose();
    }, true);
    document.addEventListener('focusout', event => {
      if (pendingClose?.button === event.target
        || (pendingClose?.opener === event.target && event.relatedTarget !== pendingClose.button)) cancelClose();
    });
    global.addEventListener('blur', () => cancelClose());

    function dismiss() { chooser = null; cancelClose(); returnFocus = null; prompt.replaceChildren(); prompt.hidden = true; markStrip(); }
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
    function show(text, actions, hint = '', keepOpen = false) {
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
        button.addEventListener('click', () => { if (!keepOpen) dismiss(); action(button); }); prompt.append(button);
      }
      const cancel = document.createElement('button'); cancel.type = 'button'; cancel.className = 'btn btn-ghost btn-sm';
      cancel.textContent = 'Cancel'; cancel.addEventListener('click', cancelPrompt); prompt.append(cancel);
      cancel.focus();
    }
    // At the cap, offer one finished tab (an inactive one first, so the
    // conversation on screen stays put) and let the strip's highlighted ×
    // buttons pick any other. Closing through the strip continues the open.
    function chooseFinished(afterClose, liveId = null) {
      const finished = model.tabs.filter(closable);
      const suggestion = finished.find(tab => tab.id !== model.active) || finished[0];
      const lead = liveId ? 'Recording. The live meeting needs a tab, and all four are in use.' : 'All four tabs are in use.';
      if (suggestion) {
        const label = `Close ${shortTitle(suggestion)}`;
        show(lead, [[label, button => requestClose(suggestion.id, button, chooser), `${label} and continue`]], finished.length > 1 ? 'Or close another highlighted tab.' : '', true);
      } else {
        show(liveId ? 'Recording. The live meeting needs a tab, and all four are busy.' : 'All four tabs are busy.', [], 'Close one with its × to continue; it asks first.');
      }
      chooser = { afterClose, liveId, signature: chooserSignature() };
      markStrip();
    }
    prompt.addEventListener('keydown', event => { if (event.key === 'Escape') { event.preventDefault(); cancelPrompt(); } });
    function update(next) {
      const previousActive = model.active;
      snapshot = model = next;
      if (pendingClose) {
        const tab = next.tabs.find(tab => tab.id === pendingClose.id);
        if (previousActive !== next.active || !tab || (pendingClose.chooser && tab.live)) cancelClose();
      }
      const focused = document.activeElement;
      const focus = strip.contains(focused) ? focused.dataset.tabId : null;
      const closeFocus = strip.contains(focused) ? focused.dataset.closeTab : null;
      const rebuildClose = pendingClose && strip.contains(pendingClose.button) ? pendingClose : null;
      const wasArmed = rebuildClose?.button.hasAttribute('data-close-armed');
      const replaceOpener = rebuildClose && rebuildClose.opener === rebuildClose.button;
      // Rebuilding the strip must not interpret removal of its old button as blur.
      if (rebuildClose) rebuildClose.button = null;
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
        end.id = `recall-close-${tab.id}`; end.dataset.closeTab = tab.id;
        end.setAttribute('aria-label', `Close ${tab.title}`);
        end.addEventListener('click', () => requestClose(tab.id, end, tab.live ? null : chooser));
        wrapper.append(button);
        if (herdrAvailable || tab.herdr_terminal) {
          const menu = document.createElement('button'); menu.type = 'button'; menu.className = 'recall-tab-close'; menu.textContent = '⋯';
          menu.setAttribute('aria-label', `Conversation actions for ${tab.title}`);
          menu.addEventListener('click', () => show(`Actions for ${shortTitle(tab)}`, tab.herdr_terminal
            ? [['Take back', () => takeBack?.(tab.id)]]
            : [['Open in Herdr', () => confirmOpen(tab.id)]]));
          wrapper.append(menu);
        }
        wrapper.append(end); strip.append(wrapper);
      }
      const add = document.createElement('button'); add.type = 'button'; add.className = 'recall-tab-add'; add.textContent = '+';
      add.setAttribute('aria-label', 'New Recall chat'); add.addEventListener('click', createTab); strip.append(add);
      const active = model.tabs.find(tab => tab.id === model.active);
      handoff.hidden = !active?.herdr_terminal;
      const owner = active?.herdr_terminal ? `${active.id}:${active.herdr_terminal}` : '';
      if (handoff.dataset.owner !== owner) {
        handoff.dataset.owner = owner; handoff.replaceChildren();
        if (active?.herdr_terminal) {
          const description = document.createElement('p'); description.textContent = `In Herdr · ${active.herdr_terminal}`; handoff.append(description);
          for (const [label, action] of [
            ['Copy address', () => copyAddress?.(active.herdr_terminal)],
            ['Open terminal', () => copyAddress?.(`hc attach ${active.herdr_terminal}`, 'Paste this command in a terminal to open the conversation.')],
            ['Take back', () => takeBack?.(active.id)],
          ]) {
            const button = document.createElement('button'); button.type = 'button'; button.className = 'btn btn-secondary btn-sm';
            button.textContent = label; button.addEventListener('click', action); handoff.append(button);
          }
        }
      }
      let resumeButton = strip.parentElement.querySelector('#recall-resume');
      if (!resumeButton && active?.shell && !active.herdr_terminal && active.claude_session_id && !active.claude_active) {
        // A compact action beside Weekly and End Session, not a row of its own.
        resumeButton = document.createElement('button'); resumeButton.id = 'recall-resume';
        resumeButton.type = 'button'; resumeButton.className = 'btn btn-primary btn-sm';
        resumeButton.textContent = 'Resume';
        resumeButton.setAttribute('aria-label', 'Resume previous conversation');
        resumeButton.title = 'Resume this tab\'s previous Claude conversation';
        const actions = strip.parentElement.querySelector('.recall-header-actions');
        if (actions) actions.prepend(resumeButton); else strip.after(resumeButton);
      }
      if (resumeButton) {
        resumeButton.hidden = !(active?.shell && !active.herdr_terminal && active.claude_session_id && !active.claude_active);
        resumeButton.onclick = () => resume?.(model.active);
      }
      for (const node of document.querySelectorAll('[data-recall-meeting]')) renderMeetingStatus(node, node.dataset.recallMeeting);
      if (rebuildClose) {
        rebuildClose.button = tabCloseButton(rebuildClose.id);
        if (replaceOpener) rebuildClose.opener = rebuildClose.button;
        if (wasArmed) showCloseConfirmation(rebuildClose);
      }
      if (closeFocus) tabCloseButton(closeFocus)?.focus();
      else if (focus) document.getElementById(`recall-tab-${focus}`)?.focus();
      revealActiveTab();
      if (!closing && !pendingClose && chooser && chooser.signature !== chooserSignature()) chooseFinished(chooser.afterClose, chooser.liveId);
      markStrip();

    }
    document.addEventListener('keydown', event => {
      const panel = document.getElementById('recall-panel');
      if (!panel?.classList.contains('expanded')) return;
      if (pendingClose && (event.key === 'Escape' || (event.repeat && event.key === 'Enter' && event.target === pendingClose.button))) {
        event.preventDefault(); event.stopImmediatePropagation();
        if (event.key === 'Escape') cancelClose(true);
        return;
      }
      const action = shortcut(event, mac);
      if (!action || (!model.tabs.length && action.type !== 'create')) return;
      event.preventDefault(); event.stopImmediatePropagation();
      if (action.type === 'select' && model.tabs[action.index]) select(model.tabs[action.index].id);
      if (action.type === 'cycle') {
        const index = model.tabs.findIndex(t => t.id === model.active);
        select(model.tabs[(index + action.direction + model.tabs.length) % model.tabs.length].id);
      }
      if (action.type === 'create') createTab();
      if (action.type === 'close' && !event.repeat) closeFromShortcut();
    }, true);
    function confirmOpen(id) {
      if (!herdrAvailable || model.tabs.find(t => t.id === id)?.herdr_terminal) return;
      if (model.tabs.find(t => t.id === id)?.status === 'working') {
        show('Wait for this reply to finish, then move it to Herdr', []); return;
      }
      show('Move this conversation to Herdr? Minutes will stop its copy so the conversation continues there.', [['Open in Herdr', () => openInHerdr?.(id)]]);
    }
    return { update,
      setHerdrAvailable: available => { herdrAvailable = !!available; update(model); },
      confirmOpen,
      confirmMissing: id => show("Herdr has no record of this terminal. If it isn't running anywhere, take the conversation back?", [['Take conversation back', () => takeBack?.(id, false, true)]]),
      confirmTakeBack: id => show('End this Herdr terminal and take the conversation back? Minutes will wait for it to exit before resuming.', [['End terminal and take back', () => takeBack?.(id, true)]]),
      closeFromShortcut,
      cancelClose,
      chooseFinished,
      syncPendingLive: (id, afterClose) => {
        if (!id) {
          pendingLive = null;
          if (chooser?.liveId) dismiss();
        } else if (id !== pendingLive) {
          pendingLive = id;
          chooseFinished(afterClose, id);
        }
      },
      dismiss,
    };
  }
  global.MinutesRecallTabs = { create, renderMeetingStatus, shortcut };
})(typeof window === 'undefined' ? globalThis : window);
