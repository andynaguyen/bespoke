/// The short name every UI shows for a mouse button in a binding: `m1` (left) to `m8`.
pub fn mouse_button_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "mouseleft" | "mouse1" => "m1",
        "mouseright" | "mouse2" => "m2",
        "mousemiddle" | "mouse3" => "m3",
        "mousex1" | "mouse4" => "m4",
        "mousex2" | "mouse5" => "m5",
        "mouse6" => "m6",
        "mouse7" => "m7",
        "mouse8" => "m8",
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
    if let Some(label) = mouse_button_label(key) {
        return label.into();
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

/// One keycap per key of a stored binding like `option+space`, with modifiers as glyphs.
pub fn key_labels(binding: &str) -> Vec<String> {
    binding
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(key_label)
        .collect()
}

fn key_label(part: &str) -> String {
    let name = part
        .strip_suffix("_left")
        .or_else(|| part.strip_suffix("_right"))
        .unwrap_or(part);
    if let Some(label) = mouse_button_label(name) {
        return label.into();
    }
    match name {
        "option" | "alt" => "⌥".into(),
        "shift" => "⇧".into(),
        "command" | "cmd" | "super" | "meta" => "⌘".into(),
        "control" | "ctrl" => "⌃".into(),
        "fn" => "fn".into(),
        "space" => "Space".into(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        }
    }
}

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
        assert_eq!(format_key_combination("ctrl+mouseleft"), "Ctrl + m1");
        assert_eq!(
            format_key_combination("option_left+mouse8"),
            "Left Option + m8"
        );
        assert_eq!(
            format_key_combination("option_left+shift+space"),
            "Left Option + Shift + Space"
        );
        assert_eq!(format_key_combination("f5"), "F5");
        assert_eq!(format_key_combination("numpad 5"), "Numpad 5");
        assert_eq!(format_key_combination("esc"), "Esc");
    }

    #[test]
    fn labels_each_key() {
        assert_eq!(key_labels("option+space"), ["⌥", "Space"]);
        assert_eq!(key_labels("option+shift+s"), ["⌥", "⇧", "S"]);
        assert_eq!(key_labels("command_left+f5"), ["⌘", "F5"]);
        assert_eq!(key_labels("option_left+mousex2"), ["⌥", "m5"]);
    }
}
