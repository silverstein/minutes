//! Optional cosmetic customizations from `customize.toml`, beside `config.toml`.
//!
//! Nothing in this file can stop Minutes launching or change what it records.
//! Each `[section]` decodes on its own, so a bad value loses that value, a bad
//! section loses that section, and a broken file loses the customizations.
//! What was ignored is reported in [`Customization::problems`].
//!
//! A new section belongs here only if its worst failure is cosmetic. Commands,
//! paths, permissions, consent, privacy, capture, and hooks stay in
//! `config.toml`, where strict loading protects them.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Largest file the reader will load.
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// WCAG AA for UI components and large text, measured against `--bg`. The app
/// also sets `var(--bg)` text on accent buttons, which is the same color pair.
const MIN_ACCENT_CONTRAST: f64 = 3.0;
const LIGHT_BACKGROUND: Rgb = Rgb(0xF8, 0xF4, 0xED);
const DARK_BACKGROUND: Rgb = Rgb(0x0D, 0x0D, 0x0B);

/// Whether windows follow the OS or stay in one appearance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follow the OS appearance.
    #[default]
    Auto,
    /// Always use the light appearance.
    Light,
    /// Always use the dark appearance.
    Dark,
}

impl ThemeMode {
    /// Parse the file and Settings spelling.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// The `[theme]` section. Absent accents keep the built-in colors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ThemeSettings {
    /// Forced appearance, if any.
    pub mode: ThemeMode,
    /// `#RRGGBB` accent for the light appearance.
    pub accent_light: Option<String>,
    /// `#RRGGBB` accent for the dark appearance.
    pub accent_dark: Option<String>,
}

/// Everything that applied from the file, plus what was ignored and why.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Customization {
    /// The `[theme]` section.
    pub theme: ThemeSettings,
    /// Plain-language notes about ignored values, for Settings to show.
    pub problems: Vec<String>,
}

/// Path of the customization file, beside the active `config.toml`.
pub fn customize_path() -> PathBuf {
    crate::config::Config::config_path().with_file_name("customize.toml")
}

/// Load customizations from the default path. Never fails.
pub fn load() -> Customization {
    load_from(&customize_path())
}

/// Load customizations from `path`. A missing file means no customizations.
pub fn load_from(path: &Path) -> Customization {
    let unusable = |detail: String| Customization {
        problems: vec![format!("customize.toml was ignored: {detail}.")],
        ..Customization::default()
    };
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => return unusable("it isn't a file".into()),
        Ok(metadata) if metadata.len() > MAX_FILE_BYTES => {
            return unusable("it's larger than 64 KB".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Customization::default();
        }
        Err(error) => return unusable(format!("it couldn't be read ({error})")),
    }
    match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(raw) => parse(&raw),
            Err(_) => unusable("it isn't valid UTF-8 text".into()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Customization::default(),
        Err(error) => unusable(format!("it couldn't be read ({error})")),
    }
}

/// Decode file contents. Unknown sections and keys are ignored silently, so a
/// file written for a newer version still works on an older one.
pub fn parse(raw: &str) -> Customization {
    let table = match raw.parse::<toml::Table>() {
        Ok(table) => table,
        Err(error) => {
            return Customization {
                problems: vec![format!(
                    "customize.toml was ignored: it isn't valid TOML ({}).",
                    error.message()
                )],
                ..Customization::default()
            };
        }
    };
    let mut customization = Customization::default();
    match table.get("theme") {
        None => {}
        Some(toml::Value::Table(theme)) => {
            customization.theme = decode_theme(theme, &mut customization.problems);
        }
        Some(_) => customization
            .problems
            .push("[theme] was ignored: it should be a table.".into()),
    }
    customization
}

