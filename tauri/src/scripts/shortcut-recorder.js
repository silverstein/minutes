/* Shared key recorder. Suspension is temporary; every exit restores or reports it. */
(() => {
  'use strict';
  let active = null;
  let busy = false;
  let cancelling = null;
  const overlay = document.getElementById('settings-overlay');
  function detach(session) {
    clearTimeout(session.timeout);
    document.removeEventListener('keydown', session.key, true);
    document.removeEventListener('pointerdown', session.outside, true);
    window.removeEventListener('blur', session.blur);
    session.button.setAttribute('aria-pressed', 'false');
    session.button.classList.remove('is-recording');
  }
  async function restore(session) {
    if (session.restoration) return session.restoration;
    session.restoration = (async () => {
      try { await session.suspension; }
      catch (_) {
        // Suspension failed: the prior binding was never released.
        session.update(session.before); session.button.disabled = false;
        return true;
      }
      try {
        if (session.before.enabled) {
          const result = await session.invoke('cmd_set_shortcut', {
            slot: session.slot, enabled: true,
            shortcut: session.before.shortcut, keycode: session.before.keycode,
          });
          session.update(result);
        } else session.update(session.before);
        return true;
      } catch (error) {
        session.update({ ...session.before, enabled: false });
        session.status('Could not restore the shortcut. Enable it again before using it. ' + String(error));
        return false;
      } finally { session.button.disabled = false; }
    })();
    return session.restoration;
  }
  async function cancel() {
    if (cancelling) return cancelling;
    if (!active) return !busy;
    const session = active;
    active = null; session.cancelled = true; busy = true;
    detach(session); session.button.disabled = true;
    cancelling = restore(session).finally(() => { busy = false; cancelling = null; });
    return cancelling;
  }
  function attach(options) {
    const button = options.button;
    button.setAttribute('aria-pressed', 'false');
    button.addEventListener('click', async () => {
      if (busy) return;
      if (active?.button === button) { await cancel(); return; }
      if (!await cancel() || busy) return;
      const session = { ...options, before: { ...options.read() }, cancelled: false };
      active = session;
      button.textContent = 'Preparing shortcut…';
      button.classList.add('is-recording');
      button.setAttribute('aria-pressed', 'true');
      session.suspension = options.invoke('cmd_suspend_shortcut', { slot: options.slot });
      try { await session.suspension; }
      catch (error) {
        if (session.cancelled) return;
        if (active === session) active = null;
        detach(session); session.update(session.before);
        session.status('Could not prepare the shortcut recorder. ' + String(error));
        return;
      }
      if (session.cancelled || active !== session) return;
      button.textContent = 'Press keys · Esc to cancel';
      session.key = async (event) => {
        event.preventDefault(); event.stopImmediatePropagation();
        if (event.key === 'Escape' || event.code === 'Escape') { await cancel(); return; }
        const choice = session.build(event);
        if (!choice) return;
        if (session.standardOnly && [57, 63].includes(choice.keycode)) {
          session.status('Use a key combination with Command, Control or Option for recovery.');
          return;
        }
        detach(session); busy = true; button.disabled = true;
        try {
          const probe = await session.invoke('cmd_probe_shortcut', { keycode: choice.keycode });
          if (session.cancelled || active !== session) return;
          if (!probe.supported) throw new Error('Those keys are unavailable on this platform.');
          // Once the save starts it owns this transition. A later focus change
          // cannot race a second restoration against that save.
          active = null;
          const result = await session.invoke('cmd_set_shortcut', {
            slot: session.slot, enabled: session.before.enabled && !probe.needs_permission,
            shortcut: choice.shortcut, keycode: choice.keycode,
          });
          session.update(result);
          if (probe.needs_permission) session.status('Shortcut saved. Allow Input Monitoring, then enable it.');
        } catch (error) {
          if (active === session) active = null;
          const restored = await restore(session);
          if (restored) session.status('Shortcut was not changed. ' + String(error));
        } finally { busy = false; button.disabled = false; }
      };
      session.outside = (event) => { if (!button.contains(event.target)) cancel(); };
      session.blur = () => cancel();
      document.addEventListener('keydown', session.key, true);
      document.addEventListener('pointerdown', session.outside, true);
      window.addEventListener('blur', session.blur);
      session.timeout = setTimeout(() => cancel(), 30000);
    });
  }
  if (overlay) new MutationObserver(() => {
    if (!active) return;
    const panel = active.button.closest('.settings-tab-panel');
    if (!overlay.classList.contains('active') || (panel && !panel.classList.contains('is-active'))) cancel();
  }).observe(overlay, { subtree: true, attributes: true, attributeFilter: ['class'] });
  window.MinutesShortcutRecorder = { attach, cancel };
})();
