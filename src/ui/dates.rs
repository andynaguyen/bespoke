//! Locale-aware dates for History and Stats, standing in for the browser's
//! `Intl.DateTimeFormat` and `Intl.RelativeTimeFormat`.

use chrono::{DateTime, Datelike, Local, Locale, NaiveDate};

fn locale() -> Locale {
    match crate::i18n::language() {
        "zh" => Locale::zh_CN,
        "zh-TW" => Locale::zh_TW,
        "es" => Locale::es_ES,
        "fr" => Locale::fr_FR,
        "de" => Locale::de_DE,
        "ja" => Locale::ja_JP,
        "ko" => Locale::ko_KR,
        "vi" => Locale::vi_VN,
        "pl" => Locale::pl_PL,
        "it" => Locale::it_IT,
        "ru" => Locale::ru_RU,
        "uk" => Locale::uk_UA,
        "pt" => Locale::pt_BR,
        "cs" => Locale::cs_CZ,
        "tr" => Locale::tr_TR,
        "ar" => Locale::ar_SA,
        "he" => Locale::he_IL,
        "sv" => Locale::sv_SE,
        "bg" => Locale::bg_BG,
        "nl" => Locale::nl_NL,
        "ne" => Locale::ne_NP,
        "hi" => Locale::hi_IN,
        "da" => Locale::da_DK,
        "ca" => Locale::ca_ES,
        "id" => Locale::id_ID,
        _ => Locale::en_US,
    }
}

/// "today" / "yesterday" as `Intl.RelativeTimeFormat` words them.
fn relative_day(days_ago: i64) -> Option<&'static str> {
    let (today, yesterday) = match crate::i18n::language() {
        "zh" => ("今天", "昨天"),
        "zh-TW" => ("今天", "昨天"),
        "es" => ("hoy", "ayer"),
        "fr" => ("aujourd’hui", "hier"),
        "de" => ("heute", "gestern"),
        "ja" => ("今日", "昨日"),
        "ko" => ("오늘", "어제"),
        "vi" => ("Hôm nay", "Hôm qua"),
        "pl" => ("dzisiaj", "wczoraj"),
        "it" => ("oggi", "ieri"),
        "ru" => ("сегодня", "вчера"),
        "uk" => ("сьогодні", "учора"),
        "pt" => ("hoje", "ontem"),
        "cs" => ("dnes", "včera"),
        "tr" => ("bugün", "dün"),
        "ar" => ("اليوم", "أمس"),
        "he" => ("היום", "אתמול"),
        "sv" => ("i dag", "i går"),
        "bg" => ("днес", "вчера"),
        "nl" => ("vandaag", "gisteren"),
        "ne" => ("आज", "हिजो"),
        "hi" => ("आज", "कल"),
        "da" => ("i dag", "i går"),
        "ca" => ("avui", "ahir"),
        "id" => ("hari ini", "kemarin"),
        _ => ("today", "yesterday"),
    };
    match days_ago {
        0 => Some(today),
        1 => Some(yesterday),
        _ => None,
    }
}

/// History day heading: "Today", "Yesterday", or the full date (with the
/// year only when it is not this year).
pub fn day_label(date: NaiveDate) -> String {
    let today = Local::now().date_naive();
    if let Some(word) = relative_day((today - date).num_days()) {
        return word.to_string();
    }
    let same_year = date.year() == today.year();
    let pattern = match crate::i18n::language() {
        "en" => {
            if same_year {
                "%A, %B %-d"
            } else {
                "%A, %B %-d, %Y"
            }
        }
        "de" | "da" => {
            if same_year {
                "%A, %-d. %B"
            } else {
                "%A, %-d. %B %Y"
            }
        }
        "zh" | "zh-TW" | "ja" => {
            if same_year {
                "%-m月%-d日%A"
            } else {
                "%Y年%-m月%-d日%A"
            }
        }
        "ko" => {
            if same_year {
                "%-m월 %-d일 %A"
            } else {
                "%Y년 %-m월 %-d일 %A"
            }
        }
        _ => {
            if same_year {
                "%A %-d %B"
            } else {
                "%A %-d %B %Y"
            }
        }
    };
    date.format_localized(pattern, locale()).to_string()
}

/// Whether the UI language shows a 12-hour clock.
fn uses_twelve_hour() -> bool {
    matches!(crate::i18n::language(), "en" | "hi" | "ko" | "ar")
}

/// Entry time, e.g. "3:45 PM" or "15:45".
pub fn time_label(time: DateTime<Local>) -> String {
    if uses_twelve_hour() {
        time.format_localized("%-I:%M %p", locale()).to_string()
    } else {
        time.format("%H:%M").to_string()
    }
}

/// `Intl.DateTimeFormat` with `dateStyle: "long"` ("October 13, 2025") for
/// the Stats day popover.
pub fn long_date(date: NaiveDate) -> String {
    let pattern = match crate::i18n::language() {
        "en" => "%B %-d, %Y",
        "de" | "da" | "cs" | "nb" | "no" => "%-d. %B %Y",
        "es" | "pt" | "pt-BR" => "%-d de %B de %Y",
        "zh" | "zh-TW" | "ja" => "%Y年%-m月%-d日",
        "ko" => "%Y년 %-m월 %-d일",
        "vi" => "%-d tháng %-m, %Y",
        "ru" | "bg" => "%-d %B %Y г.",
        "uk" => "%-d %B %Y р.",
        _ => "%-d %B %Y",
    };
    date.format_localized(pattern, locale()).to_string()
}

/// Short month name, e.g. "Oct".
pub fn short_month(date: NaiveDate) -> String {
    date.format_localized("%b", locale()).to_string()
}

/// Short weekday name, e.g. "Mon".
pub fn short_weekday(date: NaiveDate) -> String {
    date.format_localized("%a", locale()).to_string()
}
