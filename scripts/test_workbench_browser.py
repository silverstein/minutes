#!/usr/bin/env python3
"""Exercise the actual Work frontend with a synthetic, no-effects Tauri host.

This catches DOM/event/render regressions. It does not replace signed native
Minutes Dev validation of Accessibility, audio, IPC injection or OS permissions.
"""
from __future__ import annotations

import functools
import http.server
import os
import re
from pathlib import Path
import threading

from playwright.sync_api import sync_playwright, expect

ROOT = Path(__file__).resolve().parents[1]
MOCK = r"""
window.__requests = [];
window.__events = {};
let serial = 0;
let saved = null;
let view = {
  available:true, voice_active:false, starting:false, native_selection:false,
  shortcut_enabled:false, shortcut:'CmdOrCtrl+Alt+Shift+W', reviews:[],
  checkpoint:{id:'work-0',goal:'Continue my work',revision:0,focus:null,sources:[],memory:[],tasks:[],next_step:null}
};
window.__setReview = () => { view.reviews=[{id:7,session:'fixture',revision:view.checkpoint.revision,verb:'send_email',target:'fixture@example.invalid',payload:{body:'<img src=x onerror="window.pwned=true">'},expires_ms:60000}]; };
window.__setTask = () => { view.checkpoint.tasks=[{id:'task-1',instruction:'Review this code',state:'completed',spoken_summary:'Review ready',artifact:{locator:'fixture-result.md'}}]; };
window.__TAURI__ = {
  event:{listen:async(name,fn)=>{window.__events[name]=fn;return ()=>{};}},
  core:{invoke:async(command,{request:r})=>{
    if(command!=='cmd_workbench') throw Error('Unexpected command');
    window.__requests.push(JSON.parse(JSON.stringify(r)));
    switch(r.action){
      case 'capabilities': return {available:true};
      case 'view': break;
      case 'new': view.checkpoint={id:'work-'+(++serial),goal:r.goal,revision:0,focus:null,sources:[],memory:[],tasks:[],next_step:null}; view.voice_active=false;break;
      case 'select': view.voice_active=false;view.checkpoint.focus={selection:r.text,source:{locator:'selection:manual',version:'fixture-sha'}};view.checkpoint.revision++;break;
      case 'note':
        if(r.work_id!==view.checkpoint.id||r.revision!==view.checkpoint.revision)throw Error('Stale work');
        view.checkpoint.memory.push({text:r.text,attribution:r.decision?'user_confirmed_decision':'user_interpretation'});view.checkpoint.revision++;view.reviews=[];break;
      case 'start': if(!r.allow_cloud)throw Error('Consent missing');view.voice_active=true;break;
      case 'stop': view.voice_active=false;view.reviews=[];break;
      case 'say':window.__events['work:voice']?.({payload:{type:'user_transcript',text:r.text,partial:false}});break;
      case 'ptt':break;
      case 'approve': if(JSON.stringify(r.review)!==JSON.stringify(view.reviews[0]))throw Error('Review mismatch');view.reviews=[];break;
      case 'reject':view.reviews=[];break;
      case 'cancel':view.reviews=[];break;
      case 'park':view.voice_active=false;view.checkpoint.next_step=r.next_step;view.checkpoint.revision++;saved=JSON.parse(JSON.stringify(view.checkpoint));return {saved:{name:'fixture-work.md'},view:JSON.parse(JSON.stringify(view))};
      case 'list':return saved?[{name:'fixture-work.md',goal:saved.goal,revision:saved.revision}]:[];
      case 'resume':view.checkpoint=JSON.parse(JSON.stringify(saved));view.checkpoint.revision++;view.voice_active=false;view.reviews=[];break;
      case 'artifact':return {name:r.name,version:'result-v1',content:'PRIVATE RESULT <script>window.pwned=true</script>',shareable:true};
      case 'share_artifact':break;
      default:throw Error('Unexpected action: '+r.action);
    }
    return JSON.parse(JSON.stringify(view));
  }}
};
"""


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_args: object) -> None:
        pass


