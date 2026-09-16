use serde::Serialize;

pub const OPENAI_COMPATIBLE_API_KEY_ENV: &str =
    minutes_core::config::OPENAI_COMPATIBLE_DESKTOP_API_KEY_ENV;

const OPENAI_COMPATIBLE_SERVICE: &str = "Minutes OpenAI-compatible Summaries";
const OPENAI_COMPATIBLE_ACCOUNT: &str = "default";

/// The Keychain item backing the voice assistant's Gemini key. Separate from
/// the summarization key on purpose: they are different providers with
/// different blast radii, and revoking one must not disturb the other.
const VOICE_SERVICE: &str = "Minutes Voice (Gemini)";
const VOICE_ACCOUNT: &str = "default";

/// The environment variable `voice_live` reads when config leaves
/// `voice_live.api_key_env` empty. Kept in sync with
/// `minutes_core::voice_live::api_key`.
pub const VOICE_API_KEY_ENV_DEFAULT: &str = "GEMINI_API_KEY";

#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// One Keychain-backed secret. Naming the item and its environment variable
/// together is what lets the two secrets share every code path below without
/// either one being able to read or clobber the other's item.
#[derive(Copy, Clone)]
struct SecretSlot {
    service: &'static str,
    account: &'static str,
}

const OPENAI_COMPATIBLE_SLOT: SecretSlot = SecretSlot {
    service: OPENAI_COMPATIBLE_SERVICE,
    account: OPENAI_COMPATIBLE_ACCOUNT,
};

const VOICE_SLOT: SecretSlot = SecretSlot {
    service: VOICE_SERVICE,
    account: VOICE_ACCOUNT,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiCompatibleSecretStatus {
    pub supported: bool,
    pub key_set: bool,
    pub stored_key_set: bool,
    pub storage_label: &'static str,
    pub env_var: &'static str,
    pub message: String,
}

/// Status of the voice key. Shaped like the summarization one, but its
/// `env_var` is owned rather than static: the variable voice reads is
/// configurable, so the answer depends on the loaded config.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSecretStatus {
    pub supported: bool,
    pub key_set: bool,
    pub stored_key_set: bool,
    pub storage_label: &'static str,
    pub env_var: String,
    pub message: String,
}

pub fn hydrate_openai_compatible_api_key_env() -> OpenAiCompatibleSecretStatus {
    let existing_env_key = std::env::var(OPENAI_COMPATIBLE_API_KEY_ENV).is_ok();
    let stored_key = load_openai_compatible_api_key().ok().flatten();

    if !existing_env_key {
        if let Some(key) = stored_key.as_deref() {
            std::env::set_var(OPENAI_COMPATIBLE_API_KEY_ENV, key);
        }
    }

    openai_compatible_secret_status()
}

pub fn openai_compatible_secret_status() -> OpenAiCompatibleSecretStatus {
    let env_key_set = std::env::var(OPENAI_COMPATIBLE_API_KEY_ENV).is_ok();
    let stored_key_set = load_openai_compatible_api_key().ok().flatten().is_some();
    let key_set = env_key_set || stored_key_set;

    OpenAiCompatibleSecretStatus {
        supported: keychain_supported(),
        key_set,
        stored_key_set,
        storage_label: storage_label(),
        env_var: OPENAI_COMPATIBLE_API_KEY_ENV,
        message: secret_status_message(key_set, stored_key_set, OPENAI_COMPATIBLE_API_KEY_ENV),
    }
}

pub fn save_openai_compatible_api_key(api_key: &str) -> Result<(), String> {
    if api_key.trim().is_empty() {
        return Err("Paste an API key first.".into());
    }

    save_secret(OPENAI_COMPATIBLE_SLOT, api_key)
}

pub fn clear_openai_compatible_api_key() -> Result<(), String> {
    clear_secret(OPENAI_COMPATIBLE_SLOT)
}

/// The variable voice reads for its key: whatever config names, or the
/// documented default when config leaves it blank.
pub fn voice_api_key_env(config: &minutes_core::config::Config) -> String {
    let configured = config.voice_live.api_key_env.trim();
    if configured.is_empty() {
        VOICE_API_KEY_ENV_DEFAULT.to_string()
    } else {
        configured.to_string()
    }
}

/// Put the stored voice key into the environment so the engine can read it.
///
/// The desktop app is launched from Finder, which gives it no shell
/// environment, so without this a key that works in `minutes talk` is simply
/// absent in the app. An environment variable that is already set wins: a
/// developer running the app from a terminal should not be overridden by a
/// stale Keychain item.
pub fn hydrate_voice_api_key_env(env_var: &str) -> VoiceSecretStatus {
    if std::env::var(env_var).is_err() {
        if let Some(key) = load_voice_api_key().ok().flatten() {
            std::env::set_var(env_var, key);
        }
    }

    voice_secret_status(env_var)
}

