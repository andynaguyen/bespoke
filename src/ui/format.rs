//! Display formatting shared by several views (`src/lib/utils/format.ts`,
//! `modelTranslation.ts`).

use gpui::SharedString;

use crate::i18n::{t, t_or};
use crate::managers::model::ModelInfo;

/// `1.5 GB`, `142 MB`: one decimal below 10 GB / 100 MB.
pub fn model_size(size_mb: u64) -> String {
    model_size_with(size_mb, decimal_separator())
}

fn model_size_with(size_mb: u64, decimal: &str) -> String {
    if size_mb == 0 {
        return "Unknown size".into();
    }
    let size_mb = size_mb as f64;
    let formatted = if size_mb >= 1024. {
        let gb = size_mb / 1024.;
        if gb >= 10. {
            format!("{gb:.0} GB")
        } else {
            format!("{gb:.1} GB")
        }
    } else if size_mb >= 100. {
        format!("{size_mb:.0} MB")
    } else {
        format!("{size_mb:.1} MB")
    };
    formatted.replace('.', decimal)
}

/// The system locale's decimal mark. `formatModelSize` formats with
/// `Intl.NumberFormat(undefined)`, the system locale, not the app language.
fn decimal_separator() -> &'static str {
    let locale = sys_locale::get_locale().unwrap_or_default();
    let language = locale.split(['-', '_']).next().unwrap_or_default();
    match language {
        "de" | "es" | "it" | "nl" | "id" | "da" | "tr" | "pt" | "ca" | "fr" | "ru" | "uk"
        | "pl" | "cs" | "sv" | "bg" | "nb" | "no" | "fi" | "vi" => ",",
        _ => ".",
    }
}

/// The localized model name, falling back to the catalog name.
pub fn model_name(model: &ModelInfo) -> SharedString {
    t_or(&format!("onboarding.models.{}.name", model.id), &model.name)
}

/// The localized model description; custom models share one string.
pub fn model_description(model: &ModelInfo) -> SharedString {
    if model.is_custom {
        return t("onboarding.customModelDescription");
    }
    t_or(
        &format!("onboarding.models.{}.description", model.id),
        &model.description,
    )
}

/// Groups digits with the locale's separator (`Intl.NumberFormat`).
pub fn number(value: i64) -> String {
    let separator = match crate::i18n::language() {
        "de" | "es" | "it" | "nl" | "id" | "da" | "tr" | "pt" | "ca" => '.',
        "fr" | "ru" | "uk" | "pl" | "cs" | "sv" | "bg" => '\u{202f}',
        _ => ',',
    };
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(separator);
        }
        out.push(ch);
    }
    if value < 0 {
        out.insert(0, '-');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes_like_the_react_helper() {
        assert_eq!(model_size_with(0, "."), "Unknown size");
        assert_eq!(model_size_with(142, "."), "142 MB");
        assert_eq!(model_size_with(57, "."), "57.0 MB");
        assert_eq!(model_size_with(1536, "."), "1.5 GB");
        assert_eq!(model_size_with(1536, ","), "1,5 GB");
        assert_eq!(model_size_with(12 * 1024, "."), "12 GB");
    }
}
