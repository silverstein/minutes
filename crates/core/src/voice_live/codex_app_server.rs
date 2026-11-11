//! Opt-in, session-owned Codex stdio transport. No thread is ever resumed.
//!
//! The caller has already reviewed the frozen prompt and reauthorized its sources.
//! A completed turn plus archive acknowledgement, rather than an early message,
//! releases the answer. Protocol failures discard the entire owned server.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const FRAME_BYTES: usize = 256 * 1024;
const CALL_BYTES: usize = 8 * 1024 * 1024;
const ANSWER_BYTES: usize = 32 * 1024;
const POLL: Duration = Duration::from_millis(20);

#[derive(Default)]
pub(super) struct Transport {
    stopped: AtomicBool,
    server: Mutex<Option<Server>>,
    cleanup: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Transport {
    pub(super) fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        // Never wait behind an in-flight question on the capture/session thread.
        // Its worker sees the flag and owns process cleanup.
        if let Ok(mut server) = self.server.try_lock() {
            if let Some(server) = server.take() {
                let cleanup = std::thread::spawn(move || drop(server));
                *self.cleanup.lock().unwrap_or_else(|p| p.into_inner()) = Some(cleanup);
            }
        }
    }

    /// Called after audio stops and workers join. A CLI must not exit while
    /// the idle cleanup thread still owns the server's kill/reap obligation.
    pub(super) fn finish(&self) {
        // A worker can finish successfully just as stop observes its lock as
        // busy. After workers join, also collect any server left in that race.
        drop(self.server.lock().unwrap_or_else(|p| p.into_inner()).take());
        if let Some(cleanup) = self
            .cleanup
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = cleanup.join();
        }
    }

    pub(super) fn run(
        &self,
        agent: &str,
        args: &[String],
        cwd: Option<&Path>,
        prompt: &str,
        timeout: Duration,
        authorize: &dyn Fn() -> Result<(), String>,
    ) -> Result<String, String> {
        if !cfg!(unix) {
            return Err(
                "codex_app_server requires Unix process-group cleanup; use cli on this platform"
                    .into(),
            );
        }
        validate_args(args)?;
        if prompt.len() > FRAME_BYTES / 2 {
            return Err("codex_app_server prompt exceeds its bounded input budget".into());
        }
        let deadline = Instant::now() + timeout;
        let cancelled = || self.stopped.load(Ordering::SeqCst) || super::jobs::cancelled();
        // Serialize approved calls without holding capture locks. Waiting for an
        // earlier call consumes this call's own timeout and remains cancellable.
        let mut slot = loop {
            check(deadline, &cancelled)?;
            match self.server.try_lock() {
                Ok(slot) => break slot,
                Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(POLL),
                Err(_) => return Err("codex_app_server transport lock failed".into()),
            }
        };
        let result = (|| {
            if slot.is_none() {
                let mut server = Server::spawn(agent, args, cwd)?;
                server.initialize(deadline, &cancelled)?;
                *slot = Some(server);
            }
            slot.as_mut()
                .unwrap()
                .answer(cwd, prompt, deadline, &cancelled, authorize)
        })();
        if result.is_err() {
            // Do not reconnect/retry a possibly executed prompt automatically.
            // A subsequent reviewed question may launch a fresh server.
            drop(slot.take());
        }
        result
    }
}

fn validate_args(args: &[String]) -> Result<(), String> {
    if args.len() > 128 || args.iter().map(String::len).sum::<usize>() > 32 * 1024 {
        return Err("codex_app_server launch flags exceed their budget".into());
    }
    let mut pairs = args.chunks_exact(2);
    if !pairs.remainder().is_empty()
        || pairs.any(|pair| {
            !matches!(
                pair[0].as_str(),
                "-c" | "--config" | "--enable" | "--disable"
            ) || pair[1].is_empty()
        })
    {
        return Err("codex_app_server accepts only paired -c/--config, --enable and --disable launch flags; use cli for other flags".into());
    }
    Ok(())
}

fn check(deadline: Instant, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("agent_cancelled: Codex app-server handoff cancelled".into())
    } else if Instant::now() >= deadline {
        Err("agent_timeout: Codex app-server handoff exceeded its deadline; no completed answer released".into())
    } else {
        Ok(())
    }
}