pub fn voice_secret_status(env_var: &str) -> VoiceSecretStatus {
    let env_key_set = std::env::var(env_var).is_ok();
    let stored_key_set = load_voice_api_key().ok().flatten().is_some();
    let key_set = env_key_set || stored_key_set;

    VoiceSecretStatus {
        supported: keychain_supported(),
        key_set,
        stored_key_set,
        storage_label: storage_label(),
        env_var: env_var.to_string(),
        message: secret_status_message(key_set, stored_key_set, env_var),
    }
}

pub fn save_voice_api_key(api_key: &str) -> Result<(), String> {
    if api_key.trim().is_empty() {
        return Err("Paste an API key first.".into());
    }

    save_secret(VOICE_SLOT, api_key)
}

pub fn clear_voice_api_key() -> Result<(), String> {
    clear_secret(VOICE_SLOT)
}

pub fn load_voice_api_key() -> Result<Option<String>, String> {
    load_secret(VOICE_SLOT)
}

fn secret_status_message(key_set: bool, stored_key_set: bool, env_var: &str) -> String {
    if stored_key_set {
        return "Key saved in macOS Keychain.".into();
    }
    if key_set {
        return format!("Using {} from this app environment.", env_var);
    }
    if keychain_supported() {
        return "Paste a key once. Minutes stores it in macOS Keychain.".into();
    }

    format!(
        "Keychain storage is unavailable on this OS. Set {} before launching Minutes.",
        env_var
    )
}

#[cfg(target_os = "macos")]
fn keychain_supported() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
fn keychain_supported() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn storage_label() -> &'static str {
    "macOS Keychain"
}

#[cfg(not(target_os = "macos"))]
fn storage_label() -> &'static str {
    "environment variable"
}

pub fn load_openai_compatible_api_key() -> Result<Option<String>, String> {
    load_secret(OPENAI_COMPATIBLE_SLOT)
}

#[cfg(target_os = "macos")]
fn load_secret(slot: SecretSlot) -> Result<Option<String>, String> {
    let password =
        match security_framework::passwords::get_generic_password(slot.service, slot.account) {
            Ok(password) => password,
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => return Ok(None),
            Err(error) => return Err(format!("Could not read Keychain: {}", error)),
        };

    let key = String::from_utf8(password)
        .map_err(|error| format!("Keychain returned invalid UTF-8: {}", error))?
        .trim_end_matches(['\r', '\n'])
        .to_string();

    if key.is_empty() {
        Ok(None)
    } else {
        Ok(Some(key))
    }
}

#[cfg(not(target_os = "macos"))]
fn load_secret(_slot: SecretSlot) -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(target_os = "macos")]
fn save_secret(slot: SecretSlot, api_key: &str) -> Result<(), String> {
    security_framework::passwords::set_generic_password(
        slot.service,
        slot.account,
        api_key.as_bytes(),
    )
    .map_err(|error| format!("Could not save API key to Keychain: {}", error))
}

#[cfg(not(target_os = "macos"))]
fn save_secret(_slot: SecretSlot, _api_key: &str) -> Result<(), String> {
    Err("Keychain storage is unavailable on this OS. Set the API key environment variable before launching Minutes.".to_string())
}

#[cfg(target_os = "macos")]
fn clear_secret(slot: SecretSlot) -> Result<(), String> {
    match security_framework::passwords::delete_generic_password(slot.service, slot.account) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(error) => Err(format!("Could not update Keychain: {}", error)),
    }
}

#[cfg(not(target_os = "macos"))]
fn clear_secret(_slot: SecretSlot) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two secrets must never resolve to the same Keychain item. If they
    /// did, saving a voice key would silently replace the summarization key.
    #[test]
    fn the_two_secrets_are_distinct_keychain_items() {
        assert_ne!(
            (
                OPENAI_COMPATIBLE_SLOT.service,
                OPENAI_COMPATIBLE_SLOT.account
            ),
            (VOICE_SLOT.service, VOICE_SLOT.account),
        );
    }

    #[test]
    fn blank_config_falls_back_to_the_documented_variable() {
        let mut config = minutes_core::config::Config::default();
        config.voice_live.api_key_env = "   ".into();
        assert_eq!(voice_api_key_env(&config), VOICE_API_KEY_ENV_DEFAULT);

        config.voice_live.api_key_env = " MY_KEY ".into();
        assert_eq!(voice_api_key_env(&config), "MY_KEY");
    }

    /// The message must name the variable the caller actually asked about,
    /// not a hardcoded one, or a user with a custom variable is told to set
    /// the wrong thing.
    #[test]
    fn the_unavailable_message_names_the_callers_variable() {
        let message = secret_status_message(false, false, "MY_KEY");
        if keychain_supported() {
            assert!(message.contains("Keychain"));
        } else {
            assert!(message.contains("MY_KEY"), "{}", message);
        }
        assert!(secret_status_message(true, false, "MY_KEY").contains("MY_KEY"));
    }
}
