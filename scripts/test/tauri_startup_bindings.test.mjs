import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const indexHtml = new URL('../../tauri/src/index.html', import.meta.url);

test('desktop startup binds every bare Tauri event listener', async () => {
  const source = await readFile(indexHtml, 'utf8');
  const usesBareListen = /(^|[^.\w])listen\s*\(/m.test(source);

  if (usesBareListen) {
    assert.match(
      source,
      /const\s*{[^}]*\blisten\b[^}]*}\s*=\s*window\.__TAURI__\.event\s*;/,
      'a bare listen(...) call crashes startup unless listen is bound from window.__TAURI__.event',
    );
  }
});
