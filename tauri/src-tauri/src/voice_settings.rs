use minutes_core::{config::VoiceLiveConfig, Config};
use serde::Serialize;
use tauri::{Emitter, Manager};

pub(crate) fn available() -> bool {
    cfg!(all(feature = "voice-live", target_os = "macos"))
}

pub(crate) fn desktop_voice_name(configured: &str) -> &str {
    if configured.trim().is_empty() {
        "Aoede"
    } else {
        configured.trim()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoiceSettings {
    available: bool,
    enabled: bool,
    needs_consent: bool,
    provider: String,
    voice_name: String,
}

fn settings(config: VoiceLiveConfig) -> VoiceSettings {
    VoiceSettings {
        available: available(),
        enabled: config.enabled && config.allow_cloud,
        needs_consent: !config.allow_cloud,
        provider: if config.provider == "gemini" {
            "Google".into()
        } else {
            config.provider
        },
        voice_name: desktop_voice_name(&config.voice_name).into(),
    }
}

#[tauri::command]
pub(crate) fn cmd_voice_settings() -> VoiceSettings {
    settings(Config::load().voice_live)
}

fn update_enabled(
    config: &mut VoiceLiveConfig,
    enabled: bool,
    consent: bool,
) -> Result<(), String> {
    if enabled && config.provider != "gemini" {
        return Err("Choose Gemini as the Voice provider in your Minutes config.".into());
    }
    if enabled && !config.allow_cloud && !consent {
        return Err("Review and accept Voice privacy before enabling Voice.".into());
    }
    config.enabled = enabled;
    if enabled && consent {
        config.allow_cloud = true;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn cmd_set_voice_enabled(
    app: tauri::AppHandle,
    enabled: bool,
    consent: bool,
) -> Result<VoiceSettings, String> {
    if !available() {
        return Err("Desktop Voice is unavailable in this build.".into());
    }
    let mut config = Config::load();
    update_enabled(&mut config.voice_live, enabled, consent)?;
    config.save().map_err(|error| error.to_string())?;
    if !enabled {
        let state = app.state::<crate::commands::AppState>();
        crate::commands::cmd_stop_voice(app.clone(), state)?;
    }
    Ok(settings(config.voice_live))
}

#[tauri::command]
pub(crate) fn cmd_set_voice_name(voice_name: String) -> Result<(), String> {
    if !available() {
        return Err("Desktop Voice is unavailable in this build.".into());
    }
    if voice_name.len() > 64
        || !voice_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
    {
        return Err("Choose a voice from the menu.".into());
    }
    let mut config = Config::load();
    config.voice_live.voice_name = voice_name;
    config.save().map_err(|error| error.to_string())
}

pub(crate) fn toggle(app: &tauri::AppHandle) -> Result<(), String> {
    if !available() {
        return Err("Desktop Voice is unavailable in this build.".into());
    }
    let state = app.state::<crate::commands::AppState>();
    if state
        .voice_active
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return crate::commands::cmd_stop_voice(app.clone(), state).map(|_| ());
    }
    let config = Config::load();
    if !config.voice_live.enabled || !config.voice_live.allow_cloud {
        crate::show_main_window(app);
        return app
            .emit_to("main", "voice:open-settings", ())
            .map_err(|e| e.to_string());
    }
    crate::commands::cmd_start_voice(app.clone(), state).map(|_| ())
}

#[tauri::command]
pub(crate) fn cmd_toggle_desktop_voice(app: tauri::AppHandle) -> Result<(), String> {
    toggle(&app)
}

#[tauri::command]
pub(crate) async fn cmd_test_voice() -> Result<String, String> {
    if !available() {
        return Err("Desktop Voice is unavailable in this build.".into());
    }
    #[cfg(feature = "voice-live")]
    {
        tauri::async_runtime::spawn_blocking(preview)
            .await
            .map_err(|_| "The voice test could not run.".to_string())?
    }
    #[cfg(not(feature = "voice-live"))]
    Err("Desktop Voice is unavailable in this build.".into())
}

#[cfg(feature = "voice-live")]
fn preview() -> Result<String, String> {
    use base64::Engine as _;
    use minutes_core::voice_live::protocol::{
        LiveClient, ServerEvent, SessionSetup, OUTPUT_SAMPLE_RATE,
    };
    use std::time::{Duration, Instant};

    let config = Config::load();
    let voice = &config.voice_live;
    if !voice.enabled || !voice.allow_cloud || voice.provider != "gemini" {
        return Err("Enable Voice before testing it.".into());
    }
    crate::secret_store::hydrate_voice_api_key_env(&crate::secret_store::voice_api_key_env(
        &config,
    )?);
    let key = minutes_core::voice_live::api_key(&config).map_err(|error| error.to_string())?;
    let client = LiveClient::connect(&SessionSetup {
        model: voice.model.clone(),
        api_key: key,
        thinking_level: voice.thinking_level.clone(),
        system_instruction:
            "You are demonstrating a voice. Speak only the supplied sample sentence.".into(),
        function_declarations: Vec::new(),
        language: voice.language.clone(),
        voice_name: desktop_voice_name(&voice.voice_name).into(),
        manual_activity: true,
        proactive_audio: voice.proactive_audio,
        start_sensitivity: String::new(),
        end_sensitivity: String::new(),
        resume_handle: None,
    })
    .map_err(|_| {
        "Could not connect to Google. Check the Gemini key and your network.".to_string()
    })?;
    let result = (|| {
        let deadline = Instant::now() + Duration::from_secs(20);
        let capacity = OUTPUT_SAMPLE_RATE as usize * 2 * 8;
        let mut audio = Vec::new();
        let mut requested = false;
        while let Ok(event) = client.inbox.recv_deadline(deadline) {
            match event {
                ServerEvent::SetupComplete if !requested => {
                    client
                        .send_text_turn("Say: Hello, I'm the voice you selected for Minutes.")
                        .map_err(|_| "Could not request the voice sample.".to_string())?;
                    requested = true;
                }
                ServerEvent::Audio(bytes) if requested => {
                    audio.extend(bytes.into_iter().take(capacity - audio.len()));
                    if audio.len() == capacity {
                        break;
                    }
                }
                ServerEvent::TurnComplete if !audio.is_empty() => break,
                ServerEvent::Error(_) | ServerEvent::Closed(_) => {
                    return Err(
                        "Google closed the voice test. Check the Gemini key and try again.".into(),
                    );
                }
                _ => {}
            }
        }
        audio.truncate(audio.len() & !1);
        if audio.is_empty() {
            return Err("Google did not return a voice sample.".into());
        }
        Ok(base64::engine::general_purpose::STANDARD.encode(audio))
    })();
    client.close();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_voice_enable_requires_first_consent_and_keeps_it_after_turning_off() {
        let mut voice = VoiceLiveConfig::default();
        assert!(update_enabled(&mut voice, true, false).is_err());
        assert!(!voice.enabled && !voice.allow_cloud);
        update_enabled(&mut voice, false, true).unwrap();
        assert!(!voice.allow_cloud);
        update_enabled(&mut voice, true, true).unwrap();
        update_enabled(&mut voice, false, false).unwrap();
        assert!(!voice.enabled && voice.allow_cloud);
        update_enabled(&mut voice, true, false).unwrap();
        assert!(voice.enabled);
    }
}
