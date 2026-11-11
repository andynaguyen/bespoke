use std::time::Instant;

use crate::annotate::coordinator::SelectionMode;
use crate::ui::capsule::{REVEAL_MIN, RevealWindow};

pub const TAP_MS: u64 = 250;
pub const DOUBLE_PRESS_MS: u64 = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Free,
    Picker,
    Recording { elapsed_ms: u64 },
    Processing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    RecordScreen,
    Open(SelectionMode),
    Show(SelectionMode),
    Commit(SelectionMode),
    Cancel,
    RecordSelected,
    Finish,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Idle,
    Pending {
        since_ms: u64,
    },
    Switching(SelectionMode),
}

#[derive(Debug, Default)]
pub struct Switcher {
    state: State,
}

impl Switcher {
    pub fn is_held(&self) -> bool {
        self.state != State::Idle
    }

    pub fn is_switching(&self) -> bool {
        matches!(self.state, State::Switching(_))
    }

    pub fn reset(&mut self) {
        self.state = State::Idle;
    }

    pub fn follow(&mut self, mode: SelectionMode) {
        if self.is_switching() {
            self.state = State::Switching(mode);
        }
    }

    pub fn press(&mut self, now_ms: u64, activity: Activity) -> Action {
        match (self.state, activity) {
            (State::Pending { .. }, _) => self.switch_to(SelectionMode::Window, true),
            (State::Switching(mode), _) => self.switch_to(next(mode), false),
            (State::Idle, Activity::Recording { elapsed_ms }) if elapsed_ms < DOUBLE_PRESS_MS => {
                Action::None
            }
            (State::Idle, Activity::Recording { .. }) => Action::Finish,
            (State::Idle, Activity::Processing) => Action::None,
            (State::Idle, Activity::Picker) => Action::RecordSelected,
            (State::Idle, Activity::Free) => {
                self.state = State::Pending { since_ms: now_ms };
                Action::None
            }
        }
    }

    pub fn tick(&mut self, now_ms: u64, held: bool) -> Action {
        match self.state {
            State::Idle => Action::None,
            State::Pending { .. } if !held => {
                self.state = State::Idle;
                Action::RecordScreen
            }
            State::Pending { since_ms } if now_ms.saturating_sub(since_ms) >= TAP_MS => {
                self.switch_to(SelectionMode::Screen, true)
            }
            State::Pending { .. } => Action::None,
            State::Switching(mode) if !held => {
                self.state = State::Idle;
                Action::Commit(mode)
            }
            State::Switching(_) => Action::None,
        }
    }

    pub fn drag(&mut self) -> Action {
        match self.state {
            State::Switching(mode) if mode != SelectionMode::Area => {
                self.switch_to(SelectionMode::Area, false)
            }
            _ => Action::None,
        }
    }

    pub fn escape(&mut self) -> Action {
        if self.state == State::Idle {
            return Action::None;
        }
        self.state = State::Idle;
        Action::Cancel
    }

    fn switch_to(&mut self, mode: SelectionMode, open: bool) -> Action {
        self.state = State::Switching(mode);
        if open {
            Action::Open(mode)
        } else {
            Action::Show(mode)
        }
    }
}

#[derive(Debug, Default)]
pub struct Reveal {
    open: Option<RevealOpen>,
}

#[derive(Clone, Copy, Debug)]
struct RevealOpen {
    window: RevealWindow,
    baseline: usize,
}

