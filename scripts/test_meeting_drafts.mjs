import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const panel = readFileSync(new URL('../tauri/src/index.html', import.meta.url), 'utf8');
// Compile functions from the shipped frontend, including their real defaults.
function extract(name) {
  const start = panel.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert.notEqual(start, -1, `Missing ${name}`);
  const lines = panel.slice(start).split('\n');
  for (let end = 1; end <= lines.length; end++) {
    const source = lines.slice(0, end).join('\n');
    try { new vm.Script(`(${source})`); return source; } catch {}
  }
  throw new Error(`Could not compile ${name}`);
}
function context(extra = {}) {
  const ctx = vm.createContext({ ...extra });
  vm.runInContext(readFileSync(new URL('../tauri/src/vendor/marked.min.js', import.meta.url), 'utf8'), ctx);
  for (const name of ['splitFrontmatter', 'artifactDisplayTitle', 'buildFrontmatterElement', 'buildSourceOffsetMap', 'escapeHtmlForMarkdown', 'annotateBlocksWithSource']) {
    vm.runInContext(extract(name), ctx);
  }
  vm.runInContext("const INVISIBLE_MARKDOWN_TOKENS = new Set(['space', 'def']);", ctx);
  return ctx;
}
class Element {
  children = []; dataset = {}; textContent = ''; className = ''; open = false;
  classList = { add: () => {} };
  constructor(tag) { this.tagName = tag; }
  append(...nodes) { this.children.push(...nodes); }
  appendChild(node) { this.children.push(node); }
  replaceChildren(...nodes) { this.children = [...nodes]; }
  addEventListener() {}
}

test('draft title is readable while other documents retain their filename', () => {
  const ctx = context();
  assert.equal(ctx.artifactDisplayTitle('---\ntitle: Follow-up — Pricing: annual billing\nartifact_type: follow-up-email\n---\nBody', 'long-file.md'), 'Follow-up — Pricing: annual billing');
  assert.equal(ctx.artifactDisplayTitle('---\ntitle: "Quoted \\"title\\""\nartifact_type: memo\n---\n', 'file.md'), 'Quoted "title"');
  assert.equal(ctx.artifactDisplayTitle('---\ntitle: Other document\n---\n', 'file.md'), 'file.md');
  assert.equal(ctx.artifactDisplayTitle('---\ntitle: >\n  Multiline\nartifact_type: memo\n---\n', 'file.md'), 'file.md');
});

test('metadata is initially collapsed and arbitrary values stay text', () => {
  const ctx = context();
  const details = ctx.buildFrontmatterElement({ createElement: tag => new Element(tag) }, 'source_meeting: /meetings/test.md\ntitle: <img src=x onerror=alert(1)>');
  assert.equal(details.tagName, 'details');
  assert.equal(details.open, false);
  assert.equal(details.children[0].textContent, 'Source and details');
  assert.equal(details.children[1].children[3].textContent, '<img src=x onerror=alert(1)>');
});

for (const newline of ['\n', '\r\n']) {
  test(`editing a rendered block preserves metadata and other blocks (${JSON.stringify(newline)})`, () => {
    const original = ['---', 'title: Follow-up — Pricing', 'artifact_type: follow-up-email', 'source_meeting: /meetings/test.md', '---', '', '# Email', '', 'Keep annual & quarterly <plans>.', '', 'Next paragraph.', ''].join(newline);
    const ctx = context();
    const split = ctx.splitFrontmatter(original);
    const tokens = ctx.marked.lexer(ctx.escapeHtmlForMarkdown(split.body));
    const blocks = tokens.filter(t => !['space', 'def'].includes(t.type)).map(() => new Element('p'));
    assert.equal(ctx.annotateBlocksWithSource({ children: blocks }, tokens, split.body, split.bodyStart), true);
    const block = blocks[1];
    const start = Number(block.dataset.srcStart), end = Number(block.dataset.srcEnd);
    assert.match(original.slice(start, end), /^Keep annual & quarterly <plans>\./);
    const raw = original.slice(start, end);
    const trailing = raw.match(/\s*$/)[0];
    Object.assign(ctx, {
      activeBlockEdit: { editor: { value: 'Keep annual billing.' }, original: raw.trimEnd(), path: '/draft.md', sourceSnapshot: original, start, end, trailing },
      openArtifactPath: '/draft.md', openArtifactContent: original, mdViewerEditor: { value: original },
      currentArtifactBuffer: () => original, rerenderOpenArtifact: () => {}, refreshMdViewerChrome: () => {}, scheduleOpenArtifactSave: () => {},
    });
    vm.runInContext(extract('commitInlineBlockEditor'), ctx);
    ctx.commitInlineBlockEditor();
    assert.equal(ctx.mdViewerEditor.value, original.replace('Keep annual & quarterly <plans>.', 'Keep annual billing.'));
    assert.equal(ctx.openArtifactDirty, true);
  });
}

