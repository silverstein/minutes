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
  function create({ strip, prompt, invoke, select, create: createTab, close, isRecording }) {
    let model = snapshot;
    function dismiss() { prompt.replaceChildren(); prompt.hidden = true; }
    function show(text, actions) {
      dismiss(); prompt.hidden = false;
      const description = document.createElement('p'); description.textContent = text; prompt.append(description);
      for (const [label, action] of actions) {
        const button = document.createElement('button'); button.type = 'button'; button.className = 'btn btn-secondary btn-sm';
        button.textContent = label; button.addEventListener('click', () => { dismiss(); action(); }); prompt.append(button);
      }
      const cancel = document.createElement('button'); cancel.type = 'button'; cancel.className = 'btn btn-ghost btn-sm';
      cancel.textContent = 'Cancel'; cancel.addEventListener('click', dismiss); prompt.append(cancel);
      prompt.querySelector('button')?.focus();
    }
    prompt.addEventListener('keydown', event => { if (event.key === 'Escape') { event.preventDefault(); dismiss(); strip.querySelector('[aria-selected="true"]')?.focus(); } });
    function update(next) {
      const previous = model;
      snapshot = model = next;
      const focus = strip.contains(document.activeElement) ? document.activeElement?.dataset?.tabId : null;
      strip.replaceChildren();
      strip.hidden = false;
      for (const tab of model.tabs) {
        const wrapper = document.createElement('div'); wrapper.className = 'recall-tab';
        const button = document.createElement('button'); button.type = 'button'; button.setAttribute('role', 'tab');
        button.id = `recall-tab-${tab.id}`; button.dataset.tabId = tab.id;
        button.setAttribute('aria-selected', String(tab.id === model.active)); button.tabIndex = tab.id === model.active ? 0 : -1;
        button.setAttribute('aria-controls', `recall-terminal-${tab.id}`);
        const title = document.createElement('span'); title.className = 'recall-tab-title'; title.textContent = tab.title;
        button.append(glyph(tab), title); button.title = `${tab.title} — ${labels[tab.status] || labels.done}`;
        button.addEventListener('click', () => select(tab.id));
        button.addEventListener('keydown', event => {
          if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
          event.preventDefault();
          const index = model.tabs.findIndex(t => t.id === tab.id);
          const target = event.key === 'Home' ? 0 : event.key === 'End' ? model.tabs.length - 1 : (index + (event.key === 'ArrowLeft' ? -1 : 1) + model.tabs.length) % model.tabs.length;
          select(model.tabs[target].id);
        });
        const end = document.createElement('button'); end.type = 'button'; end.className = 'recall-tab-close'; end.textContent = '×';
        end.setAttribute('aria-label', `Close ${tab.title}`); end.addEventListener('click', () => close(tab.id));
        wrapper.append(button, end); strip.append(wrapper);
      }
      const add = document.createElement('button'); add.type = 'button'; add.className = 'recall-tab-add'; add.textContent = '+';
      add.setAttribute('aria-label', 'New Recall chat'); add.addEventListener('click', createTab); strip.append(add);
      for (const node of document.querySelectorAll('[data-recall-meeting]')) renderMeetingStatus(node, node.dataset.recallMeeting);
      if (focus) strip.querySelector(`[data-tab-id="${focus}"]`)?.focus();
      strip.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest', inline: 'nearest', behavior: 'instant' });

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
      confirmClose: id => show('Close this conversation? Its assistant is still working or following the live meeting.', [['Close conversation', () => close(id, true)]]),
      chooseFinished: action => {
        const finished = model.tabs.filter(t => t.status === 'done' && !t.live);
        show(finished.length ? 'Four conversations are open. Close a finished one to continue.' : 'Four conversations are open. Wait for one to finish, or close a tab first.', finished.map(t => [`Close ${t.title}`, () => action(t.id)]));
      },
      dismiss,
    };
  }
  global.MinutesRecallTabs = { create, renderMeetingStatus, shortcut };
})(typeof window === 'undefined' ? globalThis : window);
