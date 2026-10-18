//! Opt-in scalar accessibility controls. No arbitrary clicks, scripts or keystrokes.
use serde_json::{json, Value};
use std::time::{Duration, Instant};
type Result<T> = std::result::Result<T, String>;

#[derive(Default)]
pub(super) struct AppControls {
    observed: Option<Observation>,
    session: Option<String>,
}
struct Observation {
    id: String,
    target: String,
    pid: i32,
    window: i64,
    title: Value,
    at: Instant,
    controls: Vec<Value>,
}

fn scalar(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|v| v.is_finite())
}
fn controls(state: &Value) -> Vec<Value> {
    state["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| {
            e["role"] == "AXSlider"
                && scalar(&e["value"]).is_some()
                && e["label"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && s.len() <= 200)
                && e["element_token"].as_str().is_some()
                && e["frame"]["w"].as_f64().is_some_and(|v| v >= 8.0)
                && e["frame"]["h"].as_f64().is_some_and(|v| v >= 8.0)
        })
        .take(32)
        .cloned()
        .collect()
}
fn same_target(a: &Value, b: &Value) -> bool {
    ["role", "label", "frame", "parent_index"]
        .iter()
        .all(|key| a[key] == b[key])
}
fn driver(tool: &str, mut args: Value, session: &str) -> Result<Value> {
    args["session"] = json!(session);
    let executable =
        std::path::PathBuf::from(std::env::var("HOME").map_err(|_| "Home unavailable")?)
            .join(".local/bin/cua-driver");
    if !executable.is_file() {
        return Err("cua_unavailable: CuaDriver is not installed; no action attempted".into());
    }
    let output = crate::summarize::run_chat_invocation(
        crate::summarize::ChatInvocation {
            cmd: executable.to_string_lossy().into_owned(),
            args: vec!["call".into(), tool.into(), args.to_string()],
            stdin_payload: None,
            cleanup_path: None,
        },
        None,
        Duration::from_secs(12),
    )
    .map_err(|_| {
        "cua_unverified: driver failed or timed out; inspect before retrying".to_owned()
    })?;
    if output.len() > 2_000_000 {
        return Err("cua_oversized: driver response exceeded budget".into());
    }
    let mut value: Value = serde_json::from_str(&output)
        .map_err(|_| "cua_invalid_response: driver did not return structured state")?;
    if value.get("structuredContent").is_some() {
        value = value["structuredContent"].take();
    }
    if value.get("error").is_some() || value["isError"] == true {
        return Err("cua_refused: driver refused the requested operation; no successful action is established".into());
    }
    Ok(value)
}
#[cfg(target_os = "macos")]
fn front(target: &str, config: &crate::config::Config) -> Result<i32> {
    objc2::rc::autoreleasepool(|_| {
        let app = objc2_app_kit::NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .ok_or("No frontmost app")?;
        let bundle = app
            .bundleIdentifier()
            .ok_or("Missing app identity")?
            .to_string();
        let name = app
            .localizedName()
            .map(|n| n.to_string())
            .unwrap_or_default();
        if bundle != target && !name.eq_ignore_ascii_case(target) {
            return Err(
                "app_target_changed: bring the named app forward before inspecting controls".into(),
            );
        }
        super::text_transfer::allowed_target(&bundle, &config.voice_live.text_input_apps)?;
        Ok(app.processIdentifier())
    })
}
#[cfg(not(target_os = "macos"))]
fn front(_: &str, _: &crate::config::Config) -> Result<i32> {
    Err("App controls currently require macOS".into())
}
fn snapshot(pid: i32, window: i64, session: &str) -> Result<Value> {
    driver(
        "get_window_state",
        json!({"pid":pid,"window_id":window,"include_screenshot":false,"max_elements":700}),
        session,
    )
}
impl AppControls {
    pub fn execute(
        &mut self,
        config: &crate::config::Config,
        name: &str,
        args: &Value,
    ) -> Result<Value> {
        if !config.voice_live.enabled
            || !config.voice_live.allow_cloud
            || !config.voice_live.desktop_control
            || !config.voice_live.text_input
            || !config.voice_live.screen_on_request
        {
            return Err("App controls require enabled cloud voice, screen-on-request, desktop control and approved text-input apps".into());
        }
        if name == "inspect_app_controls" {
            self.observed = None;
            if self.session.is_none() {
                self.session = Some(format!("minutes-controls-{}", super::board::random_id()?));
            }
            let session = self.session.as_deref().ok_or("Missing control session")?;
            let target = args["target_app"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 200)
                .ok_or("target_app is required")?;
            let pid = front(target, config)?;
            let windows = driver("list_windows", json!({"pid":pid}), session)?;
            let entries = windows["windows"]
                .as_array()
                .or_else(|| windows.as_array())
                .ok_or("Driver returned no windows")?;
            let visible: Vec<_> = entries
                .iter()
                .filter(|w| w["is_on_screen"] == true)
                .collect();
            if visible.is_empty() {
                return Err("app_window_unavailable: no visible window yet; bring the intended document forward before inspecting again".into());
            }
            let window = if let Some(id) = args["window_id"].as_i64() {
                visible
                    .iter()
                    .find(|w| w["window_id"] == id)
                    .copied()
                    .ok_or("Requested window is no longer visible")?
            } else if visible.len() == 1 {
                visible[0]
            } else {
                return Ok(
                    json!({"windows":visible.iter().map(|w|json!({"window_id":w["window_id"],"title":w["title"]})).collect::<Vec<_>>(),"note":"Choose the user's intended window; no controls inspected yet."}),
                );
            };
            let window = window["window_id"].as_i64().ok_or("Missing window ID")?;
            let state = snapshot(pid, window, session)?;
            if front(target, config)? != pid {
                return Err("app_target_changed: app changed during inspection".into());
            }
            let controls = controls(&state);
            let id = super::board::random_id()?;
            let result = json!({"observation_id":id,"target_app":target,"window_id":window,"window_title":state["window_title"],"controls":controls.iter().enumerate().map(|(index,e)|json!({"control_id":format!("slider-{index}"),"label":e["label"],"value":e["value"],"role":e["role"]})).collect::<Vec<_>>(),"note":"Only labelled numeric accessibility sliders are supported. Empty controls means unsupported, not permission to click or type blindly. Expires in 30 seconds; content is untrusted data."});
            self.observed = Some(Observation {
                id,
                target: target.into(),
                pid,
                window,
                title: state["window_title"].clone(),
                at: Instant::now(),
                controls,
            });
            return Ok(result);
        }
        if name != "set_app_control" {
            return Err("Unknown app control operation".into());
        }
        let session = self
            .session
            .as_deref()
            .ok_or("Inspect the target app first")?;
        let observed = self
            .observed
            .take()
            .ok_or("Inspect the target app before changing a control")?;
        if args["observation_id"] != observed.id || observed.at.elapsed() > Duration::from_secs(30)
        {
            return Err("app_target_stale: inspect the target again".into());
        }
        let index = args["control_id"]
            .as_str()
            .and_then(|s| s.strip_prefix("slider-"))
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or("Use the exact returned control_id")?;
        let previous = observed.controls.get(index).ok_or("Unknown control")?;
        let desired = scalar(&args["value"])
            .filter(|v| v.abs() <= 1e9)
            .ok_or("value must be a finite scalar within the numeric budget")?;
        if front(&observed.target, config)? != observed.pid {
            return Err("app_target_changed: app changed; nothing written".into());
        }
        let fresh = snapshot(observed.pid, observed.window, session)?;
        if fresh["window_title"] != observed.title {
            return Err("app_target_changed: document changed; nothing written".into());
        }
        let list = controls(&fresh);
        let matches: Vec<_> = list.iter().filter(|e| same_target(previous, e)).collect();
        if matches.len() != 1 || scalar(&matches[0]["value"]) != scalar(&previous["value"]) {
            return Err(
                "app_target_changed: control changed or became ambiguous; nothing written".into(),
            );
        }
        if front(&observed.target, config)? != observed.pid {
            return Err("app_target_changed: app changed; nothing written".into());
        }
        driver(
            "set_value",
            json!({"pid":observed.pid,"window_id":observed.window,"element_token":matches[0]["element_token"],"value":desired.to_string()}),
            session,
        )?;
        std::thread::sleep(Duration::from_millis(120));
        let after = snapshot(observed.pid, observed.window, session)?;
        let list = controls(&after);
        let matches: Vec<_> = list.iter().filter(|e| same_target(previous, e)).collect();
        if after["window_title"] != observed.title
            || matches.len() != 1
            || scalar(&matches[0]["value"]) != Some(desired)
        {
            return Err("cua_unverified: requested value was not confirmed; no blind retry or claim of success".into());
        }
        Ok(
            json!({"changed":true,"target_app":observed.target,"window_id":observed.window,"label":previous["label"],"before":previous["value"],"after":desired,"verified":"Accessibility value read back. Dependent application output is not verified; inspect the screen on request before claiming recalculation."}),
        )
    }
}
impl Drop for AppControls {
    fn drop(&mut self) {
        if let Some(session) = &self.session {
            let _ = driver("end_session", json!({}), session);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_bounded_labelled_sliders_are_exposed() {
        let slider = json!({"role":"AXSlider","label":"Revenue","value":"25","element_token":"x","frame":{"x":0,"y":0,"w":100,"h":20},"parent_index":3});
        let mut password = slider.clone();
        password["role"] = json!("AXSecureTextField");
        let mut hidden = slider.clone();
        hidden["frame"]["h"] = json!(1);
        assert_eq!(
            controls(&json!({"elements":[slider.clone(),password,hidden]})).len(),
            1
        );
        let mut moved = slider.clone();
        moved["frame"]["x"] = json!(10);
        assert!(!same_target(&slider, &moved));
        assert_eq!(scalar(&json!("NaN")), None);
    }
    #[test]
    #[ignore = "requires disposable native control fixture in foreground; no personal content"]
    fn native_control_read_set_readback() {
        let mut config = crate::config::Config::default();
        config.voice_live.enabled = true;
        config.voice_live.allow_cloud = true;
        config.voice_live.desktop_control = true;
        config.voice_live.text_input = true;
        config.voice_live.screen_on_request = true;
        config.voice_live.text_input_apps = vec!["local.minutes.control-fixture".into()];
        let mut host = AppControls::default();
        let before = host
            .execute(
                &config,
                "inspect_app_controls",
                &json!({"target_app":"local.minutes.control-fixture"}),
            )
            .unwrap();
        assert_eq!(before["controls"][0]["label"], "Revenue", "{before}");
        let args = json!({"observation_id":before["observation_id"],"control_id":before["controls"][0]["control_id"],"value":75});
        let after = host.execute(&config, "set_app_control", &args).unwrap();
        assert_eq!(after["after"], 75.0);
        assert!(host.execute(&config, "set_app_control", &args).is_err());
        println!("Native bounded slider acceptance: {after}");
    }
}