struct Frame {
    value: Value,
    bytes: usize,
}

struct Server {
    child: Child,
    writer: Option<mpsc::SyncSender<Vec<u8>>>,
    frames: mpsc::Receiver<Frame>,
    fault: Arc<AtomicBool>,
    next_id: u64,
    bytes: usize,
    active_thread: Option<String>,
}

impl Server {
    fn spawn(agent: &str, args: &[String], cwd: Option<&Path>) -> Result<Self, String> {
        let mut command = crate::engine_process::command(agent);
        command
            .args(args)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|_| "codex_app_server could not start".to_string())?;
        // All three descriptors were explicitly requested above.
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let fault = Arc::new(AtomicBool::new(false));
        let (writer, writes) = mpsc::sync_channel::<Vec<u8>>(8);
        let write_fault = fault.clone();
        std::thread::spawn(move || {
            for bytes in writes {
                if stdin
                    .write_all(&bytes)
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    write_fault.store(true, Ordering::SeqCst);
                    break;
                }
            }
        });
        let (output, frames) = mpsc::sync_channel(64);
        let read_fault = fault.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                // `read_until` on the unlimited stream could allocate forever.
                let mut bytes = Vec::new();
                let read = reader
                    .by_ref()
                    .take((FRAME_BYTES + 1) as u64)
                    .read_until(b'\n', &mut bytes);
                if read.is_err()
                    || bytes.is_empty()
                    || bytes.len() > FRAME_BYTES
                    || !bytes.ends_with(b"\n")
                {
                    read_fault.store(true, Ordering::SeqCst);
                    break;
                }
                let value = match serde_json::from_slice::<Value>(&bytes) {
                    Ok(value) if value.is_object() => value,
                    _ => {
                        read_fault.store(true, Ordering::SeqCst);
                        break;
                    }
                };
                if output
                    .try_send(Frame {
                        value,
                        bytes: bytes.len(),
                    })
                    .is_err()
                {
                    read_fault.store(true, Ordering::SeqCst);
                    break;
                }
            }
        });
        // Drain without retaining arbitrary provider diagnostics/credentials.
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink());
        });
        Ok(Self {
            child,
            writer: Some(writer),
            frames,
            fault,
            next_id: 1,
            bytes: 0,
            active_thread: None,
        })
    }

    fn send(&self, value: Value) -> Result<(), String> {
        let mut bytes =
            serde_json::to_vec(&value).map_err(|_| "codex_app_server could not encode request")?;
        if bytes.len() > FRAME_BYTES {
            return Err("codex_app_server request exceeds budget".into());
        }
        bytes.push(b'\n');
        self.writer
            .as_ref()
            .ok_or("codex_app_server is closed")?
            .try_send(bytes)
            .map_err(|_| "codex_app_server input stalled".into())
    }

    fn receive(
        &mut self,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Value, String> {
        loop {
            check(deadline, cancelled)?;
            if self.fault.load(Ordering::SeqCst) {
                return Err(
                    "codex_app_server closed or exceeded its protocol/output budget".into(),
                );
            }
            match self.frames.recv_timeout(POLL) {
                Ok(frame) => {
                    self.bytes += frame.bytes;
                    if self.bytes > CALL_BYTES {
                        return Err("codex_app_server call exceeded output budget".into());
                    }
                    if frame.value.get("method").is_some() && frame.value.get("id").is_some() {
                        // Never grant tool/permission/elicitation requests. Stop
                        // the owned server instead of leaving an unanswered UI.
                        return Err("codex_app_server requested unsupported client authorization; no permission granted".into());
                    }
                    return Ok(frame.value);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err("codex_app_server output disconnected".into()),
            }
        }
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
        pending: &mut Vec<Value>,
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"id":id,"method":method,"params":params}))?;
        loop {
            let event = self.receive(deadline, cancelled)?;
            if event.get("id").is_some() {
                if event["id"] != id || event.get("error").is_some() {
                    return Err(
                        "codex_app_server RPC rejected or mismatched; no completed answer released"
                            .into(),
                    );
                }
                return event
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "codex_app_server missing RPC result".into());
            }
            if pending.len() >= 256 {
                return Err("codex_app_server notification backlog exceeded".into());
            }
            pending.push(event);
        }
    }

    fn initialize(
        &mut self,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        self.request(
            "initialize",
            json!({"clientInfo":{"name":"minutes-voice","version":env!("CARGO_PKG_VERSION")},
            "capabilities":{"experimentalApi":false,"explicitGatewayOauth":true}}),
            deadline,
            cancelled,
            &mut Vec::new(),
        )?;
        self.send(json!({"method":"initialized","params":{}}))?;
        Ok(())
    }

    fn answer(
        &mut self,
        cwd: Option<&Path>,
        prompt: &str,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
        authorize: &dyn Fn() -> Result<(), String>,
    ) -> Result<String, String> {
        self.bytes = 0;
        // Read effective project/profile configuration without logging it or
        // writing global settings. An empty MCP override would merely merge.
        let config = self.request(
            "config/read",
            json!({"cwd":cwd,"includeLayers":false}),
            deadline,
            cancelled,
            &mut Vec::new(),
        )?;
        let overrides = readonly_overrides(&config["config"])?;
        let result = self.request(
            "thread/start",
            json!({"cwd":cwd,"sandbox":"read-only",
            "approvalPolicy":"never","approvalsReviewer":"user","config":overrides}),
            deadline,
            cancelled,
            &mut Vec::new(),
        )?;
        let thread = result["thread"]["id"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .ok_or("codex_app_server missing fresh thread identity")?
            .to_owned();
        self.active_thread = Some(thread.clone());
        if result["sandbox"]["type"] != "readOnly"
            || result["approvalPolicy"] != "never"
            || result["sandbox"]
                .get("networkAccess")
                .is_some_and(|v| v != false)
        {
            return Err(
                "codex_app_server did not confirm read-only sandbox without network access and never approval policy"
                    .into(),
            );
        }
        authorize()?;
        check(deadline, cancelled)?;
        let mut pending = Vec::new();
        let result = self.request(
            "turn/start",
            json!({"threadId":thread,
            "input":[{"type":"text","text":prompt}]}),
            deadline,
            cancelled,
            &mut pending,
        )?;
        let turn = result["turn"]["id"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .ok_or("codex_app_server missing turn identity")?
            .to_owned();
        let mut pending = pending.into_iter();
        let mut answer = String::new();
        loop {
            let event = match pending.next() {
                Some(event) => event,
                None => self.receive(deadline, cancelled)?,
            };
            let params = &event["params"];
            if params["threadId"] != thread {
                continue;
            }
            match event["method"].as_str() {
                Some("item/completed") if params["turnId"] == turn => {
                    let item = &params["item"];
                    if item["type"] == "agentMessage" {
                        let text = item["text"]
                            .as_str()
                            .ok_or("codex_app_server missing agent message")?;
                        if text.len() > ANSWER_BYTES {
                            return Err("codex_app_server answer exceeded budget".into());
                        }
                        // Commentary is not a final answer.
                        if item["phase"].is_null() || item["phase"] == "final_answer" {
                            answer = text.to_owned();
                        }
                    }
                }
                Some("turn/completed") if params["turn"]["id"] == turn => {
                    if params["turn"]["status"] != "completed"
                        || !params["turn"]["error"].is_null()
                        || answer.trim().is_empty()
                    {
                        return Err(
                            "codex_app_server turn failed or returned no final answer".into()
                        );
                    }
                    break;
                }
                _ => {}
            }
        }
        // Completion includes the owned thread's lifecycle acknowledgement.
        self.request(
            "thread/archive",
            json!({"threadId":thread}),
            deadline,
            cancelled,
            &mut Vec::new(),
        )?;
        self.active_thread = None;
        check(deadline, cancelled)?;
        Ok(answer)
    }
}

fn readonly_overrides(config: &Value) -> Result<Value, String> {
    let mut overrides = json!({
        "features.apps":false,"features.computer_use":false,"features.multi_agent":false,
        "plugins":{"_default":{"enabled":false}},"web_search":"disabled",
        "memories.use_memories":false,"memories.generate_memories":false
    });
    if !config.is_object() {
        return Err("codex_app_server effective config unavailable".into());
    }
    for table in ["mcp_servers", "plugins"] {
        if let Some(entries) = config.get(table) {
            let entries = entries
                .as_object()
                .ok_or("codex_app_server unsupported capability configuration")?;
            if entries.len() > 256 {
                return Err("codex_app_server capability config exceeds budget".into());
            }
            for (name, entry) in entries {
                // Nested tables preserve literal names. Codex's dotted-path
                // override parser does not support quoted path segments.
                if overrides.get(table).is_none() {
                    overrides[table] = json!({});
                }
                if !entry.is_object() {
                    return Err("codex_app_server unsupported capability entry".into());
                }
                // Codex replaces a table supplied as an override, rather than
                // merging its transport fields. Preserve those required fields.
                let mut disabled = without_null_fields(entry.clone());
                disabled["enabled"] = json!(false);
                overrides[table][name] = disabled;
            }
        }
    }
    Ok(overrides)
}

// Config/read serializes absent optional fields as JSON null. The reverse
// JSON-to-TOML override conversion cannot represent null; omit those fields.
fn without_null_fields(mut value: Value) -> Value {
    match &mut value {
        Value::Object(fields) => {
            fields.retain(|_, value| !value.is_null());
            for value in fields.values_mut() {
                *value = without_null_fields(value.take());
            }
        }
        Value::Array(values) => {
            for value in values {
                *value = without_null_fields(value.take());
            }
        }
        _ => {}
    }
    value
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(thread) = self.active_thread.take() {
            // Failure/cancellation never releases an answer. Make one bounded
            // attempt to archive only our own thread, then stop regardless.
            let _ = self.request(
                "thread/archive",
                json!({"threadId":thread}),
                Instant::now() + Duration::from_millis(500),
                &|| false,
                &mut Vec::new(),
            );
        }
        self.writer.take(); // EOF permits normal SessionEnd hooks/shutdown.
        let deadline = Instant::now() + Duration::from_secs(1);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        #[cfg(unix)]
        {
            // Only this child's newly created process group; never a model PID.
            unsafe {
                libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_overrides_disable_each_effective_external_tool_source() {
        let overrides = readonly_overrides(&json!({
            "mcp_servers":{"outside.server":{"command":"synthetic-mcp","tool_timeout_sec":null},"quotes\"inside":{}},
            "plugins":{"writer@market":{"enabled":true}}
        }))
        .unwrap();
        assert_eq!(overrides["mcp_servers"]["outside.server"]["enabled"], false);
        assert_eq!(overrides["mcp_servers"]["quotes\"inside"]["enabled"], false);
        assert_eq!(overrides["plugins"]["writer@market"]["enabled"], false);
        assert_eq!(overrides["features.apps"], false);
        assert_eq!(overrides["features.computer_use"], false);
        assert_eq!(
            overrides["mcp_servers"]["outside.server"]["command"],
            "synthetic-mcp"
        );
        assert!(overrides["mcp_servers"]["outside.server"]
            .get("tool_timeout_sec")
            .is_none());
        assert!(readonly_overrides(&json!({"mcp_servers":[]})).is_err());
    }

    #[test]
    fn launch_flags_cannot_select_a_different_subcommand_or_listener() {
        for flags in [
            vec!["exec"],
            vec!["--listen", "0.0.0.0:8080"],
            vec!["-c"],
            vec!["-p", "unsafe"],
        ] {
            assert!(
                validate_args(&flags.into_iter().map(String::from).collect::<Vec<_>>()).is_err()
            );
        }
        assert!(validate_args(&[
            "-c".into(),
            "model=\"test\"".into(),
            "--disable".into(),
            "apps".into()
        ])
        .is_ok());
    }

    #[cfg(unix)]
    struct Fixture {
        root: tempfile::TempDir,
    }
    #[cfg(unix)]
    impl Fixture {
        fn new(mode: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("mode"), mode).unwrap();
            std::fs::write(root.path().join("codex"), r##"#!/usr/bin/env python3
import json,sys,pathlib,os,time,subprocess
root=pathlib.Path(__file__).parent
mode=(root/'mode').read_text()
(root/'pid').write_text(str(os.getpid()))
def emit(x):
 print(json.dumps(x),flush=True)
def record(x):
 with (root/'events').open('a') as f: f.write(json.dumps(x)+'\n')
index=0
for line in sys.stdin:
 q=json.loads(line); method=q['method']; p=q.get('params',{}); ident=q.get('id')
 if method=='initialized': continue
 record({'method':method,'params':p})
 result={}
 if method=='config/read': result={'config':{'mcp_servers':{'outside.server':{}},'plugins':{'writer@market':{'enabled':True}}}}
 if method=='thread/start':
  assert p['sandbox']=='read-only' and p['approvalPolicy']=='never'
  assert p['config']['mcp_servers']['outside.server']['enabled']==False
  assert p['config']['plugins']['writer@market']['enabled']==False
  index+=1; thread=str(os.getpid())+'.'+str(index)
  result={'thread':{'id':thread},'sandbox':{'type':'workspaceWrite' if mode=='policy' else 'readOnly','networkAccess':mode=='network'},'approvalPolicy':'never'}
 if method=='turn/start':
  thread=p['threadId']; turn=thread+'.turn'; result={'turn':{'id':turn}}
  if mode=='stall':
   subprocess.Popen([sys.executable,'-c',"import pathlib,time,sys\np=pathlib.Path(sys.argv[1])\nwhile True:\n p.write_text(str(time.monotonic_ns()))\n time.sleep(.05)",str(root/'child-heartbeat')])
   time.sleep(60)
  if mode=='malformed': print('not-json',flush=True); time.sleep(60)
  if mode=='oversize': print('x'*300000,flush=True); time.sleep(60)
  if mode=='request': emit({'id':'server-owned-request','method':'item/commandExecution/requestApproval','params':{}}); time.sleep(60)
  # Events may arrive before the turn/start acknowledgement.
  emit({'method':'item/completed','params':{'threadId':'unrelated','turnId':turn,'item':{'type':'agentMessage','text':'wrong thread'}}})
  emit({'method':'item/completed','params':{'threadId':thread,'turnId':'old-turn','item':{'type':'agentMessage','text':'old turn'}}})
  emit({'method':'item/completed','params':{'threadId':thread,'turnId':turn,'item':{'type':'agentMessage','phase':'commentary','text':'not final'}}})
  if mode!='empty': emit({'method':'item/completed','params':{'threadId':thread,'turnId':turn,'item':{'type':'agentMessage','phase':'final_answer','text':p['input'][0]['text']}}})
  if mode=='early': time.sleep(60)
  emit({'method':'turn/completed','params':{'threadId':thread,'turn':{'id':turn,'status':'failed' if mode=='failed' else 'completed','error':None}}})
 if method=='thread/archive' and mode=='archive':
  emit({'id':ident,'error':{'code':-1,'message':'synthetic archive failure'}}); continue
 emit({'id':ident,'result':result})
record({'method':'EOF'})
if mode=='slow_exit': time.sleep(60)
"##).unwrap();
            std::fs::set_permissions(
                root.path().join("codex"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            Self { root }
        }
        fn agent(&self) -> String {
            self.root
                .path()
                .join("codex")
                .to_string_lossy()
                .into_owned()
        }
        fn run(
            &self,
            transport: &Transport,
            prompt: &str,
            timeout: Duration,
        ) -> Result<String, String> {
            transport.run(
                &self.agent(),
                &[],
                Some(self.root.path()),
                prompt,
                timeout,
                &|| Ok(()),
            )
        }
        fn events(&self) -> Vec<Value> {
            std::fs::read_to_string(self.root.path().join("events"))
                .unwrap_or_default()
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect()
        }
        fn assert_reaped(&self) {
            let pid: i32 = std::fs::read_to_string(self.root.path().join("pid"))
                .unwrap()
                .parse()
                .unwrap();
            let end = Instant::now() + Duration::from_secs(3);
            while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < end {
                std::thread::sleep(POLL);
            }
            assert_eq!(
                unsafe { libc::kill(pid, 0) },
                -1,
                "owned server was not reaped"
            );
            let heartbeat = self.root.path().join("child-heartbeat");
            if heartbeat.exists() {
                let before = std::fs::read(&heartbeat).unwrap();
                std::thread::sleep(Duration::from_millis(150));
                assert_eq!(
                    before,
                    std::fs::read(&heartbeat).unwrap(),
                    "owned descendant continued running"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn reuses_only_the_server_and_archives_each_fresh_thread_before_release() {
        let fixture = Fixture::new("normal");
        let transport = Transport::default();
        assert_eq!(
            fixture
                .run(
                    &transport,
                    "first approved question",
                    Duration::from_secs(5)
                )
                .unwrap(),
            "first approved question"
        );
        assert_eq!(
            fixture
                .run(
                    &transport,
                    "second approved question",
                    Duration::from_secs(5)
                )
                .unwrap(),
            "second approved question"
        );
        let events = fixture.events();
        assert_eq!(
            events
                .iter()
                .filter(|e| e["method"] == "initialize")
                .count(),
            1
        );
        let turns: Vec<_> = events
            .iter()
            .filter(|e| e["method"] == "turn/start")
            .collect();
        assert_ne!(
            turns[0]["params"]["threadId"],
            turns[1]["params"]["threadId"]
        );
        assert_eq!(
            turns[1]["params"]["input"][0]["text"],
            "second approved question"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["method"] == "thread/archive")
                .count(),
            2
        );
        transport.stop();
        transport.finish();
        fixture.assert_reaped();
        assert!(fixture.events().iter().any(|e| e["method"] == "EOF"));
        assert!(fixture
            .run(&transport, "after stop", Duration::from_secs(1))
            .unwrap_err()
            .contains("cancelled"));
    }

    #[cfg(unix)]
    #[test]
    fn protocol_policy_and_lifecycle_failures_never_release_early_answers() {
        for mode in [
            "policy",
            "network",
            "malformed",
            "oversize",
            "request",
            "empty",
            "failed",
            "archive",
            "early",
        ] {
            let fixture = Fixture::new(mode);
            let transport = Transport::default();
            assert!(
                fixture
                    .run(&transport, "synthetic answer", Duration::from_millis(500))
                    .is_err(),
                "{mode}"
            );
            fixture.assert_reaped();
            if mode == "policy" || mode == "network" {
                assert!(!fixture.events().iter().any(|e| e["method"] == "turn/start"));
            }
            assert!(transport.server.lock().unwrap().is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_stops_running_and_waiting_calls_without_blocking_session_stop() {
        let fixture = Fixture::new("stall");
        let transport = Arc::new(Transport::default());
        let agent = fixture.agent();
        let worker = transport.clone();
        let first = std::thread::spawn(move || {
            worker.run(&agent, &[], None, "first", Duration::from_secs(10), &|| {
                Ok(())
            })
        });
        let end = Instant::now() + Duration::from_secs(3);
        while !fixture.events().iter().any(|e| e["method"] == "turn/start") {
            assert!(Instant::now() < end);
            std::thread::sleep(POLL);
        }
        let agent = fixture.agent();
        let worker = transport.clone();
        let second = std::thread::spawn(move || {
            worker.run(
                &agent,
                &[],
                None,
                "queued",
                Duration::from_secs(10),
                &|| Ok(()),
            )
        });
        let begin = Instant::now();
        transport.stop();
        assert!(begin.elapsed() < Duration::from_millis(100));
        assert!(first.join().unwrap().unwrap_err().contains("cancelled"));
        assert!(second.join().unwrap().unwrap_err().contains("cancelled"));
        fixture.assert_reaped();
        assert_eq!(
            fixture
                .events()
                .iter()
                .filter(|e| e["method"] == "turn/start")
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn source_authorization_is_checked_after_setup_before_sending_the_prompt() {
        let fixture = Fixture::new("normal");
        let transport = Transport::default();
        let result = transport.run(
            &fixture.agent(),
            &[],
            None,
            "must not be sent",
            Duration::from_secs(5),
            &|| Err("source became restricted while waiting".into()),
        );
        assert!(result.unwrap_err().contains("restricted"));
        assert!(fixture
            .events()
            .iter()
            .any(|e| e["method"] == "thread/start"));
        assert!(!fixture.events().iter().any(|e| e["method"] == "turn/start"));
        fixture.assert_reaped();
    }

    #[cfg(unix)]
    #[test]
    fn session_finish_waits_for_idle_server_kill_and_reap_after_nonblocking_stop() {
        for simulate_busy in [false, true] {
            let fixture = Fixture::new("slow_exit");
            let transport = Transport::default();
            assert_eq!(
                fixture
                    .run(&transport, "ready", Duration::from_secs(5))
                    .unwrap(),
                "ready"
            );
            let begin = Instant::now();
            let busy = simulate_busy.then(|| transport.server.lock().unwrap());
            transport.stop();
            assert!(begin.elapsed() < Duration::from_millis(100));
            drop(busy);
            transport.finish();
            let pid: i32 = std::fs::read_to_string(fixture.root.path().join("pid"))
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(
                unsafe { libc::kill(pid, 0) },
                -1,
                "finish returned before reaping idle server"
            );
            assert!(begin.elapsed() < Duration::from_secs(3));
        }
    }

    #[cfg(unix)]
    #[test]
    fn job_cancellation_reaps_descendants_and_next_review_can_start_a_new_server() {
        let fixture = Fixture::new("stall");
        let transport = Arc::new(Transport::default());
        let token = Arc::new(AtomicBool::new(false));
        let cancel = token.clone();
        let worker = transport.clone();
        let agent = fixture.agent();
        let task = std::thread::spawn(move || {
            super::super::jobs::with_cancellation(Some(cancel), || {
                worker.run(
                    &agent,
                    &[],
                    None,
                    "cancel me",
                    Duration::from_secs(10),
                    &|| Ok(()),
                )
            })
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !fixture.root.path().join("child-heartbeat").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(POLL);
        }
        token.store(true, Ordering::SeqCst);
        assert!(task.join().unwrap().unwrap_err().contains("cancelled"));
        fixture.assert_reaped();
        std::fs::write(fixture.root.path().join("mode"), "normal").unwrap();
        assert_eq!(
            fixture
                .run(&transport, "new review", Duration::from_secs(5))
                .unwrap(),
            "new review"
        );
        transport.stop();
        fixture.assert_reaped();
    }

    #[cfg(unix)]
    #[test]
    fn host_review_freezes_context_and_is_required_before_any_server_starts() {
        use super::super::{names::NameIndex, tools::ToolContext};
        use crate::config::Config;
        let fixture = Fixture::new("normal");
        let mut config = Config::default();
        config.voice_live.ask_agent = true;
        config.voice_live.delegate_transport = "codex_app_server".into();
        config.voice_live.delegate_agent = fixture.agent();
        config.voice_live.max_tool_chars = 32_000;
        let mut context = ToolContext::new(config, Arc::new(NameIndex::default()));
        context.continuity = Mutex::new(super::super::continuity::Continuity::new(
            fixture.root.path().join("work"),
        ));
        context
            .delegation_context
            .typed_user("Corrected delivery Friday, owner Nora.");
        let proposed = context.execute("ask_agent", &json!({"question":"Who owns delivery?"}));
        assert!(!proposed.is_error, "{}", proposed.text);
        let review = context.continuity.lock().unwrap().review().unwrap();
        assert!(!fixture.root.path().join("pid").exists());
        assert!(context.execute_approved(review.id).is_error);
        context
            .continuity
            .lock()
            .unwrap()
            .host_command(&format!("/approve {}", review.id))
            .unwrap()
            .unwrap();
        context
            .delegation_context
            .typed_user("Later speech is not approved context.");
        let outcome = context.execute_approved(review.id);
        assert!(!outcome.is_error, "{}", outcome.text);
        let answer: Value = serde_json::from_str(&outcome.text).unwrap();
        assert!(answer["answer"].as_str().unwrap().contains("owner Nora"));
        assert!(!answer["answer"].as_str().unwrap().contains("Later speech"));
        context.stop_agent_transport();
        fixture.assert_reaped();
    }
}
