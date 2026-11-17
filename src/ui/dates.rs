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

pub fn day_label(date: NaiveDate) -> String {
    day_label_with(date, false)
}

/// `day_label` with an abbreviated month and no weekday, for a value in a narrow column.
pub fn short_day_label(date: NaiveDate) -> String {
    day_label_with(date, true)
}

fn day_label_with(date: NaiveDate, short: bool) -> String {
    let today = Local::now().date_naive();
    if let Some(word) = relative_day((today - date).num_days()) {
        return word.to_string();
    }
    let same_year = date.year() == today.year();
    let (this_year, other_year) = match (crate::i18n::language(), short) {
        ("en", false) => ("%A, %B %-d", "%A, %B %-d, %Y"),
        ("en", true) => ("%b %-d", "%b %-d, %Y"),
        ("de" | "da", false) => ("%A, %-d. %B", "%A, %-d. %B %Y"),
        ("de" | "da", true) => ("%-d. %b", "%-d. %b %Y"),
        ("zh" | "zh-TW" | "ja", false) => ("%-m月%-d日%A", "%Y年%-m月%-d日%A"),
        ("zh" | "zh-TW" | "ja", true) => ("%-m月%-d日", "%Y年%-m月%-d日"),
        ("ko", false) => ("%-m월 %-d일 %A", "%Y년 %-m월 %-d일 %A"),
        ("ko", true) => ("%-m월 %-d일", "%Y년 %-m월 %-d일"),
        (_, false) => ("%A %-d %B", "%A %-d %B %Y"),
        (_, true) => ("%-d %b", "%-d %b %Y"),
    };
    let pattern = if same_year { this_year } else { other_year };
    date.format_localized(pattern, locale()).to_string()
}

fn uses_twelve_hour() -> bool {
    // Not Arabic: chrono's ar_SA has empty AM and PM strings, so `%p` would print nothing.
    matches!(crate::i18n::language(), "en" | "hi" | "ko")
}

pub fn time_label(time: DateTime<Local>) -> String {
    if uses_twelve_hour() {
        time.format_localized("%-I:%M %p", locale()).to_string()
    } else {
        time.format("%H:%M").to_string()
    }
}

pub fn long_date(date: NaiveDate) -> String {
    let pattern = match crate::i18n::language() {
        "en" => "%B %-d, %Y",
        "de" | "da" | "cs" => "%-d. %B %Y",
        "es" | "pt" => "%-d de %B de %Y",
        "zh" | "zh-TW" | "ja" => "%Y年%-m月%-d日",
        "ko" => "%Y년 %-m월 %-d일",
        "vi" => "%-d tháng %-m, %Y",
        "ru" | "bg" => "%-d %B %Y г.",
        "uk" => "%-d %B %Y р.",
        _ => "%-d %B %Y",
    };
    date.format_localized(pattern, locale()).to_string()
}

pub fn short_month(date: NaiveDate) -> String {
    date.format_localized("%b", locale()).to_string()
}

pub fn short_weekday(date: NaiveDate) -> String {
    date.format_localized("%a", locale()).to_string()
}
