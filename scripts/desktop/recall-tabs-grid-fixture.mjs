// Real vendored xterm, production initialization/activation/fit functions and
// CSS. Only the native PTY grid and unrelated app services are doubles.
import assert from 'node:assert/strict';

function productionFunction(source, name) {
  const starts = [`async function ${name}(`, `function ${name}(`]
    .map(marker => source.indexOf(marker)).filter(index => index >= 0);
  assert.ok(starts.length, `production function ${name} exists`);
  const start = Math.min(...starts);
  const signature = /\)\s*\{/.exec(source.slice(start));
  assert.ok(signature, `production function ${name} has a body`);
  const body = start + signature.index + signature[0].length - 1;
  let depth = 0;
  for (let index = body; index < source.length; index++) {
    if (source[index] === '{') depth++;
    if (source[index] === '}' && --depth === 0) return source.slice(start, index + 1);
  }
  throw new Error(`Unterminated ${name}`);
}

export function gridFixture(source, css, controller, { meetingOpen = false } = {}) {
  return `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="/vendor/xterm/xterm.css"><style>${css}</style></head><body>
<div id="fixture-meetings" style="position:absolute;left:780px;top:0;width:180px"></div>
<div id="recall-panel" class="recall-panel expanded" style="position:relative;width:750px;height:540px;transform:none">
<div id="recall-tabs" class="recall-tabs" role="tablist"></div><div id="recall-tab-prompt" class="recall-tab-prompt" hidden></div>
<span id="fixture-header"></span><div id="recall-provider-notice" class="recall-provider-notice"></div>
<div id="recall-terminal-container" class="recall-terminal"></div></div>
<script src="/vendor/xterm/xterm.js"></script><script src="/vendor/xterm/xterm-addon-fit.js"></script>
<script>${controller}</script><script>
let SESSION_ID='none',recallExpanded=true,recallDevMode=true,recallReadiness=null,recallSelectedMeetingPath=null;
const isMac=/Mac|iPhone|iPad/.test(navigator.platform),lastMeetingInventory=[];
let recallPhase='recall',recallProviderNoticeDismissedText='',recallReadyNoticeText='';
const RECALL_READY_NOTICE_SEEN_KEY='fixture-ready-notice',recallProviderNotice=document.getElementById('recall-provider-notice');
const recallTabStates=new Map(),recallTerminalContainer=document.getElementById('recall-terminal-container');
const recallLoading=document.createElement('div');
recallLoading.innerHTML='<span></span>';
const recallExitBanner=document.createElement('div'),syncRecallModeControls=()=>{},expandRecallPanel=async()=>{};
let recallTabsModel={active:null,tabs:${JSON.stringify(meetingOpen ? ['one','two','three','four'] : ['one','two','three'])}.map((id,index)=>({id,title:'Chat '+(index+1),status:'done',shell:true}))};
const calls=[],grids=new Map(),getTerminalFontSize=()=>13,isEffectiveDarkTheme=()=>true;
window.MINUTES_XTERM_THEME_QUERY=matchMedia('(prefers-color-scheme:dark)');
const applyRecallTerminalTheme=()=>({background:'#171717',foreground:'#fafafa'}),refreshRecallContextChips=()=>{};
const setRecallPhase=(phase,title)=>{recallPhase=phase;document.getElementById('fixture-header').textContent=phase+' — '+(title||'');};
const registerPtyListeners=async()=>{},registerRecallTerminalFileDrop=async()=>{};
const activeSearchQuery='',meetingRowsState=null,setHighlightedText=(node,text)=>{node.textContent=text;},formatTime=()=> '12:00';
const refreshes=[],scheduleMeetingsRefresh=delay=>refreshes.push(delay);
const boundary={openError:null,newMeetingShell:true};
const invoke=async(command,args)=>{
  calls.push({command,args});
  if(command==='cmd_pty_resize')grids.set(args.sessionId,{cols:args.cols,rows:args.rows});
  ${meetingOpen ? `
  if(command==='cmd_recall_tabs')return structuredClone(recallTabsModel);
  if(command==='cmd_recall_tab_select')recallTabsModel.active=args.tabId;
  if(command==='cmd_recall_tab_open'){
    if(boundary.openError)throw new Error(boundary.openError);
    let tab=args.meetingPath?recallTabsModel.tabs.find(tab=>tab.meeting_path===args.meetingPath):!args.newTab&&recallTabsModel.tabs.find(tab=>tab.id===recallTabsModel.active);
    if(!tab){
      if(recallTabsModel.tabs.length>=4)throw new Error('RECALL_TAB_LIMIT');
      tab={id:'meeting',title:args.meetingTitle||'Chat',meeting_path:args.meetingPath,phase:args.phase,status:'done',shell:boundary.newMeetingShell};
      recallTabsModel.tabs.push(tab);
    }
    recallTabsModel.active=tab.id;
    return structuredClone(recallTabsModel);
  }
  if(command==='cmd_recall_tab_close'){
    const tab=recallTabsModel.tabs.find(tab=>tab.id===args.tabId);
    if(tab.claude_session_id&&!args.confirmed)throw new Error('RECALL_CLOSE_CONFIRM');
    recallTabsModel.tabs=recallTabsModel.tabs.filter(tab=>tab.id!==args.tabId);
    if(recallTabsModel.active===args.tabId)recallTabsModel.active=recallTabsModel.tabs[0]?.id;
    return structuredClone(recallTabsModel);
  }
  ` : `if(command==='cmd_recall_tab_open')throw new Error('RECALL_TAB_LIMIT');`}
};
${['recallStateFor','activeRecallState','fitRecallTerminal','initRecallTerminal','activateRecallTab','openNewRecallChat','withRecallMeetingTitles','renderRecallTabHeader','setRecallProviderNotice','dismissRecallProviderNotice','forwardRecallTerminalData','forwardRecallTerminalInput','updateRecallTerminalInputDraft','resumeRecallTab','openRecallImpl','showRecallTabLimit','applyRecallTabsModel','refreshRecallAfterCapture','buildMeetingRow'].map(name => productionFunction(source, name)).join('\n')}
const opens=[];
${meetingOpen ? `
let recallOpenQueue=Promise.resolve();
const recallChatInput=document.createElement('textarea'),recallModePreferenceExplicit=true;
const refreshRecallReadiness=async()=>({nativeAvailable:false,terminalAvailable:true});
const terminalNoticeForReadiness=()=>'',recallEndBtn=document.createElement('button');
${['openRecall','chooseRecallExperience','recallSpawnSize','markRecallPtyAlive','markRecallPtyEnded','disposeRecallTerminal','closeRecallTab','discussMeetingWithAgent'].map(name => productionFunction(source, name)).join('\n')}
recallTabsModel.tabs[0].claude_session_id='saved-conversation';
` : `const openRecall=(...args)=>opens.push(args);`}
const recallTabsController=MinutesRecallTabs.create({strip:document.getElementById('recall-tabs'),prompt:document.getElementById('recall-tab-prompt'),invoke,select:id=>activateRecallTab(id,recallTabsModel),create:openNewRecallChat,close:${meetingOpen ? 'closeRecallTab' : '()=>{}'}});
for(const metadata of recallTabsModel.tabs){const tab=recallStateFor(metadata.id);initRecallTerminal(tab);tab.alive=true;tab.untouched=true;tab.terminal.resize(10,4);grids.set(tab.id,{cols:10,rows:4});}
window.fixture={calls,opens,grids,tabs:recallTabStates,get model(){return recallTabsModel;},inventory:lastMeetingInventory,resume:resumeRecallTab,notice:setRecallProviderNotice,activate:id=>activateRecallTab(id,recallTabsModel),fit:fitRecallTerminal,newChat:openNewRecallChat,openAtCap:()=>openRecallImpl('assistant',null,{newTab:true}),applyModel:applyRecallTabsModel,buildRow:buildMeetingRow,afterCapture:refreshRecallAfterCapture,refreshes,
${meetingOpen ? 'discuss:discussMeetingWithAgent,boundary,' : ''}
snapshot:id=>{const tab=recallTabStates.get(id);return {terminal:{cols:tab.terminal.cols,rows:tab.terminal.rows},pty:grids.get(id)};}};
setTimeout(()=>{window.fixture.ready=true;},150);
</script></body></html>`;
}
