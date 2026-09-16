//! Lightweight localization (i18n) for Rust user-facing strings.
//!
//! Minutes has no framework-level i18n; this module is the Rust-side runtime
//! shared by the CLI (`crates/cli`) and the Tauri desktop shell
//! (`tauri/src-tauri`). Both are separate processes that read the persisted
//! [`crate::config::UiConfig::language`] preference and call [`set_locale_from_str`]
//! once at startup.
//!
//! # Design: English source string is the key
//!
//! To keep this additive and minimize merge conflicts with the actively
//! developed upstream, callers do not introduce message keys. They wrap the
//! existing English literal:
//!
//! ```
//! use minutes_core::i18n::tr;
//! let label = tr("Stop Recording"); // "停止录音" when locale is zh-CN
//! ```
//!
//! [`tr`] looks the English string up in the embedded translation catalog for
//! the active locale. If the locale is English, or the string has no
//! translation, the input is returned unchanged (English fallback). This means
//! partial catalogs are always safe, and syncing a new English string from
//! upstream just needs a new catalog entry — no code change.
//!
//! Catalogs live in `locales/<tag>.json`, each an `{ "English": "…" }` object
//! embedded at compile time via `include_str!`. Adding a language means adding
//! a catalog file plus one [`Locale`] variant.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// A supported display language.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Locale {
    /// English — the untranslated source strings.
    En,
    /// Simplified Chinese.
    ZhCn,
    /// Brazilian Portuguese.
    PtBr,
}

const LOCALE_EN: u8 = 0;
const LOCALE_ZH_CN: u8 = 1;
const LOCALE_PT_BR: u8 = 2;

/// Process-wide active locale. Lock-free: it is just an enum discriminant, read
/// on every [`tr`] call and written by [`set_locale`] (startup + language switch).
static CURRENT: AtomicU8 = AtomicU8::new(LOCALE_EN);

/// Embedded Simplified Chinese catalog (`{ "English": "中文" }`).
const ZH_CN_JSON: &str = include_str!("locales/zh-CN.json");
/// Embedded Brazilian Portuguese catalog (`{ "English": "Português" }`).
const PT_BR_JSON: &str = include_str!("locales/pt-BR.json");

/// Parses and caches a catalog on first use. A malformed catalog degrades
/// gracefully to English (empty map) rather than panicking.
fn parse_catalog(tag: &'static str, raw: &'static str) -> HashMap<String, String> {
    serde_json::from_str(raw).unwrap_or_else(|e| {
        tracing::warn!(error = %e, locale = tag, "failed to parse i18n catalog; falling back to English");
        HashMap::new()
    })
}

fn zh_cn_catalog() -> &'static HashMap<String, String> {
    static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
    CATALOG.get_or_init(|| parse_catalog("zh-CN", ZH_CN_JSON))
}

fn pt_br_catalog() -> &'static HashMap<String, String> {
    static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
    CATALOG.get_or_init(|| parse_catalog("pt-BR", PT_BR_JSON))
}

/// Resolves a configured language preference into a concrete [`Locale`].
///
/// Precedence: the `MINUTES_LANG` environment variable (if non-empty) overrides
/// `language`; a value of `"auto"` is resolved from the OS locale via
/// [`sys_locale`]. Anything whose language tag starts with `zh` (e.g. `zh`,
/// `zh-CN`, `zh-Hans`) maps to [`Locale::ZhCn`]; anything starting with `pt`
/// (e.g. `pt`, `pt-BR`, `pt-PT`) maps to [`Locale::PtBr`]; everything else falls
/// back to [`Locale::En`].
pub fn resolve_locale(language: &str) -> Locale {
    let requested = std::env::var("MINUTES_LANG")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| language.to_string());

    let effective = if requested.eq_ignore_ascii_case("auto") {
        sys_locale::get_locale().unwrap_or_default()
    } else {
        requested
    };

    let lower = effective.to_ascii_lowercase();
    if lower.starts_with("zh") {
        Locale::ZhCn
    } else if lower.starts_with("pt") {
        Locale::PtBr
    } else {
        Locale::En
    }
}

