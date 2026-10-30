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
    pub annotate: String,
    pub annotate_record: String,
    pub annotate_choose: String,
    pub annotate_open_latest: String,
    pub check_for_updates: String,
    /// "Update to {{version}}…"
    pub update_to: String,
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
    // Annotate's items are newer than most translations; English beats a blank item.
    let get_or_en = |key: &str| {
        let key = format!("tray.{key}");
        i18n::lookup(language, &key)
            .or_else(|| i18n::lookup("en", &key))
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
        annotate: get_or_en("annotate"),
        annotate_record: get_or_en("annotateRecord"),
        annotate_choose: get_or_en("annotateChoose"),
        annotate_open_latest: get_or_en("annotateOpenLatest"),
        check_for_updates: get_or_en("checkForUpdates"),
        update_to: get_or_en("updateTo"),
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