fn decode_theme(table: &toml::Table, problems: &mut Vec<String>) -> ThemeSettings {
    let mode = match table.get("mode") {
        None => ThemeMode::Auto,
        Some(value) => match value.as_str().and_then(ThemeMode::parse) {
            Some(mode) => mode,
            None => {
                problems
                    .push("theme.mode was ignored: use \"auto\", \"light\", or \"dark\".".into());
                ThemeMode::Auto
            }
        },
    };
    let mut accent = |key: &str, appearance: Appearance| match table.get(key) {
        None => None,
        Some(value) => {
            match value
                .as_str()
                .ok_or_else(|| "use a color like \"#C96B4E\"".to_string())
                .and_then(|text| validate_accent(text, appearance))
            {
                Ok(color) => Some(color),
                Err(reason) => {
                    problems.push(format!("theme.{key} was ignored: {reason}."));
                    None
                }
            }
        }
    };
    ThemeSettings {
        mode,
        accent_light: accent("accent_light", Appearance::Light),
        accent_dark: accent("accent_dark", Appearance::Dark),
    }
}

/// Which appearance an accent is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    /// The light appearance.
    Light,
    /// The dark appearance.
    Dark,
}

impl Appearance {
    fn background(self) -> Rgb {
        match self {
            Self::Light => LIGHT_BACKGROUND,
            Self::Dark => DARK_BACKGROUND,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// Check an accent and return it as uppercase `#RRGGBB`, or say why it's
/// unusable. Only hex is accepted because the value ends up in a stylesheet,
/// where CSS syntax could load remote URLs.
pub fn validate_accent(value: &str, appearance: Appearance) -> Result<String, String> {
    let color =
        Rgb::parse(value.trim()).ok_or_else(|| "use a color like \"#C96B4E\"".to_string())?;
    let contrast = color.contrast(appearance.background());
    if contrast < MIN_ACCENT_CONTRAST {
        return Err(format!(
            "{} has {contrast:.1}:1 contrast against the {} background, and it needs at least {MIN_ACCENT_CONTRAST:.0}:1",
            color.hex(),
            appearance.name()
        ));
    }
    Ok(color.hex())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn parse(value: &str) -> Option<Self> {
        let hex = value.strip_prefix('#')?;
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |range| u8::from_str_radix(&hex[range], 16).ok();
        Some(Self(channel(0..2)?, channel(2..4)?, channel(4..6)?))
    }

    fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    fn rgba(self, alpha: f64) -> String {
        format!("rgba({}, {}, {}, {alpha:.2})", self.0, self.1, self.2)
    }

    /// Move each channel `amount` of the way toward `target`.
    fn mix(self, target: u8, amount: f64) -> Self {
        let channel = |value: u8| {
            (f64::from(value) + (f64::from(target) - f64::from(value)) * amount).round() as u8
        };
        Self(channel(self.0), channel(self.1), channel(self.2))
    }

    fn luminance(self) -> f64 {
        let linear = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.0) + 0.7152 * linear(self.1) + 0.0722 * linear(self.2)
    }

    fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

/// Stylesheet overriding the accent tokens, empty when no accent is set.
///
/// It redefines each tint as well as `--accent`, because the windows define
/// those as fixed `rgba()` values that wouldn't follow `--accent` on their own.
/// `--capture` and `--red` are never overridden: they mean "recording" and
/// "something is wrong". The tripled `:root` outranks each window's own rules
/// regardless of source order.
pub fn theme_css(theme: &ThemeSettings) -> String {
    let mut css = String::new();
    if let Some(color) = theme.accent_light.as_deref().and_then(Rgb::parse) {
        css.push_str(&accent_block(
            "light",
            color,
            color.mix(0, 0.08),
            &[
                ("--accent-soft", 0.10),
                ("--accent-tint", 0.10),
                ("--accent-tint-soft", 0.05),
                ("--accent-border", 0.15),
                ("--accent-border-strong", 0.28),
                ("--accent-ring", 0.14),
                ("--accent-glow", 0.20),
                ("--focus-ring", 0.16),
                ("--focus-ring-soft", 0.10),
                ("--focus-ring-subtle", 0.07),
            ],
        ));
    }
    if let Some(color) = theme.accent_dark.as_deref().and_then(Rgb::parse) {
        css.push_str(&accent_block(
            "dark",
            color,
            color.mix(255, 0.15),
            &[
                ("--accent-soft", 0.10),
                ("--accent-tint", 0.12),
                ("--accent-tint-soft", 0.06),
                ("--accent-border", 0.15),
                ("--accent-border-strong", 0.40),
                ("--accent-ring", 0.18),
                ("--accent-glow", 0.28),
                ("--focus-ring", 0.20),
                ("--focus-ring-soft", 0.12),
                ("--focus-ring-subtle", 0.08),
            ],
        ));
    }
    css
}

fn accent_block(scheme: &str, color: Rgb, hover: Rgb, tints: &[(&str, f64)]) -> String {
    let mut block = format!(
        "@media (prefers-color-scheme: {scheme}) {{ :root:root:root {{ --accent: {}; --accent-hover: {};",
        color.hex(),
        hover.hex()
    );
    for (token, alpha) in tints {
        block.push_str(&format!(" {token}: {};", color.rgba(*alpha)));
    }
    block.push_str(" } }\n");
    block
}

/// Write `[theme]` to `path`, keeping comments and unknown keys. Accents are
/// validated first. A file that doesn't parse is left untouched, because it may
/// hold a hand edit in progress.
pub fn save_theme(path: &Path, theme: &ThemeSettings) -> Result<(), String> {
    let accent_light = theme
        .accent_light
        .as_deref()
        .map(|value| validate_accent(value, Appearance::Light))
        .transpose()
        .map_err(|reason| format!("Light accent: {reason}."))?;
    let accent_dark = theme
        .accent_dark
        .as_deref()
        .map(|value| validate_accent(value, Appearance::Dark))
        .transpose()
        .map_err(|reason| format!("Dark accent: {reason}."))?;

    let original = match std::fs::read_to_string(path) {
        Ok(raw) => Some(raw),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("customize.toml couldn't be read: {error}.")),
    };
    let mut document = original
        .as_deref()
        .unwrap_or_default()
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| {
            "customize.toml isn't valid TOML, so it was left unchanged. Fix or delete it first."
                .to_string()
        })?;
    let section = document
        .entry("theme")
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or_else(|| {
            "[theme] in customize.toml isn't a table, so it was left unchanged.".to_string()
        })?;
    section["mode"] = toml_edit::value(theme.mode.as_str());
    for (key, value) in [("accent_light", accent_light), ("accent_dark", accent_dark)] {
        match value {
            Some(color) => section[key] = toml_edit::value(color),
            None => {
                section.remove(key);
            }
        }
    }

    // Write through a symlink, so a dotfile manager's link stays a link.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let failed = |error: std::io::Error| format!("customize.toml couldn't be saved: {error}.");
    std::fs::create_dir_all(parent).map_err(failed)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(failed)?;
    std::io::Write::write_all(&mut temporary, document.to_string().as_bytes()).map_err(failed)?;
    temporary.as_file().sync_all().map_err(failed)?;
    temporary
        .persist(&target)
        .map_err(|error| failed(error.error))?;
    Ok(())
}

/// Whether the OS itself is in dark mode, ignoring any appearance this app has
/// forced on its own windows. The menu bar follows the OS, so the tray icon
/// needs this answer. `None` where the OS offers no such reading.
#[cfg(target_os = "macos")]
pub fn system_prefers_dark() -> Option<bool> {
    use objc2_foundation::{ns_string, NSUserDefaults};
    let style =
        NSUserDefaults::standardUserDefaults().stringForKey(ns_string!("AppleInterfaceStyle"));
    Some(style.is_some_and(|style| style.to_string().eq_ignore_ascii_case("dark")))
}

/// See the macOS implementation.
#[cfg(not(target_os = "macos"))]
pub fn system_prefers_dark() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_means_no_customization_and_no_problems() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            load_from(&dir.path().join("customize.toml")),
            Customization::default()
        );
    }

    #[test]
    fn empty_customization_produces_no_stylesheet() {
        assert_eq!(theme_css(&ThemeSettings::default()), "");
        assert_eq!(theme_css(&parse("").theme), "");
    }

    #[test]
    fn built_in_accents_pass_their_own_contrast_check() {
        assert_eq!(
            validate_accent("#c96b4e", Appearance::Light).unwrap(),
            "#C96B4E"
        );
        assert_eq!(
            validate_accent("#30D158", Appearance::Dark).unwrap(),
            "#30D158"
        );
    }

    #[test]
    fn valid_file_applies_every_value() {
        let parsed = parse(
            "[theme]\nmode = \"dark\"\naccent_light = \"#C96B4E\"\naccent_dark = \"#30d158\"\n",
        );
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.theme.mode, ThemeMode::Dark);
        assert_eq!(parsed.theme.accent_light.as_deref(), Some("#C96B4E"));
        assert_eq!(parsed.theme.accent_dark.as_deref(), Some("#30D158"));
    }

    #[test]
    fn garbage_file_is_reported_and_changes_nothing() {
        let parsed = parse("this is = = not toml [[[");
        assert_eq!(parsed.theme, ThemeSettings::default());
        assert_eq!(parsed.problems.len(), 1);
        assert!(parsed.problems[0].contains("isn't valid TOML"));
    }

    #[test]
    fn non_utf8_and_oversized_files_are_reported() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("customize.toml");
        std::fs::write(&path, [0xFF, 0xFE, 0x00]).unwrap();
        assert!(load_from(&path).problems[0].contains("UTF-8"));
        std::fs::write(&path, "#".repeat(70 * 1024)).unwrap();
        assert!(load_from(&path).problems[0].contains("64 KB"));
        let folder = dir.path().join("folder.toml");
        std::fs::create_dir(&folder).unwrap();
        assert!(load_from(&folder).problems[0].contains("isn't a file"));
    }

    #[test]
    fn a_bad_value_loses_only_that_value() {
        let parsed =
            parse("[theme]\nmode = \"neon\"\naccent_light = \"red\"\naccent_dark = \"#30D158\"\n");
        assert_eq!(parsed.theme.mode, ThemeMode::Auto);
        assert_eq!(parsed.theme.accent_light, None);
        assert_eq!(parsed.theme.accent_dark.as_deref(), Some("#30D158"));
        assert_eq!(parsed.problems.len(), 2);
    }

    #[test]
    fn wrong_types_are_reported_not_fatal() {
        let parsed = parse("[theme]\nmode = 3\naccent_light = true\naccent_dark = [1]\n");
        assert_eq!(parsed.theme, ThemeSettings::default());
        assert_eq!(parsed.problems.len(), 3);

        let parsed = parse("theme = \"dark\"\n");
        assert_eq!(parsed.theme, ThemeSettings::default());
        assert!(parsed.problems[0].contains("should be a table"));
    }

    #[test]
    fn unknown_sections_and_keys_are_ignored_silently() {
        let parsed = parse(
            "future = 1\n[density]\nlevel = \"compact\"\n[theme]\nmode = \"light\"\nglow = true\n",
        );
        assert_eq!(parsed.theme.mode, ThemeMode::Light);
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
    }

    #[test]
    fn low_contrast_accents_are_rejected_per_appearance() {
        let parsed = parse("[theme]\naccent_light = \"#FFFFFF\"\naccent_dark = \"#111111\"\n");
        assert_eq!(parsed.theme.accent_light, None);
        assert_eq!(parsed.theme.accent_dark, None);
        assert_eq!(parsed.problems.len(), 2);
        assert!(
            parsed.problems[0].contains("contrast"),
            "{:?}",
            parsed.problems
        );
    }

    #[test]
    fn accents_cannot_smuggle_css() {
        for hostile in [
            "#fff; background: url(https://example.com/x)",
            "red",
            "#12345",
            "#1234567",
            "#GGGGGG",
            "rgb(1,2,3)",
            "url(x)",
            "#12345é",
        ] {
            assert!(
                validate_accent(hostile, Appearance::Dark).is_err(),
                "{hostile} should be rejected"
            );
        }
    }

    #[test]
    fn stylesheet_overrides_accent_tints_but_never_capture_or_red() {
        let css = theme_css(&ThemeSettings {
            mode: ThemeMode::Auto,
            accent_light: Some("#2F6F8F".into()),
            accent_dark: Some("#62BCE9".into()),
        });
        assert!(css.contains("prefers-color-scheme: light"));
        assert!(css.contains("prefers-color-scheme: dark"));
        assert!(css.contains("--accent: #2F6F8F;"));
        assert!(css.contains("--accent-soft: rgba(47, 111, 143, 0.10);"));
        assert!(css.contains("--accent-border-strong: rgba(98, 188, 233, 0.40);"));
        assert!(!css.contains("--capture"));
        assert!(!css.contains("--red"));
    }

    #[test]
    fn stylesheet_only_covers_the_appearance_that_was_set() {
        let css = theme_css(&ThemeSettings {
            accent_dark: Some("#62BCE9".into()),
            ..ThemeSettings::default()
        });
        assert!(css.contains("prefers-color-scheme: dark"));
        assert!(!css.contains("prefers-color-scheme: light"));
    }

    #[test]
    fn saving_keeps_comments_and_unknown_keys_and_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("customize.toml");
        std::fs::write(
            &path,
            "# my colors\n[density]\nlevel = \"compact\"\n\n[theme]\n# keep this\nmode = \"auto\"\nglow = true\naccent_dark = \"#30D158\"\n",
        )
        .unwrap();
        let wanted = ThemeSettings {
            mode: ThemeMode::Dark,
            accent_light: Some("#c96b4e".into()),
            accent_dark: None,
        };
        save_theme(&path, &wanted).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        for kept in [
            "# my colors",
            "level = \"compact\"",
            "# keep this",
            "glow = true",
        ] {
            assert!(raw.contains(kept), "lost {kept:?} in:\n{raw}");
        }
        assert!(!raw.contains("accent_dark"));
        let loaded = load_from(&path);
        assert_eq!(loaded.theme.mode, ThemeMode::Dark);
        assert_eq!(loaded.theme.accent_light.as_deref(), Some("#C96B4E"));
        assert_eq!(loaded.theme.accent_dark, None);
        assert!(loaded.problems.is_empty());
    }

    #[test]
    fn saving_creates_the_file_and_rejects_bad_accents_without_writing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("customize.toml");
        let bad = ThemeSettings {
            accent_light: Some("#FFFFFF".into()),
            ..ThemeSettings::default()
        };
        assert!(save_theme(&path, &bad)
            .unwrap_err()
            .starts_with("Light accent"));
        assert!(!path.exists());

        save_theme(&path, &ThemeSettings::default()).unwrap();
        assert_eq!(load_from(&path).theme, ThemeSettings::default());
    }

    #[test]
    fn saving_leaves_a_broken_file_untouched() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("customize.toml");
        let broken = "[theme\nmode = half-typed";
        std::fs::write(&path, broken).unwrap();
        assert!(save_theme(&path, &ThemeSettings::default()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    #[cfg(unix)]
    fn saving_writes_through_a_symlink() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("dotfiles-customize.toml");
        std::fs::write(&real, "[theme]\nmode = \"auto\"\n").unwrap();
        let link = dir.path().join("customize.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let wanted = ThemeSettings {
            mode: ThemeMode::Light,
            ..ThemeSettings::default()
        };
        save_theme(&link, &wanted).unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(std::fs::read_to_string(&real).unwrap().contains("light"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn system_appearance_has_an_answer_on_macos() {
        assert!(system_prefers_dark().is_some());
    }
}
