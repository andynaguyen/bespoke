use natural::phonetics::soundex;
use regex::Regex;
use strsim::levenshtein;

fn build_ngram(words: &[&str]) -> String {
    words
        .iter()
        .map(|w| build_match_key(w))
        .collect::<Vec<_>>()
        .concat()
}

/// Letters and digits, lowercased: how the dictionary compares spellings.
pub fn build_match_key(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

struct CustomWordMatchKey {
    word_index: usize,
    key: String,
}

fn build_custom_word_match_keys(word: &str, word_index: usize) -> Vec<CustomWordMatchKey> {
    let primary_key = build_match_key(word);
    let mut keys = Vec::with_capacity(2);

    if is_supported_fuzzy_key(&primary_key) {
        keys.push(CustomWordMatchKey {
            word_index,
            key: primary_key.clone(),
        });
    }

    if word.contains('&') {
        let expanded_key = build_match_key(&word.replace('&', " and "));
        if is_supported_fuzzy_key(&expanded_key) && expanded_key != primary_key {
            keys.push(CustomWordMatchKey {
                word_index,
                key: expanded_key,
            });
        }
    }

    keys
}

fn is_supported_fuzzy_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric())
}

fn supports_soundex(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphabetic())
}

fn find_best_match<'a>(
    candidate: &str,
    custom_words: &'a [String],
    custom_word_match_keys: &[CustomWordMatchKey],
    threshold: f64,
) -> Option<(&'a String, f64)> {
    if !is_supported_fuzzy_key(candidate) || candidate.chars().count() > 50 {
        return None;
    }

    let mut best_match: Option<&String> = None;
    let mut best_score = f64::MAX;

    for custom_word_key in custom_word_match_keys {
        let candidate_len = candidate.chars().count();
        let custom_word_len = custom_word_key.key.chars().count();
        let len_diff = candidate_len.abs_diff(custom_word_len) as f64;
        let max_len = candidate_len.max(custom_word_len) as f64;
        let max_allowed_diff = (max_len * 0.25).max(2.0);
        if len_diff > max_allowed_diff {
            continue;
        }

        let levenshtein_dist = levenshtein(candidate, &custom_word_key.key);
        let levenshtein_score = levenshtein_dist as f64 / max_len;

        let phonetic_match = supports_soundex(candidate)
            && supports_soundex(&custom_word_key.key)
            && soundex(candidate, &custom_word_key.key);

        let combined_score = if phonetic_match {
            levenshtein_score * 0.3
        } else {
            levenshtein_score
        };

        if combined_score < threshold && combined_score < best_score {
            best_match = Some(&custom_words[custom_word_key.word_index]);
            best_score = combined_score;
        }
    }

    best_match.map(|m| (m, best_score))
}

pub fn apply_custom_words(text: &str, custom_words: &[String], threshold: f64) -> String {
    apply_custom_words_with_fixes(text, custom_words, threshold).0
}

/// A change the dictionary made to a transcript: the words the model heard, and what Bespoke
/// wrote in their place. Both leave out punctuation at their ends.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fix {
    pub heard: String,
    pub written: String,
    /// A replacement's trigger fired, rather than a word's spelling.
    pub replacement: bool,
}

fn trim_punctuation(text: &str) -> &str {
    text.trim_matches(|c: char| !c.is_alphanumeric())
}

/// `apply_custom_words`, and the fixes it made, in order.
pub fn apply_custom_words_with_fixes(
    text: &str,
    custom_words: &[String],
    threshold: f64,
) -> (String, Vec<Fix>) {
    if custom_words.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut fixes = Vec::new();
    let mut out = Vec::new();
    for span in custom_word_spans(&words, custom_words, threshold) {
        let heard = words[span.start..span.start + span.len].join(" ");
        if heard != span.text {
            fixes.push(Fix {
                heard: trim_punctuation(&heard).to_string(),
                written: trim_punctuation(&span.text).to_string(),
                replacement: false,
            });
        }
        out.push(span.text);
    }
    (out.join(" "), fixes)
}

/// A word or segment with its start and end in milliseconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimedText {
    pub t0: u64,
    pub t1: u64,
    pub text: String,
}

/// `apply_custom_words` over words that carry times. A dictionary entry that replaces several
/// heard words becomes one word, from the first one's start to the last one's end.
pub fn apply_custom_words_to_timed(
    words: &[TimedText],
    custom_words: &[String],
    threshold: f64,
) -> Vec<TimedText> {
    if custom_words.is_empty() {
        return words.to_vec();
    }
    let texts: Vec<&str> = words.iter().map(|word| word.text.trim()).collect();
    custom_word_spans(&texts, custom_words, threshold)
        .into_iter()
        .map(|span| TimedText {
            t0: words[span.start].t0,
            t1: words[span.start + span.len - 1].t1,
            text: span.text,
        })
        .collect()
}

struct WordSpan {
    start: usize,
    len: usize,
    text: String,
}