impl Reveal {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn show(&mut self, now: Instant, activity: usize) {
        self.open = Some(RevealOpen {
            window: RevealWindow::open(now),
            baseline: activity,
        });
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    pub fn tick(&mut self, now: Instant, hovered: bool, activity: usize) -> bool {
        let Some(open) = self.open.as_mut() else {
            return false;
        };
        if hovered {
            open.window.hover(now);
            return false;
        }
        if open.window.age(now) < REVEAL_MIN {
            return false;
        }
        let working = activity > open.baseline && !open.window.lingering(now);
        if !open.window.is_open(now) || working {
            self.open = None;
            return true;
        }
        false
    }
}

fn next(mode: SelectionMode) -> SelectionMode {
    match mode {
        SelectionMode::Screen => SelectionMode::Window,
        SelectionMode::Window | SelectionMode::Area => SelectionMode::Screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::capsule::{REVEAL, REVEAL_LINGER};
    use SelectionMode::{Area, Screen, Window};
    use std::time::Duration;

    const FREE: Activity = Activity::Free;

    #[test]
    fn a_tap_records_the_screen_without_showing_the_switcher() {
        let mut s = Switcher::default();
        assert_eq!(s.press(0, FREE), Action::None);
        assert_eq!(s.tick(8, true), Action::None);
        assert_eq!(s.tick(120, true), Action::None);
        assert_eq!(s.tick(128, false), Action::RecordScreen);
        assert!(!s.is_held());
        assert_eq!(s.tick(200, false), Action::None);
    }

    #[test]
    fn holding_opens_the_switcher_on_screen_and_s_cycles_the_modes() {
        let mut s = Switcher::default();
        s.press(0, FREE);
        assert_eq!(s.tick(240, true), Action::None);
        assert_eq!(s.tick(250, true), Action::Open(Screen));
        assert_eq!(s.press(600, FREE), Action::Show(Window));
        assert_eq!(s.press(800, FREE), Action::Show(Screen));
        assert_eq!(s.press(1_000, FREE), Action::Show(Window));
        assert_eq!(s.tick(1_400, true), Action::None);
        assert_eq!(s.tick(1_408, false), Action::Commit(Window));
        assert!(!s.is_held());
    }

    #[test]
    fn a_quick_second_s_opens_the_switcher_on_window() {
        let mut s = Switcher::default();
        s.press(0, FREE);
        assert_eq!(s.press(150, FREE), Action::Open(Window));
        assert_eq!(s.tick(300, true), Action::None);
        assert_eq!(s.tick(320, false), Action::Commit(Window));
    }

    #[test]
    fn holding_and_releasing_with_no_input_records_the_screen() {
        let mut s = Switcher::default();
        s.press(0, FREE);
        assert_eq!(s.tick(250, true), Action::Open(Screen));
        assert_eq!(s.tick(900, false), Action::Commit(Screen));
    }

    #[test]
    fn dragging_while_holding_selects_an_area() {
        let mut s = Switcher::default();
        s.press(0, FREE);
        s.tick(250, true);
        assert_eq!(s.drag(), Action::Show(Area));
        assert_eq!(s.drag(), Action::None);
        assert_eq!(s.tick(1_500, false), Action::Commit(Area));
        assert_eq!(s.drag(), Action::None);
        s.press(2_000, FREE);
        s.tick(2_250, true);
        s.drag();
        assert_eq!(s.press(2_600, FREE), Action::Show(Screen));
        s.follow(Window);
        assert_eq!(s.press(2_800, FREE), Action::Show(Screen));
    }

    #[test]
    fn esc_while_holding_cancels_and_the_release_does_nothing() {
        let mut s = Switcher::default();
        s.press(0, FREE);
        s.tick(250, true);
        s.press(400, FREE);
        assert_eq!(s.escape(), Action::Cancel);
        assert_eq!(s.tick(600, true), Action::None);
        assert_eq!(s.tick(700, false), Action::None);
        s.press(1_000, FREE);
        assert_eq!(s.escape(), Action::Cancel);
        assert_eq!(s.tick(1_100, false), Action::None);
        assert_eq!(s.escape(), Action::None);
    }

    #[test]
    fn a_tap_while_recording_finishes_but_not_a_double_press() {
        let mut s = Switcher::default();
        let at = |elapsed_ms| Activity::Recording { elapsed_ms };
        assert_eq!(s.press(0, at(0)), Action::None);
        assert_eq!(s.press(0, at(399)), Action::None);
        assert_eq!(s.press(0, at(400)), Action::Finish);
        assert_eq!(s.press(0, at(60_000)), Action::Finish);
        assert!(!s.is_held());
        assert_eq!(s.tick(100, false), Action::None);
        assert_eq!(s.press(0, Activity::Processing), Action::None);
        assert!(!s.is_held());
    }

    fn at(ms: u64) -> Instant {
        thread_local! {
            static START: Instant = Instant::now();
        }
        START.with(|start| *start + Duration::from_millis(ms))
    }

    const REVEAL_MS: u64 = REVEAL.as_millis() as u64;
    const REVEAL_MIN_MS: u64 = REVEAL_MIN.as_millis() as u64;
    const REVEAL_LINGER_MS: u64 = REVEAL_LINGER.as_millis() as u64;

    #[test]
    fn reveal_folds_away_after_its_time_when_ignored() {
        let mut r = Reveal::default();
        assert!(!r.tick(at(0), false, 0));
        r.show(at(1_000), 0);
        assert!(r.is_open());
        assert!(!r.tick(at(1_000 + REVEAL_MS - 1), false, 0));
        assert!(r.tick(at(1_000 + REVEAL_MS), false, 0));
        assert!(!r.is_open());
        assert!(!r.tick(at(1_000 + REVEAL_MS + 8), false, 0));
    }

    #[test]
    fn reveal_folds_away_once_work_starts_but_is_seen_first() {
        let mut r = Reveal::default();
        r.show(at(0), 3);
        assert!(!r.tick(at(100), false, 4));
        assert!(!r.tick(at(REVEAL_MIN_MS - 1), false, 4));
        assert!(r.tick(at(REVEAL_MIN_MS), false, 4));
        r.show(at(10_000), 7);
        assert!(!r.tick(at(10_000 + REVEAL_MIN_MS + 500), false, 7));
        assert!(r.tick(at(10_000 + REVEAL_MIN_MS + 600), false, 8));
    }

    #[test]
    fn reveal_stays_while_hovered_and_lingers_after() {
        let mut r = Reveal::default();
        r.show(at(0), 0);
        assert!(!r.tick(at(3_000), true, 0));
        assert!(!r.tick(at(6_000), true, 5));
        assert!(!r.tick(at(6_000 + REVEAL_LINGER_MS - 1), false, 5));
        assert!(r.tick(at(6_000 + REVEAL_LINGER_MS), false, 5));
        r.show(at(20_000), 0);
        r.close();
        assert!(!r.is_open());
    }

    #[test]
    fn in_the_menu_picker_the_shortcut_records_its_selection() {
        let mut s = Switcher::default();
        assert_eq!(s.press(0, Activity::Picker), Action::RecordSelected);
        assert!(!s.is_held());
    }
}
