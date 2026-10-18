#!/usr/bin/env node
// Local-only diagnostics. Never sends transcripts or observations to a provider.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';

export function summarize(text) {
  const events = text.split('\n').flatMap(line => {
    if (!line.startsWith('{')) return [];
    try { const v=JSON.parse(line); return v.event ? [v] : []; } catch { return []; }
  });
  const queued=new Map(), started=new Map();
  const tools=[], responseGaps=[], playbackDelays=[];
  let lastInput, firstAudio, interruptions=0;
  for (const e of events) {
    const t=e.session_elapsed_ms;
    if (e.event==='tool_queued') queued.set(e.call_id,t);
    if (e.event==='tool_started') started.set(e.call_id,t);
    if (e.event==='tool_result') tools.push({tool:e.tool,call_id:e.call_id,execution_ms:e.elapsed_ms,queue_ms:Number.isFinite(started.get(e.call_id))&&Number.isFinite(queued.get(e.call_id))?started.get(e.call_id)-queued.get(e.call_id):null,error:e.error,action_state:e.action_state??null});
    if (e.event==='input_transcript_chunk') lastInput=t;
    if (e.event==='first_response_audio_received') {
      firstAudio=t;
      if (Number.isFinite(lastInput)) responseGaps.push(t-lastInput);
      lastInput=undefined;
    }
    if (e.event==='provider_interruption') { interruptions++; firstAudio=undefined; }
    if (e.event==='speech_render_started' && Number.isFinite(firstAudio)) {
      // No transcript text is emitted in this report.
      if (Number.isFinite(e.render_session_elapsed_ms) && e.render_session_elapsed_ms >= firstAudio) playbackDelays.push(e.render_session_elapsed_ms-firstAudio);
      firstAudio=undefined;
    }
  }
  return {timestamped:events.some(e=>Number.isFinite(e.session_elapsed_ms)),tools,interruptions,input_transcript_to_next_audio_ms:responseGaps,received_audio_to_device_render_ms:playbackDelays,
    limits:'Input-transcript arrival is not microphone end-of-speech. The next audio may be an acknowledgement or a background result, not the answer to that turn. Render events identify device-callback consumption, not acoustic sound. Old logs cannot establish conversational latency.'};
}

if (process.argv[1] && import.meta.url===pathToFileURL(process.argv[1]).href) {
  if (process.argv.includes('--self-test')) {
    const lines=[{event:'tool_queued',call_id:'a',session_elapsed_ms:10},{event:'tool_started',call_id:'a',session_elapsed_ms:25},{event:'tool_result',call_id:'a',tool:'fixture',elapsed_ms:97,error:false,session_elapsed_ms:122},{event:'input_transcript_chunk',session_elapsed_ms:150},{event:'first_response_audio_received',session_elapsed_ms:950},{event:'provider_interruption',session_elapsed_ms:970}].map(JSON.stringify).join('\n');
    const r=summarize(lines);
    assert.equal(r.tools[0].queue_ms,15);assert.equal(r.tools[0].execution_ms,97);assert.equal(r.interruptions,1);assert.deepEqual(r.input_transcript_to_next_audio_ms,[800]);
    assert.equal(summarize('{"event":"tool_result","elapsed_ms":97}').timestamped,false);
    console.log('Session timing tests passed');
  } else {
    const path=process.argv[2];if(!path)throw Error('Provide one local session-log path');
    console.log(JSON.stringify(summarize(fs.readFileSync(path,'utf8')),null,2));
  }
}
