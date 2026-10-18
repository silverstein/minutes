/* Recall chat owns turn, event, rendering and composer state. No Tauri globals:
 * the shell injects IPC and its existing safe Markdown renderer. */
(() => {
  'use strict';
  const TOOL_STATUS = {
    search_meetings: 'searching your meetings', list_meetings: 'browsing your meetings',
    get_meeting: 'reading a meeting', research_topic: 'researching a topic',
    get_person_profile: 'looking up a person', relationship_map: 'mapping relationships',
    track_commitments: 'checking commitments', get_meeting_insights: 'pulling out insights',
    consistency_report: 'checking for conflicts', get_agent_annotations: 'reading annotations',
    read_live_transcript: 'reading the live transcript', activity_summary: 'reviewing recent activity',
    search_context: 'searching desktop context', get_moment: 'looking up a moment',
    list_processing_jobs: 'checking processing status', get_status: 'checking status',
    list_voices: 'checking voices', knowledge_status: 'checking your knowledge base',
    qmd_collection_status: 'checking your collections',
  };

  /** @param {import('../contracts/recall-chat').RecallChatChunk} chunk */
  function decodeChunk(chunk) {
    switch (chunk.type) {
      case 'thinking': return { status: 'thinking' };
      case 'text': return { text: chunk.text };
      case 'result': return { result: chunk.subtype && chunk.subtype !== 'success'
        ? chunk.error || chunk.result || 'Claude returned an error.' : chunk.result || '' };
      case 'stream_event': {
        const event = chunk.event;
        const delta = /** @type {Record<string, unknown> | undefined} */ (event.delta);
        return event.type === 'content_block_delta' && delta && typeof delta === 'object' &&
          delta.type === 'text_delta' && typeof delta.text === 'string'
          ? { text: delta.text, partial: true } : {};
      }
      case 'assistant': {
        const content = chunk.message.content;
        if (!Array.isArray(content)) return {};
        let text = '', status;
        for (const part of content) {
          if (!part || typeof part !== 'object') continue;
          if (part.type === 'text' && typeof part.text === 'string') text += part.text;
          if (part.type === 'tool_use' && typeof part.name === 'string') {
            const tool = part.name.replace(/^mcp__[^_]+__/, '');
            status = TOOL_STATUS[tool] || 'consulting your memory';
          }
        }
        return { text, status, complete: true };
      }
      default: {
        /** @type {never} */
        const exhaustive = chunk;
        throw new Error(`Unhandled Recall chunk: ${exhaustive}`);
      }
    }
  }

  /** @param {import('../contracts/recall-controller').RecallChatOptions} options */
  function create({ messages, input, sendButton, jumpButton, invoke, listen, renderMarkdown, report = console.error }) {
    let current = null, last = null, streaming = false, resetting = false, frame = null, generation = 0;
    let registration = null;
    const unlisteners = [];
    const nearBottom = () => messages.scrollHeight - messages.scrollTop - messages.clientHeight < 48;
    function scrollBottom() { messages.scrollTop = messages.scrollHeight; jumpButton.style.display = 'none'; }
    function setStreaming(value) {
      streaming = value;
      sendButton.disabled = resetting;
      sendButton.textContent = value ? 'Stop' : 'Send';
      sendButton.setAttribute('aria-label', value ? 'Stop response' : 'Send message');
      input.disabled = value || resetting;
      if (!input.disabled) input.focus();
    }
    function render(turn) {
      if (!turn?.dirty || !turn.bubble.isConnected) return;
      renderMarkdown(turn.bubble, turn.raw);
      turn.dirty = false;
      if (turn.error) appendNotice(turn);
      if (turn.follow) scrollBottom();
      else jumpButton.style.display = 'block';
    }
    function flush(turn) {
      if (frame !== null) cancelAnimationFrame(frame);
      frame = null;
      render(turn);
    }
    function appendNotice(turn) {
      if (turn.bubble.querySelector('.chat-incomplete-notice')) return;
      if (!turn.raw) { turn.bubble.textContent = turn.error; return; }
      const notice = document.createElement('div');
      notice.className = 'chat-incomplete-notice';
      notice.textContent = turn.error;
      turn.bubble.appendChild(notice);
    }
    function showError(error) {
      const turn = current || last;
      if (!turn || !error) return;
      // Flush before adding the notice. A queued Markdown render must never
      // erase the failure or let an incomplete answer look finished.
      turn.error = String(error);
      flush(turn);
      turn.bubble.classList.remove('streaming');
      appendNotice(turn);
    }
    function finish() {
      if (current) { flush(current); current.bubble.classList.remove('streaming'); current = null; }
      setStreaming(false);
    }
    function receive(value) {
      if (!current) return;
      const state = window.MinutesRecallContract.validate(value);
      if (state !== 'valid') {
        // Provider control frames intentionally do not create visible text.
        // Unknown/malformed frames are observable without logging meeting data.
        if (state !== 'ignored') report(`Recall ignored ${state} provider frame`);
        return;
      }
      const decoded = decodeChunk(value);
      if (decoded.status && !current.raw) current.bubble.dataset.status = decoded.status;
      let text = decoded.complete && current.partial ? '' : decoded.text || '';
      if (decoded.partial && text) current.partial = true;
      if (decoded.result && !current.raw) text = decoded.result;
      if (!text) return;
      current.follow = nearBottom();
      current.raw += text;
      current.dirty = true;
      if (frame === null) {
        const turn = current;
        frame = requestAnimationFrame(() => { frame = null; render(turn); });
      }
    }
    async function register() {
      if (!registration) registration = (async () => {
        try {
          // Await in order so partial registration can be rolled back, retried
          // and disposed without duplicate handlers.
          unlisteners.push(await listen('recall-chat-chunk', event => receive(event.payload)));
          unlisteners.push(await listen('recall-chat-error', event => showError(event.payload)));
          unlisteners.push(await listen('recall-chat-done', finish));
        } catch (error) {
          unlisteners.splice(0).forEach(unlisten => unlisten());
          registration = null;
          throw error;
        }
      })();
      return registration;
    }
    function showWelcome() {
      if (messages.children.length) return;
      const bubble = document.createElement('div');
      bubble.className = 'chat-bubble chat-bubble-assistant';
      bubble.textContent = 'Ask me about this meeting, including action items, decisions and key moments from the transcript. I have access to your meeting workspace.';
      messages.appendChild(bubble);
    }
    async function reset() {
      const resetGeneration = ++generation;
      resetting = true;
      if (frame !== null) cancelAnimationFrame(frame);
      frame = null; current = null; last = null;
      messages.replaceChildren();
      jumpButton.style.display = 'none';
      setStreaming(false);
      input.disabled = true; sendButton.disabled = true;
      try { await invoke('cmd_recall_chat_clear'); }
      catch (error) { report('cmd_recall_chat_clear failed:', error); }
      if (resetGeneration !== generation) return;
      resetting = false;
      setStreaming(false);
      showWelcome();
    }
    async function send() {
      const text = input.value.trim();
      if (streaming || resetting || !text) return;
      const turnGeneration = generation;
      // Registration must finish before sending so a fast local response
      // cannot disappear between invoke and listener setup.
      setStreaming(true);
      try { await register(); }
      catch (error) { setStreaming(false); report('Recall listener setup failed:', error); return; }
      if (turnGeneration !== generation) return;
      const user = document.createElement('div');
      user.className = 'chat-bubble chat-bubble-user'; user.textContent = text;
      const bubble = document.createElement('div');
      bubble.className = 'chat-bubble chat-bubble-assistant streaming';
      bubble.dataset.status = 'thinking';
      current = last = { bubble, raw: '', partial: false, dirty: false, follow: true, error: '' };
      const turn = current;
      messages.append(user, bubble); scrollBottom(); input.value = '';
      try { await invoke('cmd_recall_chat_send', { message: text }); }
      catch (error) {
        if (turnGeneration !== generation || last !== turn) return;
        const message = error instanceof Error ? error.message : String(error);
        showError(message.includes('claude not found')
          ? 'Claude Code not found. Install it with: npm install -g @anthropic-ai/claude-code'
          : 'Error: ' + message);
        finish();
      }
    }
    async function cancel() {
      if (!streaming) return;
      // Stop can be pressed while listener registration is still pending.
      // Invalidate that unsent request instead of starting it after Stop.
      if (!current) { generation++; setStreaming(false); return; }
      sendButton.disabled = true;
      try { await invoke('cmd_recall_chat_cancel'); }
      catch (error) { report('cmd_recall_chat_cancel failed:', error); sendButton.disabled = false; }
    }
    const onClick = () => { if (streaming) void cancel(); else void send(); };
    const onKey = event => { if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); void send(); } };
    const onScroll = () => { if (nearBottom()) jumpButton.style.display = 'none'; };
    sendButton.addEventListener('click', onClick);
    input.addEventListener('keydown', onKey);
    messages.addEventListener('scroll', onScroll);
    jumpButton.addEventListener('click', scrollBottom);
    return Object.freeze({ register, reset, showWelcome, send, cancel,
      async dispose() {
        generation++;
        if (frame !== null) cancelAnimationFrame(frame);
        frame = null; current = null; last = null;
        await registration?.catch(() => {});
        unlisteners.splice(0).forEach(unlisten => unlisten());
        registration = null;
        sendButton.removeEventListener('click', onClick);
        input.removeEventListener('keydown', onKey);
        messages.removeEventListener('scroll', onScroll);
        jumpButton.removeEventListener('click', scrollBottom);
      },
    });
  }
  window.MinutesRecallChat = Object.freeze({ create, decodeChunk });
})();
