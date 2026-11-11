use bitflags::bitflags;
use std::fmt;

bitflags! {
    /// Modifier keys for hotkey combinations
    ///
    /// Individual flags track which side (left/right) was pressed.
    /// Compound aliases (`CMD`, `SHIFT`, etc.) match either side.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Modifiers: u32 {
        const CMD_LEFT    = 1 << 0;
        const SHIFT_LEFT  = 1 << 1;
        const CTRL_LEFT   = 1 << 2;
        const OPT_LEFT    = 1 << 3;
        const FN          = 1 << 4;
        const CMD_RIGHT   = 1 << 5;
        const SHIFT_RIGHT = 1 << 6;
        const CTRL_RIGHT  = 1 << 7;
        const OPT_RIGHT   = 1 << 8;

        const CMD   = Self::CMD_LEFT.bits()   | Self::CMD_RIGHT.bits();
        const SHIFT = Self::SHIFT_LEFT.bits() | Self::SHIFT_RIGHT.bits();
        const CTRL  = Self::CTRL_LEFT.bits()  | Self::CTRL_RIGHT.bits();
        const OPT   = Self::OPT_LEFT.bits()   | Self::OPT_RIGHT.bits();
    }
}

const GROUPS: [(Modifiers, Modifiers); 4] = [
    (Modifiers::CMD_LEFT, Modifiers::CMD_RIGHT),
    (Modifiers::SHIFT_LEFT, Modifiers::SHIFT_RIGHT),
    (Modifiers::CTRL_LEFT, Modifiers::CTRL_RIGHT),
    (Modifiers::OPT_LEFT, Modifiers::OPT_RIGHT),
];

impl fmt::Display for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();

        if self.contains(Modifiers::CTRL) {
            parts.push("Ctrl");
        } else if self.contains(Modifiers::CTRL_LEFT) {
            parts.push("CtrlLeft");
        } else if self.contains(Modifiers::CTRL_RIGHT) {
            parts.push("CtrlRight");
        }

        if self.contains(Modifiers::OPT) {
            parts.push("Opt");
        } else if self.contains(Modifiers::OPT_LEFT) {
            parts.push("OptLeft");
        } else if self.contains(Modifiers::OPT_RIGHT) {
            parts.push("OptRight");
        }

        if self.contains(Modifiers::SHIFT) {
            parts.push("Shift");
        } else if self.contains(Modifiers::SHIFT_LEFT) {
            parts.push("ShiftLeft");
        } else if self.contains(Modifiers::SHIFT_RIGHT) {
            parts.push("ShiftRight");
        }

        if self.contains(Modifiers::CMD) {
            parts.push("Cmd");
        } else if self.contains(Modifiers::CMD_LEFT) {
            parts.push("CmdLeft");
        } else if self.contains(Modifiers::CMD_RIGHT) {
            parts.push("CmdRight");
        }

        if self.contains(Modifiers::FN) {
            parts.push("Fn");
        }

        write!(f, "{}", parts.join("+"))
    }
}

impl Modifiers {
    pub(crate) fn parse_single(s: &str) -> Option<Modifiers> {
        match s.to_lowercase().as_str() {
            "cmd" | "command" | "meta" | "super" | "win" | "windows" => Some(Modifiers::CMD),
            "shift" => Some(Modifiers::SHIFT),
            "ctrl" | "control" => Some(Modifiers::CTRL),
            "opt" | "option" | "alt" => Some(Modifiers::OPT),
            "fn" | "function" => Some(Modifiers::FN),

            "cmdleft" | "cmd_left" | "lcmd" | "commandleft" | "command_left" | "lcommand"
            | "superleft" | "super_left" | "winleft" | "win_left" | "windowsleft"
            | "windows_left" | "metaleft" | "meta_left" => Some(Modifiers::CMD_LEFT),
            "shiftleft" | "shift_left" | "lshift" => Some(Modifiers::SHIFT_LEFT),
            "ctrlleft" | "ctrl_left" | "lctrl" | "controlleft" | "control_left" | "lcontrol" => {
                Some(Modifiers::CTRL_LEFT)
            }
            "optleft" | "opt_left" | "lopt" | "optionleft" | "option_left" | "loption"
            | "altleft" | "alt_left" | "lalt" => Some(Modifiers::OPT_LEFT),

            "cmdright" | "cmd_right" | "rcmd" | "commandright" | "command_right" | "rcommand"
            | "superright" | "super_right" | "winright" | "win_right" | "windowsright"
            | "windows_right" | "metaright" | "meta_right" => Some(Modifiers::CMD_RIGHT),
            "shiftright" | "shift_right" | "rshift" => Some(Modifiers::SHIFT_RIGHT),
            "ctrlright" | "ctrl_right" | "rctrl" | "controlright" | "control_right"
            | "rcontrol" => Some(Modifiers::CTRL_RIGHT),
            "optright" | "opt_right" | "ropt" | "optionright" | "option_right" | "roption"
            | "altright" | "alt_right" | "ralt" | "altgr" => Some(Modifiers::OPT_RIGHT),

            _ => None,
        }
    }