test('creating a draft opens an editable preview and cancellation creates no file', async () => {
  const calls = [], opened = [];
  let kind = 'follow-up-email';
  const ctx = context({
    mdViewer: { classList: { contains: () => false } },
    requestArtifactTemplateKind: async () => kind,
    invoke: async (command, args) => { calls.push({ command, args }); return { path: '/draft.md', content: 'Body' }; },
    showMdViewer: async (...args) => { opened.push(args); return true; },
  });
  vm.runInContext(extract('createArtifactFromMeetingPath'), ctx);
  await ctx.createArtifactFromMeetingPath('/meeting.md');
  assert.equal(calls[0].args.meetingPath, '/meeting.md');
  assert.equal(opened[0][3].mode, 'preview');
  assert.equal(opened[0][3].editable, true);
  kind = null;
  await ctx.createArtifactFromMeetingPath('/meeting.md');
  assert.equal(calls.length, 1);
});


test('completed meetings lead with decisions and next steps without losing speaker controls', () => {
  const scroll = new Element('div');
  const ctx = context({
    document: { createElement: tag => new Element(tag) },
    detailTitle: new Element('div'), detailMeta: new Element('div'), detailScroll: scroll,
    detailOverlay: new Element('div'), activeSearchQuery: '',
    formatDetailDate: date => date,
    setHighlightedText: (element, text) => { element.textContent = text; },
    persistRecallWorkspaceState: () => {},
  });
  vm.runInContext(extract('renderDetailSections'), ctx);
  ctx.renderDetailSections({
    title: 'Pricing', path: '/meeting.md', date: '2026-03-25', context: 'Renewal discussion',
    decisions: [{ text: 'Keep annual billing' }],
    action_items: [{ assignee: 'Alex', task: 'Send proposal', due: 'Friday' }],
    speaker_map: [{ speaker_label: 'Speaker 1', name: 'Alex', confidence: 'medium' }],
    sections: [{ heading: 'Summary', content: 'Reviewed pricing' }],
  });
  assert.deepEqual(scroll.children.slice(0, 4).map(card => card.children[0].textContent), ['Decisions', 'Next steps', 'Context', 'Speakers']);
  const speaker = scroll.children[3].children[1];
  assert.equal(speaker.children.at(-1).textContent, 'Confirm');
});

test('opening source details does not switch into the source editor', async () => {
  const start = panel.indexOf("mdViewerContent.addEventListener('click', async (event) => {");
  const end = panel.indexOf('\n    });', start);
  const handler = panel.slice(start, end + '\n    });'.length);
  let listener, edits = 0;
  vm.runInNewContext(handler, {
    mdViewerContent: { addEventListener: (_event, fn) => { listener = fn; } },
    window: { getSelection: () => ({ toString: () => '' }) },
    openArtifactEditable: true, mdViewerMode: 'preview',
    setMdViewerMode: async () => { edits++; },
  });
  const target = { closest: selector => selector === '.md-source-details' ? {} : null };
  await listener({ target });
  assert.equal(edits, 0);
});
