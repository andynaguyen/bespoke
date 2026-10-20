//! Per-app writing styles applied to dictated text before it's pasted.

use crate::settings::{AppStyleRule, WritingStyle};

/// Style for a dictation started in the app with `bundle_id`. Apps without a
/// rule, and platforms where the app is unknown, get `Formal`.
pub fn style_for_app(rules: &[AppStyleRule], bundle_id: Option<&str>) -> WritingStyle {
    let Some(bundle_id) = bundle_id else {
        return WritingStyle::Formal;
    };
    rules
        .iter()
        .find(|rule| rule.bundle_id == bundle_id)
        .map_or(WritingStyle::Formal, |rule| rule.style)
}

/// Formats `text` in `style`.
///
/// - Formal returns the text unchanged.
/// - Casual removes commas (except inside numbers like "1,000") and drops one
///   trailing period. `?`, `!`, `...`, and periods between sentences stay.
/// - Very casual also lowercases the first letter of each sentence (acronyms
///   like "API" excepted) and the pronoun "I" and its contractions anywhere,
///   and drops apostrophes inside words ("don't" -> "dont") except in
///   [`KEPT_CONTRACTIONS`].
pub fn apply_style(text: &str, style: WritingStyle) -> String {
    if style == WritingStyle::Formal {
        return text.to_string();
    }
    let very_casual = style == WritingStyle::VeryCasual;

    let trimmed = text.trim_end();
    let trailing_whitespace = &text[trimmed.len()..];
    let body = match trimmed.strip_suffix('.') {
        Some(rest) if !rest.ends_with('.') => rest,
        _ => trimmed,
    };

    let mut styled = String::with_capacity(text.len());
    let mut sentence_start = true;
    let mut after_terminator = false;
    let mut prev: Option<char> = None;
    let mut chars = body.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let next = chars.peek().map(|&(_, next)| next);
        if c.is_whitespace() {
            if after_terminator || c == '\n' {
                sentence_start = true;
            }
            styled.push(c);
        } else if c == ',' && !is_digit_group_separator(prev, next) {
            // "a , b" would otherwise become "a  b"
            let spaced_before = styled.is_empty() || styled.ends_with(char::is_whitespace);
            if spaced_before && matches!(next, Some(' ' | '\t')) {
                chars.next();
            }
        } else if very_casual && is_droppable_apostrophe(body, index, prev, next) {
            // "don't" -> "dont"
        } else {
            let rest = &body[index..];
            let lowercase = very_casual
                && ((sentence_start && should_lowercase(rest)) || is_pronoun_i(prev, rest));
            if lowercase {
                styled.extend(c.to_lowercase());
            } else {
                styled.push(c);
            }
            sentence_start = false;
            after_terminator = matches!(c, '.' | '!' | '?');
        }
        prev = Some(c);
    }
    styled.push_str(trailing_whitespace);
    styled
}

/// Contractions that read as a different word without the apostrophe ("we'll"
/// vs "well"), so Very casual keeps it.
const KEPT_CONTRACTIONS: [&str; 8] = [
    "we'll", "we're", "i'll", "he'll", "she'll", "i'd", "she'd", "we'd",
];

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}')
}

/// Whether the apostrophe at `index` sits between two letters in a word that
/// isn't one of [`KEPT_CONTRACTIONS`]. Quote marks at word edges don't count.
fn is_droppable_apostrophe(
    body: &str,
    index: usize,
    prev: Option<char>,
    next: Option<char>,
) -> bool {
    let between_letters =
        prev.is_some_and(char::is_alphabetic) && next.is_some_and(char::is_alphabetic);
    if !between_letters || !body[index..].starts_with(is_apostrophe) {
        return false;
    }
    let is_word_char = |c: char| c.is_alphabetic() || is_apostrophe(c);
    let start = body[..index]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_word_char(c))
        .last()
        .map_or(index, |(i, _)| i);
    let end = body[index..]
        .char_indices()
        .find(|&(_, c)| !is_word_char(c))
        .map_or(body.len(), |(i, _)| index + i);
    let word: String = body[start..end]
        .chars()
        .map(|c| if is_apostrophe(c) { '\'' } else { c })
        .flat_map(char::to_lowercase)
        .collect();
    !KEPT_CONTRACTIONS.contains(&word.trim_matches('\''))
}

/// A comma between two digits, as in "1,000".
fn is_digit_group_separator(prev: Option<char>, next: Option<char>) -> bool {
    prev.is_some_and(|c| c.is_ascii_digit()) && next.is_some_and(|c| c.is_ascii_digit())
}

