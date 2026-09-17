(() => {
  const instance = crypto.randomUUID();
  const ids = new WeakMap();
  let next = 0, revision = 0, signature = '', undo = [];
  const text = value => String(value || '').trim().slice(0, 180);
  function elements() {
    return [...document.querySelectorAll('input[type="range"],input[type="number"],input[type="checkbox"],select')]
      .filter(el => !el.disabled && !el.readOnly && el.getClientRects().length && getComputedStyle(el).visibility !== 'hidden')
      .slice(0, 64);
  }
  function value(el) { return el.type === 'checkbox' ? String(el.checked) : el.value; }
  function controls() {
    return elements().map(el => {
      if (!ids.has(el)) ids.set(el, `control-${++next}`);
      const labelled = (el.getAttribute('aria-labelledby') || '').split(/\s+/).map(id => document.getElementById(id)?.textContent || '').join(' ');
      return { control_id: ids.get(el), label: text(el.getAttribute('aria-label') || labelled || [...el.labels || []].map(l => l.textContent).join(' ') || el.name || el.id),
        type: el.tagName === 'SELECT' ? 'select' : el.type, value: value(el), min: el.min || '', max: el.max || '', step: el.step || '',
        options: el.tagName === 'SELECT' ? [...el.options].slice(0, 64).map(o => ({ value: o.value, label: text(o.label), disabled: o.disabled })) : undefined };
    });
  }
  function state() {
    const list = controls();
    const output = [...document.querySelectorAll('output,[role="status"],[aria-live="polite"]')].slice(0, 12).map(el => text(el.textContent)).join('\n');
    const sig = JSON.stringify([list, output]);
    if (sig !== signature) { signature = sig; revision++; }
    return { snapshot_id: `${instance}:${revision}`, controls: list, output, undo_id: undo.at(-1)?.id || null };
  }
  function validate(el, raw) {
    if (typeof raw !== 'string' || raw.length > 256) throw Error('Use the exact string value for this control');
    if (el.tagName === 'SELECT') {
      if (el.multiple || ![...el.options].some(o => !o.disabled && o.value === raw)) throw Error('Unsupported or unavailable option');
      return raw;
    }
    if (el.type === 'checkbox') {
      if (!['true', 'false'].includes(raw)) throw Error('Checkbox value must be true or false');
      return raw;
    }
    if (!raw.trim() || !Number.isFinite(Number(raw))) throw Error('Value must be a finite number');
    const n = Number(raw), min = el.min === '' ? (el.type === 'range' ? 0 : -Infinity) : Number(el.min);
    const max = el.max === '' ? (el.type === 'range' ? 100 : Infinity) : Number(el.max);
    if (n < min || n > max) throw Error('Value is outside this control range');
    const step = el.step === 'any' ? 0 : Number(el.step || 1);
    const base = el.min !== '' ? Number(el.min) : Number(el.getAttribute('value') || 0);
    if (step && Math.abs((n - base) / step - Math.round((n - base) / step)) > 1e-7) throw Error('Value does not match this control step');
    return String(n);
  }
  async function write(el, raw) {
    const property = el.type === 'checkbox' ? 'checked' : 'value';
    const prototype = el.tagName === 'SELECT' ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, property).set.call(el, property === 'checked' ? raw === 'true' : raw);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(resolve => setTimeout(resolve, 80));
    if (!el.isConnected || value(el) !== raw) throw Error('Control did not retain the requested value; inspect before retrying');
  }
  let busy = false;
  addEventListener('message', async event => {
    if (event.source !== parent || event.data?.kind !== 'minutes-control' || busy) return;
    busy = true;
    const { id, command } = event.data;
    let result;
    try {
      const before = state();
      if (command.operation === 'inspect') result = before;
      else {
        if (command.snapshot_id !== before.snapshot_id) throw Error('Preview changed; inspect again before editing');
        let controlId = command.control_id, raw = command.value, change;
        if (command.operation === 'undo') {
          change = undo.at(-1);
          if (!change || command.undo_id !== change.id || before.snapshot_id !== change.after) throw Error('Undo conflicts with a later edit');
          controlId = change.control; raw = change.before;
        } else if (command.operation !== 'set') throw Error('Unsupported command');
        const el = elements().find(el => ids.get(el) === controlId);
        if (!el) throw Error('Control is no longer available');
        const old = value(el), desired = validate(el, raw);
        await write(el, desired);
        result = state();
        if (change) undo.pop();
        else {
          undo.push({ id: crypto.randomUUID(), control: controlId, before: old, after: result.snapshot_id });
          if (undo.length > 32) undo.shift();
        }
        result.undo_id = undo.at(-1)?.id || null;
        result.changed = { control_id: controlId, before: old, after: value(el) };
      }
    } catch (error) { result = { error: String(error.message || error).slice(0, 240) }; }
    parent.postMessage({ kind: 'minutes-control-result', id, result }, '*');
    busy = false;
  });
})();
