//! UI strings, read from the i18next locale files in `src/i18n/locales`.
//!
//! The same JSON files feed the UI and the tray menu. Keys use i18next's
//! dotted paths (`settings.sound.title`), `{{name}}` interpolation, and
//! `_one`/`_few`/`_many`/`_other` plural suffixes chosen by CLDR rules.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use gpui::SharedString;

include!(concat!(env!("OUT_DIR"), "/locales.rs"));

/// A UI language Bespoke ships a translation for.
pub struct LanguageInfo {
    pub code: &'static str,
    pub name: &'static str,
    pub native_name: &'static str,
    pub rtl: bool,
}

/// Supported languages in dropdown order (mirrors `src/i18n/languages.ts`).
pub static LANGUAGES: &[LanguageInfo] = &[
    lang("en", "English", "English"),
    lang("zh", "Simplified Chinese", "简体中文"),
    lang("zh-TW", "Traditional Chinese", "繁體中文"),
    lang("es", "Spanish", "Español"),
    lang("fr", "French", "Français"),
    lang("de", "German", "Deutsch"),
    lang("ja", "Japanese", "日本語"),
    lang("ko", "Korean", "한국어"),
    lang("vi", "Vietnamese", "Tiếng Việt"),
    lang("pl", "Polish", "Polski"),
    lang("it", "Italian", "Italiano"),
    lang("ru", "Russian", "Русский"),
    lang("uk", "Ukrainian", "Українська"),
    lang("pt", "Portuguese", "Português"),
    lang("cs", "Czech", "Čeština"),
    lang("tr", "Turkish", "Türkçe"),
    LanguageInfo {
        code: "ar",
        name: "Arabic",
        native_name: "العربية",
        rtl: true,
    },
    LanguageInfo {
        code: "he",
        name: "Hebrew",
        native_name: "עברית",
        rtl: true,
    },
    lang("sv", "Swedish", "Svenska"),
    lang("bg", "Bulgarian", "Български"),
    lang("nl", "Dutch", "Nederlands"),
    lang("ne", "Nepali", "नेपाली"),
    lang("hi", "Hindi", "हिन्दी"),
    lang("da", "Danish", "Dansk"),
    lang("ca", "Catalan", "Català"),
    lang("id", "Indonesian", "Bahasa Indonesia"),
];

const fn lang(code: &'static str, name: &'static str, native_name: &'static str) -> LanguageInfo {
    LanguageInfo {
        code,
        name,
        native_name,
        rtl: false,
    }
}

static TABLES: LazyLock<HashMap<&'static str, serde_json::Value>> = LazyLock::new(|| {
    LOCALES
        .iter()
        .filter_map(|(code, json)| match serde_json::from_str(json) {
            Ok(value) => Some((*code, value)),
            Err(e) => {
                log::error!("Failed to parse locale '{}': {}", code, e);
                None
            }
        })
        .collect()
});

static CURRENT: RwLock<&'static str> = RwLock::new("en");

/// The active UI language code, e.g. `"en"` or `"zh-TW"`.
pub fn language() -> &'static str {
    *CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

/// Switches the UI language. Unsupported codes resolve the way i18next's
/// `getSupportedLanguage` does, and fall back to English.
pub fn set_language(code: &str) {
    let resolved = supported_language(code).unwrap_or("en");
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = resolved;
}

pub fn is_rtl() -> bool {
    let code = language();
    LANGUAGES.iter().any(|l| l.code == code && l.rtl)
}

/// Maps a locale tag to a shipped translation: exact match, then Chinese
/// script and region rules (`zh-Hant`, `zh-HK` → `zh-TW`), then the base
/// language. Mirrors `getSupportedLanguage` in `src/i18n/index.ts`.
pub fn supported_language(code: &str) -> Option<&'static str> {
    let normalized = code.trim().to_lowercase().replace('_', "-");
    if normalized.is_empty() {
        return None;
    }
    if let Some(info) = LANGUAGES
        .iter()
        .find(|l| l.code.eq_ignore_ascii_case(&normalized))
    {
        return Some(info.code);
    }

    let subtags: Vec<&str> = normalized.split('-').collect();
    let base = subtags[0];
    let has = |tag: &str| subtags.contains(&tag);
    let traditional_region = has("tw") || has("hk") || has("mo");
    let fallback = match base {
        "zh" if has("hant") || (!has("hans") && traditional_region) => "zh-TW",
        "yue" if has("hans") => "zh",
        "yue" => "zh-TW",
        other => other,
    };
    LANGUAGES
        .iter()
        .find(|l| l.code.eq_ignore_ascii_case(fallback))
        .map(|l| l.code)
}

/// Looks up `key` in one locale table without fallback.
pub fn lookup(language: &str, key: &str) -> Option<&'static str> {
    let mut node = TABLES.get(language)?;
    for part in key.split('.') {
        node = node.get(part)?;
    }
    node.as_str()
}

