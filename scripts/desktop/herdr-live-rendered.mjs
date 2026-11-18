import assert from 'node:assert/strict';
import { readFileSync, mkdirSync } from 'node:fs';

export async function herdrLiveContracts(browser, source, css) {
  const controller = readFileSync('tauri/src/scripts/herdr-live.js', 'utf8');
  const page = await browser.newPage({ viewport: { width: 640, height: 720 } });
  try {
    await page.emulateMedia({colorScheme:'dark'});
    await page.setContent(`<!doctype html><html><head><style>${css}</style></head><body>
<script>
const original = new DOMParser().parseFromString(${JSON.stringify(source).replace(/</g, '\\u003c')}, 'text/html');
for (const id of ['recording-bar','live-bar','herdr-live-dialog']) document.body.append(document.importNode(original.getElementById(id), true));
document.getElementById('recording-bar').classList.add('active');
</script><script>${controller}</script><script>
let view = { active: false, session: 's1', generation: 0, selected: null, phase: null, notice: null };
const rows = [{address:'t42',name:'Board conversation',agent:'claude-code',location:'Development / Recall',labels:{'minutes.tab':'tab-1',team:'QA','herdr.seen':'288','herdr.space':'sp-private'}}, {address:'t43',name:'<img src=x onerror="window.injection=true">',agent:'codex',location:'Development / QA',labels:{}}, {address:'remote/t9',name:'Remote',agent:'codex',labels:{}}, {address:'t44',name:'Shell target',agent:'zsh',labels:{}}, {address:'t45',name:'Unknown target',agent:'python3',labels:{}}, {address:'t46',name:'Missing agent',labels:{}}];
const calls = []; let releaseOptions=null, releasePick=null, hangOptions=false, hangPick=false, reject=false, stopped=0;
const invoke=async(command,args)=>{
  calls.push({command,args});
  if(command==='cmd_herdr_live_view') return structuredClone(view);
  if(command==='cmd_herdr_live_options') {
    const options={session:view.session,generation:view.generation,terminals:rows};
    if(hangOptions) await new Promise(resolve=>{releaseOptions=resolve;});
    return options;
  }
  if(command==='cmd_herdr_live_pick') {
    if(hangPick) await new Promise(resolve=>{releasePick=resolve;});
    if(reject || args.generation!==view.generation || args.session!==view.session || !view.active) throw new Error('The recording changed; nothing was sent');
    view.selected=rows.find(row=>row.address===args.target); view.phase='sent'; view.generation++;
  }
  if(command==='cmd_herdr_live_cancel') {view.selected=null;view.phase=null;view.generation++;}
};
const controls=MinutesHerdrLive.create({dialog:document.getElementById('herdr-live-dialog'),pickButtons:[document.getElementById('btn-herdr-recording'),document.getElementById('btn-herdr-live')],cancelButtons:[document.getElementById('btn-herdr-recording-cancel'),document.getElementById('btn-herdr-live-cancel')],notices:[...document.querySelectorAll('[data-herdr-notice]')],invoke});
document.getElementById('btn-stop').addEventListener('click',()=>{stopped++;view.active=false;view.session=null;view.selected=null;void controls.refresh();});
window.fixture={controls,calls,view,rows,hangOptions:()=>{hangOptions=true;},releaseOptions:()=>{hangOptions=false;releaseOptions?.();},hangPick:()=>{hangPick=true;},releasePick:()=>{hangPick=false;releasePick?.();},reject:value=>{reject=value;},stopped:()=>stopped};
</script></body></html>`);
    const pick = page.locator('#btn-herdr-recording');
    const cancel = page.locator('#btn-herdr-recording-cancel');
    const dialog = page.locator('#herdr-live-dialog');
    await page.evaluate(() => fixture.controls.refresh());
    assert.equal(await pick.isVisible(), false);
    await page.evaluate(() => { fixture.view.active=true; fixture.controls.setAvailable(false); return fixture.controls.refresh(); });
    assert.equal(await pick.isVisible(), false, 'No hc means no picker');
    await page.evaluate(() => fixture.controls.setAvailable(true));
    await pick.click();
    await page.waitForFunction(() => document.querySelectorAll('.herdr-live-terminal').length===2);
    assert.equal(await dialog.getByRole('button', {name:'Cancel',exact:true}).evaluate(node=>node===document.activeElement), true);
    assert.equal(await dialog.getByRole('button', {name:'Cancel',exact:true}).evaluate(node=>getComputedStyle(node).textTransform), 'none');
    const wide = await dialog.boundingBox();
    assert.ok(Math.abs(wide.x - (640-wide.width)/2)<1 && wide.y>0, 'Dialog is centered');
    assert.match(await dialog.textContent(), /Recall conversation/);
    assert.match(await dialog.textContent(), /Development \/ Recall · Claude Code · Recall conversation/);
    assert.doesNotMatch(await dialog.textContent(), /herdr\.|sp-private|team=|minutes\.tab|Shell target|Unknown target|Missing agent/);
    assert.equal(await dialog.locator('.herdr-live-terminal').first().locator(':scope > *').count(), 2);
    mkdirSync('target/recall-herdr-evidence', { recursive: true });
    await page.screenshot({path:'target/recall-herdr-evidence/lrad2-picker-dark.png'});
    assert.equal(await dialog.getByText('Remote', {exact:true}).count(), 0);
    assert.equal(await dialog.locator('img').count(), 0, 'Terminal metadata is text, not HTML');
    assert.equal(await page.evaluate(() => fixture.calls.filter(c=>c.command==='cmd_herdr_live_pick').length), 0);
    await page.keyboard.press('Escape');
    assert.equal(await dialog.isVisible(), false);
    assert.equal(await pick.evaluate(node=>node===document.activeElement), true);
    await pick.click();
    await dialog.getByRole('button', {name:/Board conversation/}).click();
    await cancel.waitFor({state:'visible'});
    assert.deepEqual(await page.evaluate(() => fixture.calls.find(c=>c.command==='cmd_herdr_live_pick').args), {session:'s1',generation:0,target:'t42'});
    assert.match(await page.locator('[data-herdr-notice]').first().textContent(), /Live context sent to Board conversation/);
    await cancel.click();
    await pick.waitFor({state:'visible'});
    // A pending list does not trap cancellation or delay Stop; its late reply
    // cannot offer destinations for a different recording.
    await page.evaluate(() => fixture.hangOptions());
    await pick.click();
    await dialog.getByRole('button', {name:'Cancel',exact:true}).click();
    await page.locator('#btn-stop').click();
    assert.equal(await page.evaluate(() => fixture.stopped()), 1);
    await page.evaluate(() => fixture.releaseOptions());
    assert.equal(await dialog.isVisible(), false);
    assert.equal(await pick.isVisible(), false);
    // Cancel invalidates a selection still being checked by the backend.
    await page.evaluate(async () => { Object.assign(fixture.view,{active:true,session:'s2'}); fixture.hangPick(); await fixture.controls.refresh(); });
    await pick.click();
    await dialog.getByRole('button', {name:/Board conversation/}).click();
    await cancel.click();
    await page.evaluate(() => fixture.releasePick());
    await pick.waitFor({state:'visible'});
    assert.equal(await page.evaluate(() => fixture.view.selected), null);
    // A rejected pick stays visible as an error and does not invent success.
    await page.evaluate(() => fixture.reject(true));
    await pick.click();
    await dialog.getByRole('button', {name:/Board conversation/}).click();
    await page.waitForFunction(() => document.querySelector('[data-herdr-notice]').textContent.includes('nothing was sent'));
    assert.equal(await cancel.isVisible(), false);
    await page.evaluate(() => fixture.reject(false));
    await page.setViewportSize({width:320,height:640});
    await page.emulateMedia({colorScheme:'light',reducedMotion:'reduce'});
    await pick.click();
    const bounds = await dialog.boundingBox();
    assert.ok(bounds.x>=0 && bounds.x+bounds.width<=320);
    assert.ok(Math.abs(bounds.x-(320-bounds.width)/2)<1 && bounds.y>0);
    assert.equal(await dialog.getByRole('button',{name:'Cancel',exact:true}).evaluate(node=>node===document.activeElement), true);
    const row = dialog.locator('.herdr-live-terminal').first();
    assert.ok((await row.boundingBox()).height < 80, 'Rows stay two short lines at narrow width');
    assert.equal(await row.locator('small').evaluate(node=>getComputedStyle(node).whiteSpace), 'nowrap');
    await page.screenshot({path:'target/recall-herdr-evidence/lrad2-picker-light-narrow.png'});
    await page.keyboard.press('Escape');
    assert.equal(await page.evaluate(() => fixture.calls.filter(c=>c.command==='cmd_herdr_live_pick').some(c=>c.args.target!=='t42')), false);
  } finally { await page.close(); }
}
