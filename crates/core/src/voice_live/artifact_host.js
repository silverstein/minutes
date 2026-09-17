(() => {
  const token = location.hash.slice(1);
  history.replaceState(null, '', '/');
  const frame = document.querySelector('iframe');
  let current = null;
  const headers = { 'X-Minutes-Token': token };
  function receipt(id, original) {
    const encode = result => new TextEncoder().encode(JSON.stringify({ id, result }));
    let result = original;
    let bytes = encode(result);
    if (bytes.length <= 32000) return bytes;
    result = { ...original, transport_shortened: true };
    if (Array.isArray(result.controls)) {
      result.controls = result.controls.slice();
      while (result.controls.length > 1 && encode(result).length > 32000) result.controls.pop();
    }
    if (encode(result).length > 32000) {
      result.output = '';
      result.output_omitted = true;
    }
    if (encode(result).length > 32000) {
      result = { snapshot_id: original?.snapshot_id, undo_id: original?.undo_id,
        changed: original?.changed, details_omitted: true,
        note: 'Receipt details exceeded the transport budget. Inspect a specific control; do not repeat a completed edit.' };
    }
    bytes = encode(result);
    return bytes.length <= 32000 ? bytes : encode({ error: 'Oversized preview receipt. An edit may have applied; inspect before retrying.' });
  }
  addEventListener('message', async event => {
    if (event.source !== frame.contentWindow || event.data?.kind !== 'minutes-control-result' || !current || event.data.id !== current) return;
    const bytes = receipt(current, event.data.result);
    current = null;
    const encoded = btoa(String.fromCharCode(...bytes));
    try { await fetch('/result', { method: 'POST', headers: { ...headers, 'X-Minutes-Result': encoded }, body: '' }); } catch (_) {}
  });
  async function poll() {
    try {
      const response = await fetch('/command', { headers });
      const request = response.ok ? await response.json() : null;
      if (request) {
        current = request.id;
        frame.contentWindow.postMessage({ kind: 'minutes-control', ...request }, '*');
      }
    } catch (_) {}
    setTimeout(poll, 150);
  }
  frame.addEventListener('load', poll, { once: true });
})();