fn resolve(key: &str) -> Option<&'static str> {
    lookup(language(), key).or_else(|| lookup("en", key))
}

/// Translates `key`, falling back to English and then to the key itself.
pub fn t(key: &str) -> SharedString {
    match resolve(key) {
        Some(text) => SharedString::new_static(text),
        None => SharedString::from(key.to_string()),
    }
}

/// Translates `key` and fills `{{name}}` placeholders from `args`.
pub fn t_with(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> SharedString {
    interpolate(resolve(key).unwrap_or(key), args).into()
}

/// Translates `key`, or returns `default` when no locale has it. Matches
/// i18next's `defaultValue` option.
pub fn t_or(key: &str, default: &str) -> SharedString {
    match resolve(key) {
        Some(text) => SharedString::new_static(text),
        None => SharedString::from(default.to_string()),
    }
}

/// Translates a pluralized key: picks `key_one`, `key_few` and so on for
/// `count` in the active language, then interpolates `{{count}}` and `args`.
pub fn t_count(key: &str, count: i64, args: &[(&str, &dyn std::fmt::Display)]) -> SharedString {
    let category = plural_category(language(), count);
    let template = lookup(language(), &format!("{key}_{category}"))
        .or_else(|| lookup(language(), &format!("{key}_other")))
        .or_else(|| lookup("en", &format!("{key}_{}", plural_category("en", count))))
        .or_else(|| lookup("en", &format!("{key}_other")))
        .or_else(|| resolve(key))
        .unwrap_or(key);
    let mut all: Vec<(&str, &dyn std::fmt::Display)> = vec![("count", &count)];
    all.extend_from_slice(args);
    interpolate(template, &all).into()
}

fn interpolate(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end].trim();
        match args.iter().find(|(arg, _)| *arg == name) {
            Some((_, value)) => out.push_str(&value.to_string()),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// CLDR cardinal plural category for an integer in `language`.
fn plural_category(language: &str, n: i64) -> &'static str {
    let n = n.unsigned_abs();
    let (n10, n100) = (n % 10, n % 100);
    match language.split('-').next().unwrap_or(language) {
        "ja" | "ko" | "zh" | "vi" | "id" => "other",
        "fr" | "pt" | "hi" => {
            if n <= 1 {
                "one"
            } else if n != 0 && n.is_multiple_of(1_000_000) && language == "fr" {
                "many"
            } else {
                "other"
            }
        }
        "ru" | "uk" => {
            if n10 == 1 && n100 != 11 {
                "one"
            } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
                "few"
            } else {
                "many"
            }
        }
        "pl" => {
            if n == 1 {
                "one"
            } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
                "few"
            } else {
                "many"
            }
        }
        "cs" => match n {
            1 => "one",
            2..=4 => "few",
            _ => "other",
        },
        "ar" => match (n, n100) {
            (0, _) => "zero",
            (1, _) => "one",
            (2, _) => "two",
            (_, 3..=10) => "few",
            (_, 11..=99) => "many",
            _ => "other",
        },
        "he" => match n {
            1 => "one",
            2 => "two",
            _ => "other",
        },
        "it" | "es" | "ca" => {
            if n == 1 {
                "one"
            } else if n != 0 && n.is_multiple_of(1_000_000) {
                "many"
            } else {
                "other"
            }
        }
        _ => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_supported_languages() {
        assert_eq!(supported_language("en-US"), Some("en"));
        assert_eq!(supported_language("pt_BR"), Some("pt"));
        assert_eq!(supported_language("zh-Hant-HK"), Some("zh-TW"));
        assert_eq!(supported_language("zh-HK"), Some("zh-TW"));
        assert_eq!(supported_language("zh-Hans-CN"), Some("zh"));
        assert_eq!(supported_language("yue"), Some("zh-TW"));
        assert_eq!(supported_language("nb-NO"), None);
    }

    #[test]
    fn interpolates_and_keeps_unknown_placeholders() {
        let name = "Slack";
        assert_eq!(
            interpolate(
                "{{name}} may block {{ count }}",
                &[("name", &name), ("count", &2)]
            ),
            "Slack may block 2"
        );
        assert_eq!(interpolate("keep {{missing}}", &[]), "keep {{missing}}");
    }

    #[test]
    fn english_plurals_use_one_and_other() {
        assert_eq!(plural_category("en", 1), "one");
        assert_eq!(plural_category("en", 0), "other");
        assert_eq!(plural_category("ru", 22), "few");
        assert_eq!(plural_category("ru", 25), "many");
        assert_eq!(plural_category("ar", 0), "zero");
    }

    #[test]
    fn every_locale_parses() {
        assert_eq!(TABLES.len(), LOCALES.len());
        assert!(lookup("en", "sidebar.dictate").is_some());
    }
}