def main() -> None:
    handler = functools.partial(QuietHandler, directory=str(ROOT / 'tauri' / 'src'))
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with sync_playwright() as p:
            executable = os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE')
            browser = p.chromium.launch(headless=True, executable_path=executable)
            page = browser.new_page(viewport={'width': 1050, 'height': 790})
            errors: list[str] = []
            page.on('pageerror', lambda e: errors.append(str(e)))
            if os.environ.get('WORKBENCH_OFFLINE'):
                # DOM-only fallback for environments whose browser denies all
                # navigation. Native IPC/CSP are explicitly NOT covered here.
                html = (ROOT / 'tauri/src/work.html').read_text()
                html = re.sub(r'<meta http-equiv="Content-Security-Policy"[^>]*>', '', html)
                html = re.sub(r'<script[^>]*>.*?</script>|<link[^>]*>', '', html)
                page.set_content(html)
                page.add_style_tag(content=(ROOT / 'tauri/src/styles/theme-tokens.css').read_text())
                page.add_style_tag(content=(ROOT / 'tauri/src/styles/work.css').read_text())
                page.evaluate(MOCK)
                page.add_script_tag(content=(ROOT / 'tauri/src/scripts/work.js').read_text())
            else:
                page.add_init_script(MOCK)
                page.goto(f'http://127.0.0.1:{server.server_port}/work.html')
            expect(page.locator('#goal')).to_have_value('Continue my work')
            expect(page.locator('#shortcut')).to_be_disabled()
            page.locator('#start').click()
            expect(page.locator('#notice')).to_contain_text('consent box')
            assert not page.evaluate("window.__requests.some(r=>r.action==='start')")
            page.locator('#goal').fill('Clarify the proposal')
            page.locator('#goal-form button').click()
            page.locator('summary').filter(has_text='Paste or replace').click()
            hostile = '<img src=x onerror="window.pwned=true"> Keep pricing unchanged.'
            page.locator('#selection-input').fill(hostile)
            page.locator('#selection-form button').click()
            expect(page.locator('#selection')).to_have_text(hostile)
            assert page.locator('#selection img').count() == 0
            page.locator('#note').fill('Interest was not agreement.')
            page.locator('#note-form button[type=submit]').click()
            expect(page.locator('#memory')).to_contain_text('Your interpretation')
            page.locator('#note').fill('Keep existing scope.')
            page.locator('#decision').check()
            page.locator('#note-form button[type=submit]').click()
            expect(page.locator('#memory')).to_contain_text('You confirmed this decision')
            page.locator('#consent').check()
            page.locator('#start').click()
            expect(page.locator('#talk')).to_be_enabled()
            page.locator('#talk').hover()
            page.mouse.down(); page.mouse.up()
            # Keyboard is a real supported input path too.
            page.locator('#talk').focus()
            page.keyboard.down('Space'); page.keyboard.up('Space')
            page.locator('#say').fill('Compare this with the previous discussion.')
            page.locator('#send').click()
            expect(page.locator('#transcript')).to_contain_text('Compare this')
            page.evaluate('window.__setReview()')
            expect(page.locator('#approval-section')).to_be_visible()
            assert page.locator('#reviews img').count() == 0
            page.get_by_role('button', name='Approve this exact action').click()
            expect(page.locator('#approval-section')).to_be_hidden()
            approved = page.evaluate("window.__requests.find(r=>r.action==='approve')")
            assert approved['review']['payload']['body'].startswith('<img')
            page.evaluate('window.__setTask()')
            page.get_by_role('button', name='Review local result').click()
            expect(page.locator('#artifact-dialog')).to_be_visible()
            expect(page.locator('#artifact-content')).to_contain_text('PRIVATE RESULT')
            assert not page.evaluate("window.__requests.some(r=>r.action==='share_artifact')")
            page.locator('#close-artifact').click()
            page.locator('#next-step').fill('Compare the revision next time.')
            page.locator('#park-form button').click()
            expect(page.locator('#notice')).to_contain_text('Saved locally')
            expect(page.locator('#consent')).not_to_be_checked()
            expect(page.locator('#talk')).to_be_disabled()
            page.locator('#resume-area summary').click()
            page.locator('#saved button').click()
            expect(page.locator('#notice')).to_contain_text('restored locally')
            expect(page.locator('#selection')).to_have_text(hostile)
            expect(page.locator('#memory')).to_contain_text('Keep existing scope.')
            assert not page.evaluate('Boolean(window.pwned)')
            assert page.evaluate("window.__requests.some(r=>r.action==='ptt'&&r.down===false)")
            for width in [1050, 760]:
                page.set_viewport_size({'width':width,'height':790})
                assert page.evaluate('document.documentElement.scrollWidth <= window.innerWidth')
            path = os.environ.get('WORKBENCH_SCREENSHOT')
            if path:
                page.set_viewport_size({'width':1050,'height':790})
                page.screenshot(path=path, full_page=True)
            assert not errors, errors
            browser.close()
            print('Work frontend: consent, selection, attribution, PTT, exact review, local artifact, park/resume, XSS, and responsive layout passed.')
    finally:
        server.shutdown(); server.server_close()


if __name__ == '__main__':
    main()
