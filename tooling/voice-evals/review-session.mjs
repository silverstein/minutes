#!/usr/bin/env node
// Local-only metadata review. No network, transcript excerpts, or automatic scan.
import { open } from 'node:fs/promises';
import assert from 'node:assert/strict';

const kinds=new Set(['none','agent_timeout','agent_auth_required','agent_exit','agent_cancelled','tool_error']);
export function review(text){
  const calls=[], seen=new Set();
  let approvals=0, duplicates=0, legacyProgress=0, malformed=0;
  for(const line of text.split('\n')){
    if(/Type \/approve \d+|type.*slash approve/i.test(line))approvals++;
    if(/still running, \d+s/.test(line))legacyProgress++;
    const start=line.indexOf('{');
    if(start<0 || !line.includes('"tool_result"'))continue;
    let event;try{event=JSON.parse(line.slice(start))}catch{malformed++;continue}
    if(event.event!=='tool_result')continue;
    if(typeof event.call_id!=='string'||typeof event.tool!=='string'||!Number.isFinite(event.elapsed_ms)||event.elapsed_ms<0||typeof event.error!=='boolean'){malformed++;continue}
    if(seen.has(event.call_id)){duplicates++;continue}seen.add(event.call_id);
    // Treat tool names as data too; never echo arbitrary identifiers or errors.
    const tool=/^[a-z_]{1,64}$/.test(event.tool)?event.tool:'unknown';
    calls.push({tool,elapsed_ms:event.elapsed_ms,error:event.error,failure_kind:kinds.has(event.failure_kind)?event.failure_kind:'tool_error'});
  }
  const timings=calls.map(c=>c.elapsed_ms).sort((a,b)=>a-b);
  const groups={};for(const c of calls){const key=c.tool;if(!Object.hasOwn(groups,key))Object.defineProperty(groups,key,{value:{calls:0,errors:0,slow_calls:0,max_ms:0},enumerable:true});const g=groups[key];g.calls++;g.errors+=Number(c.error);g.slow_calls+=Number(c.elapsed_ms>=15000);g.max_ms=Math.max(g.max_ms,c.elapsed_ms)}
  return {local_only:true,content_exported:false,structured_calls:calls.length,errors:calls.filter(c=>c.error).length,median_ms:timings.length?timings[Math.floor(timings.length/2)]:null,max_ms:timings.at(-1)??null,tools:groups,failure_kinds:calls.filter(c=>c.error).reduce((r,c)=>{r[c.failure_kind]=(r[c.failure_kind]||0)+1;return r},{}),approval_cue_lines:approvals,legacy_progress_lines:legacyProgress,duplicate_receipts:duplicates,malformed_receipts:malformed,limitations:['Counts are diagnostic signals, not a quality score.','Older sessions without structured receipts cannot provide reliable call timing here.','Audible progress, factual accuracy, retained corrections and goal recovery require listening or human review.','This output contains no transcript excerpts, paths, raw errors, arguments or results.']};
}

async function main(){
  if(process.argv.includes('--self-test')){
    const event={event:'tool_result',call_id:'private-id',tool:'build_prototype',elapsed_ms:120000,error:true,failure_kind:'agent_timeout',result:'PRIVATE TEXT'};
    const r=review('you: PRIVATE TEXT\n'+JSON.stringify(event)+'\n'+JSON.stringify(event)+'\nType /approve 1');
    assert.equal(r.errors,1);assert.equal(r.duplicate_receipts,1);assert.equal(r.approval_cue_lines,1);assert(!JSON.stringify(r).includes('PRIVATE'));assert(!JSON.stringify(r).includes('private-id'));
    assert.equal(review(JSON.stringify({...event,tool:'__proto__'})).tools.__proto__.calls,1);
    assert.equal(review('older session').structured_calls,0);console.log('Local review privacy and receipt tests passed');return;
  }
  if(process.argv.length!==3)throw Error('Pass one explicitly selected local session path, or --self-test');
  const file=await open(process.argv[2],'r');
  try{const stat=await file.stat();if(!stat.isFile()||stat.size>8*1024*1024)throw Error('Select one regular session file smaller than 8 MiB');
    const buffer=Buffer.alloc(8*1024*1024+1);let size=0;
    while(size<buffer.length){const {bytesRead}=await file.read(buffer,size,buffer.length-size,null);if(!bytesRead)break;size+=bytesRead}
    if(size>8*1024*1024)throw Error('Session exceeded 8 MiB');
    console.log(JSON.stringify(review(buffer.subarray(0,size).toString('utf8')),null,2));
  }finally{await file.close()}
}
if(process.argv[1]&&import.meta.url===new URL(process.argv[1],'file:').href)main().catch(()=>{console.error('Review failed. Check the explicitly selected file and its size; no content was exported.');process.exitCode=1});