fn custom_word_spans(words: &[&str], custom_words: &[String], threshold: f64) -> Vec<WordSpan> {
    let custom_word_match_keys: Vec<CustomWordMatchKey> = custom_words
        .iter()
        .enumerate()
        .flat_map(|(index, word)| build_custom_word_match_keys(word, index))
        .collect();

    let mut result = Vec::new();
    let mut i = 0;

    while i < words.len() {
        let mut best_match: Option<(usize, &String, f64)> = None;

        for n in (1..=3).rev() {
            if i + n > words.len() {
                continue;
            }

            let ngram_words = &words[i..i + n];
            if ngram_words[..n.saturating_sub(1)]
                .iter()
                .any(|word| !extract_punctuation(word).1.is_empty())
            {
                continue;
            }
            let ngram = build_ngram(ngram_words);

            if let Some((replacement, score)) =
                find_best_match(&ngram, custom_words, &custom_word_match_keys, threshold)
            {
                let is_better = best_match
                    .as_ref()
                    .is_none_or(|(_, _, best_score)| score < *best_score);
                if is_better {
                    best_match = Some((n, replacement, score));
                }
            }
        }

        if let Some((n, replacement, _)) = best_match {
            let ngram_words = &words[i..i + n];
            let (prefix, _) = extract_punctuation(ngram_words[0]);
            let (_, suffix) = extract_punctuation(ngram_words[n - 1]);

            let corrected = preserve_case_pattern(ngram_words[0], replacement);

            result.push(WordSpan {
                start: i,
                len: n,
                text: format!("{}{}{}", prefix, corrected, suffix),
            });
            i += n;
        } else {
            result.push(WordSpan {
                start: i,
                len: 1,
                text: words[i].to_string(),
            });
            i += 1;
        }
    }

    result
}

fn preserve_case_pattern(original: &str, replacement: &str) -> String {
    if original.chars().all(|c| c.is_uppercase()) {
        replacement.to_uppercase()
    } else if original.chars().next().is_some_and(|c| c.is_uppercase()) {
        let mut chars: Vec<char> = replacement.chars().collect();
        if let Some(first_char) = chars.get_mut(0) {
            *first_char = first_char.to_uppercase().next().unwrap_or(*first_char);
        }
        chars.into_iter().collect()
    } else {
        replacement.to_string()
    }
}

fn extract_punctuation(word: &str) -> (&str, &str) {
    let prefix_end = word
        .char_indices()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(index, _)| index)
        .unwrap_or(word.len());
    let suffix_start = word
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(index, c)| index + c.len_utf8())
        .unwrap_or(0);

    let prefix = if prefix_end > 0 {
        &word[..prefix_end]
    } else {
        ""
    };

    let suffix = if suffix_start < word.len() {
        &word[suffix_start..]
    } else {
        ""
    };

    (prefix, suffix)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputLanguageEvidence {
    UserSelected(String),
    ModelConstrained(String),
    ModelDetected(String),
    TextDetected(String),
    TranslatedToEnglish,
    Unknown,
}

impl OutputLanguageEvidence {
    fn language(&self) -> Option<&str> {
        match self {
            Self::UserSelected(language)
            | Self::ModelConstrained(language)
            | Self::ModelDetected(language)
            | Self::TextDetected(language) => Some(language),
            Self::TranslatedToEnglish => Some("en"),
            Self::Unknown => None,
        }
    }
}

const UNIVERSAL_FILLER_WORDS: &[&str] = &[
    "uh", "uhm", "umm", "uhh", "uhhh", "ehh", "ehm", "ahm", "hmm", "hm", "mmm", "хм", "ммм",
];

fn gated_filler_words_for_language(lang: &str) -> &'static [&'static str] {
    let base_lang = lang.split(&['-', '_'][..]).next().unwrap_or(lang);

    match base_lang {
        "en" => &["um", "ah", "eh", "ha"],
        "de" => &["äh", "ähm"],
        "fr" => &["euh"],
        _ => &[],
    }
}

fn collapse_stutters(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return text.to_string();
    }

    let mut result: Vec<&str> = Vec::new();
    let mut i = 0;

    while i < words.len() {
        let word = words[i];
        let word_lower = word.to_lowercase();

        if word_lower.chars().all(|c| c.is_alphabetic()) {
            let mut count = 1;
            while i + count < words.len() && words[i + count].to_lowercase() == word_lower {
                count += 1;
            }

            if count >= 3 {
                result.push(word);
                i += count;
            } else {
                result.push(word);
                i += 1;
            }
        } else {
            result.push(word);
            i += 1;
        }
    }

    result.join(" ")
}

pub fn remove_filler_words(
    text: &str,
    language: &OutputLanguageEvidence,
    custom_filler_words: &Option<Vec<String>>,
    enabled: bool,
) -> String {
    if !enabled {
        return text.to_string();
    }

    let patterns: Vec<Regex> = match custom_filler_words {
        Some(words) => words
            .iter()
            .filter_map(|word| Regex::new(&format!(r"(?i)\b{}\b[,.]?", regex::escape(word))).ok())
            .collect(),
        None => UNIVERSAL_FILLER_WORDS
            .iter()
            .chain(
                language
                    .language()
                    .map(gated_filler_words_for_language)
                    .unwrap_or_default(),
            )
            .map(|word| Regex::new(&format!(r"(?i)\b{}\b[,.]?", regex::escape(word))).unwrap())
            .collect(),
    };

    let mut filtered = text.to_string();
    for pattern in &patterns {
        filtered = pattern.replace_all(&filtered, "").to_string();
    }

    filtered
}

