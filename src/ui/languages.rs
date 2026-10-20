//! Transcription languages, from `src/lib/constants/languages.ts`.

/// `(code, English label)` in picker order. `auto` is "Auto Detect".
pub static LANGUAGES: &[(&str, &str)] = &[
    ("auto", "Auto Detect"),
    ("en", "English"),
    ("zh", "Chinese"),
    ("zh-Hans", "Chinese (Simplified)"),
    ("zh-Hant", "Chinese (Traditional)"),
    ("yue", "Cantonese"),
    ("de", "German"),
    ("es", "Spanish"),
    ("ru", "Russian"),
    ("ko", "Korean"),
    ("fr", "French"),
    ("ja", "Japanese"),
    ("pt", "Portuguese"),
    ("tr", "Turkish"),
    ("pl", "Polish"),
    ("ca", "Catalan"),
    ("nl", "Dutch"),
    ("ar", "Arabic"),
    ("sv", "Swedish"),
    ("it", "Italian"),
    ("id", "Indonesian"),
    ("hi", "Hindi"),
    ("fi", "Finnish"),
    ("vi", "Vietnamese"),
    ("he", "Hebrew"),
    ("uk", "Ukrainian"),
    ("el", "Greek"),
    ("ms", "Malay"),
    ("cs", "Czech"),
    ("ro", "Romanian"),
    ("da", "Danish"),
    ("hu", "Hungarian"),
    ("ta", "Tamil"),
    ("no", "Norwegian"),
    ("th", "Thai"),
    ("ur", "Urdu"),
    ("hr", "Croatian"),
    ("bg", "Bulgarian"),
    ("lt", "Lithuanian"),
    ("la", "Latin"),
    ("mi", "Maori"),
    ("ml", "Malayalam"),
    ("cy", "Welsh"),
    ("sk", "Slovak"),
    ("te", "Telugu"),
    ("fa", "Persian"),
    ("lv", "Latvian"),
    ("bn", "Bengali"),
    ("sr", "Serbian"),
    ("az", "Azerbaijani"),
    ("sl", "Slovenian"),
    ("kn", "Kannada"),
    ("et", "Estonian"),
    ("mk", "Macedonian"),
    ("br", "Breton"),
    ("eu", "Basque"),
    ("is", "Icelandic"),
    ("ga", "Irish"),
    ("hy", "Armenian"),
    ("ne", "Nepali"),
    ("mn", "Mongolian"),
    ("bs", "Bosnian"),
    ("kk", "Kazakh"),
    ("sq", "Albanian"),
    ("sw", "Swahili"),
    ("gl", "Galician"),
    ("mr", "Marathi"),
    ("pa", "Punjabi"),
    ("si", "Sinhala"),
    ("km", "Khmer"),
    ("sn", "Shona"),
    ("yo", "Yoruba"),
    ("so", "Somali"),
    ("af", "Afrikaans"),
    ("oc", "Occitan"),
    ("ka", "Georgian"),
    ("be", "Belarusian"),
    ("tg", "Tajik"),
    ("sd", "Sindhi"),
    ("gu", "Gujarati"),
    ("am", "Amharic"),
    ("yi", "Yiddish"),
    ("lo", "Lao"),
    ("uz", "Uzbek"),
    ("fo", "Faroese"),
    ("ht", "Haitian Creole"),
    ("ps", "Pashto"),
    ("tk", "Turkmen"),
    ("nn", "Nynorsk"),
    ("mt", "Maltese"),
    ("sa", "Sanskrit"),
    ("lb", "Luxembourgish"),
    ("my", "Myanmar"),
    ("bo", "Tibetan"),
    ("tl", "Filipino (Tagalog)"),
    ("mg", "Malagasy"),
    ("as", "Assamese"),
    ("tt", "Tatar"),
    ("haw", "Hawaiian"),
    ("ln", "Lingala"),
    ("ha", "Hausa"),
    ("ba", "Bashkir"),
    ("jw", "Javanese"),
    ("su", "Sundanese"),
];

pub const CHINESE_LANGUAGE_CODE: &str = "zh";

/// Collapses a language tag to the recognition intent the UI exposes:
/// region/script subtags drop (`en-US` → `en`, `zh-Hant` → `zh`), `nb` maps
/// to `no` and `fil` to `tl`.
pub fn recognition_language(code: &str) -> &str {
    let base = code.split('-').next().unwrap_or(code);
    match base {
        "nb" => "no",
        "fil" => "tl",
        other => other,
    }
}

pub fn supports_language_code(supported: &[String], code: &str) -> bool {
    let wanted = recognition_language(code);
    supported.iter().any(|s| recognition_language(s) == wanted)
}

/// Supported languages collapsed to recognition codes, first occurrence kept.
pub fn unique_capability_languages(supported: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    supported
        .iter()
        .map(|s| recognition_language(s).to_string())
        .filter(|code| seen.insert(code.clone()))
        .collect()
}

pub fn language_label(code: &str) -> Option<&'static str> {
    let find = |code: &str| LANGUAGES.iter().find(|(c, _)| *c == code).map(|(_, l)| *l);
    find(code).or_else(|| find(recognition_language(code)))
}

/// Languages offered in the transcription-language picker: everything but
/// the bare `zh`, which is replaced by the Simplified/Traditional variants.
pub fn selectable_languages() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    LANGUAGES
        .iter()
        .filter(|(code, _)| *code != CHINESE_LANGUAGE_CODE)
}

/// Languages a model can declare: everything but `auto` and the Chinese
/// output variants.
pub fn model_capability_languages() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    LANGUAGES
        .iter()
        .filter(|(code, _)| *code != "auto" && *code != "zh-Hans" && *code != "zh-Hant")
}
