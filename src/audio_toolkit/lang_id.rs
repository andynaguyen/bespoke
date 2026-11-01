use whatlang::{Detector, Lang};

const MIN_CONFIDENCE: f64 = 0.9;

/// Whatlang represents Mandarin as `cmn`, which has no ISO 639-1 form.
fn whatlang_lang_for_model_code(code: &str) -> Option<Lang> {
    let primary = code
        .trim()
        .split(&['-', '_'][..])
        .next()?
        .to_ascii_lowercase();

    if primary == "zh" {
        return Some(Lang::Cmn);
    }

    let language = match primary.len() {
        2 => isolang::Language::from_639_1(&primary)?,
        3 => isolang::Language::from_639_3(&primary)?,
        _ => return None,
    };

    Lang::from_code(language.to_639_3())
}

fn iso639_1_for_whatlang(lang: Lang) -> Option<&'static str> {
    match lang {
        Lang::Cmn => Some("zh"),
        other => isolang::Language::from_639_3(other.code())?.to_639_1(),
    }
}

pub fn detect_output_language(text: &str, supported_languages: &[String]) -> Option<String> {
    // Codes whatlang cannot represent (e.g. Maltese in Parakeet V3's list,
    // Cantonese in SenseVoice's) are dropped rather than disabling detection
    // for the whole model. Text in a dropped language only causes harm if it
    // clears the confidence gate as en/de/fr — the only gated filler profiles —
    // which is the same misdetection risk the gate already absorbs for in-list
    // confusions like pt vs es.
    let allowlist: Vec<Lang> = supported_languages
        .iter()
        .filter_map(|code| whatlang_lang_for_model_code(code))
        .collect();

    let detector = if supported_languages.is_empty() {
        Detector::new()
    } else if allowlist.is_empty() {
        return None;
    } else {
        Detector::with_allowlist(allowlist)
    };
    let info = detector.detect(text)?;
    if !info.is_reliable() || info.confidence() < MIN_CONFIDENCE {
        return None;
    }

    iso639_1_for_whatlang(info.lang()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn langs(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn detects_portuguese_sentence_containing_um() {
        let detected = detect_output_language(
            "eu vi um carro na rua ontem de manhã quando fui ao mercado",
            &langs(&["en", "pt", "es"]),
        );
        assert_eq!(detected.as_deref(), Some("pt"));
    }

    #[test]
    fn short_ambiguous_text_returns_none() {
        let detected = detect_output_language("um ok", &langs(&["en", "pt"]));
        assert_eq!(detected, None);
    }

    #[test]
    fn normalizes_model_language_codes() {
        assert_eq!(whatlang_lang_for_model_code("pt-BR"), Some(Lang::Por));
        assert_eq!(whatlang_lang_for_model_code("PT_br"), Some(Lang::Por));
        assert_eq!(whatlang_lang_for_model_code("eng"), Some(Lang::Eng));
        assert_eq!(whatlang_lang_for_model_code("zh-Hant"), Some(Lang::Cmn));
        assert_eq!(iso639_1_for_whatlang(Lang::Cmn), Some("zh"));
    }

    #[test]
    fn regional_allowlist_preserves_portuguese_detection() {
        let detected = detect_output_language(
            "eu vi um carro na rua ontem de manhã quando fui ao mercado",
            &langs(&["en", "pt-BR"]),
        );
        assert_eq!(detected.as_deref(), Some("pt"));
    }

    #[test]
    fn unmappable_codes_are_dropped_not_fatal() {
        let detected = detect_output_language(
            "um so the weather forecast said it would probably rain throughout the whole weekend",
            &langs(&["zh", "yue", "en", "ja", "ko"]),
        );
        assert_eq!(detected.as_deref(), Some("en"));
    }

    #[test]
    fn parakeet_v3_language_list_still_detects() {
        let parakeet_v3 = langs(&[
            "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv",
            "lt", "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk",
        ]);
        let detected = detect_output_language(
            "eu vi um carro na rua ontem de manhã quando fui ao mercado",
            &parakeet_v3,
        );
        assert_eq!(detected.as_deref(), Some("pt"));
    }

    #[test]
    fn fully_unmappable_metadata_fails_closed() {
        let text = "eu vi um carro na rua ontem de manhã quando fui ao mercado";
        assert_eq!(detect_output_language(text, &langs(&["yue"])), None);
    }

    #[test]
    fn missing_metadata_detects_unconstrained() {
        let detected = detect_output_language(
            "eu vi um carro na rua ontem de manhã quando fui ao mercado",
            &[],
        );
        assert_eq!(detected.as_deref(), Some("pt"));
    }
}
