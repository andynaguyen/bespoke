use gpui::SharedString;

use crate::i18n::t_or;

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

pub fn unique_capability_languages(supported: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    supported
        .iter()
        .map(|s| recognition_language(s).to_string())
        .filter(|code| seen.insert(code.clone()))
        .collect()
}

pub fn language_name(code: &str, english: &str) -> SharedString {
    t_or(&format!("languages.{code}"), english)
}

pub fn language_label(code: &str) -> Option<SharedString> {
    let find = |code: &str| LANGUAGES.iter().find(|(c, _)| *c == code);
    find(code)
        .or_else(|| find(recognition_language(code)))
        .map(|(code, english)| language_name(code, english))
}

pub fn selectable_languages() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    LANGUAGES
        .iter()
        .filter(|(code, _)| *code != CHINESE_LANGUAGE_CODE)
}

pub fn model_capability_languages() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    LANGUAGES
        .iter()
        .filter(|(code, _)| *code != "auto" && *code != "zh-Hans" && *code != "zh-Hant")
}