/// Whether the word at the start of `word` should lose its capital letter.
fn should_lowercase(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_uppercase() {
        return false;
    }
    // Acronyms like "API" or "NASA"
    !chars.next().is_some_and(char::is_uppercase)
}

/// Whether `rest` starts with the whole word "I", "I'm", "I'll", "I've", or
/// "I'd" (straight or curly apostrophe). `prev` is the character before it.
fn is_pronoun_i(prev: Option<char>, rest: &str) -> bool {
    if prev.is_some_and(char::is_alphanumeric) {
        return false;
    }
    let Some(mut after) = rest.strip_prefix('I') else {
        return false;
    };
    if let Some(suffix) = after.strip_prefix(['\'', '\u{2019}']) {
        match ["m", "ll", "ve", "d"]
            .iter()
            .find_map(|contraction| suffix.strip_prefix(contraction))
        {
            Some(tail) => after = tail,
            None => return false,
        }
    }
    !after.starts_with(char::is_alphanumeric)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str =
        "Hey, are you free for lunch tomorrow? Let's do 12 if that works for you.";

    fn casual(text: &str) -> String {
        apply_style(text, WritingStyle::Casual)
    }

    fn very_casual(text: &str) -> String {
        apply_style(text, WritingStyle::VeryCasual)
    }

    #[test]
    fn formal_leaves_text_unchanged() {
        assert_eq!(apply_style(EXAMPLE, WritingStyle::Formal), EXAMPLE);
    }

    #[test]
    fn casual_matches_the_example() {
        assert_eq!(
            casual(EXAMPLE),
            "Hey are you free for lunch tomorrow? Let's do 12 if that works for you"
        );
    }

    #[test]
    fn very_casual_matches_the_example() {
        assert_eq!(
            very_casual(EXAMPLE),
            "hey are you free for lunch tomorrow? lets do 12 if that works for you"
        );
    }

    #[test]
    fn casual_removes_list_commas_but_keeps_number_separators() {
        assert_eq!(casual("Eggs, milk, and bread."), "Eggs milk and bread");
        assert_eq!(casual("It costs 1,000 dollars."), "It costs 1,000 dollars");
        assert_eq!(casual("Pick 5, 6, or 7."), "Pick 5 6 or 7");
    }

    #[test]
    fn casual_never_leaves_a_double_space() {
        assert_eq!(casual("Yes , sure."), "Yes sure");
        assert_eq!(casual(", Right."), "Right");
        assert_eq!(casual("Well,"), "Well");
    }

    #[test]
    fn casual_keeps_capitals() {
        assert_eq!(casual("Sounds good. See you."), "Sounds good. See you");
    }

    #[test]
    fn very_casual_lowercases_after_every_sentence_terminator() {
        assert_eq!(
            very_casual("Really? Yes! Okay. Done"),
            "really? yes! okay. done"
        );
    }

    #[test]
    fn very_casual_only_lowercases_sentence_starts() {
        assert_eq!(very_casual("Meet Sarah in Paris."), "meet Sarah in Paris");
        // A period not followed by whitespace doesn't end a sentence
        assert_eq!(very_casual("Version 2.Five"), "version 2.Five");
    }

    #[test]
    fn very_casual_lowercases_the_pronoun_i_everywhere() {
        assert_eq!(
            very_casual("Sounds good. I'll be there, I think."),
            "sounds good. i'll be there i think"
        );
        assert_eq!(
            very_casual("Sure, I'm in and I've seen it. I'd say yes, I."),
            "sure im in and ive seen it. i'd say yes i"
        );
        assert_eq!(
            very_casual("So I\u{2019}m told, and I\u{2019}d agree."),
            "so im told and i\u{2019}d agree"
        );
    }

    #[test]
    fn very_casual_leaves_words_that_only_start_with_i() {
        assert_eq!(
            very_casual("We saw Italy and IBM. It was fun."),
            "we saw Italy and IBM. it was fun"
        );
        // Neither a standalone "I" nor one of its contractions
        assert_eq!(very_casual("Say 'I' twice, or I's."), "say 'I' twice or Is");
    }

    #[test]
    fn very_casual_drops_apostrophes_inside_words() {
        assert_eq!(
            very_casual("Sounds good. I'll be there, I don't think I'm late."),
            "sounds good. i'll be there i dont think im late"
        );
        assert_eq!(
            very_casual("don't won't can't it's let's I'm I've you're that's andy's"),
            "dont wont cant its lets im ive youre thats andys"
        );
        assert_eq!(very_casual("Andy's car."), "andys car");
        assert_eq!(
            very_casual("Don\u{2019}t worry, it\u{2019}s fine."),
            "dont worry its fine"
        );
    }

    #[test]
    fn very_casual_keeps_apostrophes_that_change_the_word() {
        assert_eq!(
            very_casual("we'll we're I'll he'll she'll I'd she'd we'd"),
            "we'll we're i'll he'll she'll i'd she'd we'd"
        );
        assert_eq!(very_casual("We'll see."), "we'll see");
        assert_eq!(very_casual("Sure. SHE'LL go."), "sure. SHE'LL go");
        assert_eq!(
            very_casual("Maybe we\u{2019}re late, she\u{2019}d know."),
            "maybe we\u{2019}re late she\u{2019}d know"
        );
    }

    #[test]
    fn very_casual_keeps_quote_marks_at_word_edges() {
        assert_eq!(
            very_casual("She said 'don't' and 'we'll'."),
            "she said 'dont' and 'we'll'"
        );
        assert_eq!(very_casual("The '90s."), "the '90s");
    }

    #[test]
    fn casual_keeps_apostrophes() {
        assert_eq!(casual("I don't know, it's fine."), "I don't know it's fine");
    }

    #[test]
    fn casual_keeps_the_pronoun_i() {
        assert_eq!(
            casual("Sounds good. I'll be there, I think."),
            "Sounds good. I'll be there I think"
        );
    }

    #[test]
    fn very_casual_keeps_acronyms() {
        assert_eq!(
            very_casual("API is down. NASA called."),
            "API is down. NASA called"
        );
        assert_eq!(very_casual("OK. A cat."), "OK. a cat");
    }

    #[test]
    fn casual_styles_keep_question_exclamation_and_ellipsis_endings() {
        assert_eq!(casual("Are you coming?"), "Are you coming?");
        assert_eq!(casual("Nice!"), "Nice!");
        assert_eq!(casual("Well..."), "Well...");
        assert_eq!(very_casual("Are you coming?"), "are you coming?");
        assert_eq!(very_casual("Nice!"), "nice!");
        assert_eq!(very_casual("Well..."), "well...");
    }

    #[test]
    fn casual_drops_only_one_trailing_period_and_keeps_trailing_whitespace() {
        assert_eq!(very_casual("Done. "), "done ");
        assert_eq!(very_casual("See you at 5."), "see you at 5");
    }

    #[test]
    fn very_casual_lowercases_after_newlines() {
        assert_eq!(
            very_casual("Hi team,\nThanks for this.\n\nBest"),
            "hi team\nthanks for this.\n\nbest"
        );
    }

    #[test]
    fn casual_styles_handle_empty_and_punctuation_only_text() {
        for style in [WritingStyle::Casual, WritingStyle::VeryCasual] {
            assert_eq!(apply_style("", style), "");
            assert_eq!(apply_style("   ", style), "   ");
            assert_eq!(apply_style(".", style), "");
            assert_eq!(apply_style(",", style), "");
            assert_eq!(apply_style("?!", style), "?!");
            assert_eq!(apply_style("...", style), "...");
        }
    }

    #[test]
    fn very_casual_skips_leading_whitespace_and_non_letters() {
        assert_eq!(very_casual("  Hello."), "  hello");
        assert_eq!(very_casual("\"Hello,\" she said."), "\"Hello\" she said");
        assert_eq!(very_casual("5 Things."), "5 Things");
    }

    fn rule(bundle_id: &str, style: WritingStyle) -> AppStyleRule {
        AppStyleRule {
            bundle_id: bundle_id.to_string(),
            app_name: bundle_id.to_string(),
            style,
        }
    }

    #[test]
    fn apps_without_a_rule_are_formal() {
        let rules = [
            rule("com.tinyspeck.slackmacgap", WritingStyle::VeryCasual),
            rule("com.apple.mail", WritingStyle::Formal),
        ];
        assert_eq!(
            style_for_app(&rules, Some("com.tinyspeck.slackmacgap")),
            WritingStyle::VeryCasual
        );
        assert_eq!(
            style_for_app(&rules, Some("com.apple.mail")),
            WritingStyle::Formal
        );
        assert_eq!(
            style_for_app(&rules, Some("com.apple.Notes")),
            WritingStyle::Formal
        );
        assert_eq!(style_for_app(&rules, None), WritingStyle::Formal);
    }
}