pub fn normalize_transcription_output(text: &str) -> String {
    collapse_stutters(text).trim().to_string()
}

fn keyword_match_keys(keyword: &str) -> Option<Vec<String>> {
    let keys: Vec<String> = keyword.split_whitespace().map(build_match_key).collect();
    (!keys.is_empty() && !keys.iter().any(String::is_empty)).then_some(keys)
}

fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub fn strip_trailing_keyword(text: &str, keyword: &str) -> Option<String> {
    let keyword_keys = keyword_match_keys(keyword)?;

    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < keyword_keys.len() {
        return None;
    }
    let tail = &tokens[tokens.len() - keyword_keys.len()..];
    if tail
        .iter()
        .zip(&keyword_keys)
        .any(|(token, key)| build_match_key(token) != *key)
    {
        return None;
    }

    let keyword_start = tail[0].as_ptr() as usize - text.as_ptr() as usize;
    let rest = text[..keyword_start]
        .trim_end()
        .trim_end_matches([
            ',', ';', ':', '-', '\u{2013}', '\u{2014}', '\u{3001}', '\u{FF0C}',
        ])
        .trim_end();
    Some(rest.to_string())
}

pub fn scrub_before_keyword(text: &str, keyword: &str) -> Option<String> {
    let keyword_keys = keyword_match_keys(keyword)?;

    let tokens: Vec<&str> = text.split_whitespace().collect();
    let keys: Vec<String> = tokens.iter().map(|token| build_match_key(token)).collect();
    let start = keys
        .windows(keyword_keys.len())
        .rposition(|window| window == keyword_keys.as_slice())?;

    let last = tokens[start + keyword_keys.len() - 1];
    let keyword_end = last.as_ptr() as usize - text.as_ptr() as usize + last.len();
    let rest = text[keyword_end..].trim_start_matches(|c: char| {
        c.is_whitespace()
            || ".,;:!?-\u{2013}\u{2014}\u{3001}\u{3002}\u{FF0C}\u{FF01}\u{FF1F}".contains(c)
    });

    Some(capitalize_first(rest))
}

const ABBREVIATIONS_THAT_NEVER_END_A_SENTENCE: &[&str] =
    &["dr", "mr", "mrs", "ms", "prof", "vs", "eg", "ie"];

fn ends_sentence(token: &str) -> bool {
    token
        .trim_end_matches(['"', '\'', ')', '\u{201D}', '\u{2019}'])
        .ends_with([
            '.', '!', '?', '\u{2026}', '\u{3002}', '\u{FF01}', '\u{FF1F}',
        ])
        && !ABBREVIATIONS_THAT_NEVER_END_A_SENTENCE.contains(&build_match_key(token).as_str())
}

fn has_boundary_on_both_sides(tokens: &[&str], start: usize, end: usize) -> bool {
    let is_break = |c: Option<char>| !c.is_some_and(char::is_alphanumeric);
    let before = start == 0
        || is_break(tokens[start - 1].chars().last())
        || is_break(tokens[start].chars().next());
    let after = end == tokens.len()
        || is_break(tokens[end - 1].chars().last())
        || is_break(tokens[end].chars().next());
    before && after
}

pub fn scratch_before_keyword(text: &str, keyword: &str) -> Option<String> {
    let keyword_keys = keyword_match_keys(keyword)?;
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let keys: Vec<String> = tokens.iter().map(|token| build_match_key(token)).collect();

    let mut kept: Vec<String> = Vec::with_capacity(tokens.len());
    let mut matched = false;
    let mut after_keyword = false;
    let mut index = 0;
    while index < tokens.len() {
        let end = index + keyword_keys.len();
        if end <= tokens.len()
            && keys[index..end] == keyword_keys[..]
            && has_boundary_on_both_sides(&tokens, index, end)
        {
            while kept
                .last()
                .is_some_and(|token| build_match_key(token).is_empty())
            {
                kept.pop();
            }
            kept.pop();
            while kept.last().is_some_and(|token| !ends_sentence(token)) {
                kept.pop();
            }
            matched = true;
            after_keyword = true;
            index = end;
        } else if after_keyword && keys[index].is_empty() {
            index += 1;
        } else if after_keyword {
            kept.push(capitalize_first(tokens[index]));
            after_keyword = false;
            index += 1;
        } else {
            kept.push(tokens[index].to_string());
            index += 1;
        }
    }

    matched.then(|| kept.join(" "))
}

pub fn expand_snippets<'a>(
    text: &str,
    snippets: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    expand_snippets_with_fixes(text, snippets).0
}

