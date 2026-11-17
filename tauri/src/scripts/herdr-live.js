/* Optional local destination picker. Capture and Stop never await this controller. */
(() => {
  function create({ dialog, pickButtons, cancelButtons, notices, invoke }) {
    const list = dialog.querySelector('[data-herdr-terminals]');
    const message = dialog.querySelector('[data-herdr-message]');
    const dismiss = dialog.querySelector('[data-herdr-dismiss]');
    let available = false, model = { active: false }, refreshing = null;
    let opening = 0, origin = null, pending = false, request = 0, localError = '';
    const local = address => typeof address === 'string' && address.length > 0 && !address.includes('/') && /^[a-zA-Z0-9_-]+$/.test(address);
    function render() {
      const selected = !!model.selected || pending;
      pickButtons.forEach(button => { button.hidden = !(available && model.active && !selected); });
      cancelButtons.forEach(button => { button.hidden = !(model.active && selected); });
      const text = localError || model.notice || (selected ? model.phase === 'sent'
        ? `Live context sent to ${model.selected.name || model.selected.address}.`
        : model.phase === 'failed' ? 'Live context could not be sent. Cancel the pick to try again.'
          : 'Sending live context to the picked terminal…' : '');
      notices.forEach(node => { node.textContent = text; node.hidden = !text || !model.active; node.classList.toggle('is-error', !!localError || !!model.notice || model.phase === 'failed'); });
    }
    function close() {
      opening++;
      if (dialog.open) dialog.close();
    }
    dialog.addEventListener('close', () => { opening++; if (origin?.isConnected && !origin.hidden) origin.focus(); });
    dismiss.addEventListener('click', close);
    async function refresh() {
      if (refreshing) return refreshing;
      refreshing = (async () => {
        try {
          const next = await invoke('cmd_herdr_live_view');
          if (!next || typeof next.active !== 'boolean') return;
          const changed = model.session && model.session !== next.session;
          model = next;
          if (changed || !model.active) { pending = false; localError = ''; request++; close(); }
          render();
        } catch (_) { /* Optional status must not disturb capture status polling. */ }
        finally { refreshing = null; }
      })();
      return refreshing;
    }
    async function cancel() {
      request++; pending = false; localError = ''; close();
      await invoke('cmd_herdr_live_cancel').catch(error => { localError = String(error); });
      await refresh();
      render();
      pickButtons.find(button => !button.hidden && button.getClientRects().length)?.focus();
    }
    cancelButtons.forEach(button => button.addEventListener('click', cancel));
    async function open(button) {
      await refresh();
      if (!available || !model.active || model.selected || pending) return;
      origin = button || document.activeElement;
      const token = ++opening;
      list.replaceChildren();
      message.textContent = 'Loading local live terminals…';
      dialog.showModal(); dismiss.focus();
      try {
        const options = await invoke('cmd_herdr_live_options');
        if (token !== opening || !dialog.open) return;
        if (options.session !== model.session) { close(); await refresh(); return; }
        const rows = (options.terminals || []).filter(row => local(row.address));
        message.textContent = rows.length ? 'Send a live transcript pointer only to the terminal you choose. Local terminals only.' : 'No local live terminals are available.';
        rows.forEach(row => {
          const entry = document.createElement('button');
          entry.type = 'button'; entry.className = 'herdr-live-terminal';
          const title = document.createElement('span'); title.textContent = `${row.name || row.address} · ${row.address}`;
          const labels = document.createElement('small');
          const tags = Object.entries(row.labels || {}).map(([key, value]) => `${key}=${value}`);
          if (row.labels?.['minutes.tab']) tags.unshift('Recall conversation');
          labels.textContent = tags.join(' · ');
          entry.append(title, labels);
          entry.addEventListener('click', async () => {
            const selectedRequest = ++request;
            pending = true; localError = ''; close(); render();
            try {
              await invoke('cmd_herdr_live_pick', { session: options.session, generation: options.generation, target: row.address });
            } catch (error) {
              if (selectedRequest === request) {
                localError = String(error);
              }
            } finally {
              if (selectedRequest === request) {
                pending = false; await refresh(); render();
                cancelButtons.find(button => !button.hidden && button.getClientRects().length)?.focus();
              }
            }
          });
          list.append(entry);
        });
      } catch (error) {
        if (token === opening && dialog.open) message.textContent = String(error);
      }
    }
    pickButtons.forEach(button => button.addEventListener('click', () => { void open(button); }));
    return { refresh, open, cancel, setAvailable(value) { available = !!value; render(); } };
  }
  window.MinutesHerdrLive = { create };
})();
