//! Shortcut display formatting, from `src/lib/utils/keyboard.ts`.
//! Stored bindings are lowercase tokens joined by `+` (`option+shift+space`).

use crate::i18n::t_or;

fn mouse_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "mouseleft" => "Left Mouse",
        "mouseright" => "Right Mouse",
        "mousemiddle" => "Middle Mouse",
        "mousex1" => "Mouse 4",
        "mousex2" => "Mouse 5",
        "mouse6" => "Mouse 6",
        "mouse7" => "Mouse 7",
        "mouse8" => "Mouse 8",
        _ => return None,
    })
}

fn compound_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "capslock" => "Caps Lock",
        "numlock" => "Num Lock",
        "pageup" => "Page Up",
        "pagedown" => "Page Down",
        "printscreen" => "Print Screen",
        "scrolllock" => "Scroll Lock",
        _ => return None,
    })
}

fn capitalize_key(key: &str) -> String {
    if let Some(label) = mouse_label(key) {
        return t_or(
            &format!("settings.general.shortcut.mouseButtons.{key}"),
            label,
        )
        .to_string();
    }
    if key == "fn" {
        return "fn".into();
    }
    let is_function_key =
        key.len() > 1 && key.starts_with('f') && key[1..].chars().all(|c| c.is_ascii_digit());
    if is_function_key || key.chars().count() == 1 {
        return key.to_uppercase();
    }
    if let Some(label) = compound_label(key) {
        return label.into();
    }
    // Capitalize the first letter of each word.
    let mut out = String::with_capacity(key.len());
    let mut at_word_start = true;
    for ch in key.chars() {
        if at_word_start && ch.is_alphanumeric() {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
        at_word_start = !(ch.is_alphanumeric() || ch == '_');
    }
    out
}

fn format_key_part(part: &str) -> String {
    let part = part.trim();
    if let Some(name) = part.strip_suffix("_left") {
        format!("Left {}", capitalize_key(name))
    } else if let Some(name) = part.strip_suffix("_right") {
        format!("Right {}", capitalize_key(name))
    } else {
        capitalize_key(part)
    }
}

/// `option_left+shift+space` → `Left Option + Shift + Space`.
pub fn format_key_combination(combination: &str) -> String {
    if combination.is_empty() {
        return String::new();
    }
    combination
        .split('+')
        .map(format_key_part)
        .collect::<Vec<_>>()
        .join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_keyboard_ts() {
        assert_eq!(format_key_combination("scrolllock"), "Scroll Lock");
        assert_eq!(
            format_key_combination("ctrl+mouseleft"),
            "Ctrl + Left Mouse"
        );
        assert_eq!(
            format_key_combination("option_left+shift+space"),
            "Left Option + Shift + Space"
        );
        assert_eq!(format_key_combination("f5"), "F5");
        assert_eq!(format_key_combination("numpad 5"), "Numpad 5");
        assert_eq!(format_key_combination("esc"), "Esc");
    }
}
