/* Stored offers must be strictly newer under SemVer precedence. */
(() => {
  'use strict';
  const key = 'minutes-pending-update';
  function parse(version) {
    if (typeof version !== 'string') return null;
    const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.exec(version);
    if (!match) return null;
    const pre = match[4]?.split('.') || [];
    if (pre.some(part => /^\d+$/.test(part) && part.length > 1 && part[0] === '0')) return null;
    return { core: match.slice(1, 4), pre };
  }
  const numeric = (a, b) => a.length === b.length ? (a < b ? -1 : a > b ? 1 : 0) : Math.sign(a.length - b.length);
  function compare(a, b) {
    const left = parse(a), right = parse(b);
    if (!left || !right) return null;
    for (let i = 0; i < 3; i++) { const result = numeric(left.core[i], right.core[i]); if (result) return result; }
    if (!left.pre.length || !right.pre.length) return Math.sign(right.pre.length - left.pre.length);
    for (let i = 0; i < Math.max(left.pre.length, right.pre.length); i++) {
      const a = left.pre[i], b = right.pre[i];
      if (a === undefined || b === undefined) return a === undefined ? -1 : 1;
      if (a === b) continue;
      const an = /^\d+$/.test(a), bn = /^\d+$/.test(b);
      if (an && bn) return numeric(a, b);
      if (an !== bn) return an ? -1 : 1;
      return a < b ? -1 : 1;
    }
    return 0;
  }
  async function restore({ storage, app, show }) {
    try {
      const stored = storage.getItem(key);
      if (!stored) return;
      let offer;
      try { offer = JSON.parse(stored); }
      catch (_) { storage.removeItem(key); return; }
      const [name, current] = await Promise.all([app.getName?.() ?? '', app.getVersion()]);
      if (!offer || (typeof name === 'string' && name.includes('Dev')) || compare(offer.version, current) !== 1) {
        storage.removeItem(key); return;
      }
      show(offer.version, offer.body, offer.downloadBytes);
    } catch (_) { /* Unavailable storage/app metadata must not interrupt startup. */ }
  }
  window.MinutesUpdateOffer = Object.freeze({ compare, restore });
})();
