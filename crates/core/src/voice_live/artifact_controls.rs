//! A narrowly scoped bridge into our own opaque-origin HTML previews.
//! Generated content can report its own state, never gain authority over other apps.
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server};

type Result<T> = std::result::Result<T, String>;
#[derive(Default)]
pub(super) struct Artifacts {
    previews: HashMap<String, Preview>,
}
struct Pending {
    id: String,
    command: Value,
    deadline: Instant,
    sent: bool,
    tx: mpsc::Sender<Value>,
}
struct Preview {
    title: String,
    origin: String,
    token: String,
    pending: Arc<Mutex<Option<Pending>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Artifacts {
    pub fn list(&self) -> Value {
        let mut previews: Vec<_> = self
            .previews
            .iter()
            .map(|(id, preview)| json!({"prototype_id":id,"title":preview.title}))
            .collect();
        previews.sort_by_key(|p| p["prototype_id"].as_str().unwrap().to_owned());
        json!({"artifacts":previews,"note":"These are this session's registered previews, not proof their tabs remain open. Use the exact prototype_id, never a job_id or call_id. Inspect before changing any value."})
    }
    pub fn open(&mut self, id: &str) -> Result<bool> {
        if !self.previews.contains_key(id) {
            if self.previews.len() >= 8 {
                return Err("Eight previews are active; start a new session to open more".into());
            }
            let record = super::prototype::load(
                &crate::config::Config::minutes_dir().join("prototypes"),
                id,
            )?;
            let preview = Preview::start(&record)?;
            let opened = super::prototype::open_preview(Path::new(&format!(
                "{}/#{}",
                preview.origin, preview.token
            )));
            self.previews.insert(id.to_owned(), preview);
            return Ok(opened);
        }
        Ok(true)
    }
    pub fn execute(&mut self, name: &str, args: &Value) -> Result<Value> {
        let id = args["prototype_id"]
            .as_str()
            .ok_or("prototype_id is required")?;
        if !self.previews.contains_key(id) {
            return Ok(
                json!({"error":"artifact_reference_unknown: no matching preview; no command delivered", "recovery":self.list(),"next_step":"Choose the matching artifact from recovery.artifacts and inspect its exact prototype_id. Do not switch to app controls, regenerate, or infer the sliders are unsupported from a reference error. If no artifact matches, ask the user."}),
            );
        }
        let operation = match name {
            "inspect_prototype" => "inspect",
            "set_prototype_control" => "set",
            "undo_prototype_control" => "undo",
            _ => return Err("Unsupported preview operation".into()),
        };
        let mut command = args.clone();
        command["operation"] = json!(operation);
        let mut result = self.previews[id].request(command)?;
        result["prototype_id"] = json!(id);
        result["evidence"] = json!("Live DOM state in this sandboxed artifact, not independent verification of its calculations. Changes last while this preview remains open; they do not rewrite the saved HTML.");
        Ok(result)
    }
}
impl Preview {
    fn start(record: &Value) -> Result<Self> {
        let server = Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
        let origin = format!("http://{}", server.server_addr());
        let token = format!(
            "{}{}",
            super::board::random_id()?,
            super::board::random_id()?
        );
        let html = super::prototype::controlled_viewer(
            record["title"].as_str().unwrap_or("Prototype"),
            record["html"].as_str().ok_or("Missing HTML")?,
        );
        let pending = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, s, o, t) = (pending.clone(), stop.clone(), origin.clone(), token.clone());
        let thread = std::thread::Builder::new()
            .name("minutes-artifact".into())
            .spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(100)) {
                        serve(request, &o, &t, &html, &p);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            title: record["title"].as_str().unwrap_or("Prototype").to_owned(),
            origin,
            token,
            pending,
            stop,
            thread: Some(thread),
        })
    }
    fn request(&self, command: Value) -> Result<Value> {
        let (tx, rx) = mpsc::channel();
        let id = super::board::random_id()?;
        *self.pending.lock().map_err(|_| "Preview unavailable")? = Some(Pending {
            id: id.clone(),
            command,
            deadline: Instant::now() + Duration::from_secs(5),
            sent: false,
            tx,
        });
        let received = rx.recv_timeout(Duration::from_secs(5));
        let mut pending = self.pending.lock().map_err(|_| "Preview unavailable")?;
        let sent = pending.as_ref().is_some_and(|p| p.id == id && p.sent);
        *pending = None;
        match received {
            Ok(value) if value.get("error").is_some() => Err(format!("artifact_control_refused: {}", value["error"].as_str().unwrap_or("Invalid preview response"))),
            Ok(value) => Ok(value),
            Err(_) => Err(if sent { "artifact_control_unverified: preview did not confirm the result; inspect before trying again, do not assume no change" } else { "artifact_preview_unavailable: keep the controlled preview open; no command was delivered" }.into()),
        }
    }
}
impl Drop for Preview {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn serve(
    request: Request,
    origin: &str,
    token: &str,
    html: &str,
    pending: &Mutex<Option<Pending>>,
) {
    let page = request.url() == "/" && request.method() == &Method::Get;
    let result: Result<String> = (|| {
        if super::board::header(&request, "Host") != origin.strip_prefix("http://") {
            return Err("Wrong host".into());
        }
        if page {
            return Ok("<!doctype html><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Minutes preview</title><script>fetch('/viewer',{headers:{'X-Minutes-Token':location.hash.slice(1)}}).then(r=>{if(!r.ok)throw Error('Preview unavailable');return r.text()}).then(html=>{document.open();document.write(html);document.close()}).catch(()=>{document.body.textContent='Preview unavailable'})</script>".to_owned());
        }
        if !super::board::authorized(&request, origin, token) {
            return Err("Unauthorized preview request".into());
        }
        if request.url() == "/viewer" && request.method() == &Method::Get {
            return Ok(html.to_owned());
        }
        let mut lock = pending.lock().map_err(|_| "Preview unavailable")?;
        if request.url() == "/command" && request.method() == &Method::Get {
            return Ok(
                match lock
                    .as_mut()
                    .filter(|p| !p.sent && Instant::now() < p.deadline)
                {
                    Some(p) => {
                        p.sent = true;
                        json!({"id":p.id,"command":p.command}).to_string()
                    }
                    None => "null".into(),
                },
            );
        }
        if request.url() == "/result"
            && request.method() == &Method::Post
            && request.body_length() == Some(0)
        {
            let encoded = super::board::header(&request, "X-Minutes-Result")
                .filter(|s| s.len() <= 48_000)
                .ok_or("Missing or oversized result")?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| "Invalid result encoding")?;
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Invalid result JSON")?;
            let p = lock
                .as_ref()
                .filter(|p| p.sent && value["id"] == p.id && Instant::now() < p.deadline)
                .ok_or("Stale result")?;
            if !value["result"].is_object() {
                return Err("Invalid result".into());
            }
            let _ = p.tx.send(value["result"].clone());
            return Ok("{}".into());
        }
        Err("Unsupported preview request".into())
    })();
    let (status, body) = match result {
        Ok(v) => (200, v),
        Err(e) => (409, json!({"error":e}).to_string()),
    };
    let mut response = Response::from_string(body).with_status_code(status);
    for (key,value) in [("Content-Type",if page{"text/html; charset=utf-8"}else{"application/json"}), ("Cache-Control","no-store"),("Referrer-Policy","no-referrer"),("X-Content-Type-Options","nosniff"),("Content-Security-Policy","default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-src about:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")] {
        if let Ok(h)=Header::from_bytes(key,value) { response.add_header(h); }
    }
    let _ = request.respond(response);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrong_job_id_returns_artifact_identity_without_changing_it() {
        let preview =
            Preview::start(&json!({"title":"Truffle Revenue Sim","html":"<input type=range>"}))
                .unwrap();
        let mut artifacts = Artifacts::default();
        artifacts.previews.insert("artifact-exact".into(), preview);
        let result = artifacts
            .execute(
                "inspect_prototype",
                &json!({"prototype_id":"call_346434_fc_0_0"}),
            )
            .unwrap();
        assert!(result["error"]
            .as_str()
            .unwrap()
            .starts_with("artifact_reference_unknown"));
        assert_eq!(
            result["recovery"]["artifacts"][0]["prototype_id"],
            "artifact-exact"
        );
        assert_eq!(
            result["recovery"]["artifacts"][0]["title"],
            "Truffle Revenue Sim"
        );
        assert!(artifacts.previews["artifact-exact"]
            .pending
            .lock()
            .unwrap()
            .is_none());
        let result = artifacts
            .execute(
                "set_prototype_control",
                &json!({"prototype_id":"call_346434_fc_0_0","value":"100"}),
            )
            .unwrap();
        assert!(result.get("error").is_some());
        assert!(artifacts.previews["artifact-exact"]
            .pending
            .lock()
            .unwrap()
            .is_none());
    }
    #[test]
    fn preview_requires_token_and_exact_origin_and_bounds_wait() {
        let preview = Preview::start(&json!({"title":"Test","html":"<input type=range>"})).unwrap();
        let client = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(2)))
                .build(),
        );
        assert_eq!(
            client
                .get(format!("{}/command", preview.origin))
                .call()
                .unwrap()
                .status(),
            409
        );
        assert_eq!(
            client
                .get(format!("{}/viewer", preview.origin))
                .call()
                .unwrap()
                .status(),
            409
        );
        assert!(!client
            .get(&preview.origin)
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap()
            .contains("input type=range"));
        assert_eq!(
            client
                .post(format!("{}/result", preview.origin))
                .header("X-Minutes-Token", &preview.token)
                .header("Origin", "https://example.invalid")
                .send_empty()
                .unwrap()
                .status(),
            409
        );
        assert_eq!(
            client
                .get(format!("{}/command", preview.origin))
                .header("X-Minutes-Token", &preview.token)
                .call()
                .unwrap()
                .status(),
            200
        );
    }
    #[test]
    #[ignore = "browser acceptance fixture; open printed URL within 30 seconds"]
    fn controlled_preview_fixture() {
        let html="<label>Revenue<input id='revenue' type='range' min='0' max='100' step='5' value='25' oninput=\"document.querySelector('output').textContent=Number(this.value)*2\"></label><output>50</output><label>Cost<input type='number' min='0' max='100' value='10'></label>";
        let preview = Preview::start(&json!({"title":"Control fixture","html":html})).unwrap();
        println!("PREVIEW={}/#{}", preview.origin, preview.token);
        std::thread::sleep(Duration::from_secs(30));
        let initial = preview.request(json!({"operation":"inspect"})).unwrap();
        assert_eq!(initial["controls"][0]["value"], "25");
        let control = initial["controls"][0]["control_id"].clone();
        assert!(preview.request(json!({"operation":"set","snapshot_id":initial["snapshot_id"],"control_id":control,"value":"102"})).is_err());
        assert!(preview.request(json!({"operation":"set","snapshot_id":initial["snapshot_id"],"control_id":control,"value":"27"})).is_err());
        let changed=preview.request(json!({"operation":"set","snapshot_id":initial["snapshot_id"],"control_id":control,"value":"75"})).unwrap();
        assert_eq!(changed["controls"][0]["value"], "75");
        assert_eq!(changed["controls"][1]["value"], "10");
        assert_eq!(changed["output"], "150");
        assert!(preview.request(json!({"operation":"set","snapshot_id":initial["snapshot_id"],"control_id":control,"value":"50"})).is_err());
        let undone=preview.request(json!({"operation":"undo","snapshot_id":changed["snapshot_id"],"undo_id":changed["undo_id"]})).unwrap();
        assert_eq!(undone["controls"][0]["value"], "25");
        assert_eq!(undone["output"], "50");
        println!("CONTROL_ACCEPTANCE=passed");
        std::thread::sleep(Duration::from_secs(15));
    }

    #[test]
    #[ignore = "explicit existing artifact fixture; opens no app; attach a test browser to the printed URL"]
    fn existing_truffle_preview_read_write_undo() {
        let id =
            std::env::var("MINUTES_PROTOTYPE_FIXTURE_ID").expect("explicit artifact ID required");
        let record = super::super::prototype::load(
            &crate::config::Config::minutes_dir().join("prototypes"),
            &id,
        )
        .unwrap();
        assert_eq!(record["title"], "Truffle Revenue Simulator");
        let preview = Preview::start(&record).unwrap();
        println!("TRUFFLE_PREVIEW={}/#{}", preview.origin, preview.token);
        let mut artifacts = Artifacts::default();
        artifacts.previews.insert(id.clone(), preview);
        let recovery = artifacts
            .execute(
                "inspect_prototype",
                &json!({"prototype_id":"call_346434_fc_0_0"}),
            )
            .unwrap();
        assert_eq!(recovery["recovery"]["artifacts"][0]["prototype_id"], id);
        std::thread::sleep(Duration::from_secs(30));
        let initial = artifacts
            .execute("inspect_prototype", &json!({"prototype_id":id}))
            .unwrap();
        let price = initial["controls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["type"] == "range" && c["label"].as_str().unwrap_or("").contains("Price"))
            .unwrap();
        assert_eq!(price["value"], "45");
        let changed=artifacts.execute("set_prototype_control",&json!({"prototype_id":id,"snapshot_id":initial["snapshot_id"],"control_id":price["control_id"],"value":"100"})).unwrap();
        assert!(
            changed["output"].as_str().unwrap().contains("25,000"),
            "{changed}"
        );
        println!("TRUFFLE_CHANGED={changed}");
        std::thread::sleep(Duration::from_secs(10));
        let undone=artifacts.execute("undo_prototype_control",&json!({"prototype_id":id,"snapshot_id":changed["snapshot_id"],"undo_id":changed["undo_id"]})).unwrap();
        assert!(
            undone["output"].as_str().unwrap().contains("11,250"),
            "{undone}"
        );
        println!("TRUFFLE_ACCEPTANCE=passed");
    }
}