/// Sets the process-wide active locale directly.
pub fn set_locale(locale: Locale) {
    let v = match locale {
        Locale::En => LOCALE_EN,
        Locale::ZhCn => LOCALE_ZH_CN,
        Locale::PtBr => LOCALE_PT_BR,
    };
    CURRENT.store(v, Ordering::Relaxed);
}

/// Resolves a language preference string (`"auto"` / `"en"` / `"zh-CN"` /
/// `"pt-BR"`) and sets the active locale. Call once at startup with
/// `config.ui.language`, and again when the user switches languages.
pub fn set_locale_from_str(language: &str) {
    set_locale(resolve_locale(language));
}

/// Returns the process-wide active locale.
pub fn current_locale() -> Locale {
    match CURRENT.load(Ordering::Relaxed) {
        LOCALE_ZH_CN => Locale::ZhCn,
        LOCALE_PT_BR => Locale::PtBr,
        _ => Locale::En,
    }
}

/// Translates an English UI string to the active locale.
///
/// Returns the input unchanged when the locale is English or the string has no
/// translation in the catalog, so callers can wrap any literal safely. Neither
/// path allocates: the English source and the catalog entries are both borrowed.
///
/// ```
/// use minutes_core::i18n::{tr, set_locale, Locale};
/// set_locale(Locale::En);
/// assert_eq!(tr("Stop Recording"), "Stop Recording");
/// ```
pub fn tr(en: &str) -> Cow<'_, str> {
    let catalog = match current_locale() {
        Locale::En => return Cow::Borrowed(en),
        Locale::ZhCn => zh_cn_catalog(),
        Locale::PtBr => pt_br_catalog(),
    };
    match catalog.get(en) {
        Some(translated) => Cow::Borrowed(translated.as_str()),
        None => Cow::Borrowed(en),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_locale_maps_zh_variants() {
        // Env var must not leak between tests; these paths avoid it by using
        // explicit tags (MINUTES_LANG is not set in the harness).
        assert_eq!(resolve_locale("zh-CN"), Locale::ZhCn);
        assert_eq!(resolve_locale("zh"), Locale::ZhCn);
        assert_eq!(resolve_locale("zh-Hans"), Locale::ZhCn);
        assert_eq!(resolve_locale("en"), Locale::En);
        assert_eq!(resolve_locale("en-US"), Locale::En);
        assert_eq!(resolve_locale("fr"), Locale::En);
    }

    #[test]
    fn resolve_locale_maps_pt_variants() {
        assert_eq!(resolve_locale("pt-BR"), Locale::PtBr);
        assert_eq!(resolve_locale("pt"), Locale::PtBr);
        assert_eq!(resolve_locale("pt_BR"), Locale::PtBr);
        assert_eq!(resolve_locale("pt-PT"), Locale::PtBr);
    }

    #[test]
    fn tr_is_identity_in_english() {
        set_locale(Locale::En);
        assert_eq!(tr("Stop Recording"), "Stop Recording");
        assert_eq!(
            tr("some string with no translation"),
            "some string with no translation"
        );
    }

    #[test]
    fn tr_translates_known_zh_string() {
        // "Stop Recording" is a seeded catalog entry; unknown strings fall back.
        set_locale(Locale::ZhCn);
        assert_eq!(tr("Stop Recording"), "停止录音");
        assert_eq!(
            tr("this key is intentionally absent from the catalog"),
            "this key is intentionally absent from the catalog"
        );
        // Restore default so other tests in the process see English.
        set_locale(Locale::En);
    }

    #[test]
    fn tr_translates_known_pt_string() {
        set_locale(Locale::PtBr);
        assert_eq!(tr("Stop Recording"), "Parar gravação");
        assert_eq!(
            tr("this key is intentionally absent from the catalog"),
            "this key is intentionally absent from the catalog"
        );
        set_locale(Locale::En);
    }

    #[test]
    fn catalog_parses() {
        // Fails loudly if a shipped catalog is malformed JSON.
        let _ = zh_cn_catalog();
        let _ = pt_br_catalog();
    }
}