    /// Check whether `self` (as a hotkey pattern) matches `event` (the actual modifier state).
    ///
    /// For each modifier group (Cmd, Shift, Ctrl, Opt):
    /// - Hotkey has both bits (compound): event must have at least one bit from the group
    /// - Hotkey has a specific side: event must have that specific side (extra same-group bits OK)
    /// - Hotkey has neither: event must not have either bit from the group
    ///
    /// FN is matched exactly.
    pub fn matches(self, event: Modifiers) -> bool {
        for &(left, right) in &GROUPS {
            let hotkey_has_left = self.contains(left);
            let hotkey_has_right = self.contains(right);
            let event_has_left = event.contains(left);
            let event_has_right = event.contains(right);
            let event_has_any = event_has_left || event_has_right;

            if hotkey_has_left && hotkey_has_right {
                if !event_has_any {
                    return false;
                }
            } else if hotkey_has_left {
                if !event_has_left {
                    return false;
                }
            } else if hotkey_has_right {
                if !event_has_right {
                    return false;
                }
            } else {
                if event_has_any {
                    return false;
                }
            }
        }

        self.contains(Modifiers::FN) == event.contains(Modifiers::FN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_modifiers() {
        assert_eq!(Modifiers::parse_single("Cmd").unwrap(), Modifiers::CMD);
        assert_eq!(Modifiers::parse_single("command").unwrap(), Modifiers::CMD);
        assert_eq!(Modifiers::parse_single("meta").unwrap(), Modifiers::CMD);
        assert_eq!(Modifiers::parse_single("super").unwrap(), Modifiers::CMD);
        assert_eq!(Modifiers::parse_single("win").unwrap(), Modifiers::CMD);

        assert_eq!(Modifiers::parse_single("Shift").unwrap(), Modifiers::SHIFT);
        assert_eq!(Modifiers::parse_single("SHIFT").unwrap(), Modifiers::SHIFT);

        assert_eq!(Modifiers::parse_single("Ctrl").unwrap(), Modifiers::CTRL);
        assert_eq!(Modifiers::parse_single("control").unwrap(), Modifiers::CTRL);

        assert_eq!(Modifiers::parse_single("Opt").unwrap(), Modifiers::OPT);
        assert_eq!(Modifiers::parse_single("option").unwrap(), Modifiers::OPT);
        assert_eq!(Modifiers::parse_single("alt").unwrap(), Modifiers::OPT);

        assert_eq!(Modifiers::parse_single("Fn").unwrap(), Modifiers::FN);
        assert_eq!(Modifiers::parse_single("function").unwrap(), Modifiers::FN);
    }

    #[test]
    fn parse_side_specific_modifiers() {
        assert_eq!(
            Modifiers::parse_single("CmdLeft").unwrap(),
            Modifiers::CMD_LEFT
        );
        assert_eq!(
            Modifiers::parse_single("LCmd").unwrap(),
            Modifiers::CMD_LEFT
        );
        assert_eq!(
            Modifiers::parse_single("CmdRight").unwrap(),
            Modifiers::CMD_RIGHT
        );
        assert_eq!(
            Modifiers::parse_single("RCmd").unwrap(),
            Modifiers::CMD_RIGHT
        );

        assert_eq!(
            Modifiers::parse_single("ShiftLeft").unwrap(),
            Modifiers::SHIFT_LEFT
        );
        assert_eq!(
            Modifiers::parse_single("ShiftRight").unwrap(),
            Modifiers::SHIFT_RIGHT
        );

        assert_eq!(
            Modifiers::parse_single("CtrlLeft").unwrap(),
            Modifiers::CTRL_LEFT
        );
        assert_eq!(
            Modifiers::parse_single("CtrlRight").unwrap(),
            Modifiers::CTRL_RIGHT
        );

        assert_eq!(
            Modifiers::parse_single("OptLeft").unwrap(),
            Modifiers::OPT_LEFT
        );
        assert_eq!(
            Modifiers::parse_single("AltRight").unwrap(),
            Modifiers::OPT_RIGHT
        );
        assert_eq!(
            Modifiers::parse_single("AltGr").unwrap(),
            Modifiers::OPT_RIGHT
        );
    }

    #[test]
    fn parse_unknown_modifier_fails() {
        assert_eq!(Modifiers::parse_single("Unknown"), None);
        assert_eq!(Modifiers::parse_single("Cmd+Shift"), None);
    }

    #[test]
    fn modifiers_display() {
        assert_eq!(format!("{}", Modifiers::CMD), "Cmd");
        assert_eq!(format!("{}", Modifiers::SHIFT), "Shift");
        assert_eq!(
            format!("{}", Modifiers::CMD | Modifiers::SHIFT),
            "Shift+Cmd"
        );
    }

    #[test]
    fn modifiers_display_side_specific() {
        assert_eq!(format!("{}", Modifiers::CMD_LEFT), "CmdLeft");
        assert_eq!(format!("{}", Modifiers::CMD_RIGHT), "CmdRight");
        assert_eq!(format!("{}", Modifiers::SHIFT_LEFT), "ShiftLeft");
        assert_eq!(format!("{}", Modifiers::CTRL_RIGHT), "CtrlRight");
        assert_eq!(format!("{}", Modifiers::OPT_LEFT), "OptLeft");
    }

    #[test]
    fn matches_compound_hotkey() {
        let hotkey = Modifiers::CMD;
        assert!(hotkey.matches(Modifiers::CMD_LEFT));
        assert!(hotkey.matches(Modifiers::CMD_RIGHT));
        assert!(hotkey.matches(Modifiers::CMD_LEFT | Modifiers::CMD_RIGHT));
        assert!(!hotkey.matches(Modifiers::empty()));
        assert!(!hotkey.matches(Modifiers::SHIFT_LEFT));
    }

    #[test]
    fn matches_side_specific_hotkey() {
        let hotkey = Modifiers::CMD_LEFT;
        assert!(hotkey.matches(Modifiers::CMD_LEFT));
        assert!(!hotkey.matches(Modifiers::CMD_RIGHT));
        assert!(hotkey.matches(Modifiers::CMD_LEFT | Modifiers::CMD_RIGHT));
        assert!(!hotkey.matches(Modifiers::empty()));
    }

    #[test]
    fn matches_rejects_extra_groups() {
        let hotkey = Modifiers::CMD;
        assert!(!hotkey.matches(Modifiers::CMD_LEFT | Modifiers::SHIFT_LEFT));

        let hotkey = Modifiers::CMD_LEFT | Modifiers::SHIFT_LEFT;
        assert!(hotkey.matches(Modifiers::CMD_LEFT | Modifiers::SHIFT_LEFT));
    }

    #[test]
    fn matches_fn_exact() {
        let hotkey = Modifiers::CMD | Modifiers::FN;
        assert!(hotkey.matches(Modifiers::CMD_LEFT | Modifiers::FN));
        assert!(!hotkey.matches(Modifiers::CMD_LEFT));

        let hotkey = Modifiers::CMD;
        assert!(!hotkey.matches(Modifiers::CMD_LEFT | Modifiers::FN));
    }

    #[test]
    fn matches_empty() {
        let hotkey = Modifiers::empty();
        assert!(hotkey.matches(Modifiers::empty()));
        assert!(!hotkey.matches(Modifiers::CMD_LEFT));
    }
}
