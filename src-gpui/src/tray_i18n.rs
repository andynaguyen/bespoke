//! Tray menu strings, read from the `tray` section of the shared locale files.
//!
//! A locale missing a key gets an empty string for it, matching the old
//! build-time table; callers that must never show a blank item fall back to
//! English themselves.

use crate::i18n;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayStrings {
    pub settings: String,
    pub copy_last_transcript: String,
    pub unload_model: String,
    pub model: String,
    pub quit: String,
    pub cancel: String,
    pub secure_input_warning: String,
}

/// Get localized tray menu strings for `locale` (the `app_language` setting).
///
/// Lookup order: exact locale → Chinese script/region fallback → language code → English.
pub fn get_tray_translations(locale: Option<String>) -> TrayStrings {
    let language = locale
        .as_deref()
        .and_then(i18n::supported_language)
        .unwrap_or("en");
    let get = |key: &str| {
        i18n::lookup(language, &format!("tray.{key}"))
            .unwrap_or_default()
            .to_string()
    };
    TrayStrings {
        settings: get("settings"),
        copy_last_transcript: get("copyLastTranscript"),
        unload_model: get("unloadModel"),
        model: get("model"),
        quit: get("quit"),
        cancel: get("cancel"),
        secure_input_warning: get("secureInputWarning"),
    }
}

#[cfg(test)]
mod tests {
    use super::get_tray_translations;

    #[test]
    fn resolves_locale_fallbacks() {
        for (locale, expected) in [
            ("zh-Hant-TW", "zh-TW"),
            ("zh-Hant-HK", "zh-TW"),
            ("zh-HK", "zh-TW"),
            ("zh-MO", "zh-TW"),
            ("ZH-TW", "zh-TW"),
            ("zh_Hant_TW", "zh-TW"),
            ("zh-Hans-CN", "zh"),
            ("yue-Hant-HK", "zh-TW"),
            ("yue-Hans-CN", "zh"),
            ("fr-FR", "fr"),
            ("xx-YY", "en"),
        ] {
            assert_eq!(
                get_tray_translations(Some(locale.into())),
                get_tray_translations(Some(expected.into())),
                "{locale} should resolve to {expected}"
            );
        }
    }
}