/// `expand_snippets`, and the triggers it expanded, in order.
pub fn expand_snippets_with_fixes<'a>(
    text: &str,
    snippets: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> (String, Vec<Fix>) {
    let mut triggers: Vec<(Vec<String>, &str)> = snippets
        .into_iter()
        .map(|(trigger, expansion)| {
            let keys: Vec<String> = trigger.split_whitespace().map(build_match_key).collect();
            (keys, expansion)
        })
        .filter(|(keys, _)| !keys.is_empty() && keys.iter().all(|key| !key.is_empty()))
        .collect();
    if triggers.is_empty() {
        return (text.to_string(), Vec::new());
    }
    triggers.sort_by_key(|(keys, _)| std::cmp::Reverse(keys.len()));

    let tokens: Vec<&str> = text.split_whitespace().collect();
    let keys: Vec<String> = tokens.iter().map(|token| build_match_key(token)).collect();
    let offset = |token: &str| token.as_ptr() as usize - text.as_ptr() as usize;

    let mut expanded = String::with_capacity(text.len());
    let mut fixes = Vec::new();
    let mut copied = 0;
    let mut i = 0;
    while i < tokens.len() {
        let hit = triggers
            .iter()
            .find(|(trigger, _)| keys.get(i..i + trigger.len()) == Some(trigger.as_slice()));
        let Some((trigger, expansion)) = hit else {
            i += 1;
            continue;
        };
        let first = tokens[i];
        let last = tokens[i + trigger.len() - 1];
        let lead_end = first.find(char::is_alphanumeric).unwrap_or(0);
        let trail_start = last
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_alphanumeric())
            .map_or(last.len(), |(index, c)| index + c.len_utf8());

        let heard_end = offset(last) + trail_start;
        let heard_start = offset(first) + lead_end;
        fixes.push(Fix {
            heard: text[heard_start..heard_end.max(heard_start)].to_string(),
            written: expansion.to_string(),
            replacement: true,
        });
        expanded.push_str(&text[copied..offset(first)]);
        expanded.push_str(&first[..lead_end]);
        expanded.push_str(expansion);
        expanded.push_str(&last[trail_start..]);
        copied = offset(last) + last.len();
        i += trigger.len();
    }
    expanded.push_str(&text[copied..]);
    (expanded, fixes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_words_report_what_they_heard_and_wrote() {
        let words = ["IntraFi".to_string(), "Karat".to_string()];
        let (text, fixes) =
            apply_custom_words_with_fixes("ping the karet team about intra fi.", &words, 0.18);
        assert_eq!(text, "ping the Karat team about IntraFi.");
        assert_eq!(
            fixes,
            [
                Fix {
                    heard: "karet".into(),
                    written: "Karat".into(),
                    replacement: false,
                },
                Fix {
                    heard: "intra fi".into(),
                    written: "IntraFi".into(),
                    replacement: false,
                },
            ]
        );
    }

    #[test]
    fn custom_words_report_nothing_for_a_word_already_spelled_right() {
        let words = ["Karat".to_string()];
        let (_, fixes) = apply_custom_words_with_fixes("Karat said hi", &words, 0.18);
        assert!(fixes.is_empty());
    }

    #[test]
    fn expand_snippets_reports_the_trigger_it_heard() {
        let (text, fixes) =
            expand_snippets_with_fixes("Send it to My Email.", [("my email", "a@example.com")]);
        assert_eq!(text, "Send it to a@example.com.");
        assert_eq!(
            fixes,
            [Fix {
                heard: "My Email".into(),
                written: "a@example.com".into(),
                replacement: true,
            }]
        );
    }

    #[test]
    fn expand_snippets_replaces_a_trigger_and_keeps_punctuation() {
        let snippets = [("my email", "andy@example.com")];
        assert_eq!(
            expand_snippets("Send it to My email.", snippets),
            "Send it to andy@example.com."
        );
        assert_eq!(
            expand_snippets("(my, email) twice: my email", snippets),
            "(andy@example.com) twice: andy@example.com"
        );
    }

    #[test]
    fn expand_snippets_matches_whole_words_only() {
        let snippets = [("my email", "andy@example.com")];
        assert_eq!(
            expand_snippets("check my emails", snippets),
            "check my emails"
        );
    }

    #[test]
    fn expand_snippets_prefers_the_longer_trigger() {
        let snippets = [
            ("my email", "short"),
            ("my email address", "andy@example.com"),
        ];
        assert_eq!(
            expand_snippets("it's my email address", snippets),
            "it's andy@example.com"
        );
    }

    #[test]
    fn expand_snippets_keeps_multiline_expansions() {
        let snippets = [("sign off", "Thanks,\nAndy")];
        assert_eq!(
            expand_snippets("see you. Sign off", snippets),
            "see you. Thanks,\nAndy"
        );
    }

    #[test]
    fn expand_snippets_ignores_triggers_without_letters_or_digits() {
        let snippets = [("", "x"), ("--", "y")];
        assert_eq!(expand_snippets("a -- b", snippets), "a -- b");
    }

    #[test]
    fn strip_trailing_keyword_matches_the_final_word() {
        assert_eq!(
            strip_trailing_keyword("hello there submit", "submit").as_deref(),
            Some("hello there")
        );
        assert_eq!(
            strip_trailing_keyword("Hello there, Submit.", "submit").as_deref(),
            Some("Hello there")
        );
        assert_eq!(
            strip_trailing_keyword("Ship it  submit!  ", "Submit").as_deref(),
            Some("Ship it")
        );
        assert_eq!(
            strip_trailing_keyword("Submit.", "submit").as_deref(),
            Some("")
        );
    }

    #[test]
    fn strip_trailing_keyword_supports_phrases() {
        assert_eq!(
            strip_trailing_keyword("Sounds good. Send it.", "send it").as_deref(),
            Some("Sounds good.")
        );
        assert_eq!(strip_trailing_keyword("Sounds good, it.", "send it"), None);
    }

    #[test]
    fn strip_trailing_keyword_ignores_other_positions_and_words() {
        assert_eq!(
            strip_trailing_keyword("please submit the form by Friday", "submit"),
            None
        );
        assert_eq!(strip_trailing_keyword("I will resubmit", "submit"), None);
        assert_eq!(strip_trailing_keyword("It was submitted", "submit"), None);
        assert_eq!(strip_trailing_keyword("", "submit"), None);
        assert_eq!(strip_trailing_keyword("hello submit", ""), None);
        assert_eq!(strip_trailing_keyword("hello submit", "..."), None);
    }

    #[test]
    fn strip_trailing_keyword_removes_only_one_occurrence() {
        assert_eq!(
            strip_trailing_keyword("submit submit", "submit").as_deref(),
            Some("submit")
        );
    }

    fn scratch(text: &str) -> Option<String> {
        scratch_before_keyword(text, "scratch that")
    }

    #[test]
    fn scratch_removes_only_the_sentence_before_the_keyword() {
        assert_eq!(
            scratch("Hello there. Let's meet Tuesday. Scratch that. Let's meet Wednesday.")
                .as_deref(),
            Some("Hello there. Let's meet Wednesday.")
        );
        assert_eq!(
            scratch("Hello there. Let's meet Tuesday, scratch that, let's meet Wednesday.")
                .as_deref(),
            Some("Hello there. Let's meet Wednesday.")
        );
    }

    #[test]
    fn scratch_removes_all_text_when_no_earlier_sentence_ends() {
        assert_eq!(
            scratch("Hi Bob, scratch that, hey Alice.").as_deref(),
            Some("Hey Alice.")
        );
    }

    #[test]
    fn scratch_stacks_from_left_to_right() {
        assert_eq!(
            scratch("One. Two. Scratch that. Three. Scratch that. Four.").as_deref(),
            Some("One. Four.")
        );
        assert_eq!(
            scratch("One. Two. Scratch that. Scratch that. Three.").as_deref(),
            Some("Three.")
        );
    }

    #[test]
    fn scratch_at_the_end_or_alone_leaves_what_came_first() {
        assert_eq!(scratch("One. Two. Scratch that.").as_deref(), Some("One."));
        assert_eq!(scratch("Draft email. Scratch that.").as_deref(), Some(""));
        assert_eq!(scratch("scratch that").as_deref(), Some(""));
    }

    #[test]
    fn scratch_handles_punctuation_variants() {
        for text in [
            "Keep. Drop this. Scratch that. new text",
            "Keep. Drop this, SCRATCH THAT! New text",
            "Keep. Drop this - scratch that - new text",
            "Keep. Drop this \u{2014} scratch that \u{2014} new text",
            "Keep. Drop this \"scratch that\" new text",
        ] {
            assert_eq!(scratch(text).as_deref(), Some("Keep. New text"), "{text}");
        }
    }

    #[test]
    fn scratch_treats_a_closing_quote_as_part_of_the_sentence_end() {
        assert_eq!(
            scratch("He said \"no.\" Drop this. Scratch that.").as_deref(),
            Some("He said \"no.\"")
        );
    }

    #[test]
    fn scratch_removes_a_finished_sentence_behind_a_lone_separator() {
        for text in [
            "Keep. Drop this. - Scratch that - next text.",
            "Keep. Drop this. \u{2014} scratch that \u{2014} next text.",
        ] {
            assert_eq!(scratch(text).as_deref(), Some("Keep. Next text."), "{text}");
        }
    }

    #[test]
    fn scratch_reads_past_abbreviations_that_never_end_a_sentence() {
        assert_eq!(
            scratch("Keep this. Send it to Dr. Smith. Scratch that. Next.").as_deref(),
            Some("Keep this. Next.")
        );
        assert_eq!(
            scratch("Keep this. Bring fruit, e.g. apples. Scratch that. Next.").as_deref(),
            Some("Keep this. Next.")
        );
    }

    #[test]
    fn scratch_stops_at_abbreviations_that_can_end_a_sentence() {
        assert_eq!(
            scratch("Meet at 5 p.m. Drop this. Scratch that. Next.").as_deref(),
            Some("Meet at 5 p.m. Next.")
        );
    }

    #[test]
    fn scratch_ignores_the_keyword_inside_a_sentence() {
        assert_eq!(scratch("Please scratch that itch tonight."), None);
        assert_eq!(scratch("I'd scratch that, thanks."), None);
        assert_eq!(scratch("One. Scratch that itch."), None);
        assert_eq!(scratch("Nothing to see here."), None);
        assert_eq!(scratch(""), None);
    }

    #[test]
    fn scratch_supports_custom_keywords() {
        assert_eq!(
            scratch_before_keyword("One. Two. Undo. Three.", "Undo").as_deref(),
            Some("One. Three.")
        );
        assert_eq!(scratch_before_keyword("One. Two.", ""), None);
        assert_eq!(scratch_before_keyword("One. -- Two.", "--"), None);
    }

    #[test]
    fn scrub_before_keyword_keeps_text_after_the_keyword() {
        assert_eq!(
            scrub_before_keyword(
                "Let's meet Tuesday. Scrub that. Let's meet Wednesday.",
                "scrub that"
            )
            .as_deref(),
            Some("Let's meet Wednesday.")
        );
        assert_eq!(
            scrub_before_keyword("Hi Bob, scrub that, hey Alice, how are you?", "scrub that")
                .as_deref(),
            Some("Hey Alice, how are you?")
        );
        assert_eq!(
            scrub_before_keyword("Please scrub that pan tonight.", "scrub that").as_deref(),
            Some("Pan tonight.")
        );
    }

    #[test]
    fn scrub_before_keyword_uses_the_last_occurrence() {
        assert_eq!(
            scrub_before_keyword("One. Scrub that. Two. Scrub that. Three.", "scrub that")
                .as_deref(),
            Some("Three.")
        );
    }

    #[test]
    fn scrub_before_keyword_returns_empty_when_nothing_follows() {
        assert_eq!(
            scrub_before_keyword("Draft email. Scrub that.", "scrub that").as_deref(),
            Some("")
        );
        assert_eq!(
            scrub_before_keyword("scrub that", "scrub that").as_deref(),
            Some("")
        );
        assert_eq!(
            scrub_before_keyword("Draft email, scrub that!  ", "scrub that").as_deref(),
            Some("")
        );
    }

    #[test]
    fn scrub_before_keyword_handles_punctuation_variants() {
        for text in [
            "Old text. Scrub that. New text",
            "Old text, scrub that, new text",
            "Old text scrub that! new text",
            "Old text SCRUB THAT? new text",
            "Old text scrub that new text",
            "Old text \"scrub that\" new text",
            "Old text scrub that - new text",
            "Old text scrub that \u{2014} new text",
        ] {
            assert_eq!(
                scrub_before_keyword(text, "scrub that").as_deref(),
                Some("New text"),
                "{text}"
            );
        }
    }

    #[test]
    fn scrub_before_keyword_supports_custom_keywords() {
        assert_eq!(
            scrub_before_keyword(
                "Hello there. Start over please. Goodbye.",
                "start over please"
            )
            .as_deref(),
            Some("Goodbye.")
        );
        assert_eq!(
            scrub_before_keyword("Hello there. Restart, goodbye.", "Restart").as_deref(),
            Some("Goodbye.")
        );
        assert_eq!(
            scrub_before_keyword("I will restart the server", "start"),
            None
        );
    }

    #[test]
    fn scrub_before_keyword_leaves_other_text_alone() {
        assert_eq!(
            scrub_before_keyword("Scrub the pan, then that.", "scrub that"),
            None
        );
        assert_eq!(scrub_before_keyword("", "scrub that"), None);
        assert_eq!(scrub_before_keyword("Hi Bob, scrub that, hey", ""), None);
        assert_eq!(scrub_before_keyword("Hi Bob, scrub that, hey", "   "), None);
        assert_eq!(scrub_before_keyword("Hi Bob, -- hey", "--"), None);
    }

    fn filter_transcription_output(
        text: &str,
        language: &str,
        custom_filler_words: &Option<Vec<String>>,
    ) -> String {
        let language = OutputLanguageEvidence::UserSelected(language.to_string());
        let filtered = remove_filler_words(text, &language, custom_filler_words, true);
        normalize_transcription_output(&filtered)
    }

    #[test]
    fn test_apply_custom_words_exact_match() {
        let text = "hello world";
        let custom_words = vec!["Hello".to_string(), "World".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_apply_custom_words_fuzzy_match() {
        let text = "helo wrold";
        let custom_words = vec!["hello".to_string(), "world".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_preserve_case_pattern() {
        assert_eq!(preserve_case_pattern("HELLO", "world"), "WORLD");
        assert_eq!(preserve_case_pattern("Hello", "world"), "World");
        assert_eq!(preserve_case_pattern("hello", "WORLD"), "WORLD");
    }

    #[test]
    fn test_extract_punctuation() {
        assert_eq!(extract_punctuation("hello"), ("", ""));
        assert_eq!(extract_punctuation("!hello?"), ("!", "?"));
        assert_eq!(extract_punctuation("...hello..."), ("...", "..."));
    }

    #[test]
    fn test_extract_punctuation_uses_unicode_boundaries() {
        assert_eq!(extract_punctuation("你好。"), ("", "。"));
        assert_eq!(extract_punctuation("「你好」"), ("「", "」"));
        assert_eq!(extract_punctuation("你好！"), ("", "！"));
    }

    #[test]
    fn test_empty_custom_words() {
        let text = "hello world";
        let custom_words = vec![];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_filter_filler_words() {
        let text = "So uhm I was thinking uh about this";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "So I was thinking about this");
    }

    #[test]
    fn test_filter_filler_words_case_insensitive() {
        let text = "UHM this is UH a test";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "this is a test");
    }

    #[test]
    fn test_filter_filler_words_with_punctuation() {
        let text = "Well, uhm, I think, uh. that's right";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "Well, I think, that's right");
    }

    #[test]
    fn test_filter_cleans_whitespace() {
        let text = "Hello    world   test";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "Hello world test");
    }

    #[test]
    fn test_filter_trims() {
        let text = "  Hello world  ";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_filter_combined() {
        let text = "  Uhm, so I was, uh, thinking about this  ";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "so I was, thinking about this");
    }

    #[test]
    fn test_filter_preserves_valid_text() {
        let text = "This is a completely normal sentence.";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "This is a completely normal sentence.");
    }

    #[test]
    fn test_filter_stutter_collapse() {
        let text = "w wh wh wh wh wh wh wh wh wh why";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "w wh why");
    }

    #[test]
    fn test_filter_stutter_short_words() {
        let text = "I I I I think so so so so";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "I think so");
    }

    #[test]
    fn test_filter_stutter_longer_words() {
        let text = "Check data doc doc doc doc documentation.";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "Check data doc documentation.");
    }

    #[test]
    fn test_filter_stutter_mixed_case() {
        let text = "No NO no NO no";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "No");
    }

    #[test]
    fn test_filter_stutter_preserves_two_repetitions() {
        let text = "no no is fine";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "no no is fine");
    }

    #[test]
    fn test_filter_english_removes_um() {
        let text = "um I think um this is good";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "I think this is good");
    }

    #[test]
    fn test_filter_portuguese_preserves_um() {
        let text = "um gato bonito";
        let result = filter_transcription_output(text, "pt", &None);
        assert_eq!(result, "um gato bonito");
    }

    #[test]
    fn test_filter_spanish_preserves_ha() {
        let text = "ha sido un buen día";
        let result = filter_transcription_output(text, "es", &None);
        assert_eq!(result, "ha sido un buen día");
    }

    #[test]
    fn test_filter_language_code_with_region() {
        let text = "um gato bonito";
        let result = filter_transcription_output(text, "pt-BR", &None);
        assert_eq!(result, "um gato bonito");
    }

    #[test]
    fn test_filter_custom_filler_words_override() {
        let custom = Some(vec!["okay".to_string(), "right".to_string()]);
        let text = "okay so I think right this works";
        let result = filter_transcription_output(text, "en", &custom);
        assert_eq!(result, "so I think this works");
    }

    #[test]
    fn test_filter_custom_filler_words_empty_disables() {
        let custom = Some(vec![]);
        let text = "So uhm I was thinking uh about this";
        let result = filter_transcription_output(text, "en", &custom);
        assert_eq!(result, "So uhm I was thinking uh about this");
    }

    #[test]
    fn test_filter_unknown_language_still_removes_universal_fillers() {
        let text = "uh I think uhm this works";
        let result = filter_transcription_output(text, "xx", &None);
        assert_eq!(result, "I think this works");
    }

    #[test]
    fn test_filter_unknown_language_does_not_remove_um() {
        let text = "um I think this works";
        let result = filter_transcription_output(text, "xx", &None);
        assert_eq!(result, "um I think this works");
    }

    #[test]
    fn test_filter_unknown_evidence_removes_universal_keeps_gated() {
        let filtered = remove_filler_words(
            "uhh bueno hmm creo que um ha llegado",
            &OutputLanguageEvidence::Unknown,
            &None,
            true,
        );
        assert_eq!(
            normalize_transcription_output(&filtered),
            "bueno creo que um ha llegado"
        );

        let cyrillic = remove_filler_words(
            "хм я думаю ммм это работает",
            &OutputLanguageEvidence::Unknown,
            &None,
            true,
        );
        assert_eq!(
            normalize_transcription_output(&cyrillic),
            "я думаю это работает"
        );
    }

    #[test]
    fn test_filter_german_gated_fillers_require_evidence() {
        let text = "äh ich glaube ähm das passt";

        let unknown = remove_filler_words(text, &OutputLanguageEvidence::Unknown, &None, true);
        assert_eq!(normalize_transcription_output(&unknown), text);

        let result = filter_transcription_output(text, "de", &None);
        assert_eq!(result, "ich glaube das passt");
    }

    #[test]
    fn test_filter_preserves_millimetre_unit() {
        let text = "the screw is 5 mm long";
        let result = filter_transcription_output(text, "en", &None);
        assert_eq!(result, "the screw is 5 mm long");
    }

    #[test]
    fn test_filter_detected_evidence_unlocks_gated_fillers() {
        let model = remove_filler_words(
            "um I think this works",
            &OutputLanguageEvidence::ModelDetected("en".to_string()),
            &None,
            true,
        );
        assert_eq!(normalize_transcription_output(&model), "I think this works");

        let text = remove_filler_words(
            "euh je pense que ça marche",
            &OutputLanguageEvidence::TextDetected("fr".to_string()),
            &None,
            true,
        );
        assert_eq!(
            normalize_transcription_output(&text),
            "je pense que ça marche"
        );
    }

    #[test]
    fn test_filter_master_toggle_disables_custom_and_builtin_removal() {
        let text = "um customword I think";
        let language = OutputLanguageEvidence::UserSelected("en".to_string());
        let custom = Some(vec!["customword".to_string()]);

        let result = remove_filler_words(text, &language, &custom, false);

        assert_eq!(result, text);
    }

    #[test]
    fn test_filter_custom_words_apply_without_language_evidence() {
        let custom = Some(vec!["customword".to_string()]);
        let text = "customword should be removed but um should remain";

        let filtered = remove_filler_words(text, &OutputLanguageEvidence::Unknown, &custom, true);
        let result = normalize_transcription_output(&filtered);

        assert_eq!(result, "should be removed but um should remain");
    }

    #[test]
    fn test_apply_custom_words_ngram_two_words() {
        let text = "il cui nome è Charge B, che permette";
        let custom_words = vec!["ChargeBee".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert!(result.contains("ChargeBee,"), "unexpected result: {result}");
        assert!(!result.contains("Charge B"));
    }

    #[test]
    fn test_apply_custom_words_ngram_three_words() {
        let text = "use Chat G P T for this";
        let custom_words = vec!["ChatGPT".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert!(result.contains("ChatGPT"));
    }

    #[test]
    fn timed_words_merge_into_one_dictionary_entry_spanning_their_times() {
        let word = |t0, t1, text: &str| TimedText {
            t0,
            t1,
            text: text.to_string(),
        };
        let heard = vec![
            word(0, 200, "ask"),
            word(250, 400, "Charge"),
            word(400, 600, "B,"),
            word(700, 900, "today"),
        ];
        let custom_words = vec!["ChargeBee".to_string()];
        assert_eq!(
            apply_custom_words_to_timed(&heard, &custom_words, 0.5),
            vec![
                word(0, 200, "ask"),
                word(250, 600, "ChargeBee,"),
                word(700, 900, "today"),
            ]
        );
        assert_eq!(apply_custom_words_to_timed(&heard, &[], 0.5), heard);
    }

    #[test]
    fn test_apply_custom_words_prefers_longer_ngram() {
        let text = "Open AI GPT model";
        let custom_words = vec!["OpenAI".to_string(), "GPT".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert_eq!(result, "OpenAI GPT model");
    }

    #[test]
    fn test_apply_custom_words_ngram_preserves_case() {
        let text = "CHARGE B is great";
        let custom_words = vec!["ChargeBee".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert!(result.contains("CHARGEBEE"));
    }

    #[test]
    fn test_apply_custom_words_ngram_with_spaces_in_custom() {
        let text = "using Mac Book Pro";
        let custom_words = vec!["MacBook Pro".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert_eq!(result, "using MacBook Pro");
    }

    #[test]
    fn test_apply_custom_words_trailing_number_not_doubled() {
        let text = "use GPT4 for this";
        let custom_words = vec!["GPT-4".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert!(
            !result.contains("GPT-44"),
            "got double-counted result: {}",
            result
        );
    }

    #[test]
    fn test_apply_custom_words_matches_ampersand_word() {
        let text = "send it to RD for review";
        let custom_words = vec!["R&D".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.18);
        assert_eq!(result, "send it to R&D for review");
    }

    #[test]
    fn test_apply_custom_words_matches_spoken_ampersand_word() {
        let text = "send it to R and D for review";
        let custom_words = vec!["R&D".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.18);
        assert_eq!(result, "send it to R&D for review");
    }

    #[test]
    fn test_apply_custom_words_preserves_ampersand_word() {
        let text = "send it to R&D for review";
        let custom_words = vec!["R&D".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.18);
        assert_eq!(result, "send it to R&D for review");
    }

    #[test]
    fn test_apply_custom_words_handles_unicode_punctuation() {
        let text = "「Bespok。」";
        let custom_words = vec!["Bespoke".to_string()];
        let result = apply_custom_words(text, &custom_words, 0.5);
        assert_eq!(result, "「Bespoke。」");
    }

    #[test]
    fn test_apply_custom_words_skips_cjk_fuzzy_matching() {
        let text = "你好。";
        let custom_words = vec!["你号".to_string()];
        let result = apply_custom_words(text, &custom_words, 1.0);
        assert_eq!(result, text);
    }
}
