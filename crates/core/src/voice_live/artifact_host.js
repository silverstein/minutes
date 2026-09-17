(() => {
  const token = location.hash.slice(1);
  history.replaceState(null, '', '/');
  const frame = document.querySelector('iframe');
  let current = null;
  const headers = { 'X-Minutes-Token': token };
  addEventListener('message', async event => {
    if (event.source !== frame.contentWindow || event.data?.kind !== 'minutes-control-result' || !current || event.data.id !== current) return;
    const bytes = new TextEncoder().encode(JSON.stringify({ id: current, result: event.data.result }));
    current = null;
    if (bytes.length > 34000) return;
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
