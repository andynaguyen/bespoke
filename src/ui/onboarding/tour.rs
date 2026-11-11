use std::time::{Duration, Instant};

use crate::settings::TourProgress;

const EMPTY_RECORDING_RETRY_AFTER: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Microphone,
    Accessibility,
    Preparing,
    Hold,
    Scratch,
    Screen,
    Restart,
    Record,
    Review,
}

impl Stage {
    /// Which progress dot the stage lights: set up, Dictate, Screen Recording or Annotate.
    pub fn dot(self) -> usize {
        match self {
            Stage::Microphone | Stage::Accessibility => 0,
            Stage::Preparing | Stage::Hold | Stage::Scratch => 1,
            Stage::Screen | Stage::Restart => 2,
            Stage::Record | Stage::Review => 3,
        }
    }

    pub fn is_practicing_dictation(self) -> bool {
        matches!(self, Stage::Hold | Stage::Scratch)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Attempt {
    Ready,
    Talking,
    Working(Instant),
    /// The text left after the lesson worked. A cut can leave none.
    Passed(String),
    /// What Bespoke heard, if anything, when the lesson didn't work.
    Retry(Option<String>),
}

pub fn first_stage(
    tour: TourProgress,
    microphone: bool,
    accessibility: bool,
    model_ready: bool,
    screen_usable: bool,
) -> Stage {
    if !microphone {
        Stage::Microphone
    } else if !accessibility {
        Stage::Accessibility
    } else {
        stage_after_permissions(tour, model_ready, screen_usable)
    }
}

pub fn stage_after_permissions(
    tour: TourProgress,
    model_ready: bool,
    screen_usable: bool,
) -> Stage {
    match tour {
        TourProgress::Review => Stage::Review,
        TourProgress::Annotate if screen_usable => Stage::Record,
        TourProgress::Annotate => Stage::Screen,
        _ if model_ready => practice_stage(tour),
        _ => Stage::Preparing,
    }
}

pub fn practice_stage(tour: TourProgress) -> Stage {
    if tour == TourProgress::Scratch {
        Stage::Scratch
    } else {
        Stage::Hold
    }
}

pub fn checkpoint(stage: Stage) -> Option<TourProgress> {
    match stage {
        Stage::Hold => Some(TourProgress::Dictate),
        Stage::Scratch => Some(TourProgress::Scratch),
        Stage::Screen | Stage::Restart | Stage::Record => Some(TourProgress::Annotate),
        Stage::Review => Some(TourProgress::Review),
        Stage::Microphone | Stage::Accessibility | Stage::Preparing => None,
    }
}

pub fn needs_model(tour: TourProgress) -> bool {
    matches!(tour, TourProgress::Dictate | TourProgress::Scratch)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Starter {
    pub busy: bool,
    pub on_disk: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModelAction {
    Practice,
    Wait,
    Select,
    OpenPicker,
}

pub fn model_action(ready: bool, starter: Option<Starter>) -> ModelAction {
    match starter {
        _ if ready => ModelAction::Practice,
        Some(Starter { busy: true, .. }) => ModelAction::Wait,
        Some(Starter { on_disk: true, .. }) => ModelAction::Select,
        _ => ModelAction::OpenPicker,
    }
}

pub fn next_attempt(attempt: &Attempt, dictating: bool, now: Instant) -> Option<Attempt> {
    match (attempt, dictating) {
        (Attempt::Ready | Attempt::Retry(_) | Attempt::Passed(_), true) => Some(Attempt::Talking),
        (Attempt::Talking, false) => Some(Attempt::Working(now)),
        (Attempt::Working(started), false)
            if now.duration_since(*started) > EMPTY_RECORDING_RETRY_AFTER =>
        {
            Some(Attempt::Retry(None))
        }
        _ => None,
    }
}

/// Judges a finished take: the scratch lesson by whether a spoken cut ran, the hold lesson by
/// whether Bespoke heard anything.
pub fn judge_take(stage: Stage, text: &str, cut_by_voice: bool) -> Attempt {
    let text = text.trim();
    let heard = (!text.is_empty()).then(|| text.to_string());
    let passed = if stage == Stage::Scratch {
        cut_by_voice
    } else {
        heard.is_some()
    };
    if passed {
        Attempt::Passed(text.to_string())
    } else {
        Attempt::Retry(heard)
    }
}

pub fn scratch_available(enabled: bool, keyword: &str) -> bool {
    enabled && !keyword.trim().is_empty()
}

/// The step after macOS reports Screen Recording, or `None` while it is off.
pub fn stage_after_screen(granted: bool, needs_relaunch: bool) -> Option<Stage> {
    match (granted, needs_relaunch) {
        (false, _) => None,
        (true, true) => Some(Stage::Restart),
        (true, false) => Some(Stage::Record),
    }
}

pub fn keyframe_lines(summary: &str, limit: usize) -> Vec<String> {
    summary
        .lines()
        .filter(|line| line.starts_with("- #"))
        .take(limit)
        .map(str::to_string)
        .collect()
}

const AV_AUTHORIZATION_RESTRICTED: isize = 1;
const AV_AUTHORIZATION_DENIED: isize = 2;

pub fn microphone_blocked(av_authorization_status: isize) -> bool {
    matches!(
        av_authorization_status,
        AV_AUTHORIZATION_RESTRICTED | AV_AUTHORIZATION_DENIED
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreviewDetail {
    Plain,
    Talking,
    Passed,
    Retry,
    Unheard,
    Cleared,
    Denied,
    Waiting,
    Live,
    Empty,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CoachPreview {
    pub stage: Stage,
    pub detail: PreviewDetail,
}

impl CoachPreview {
    pub fn parse(id: &str) -> Self {
        let (stage, detail) = id.split_once('-').unwrap_or((id, ""));
        Self {
            stage: match stage {
                "accessibility" => Stage::Accessibility,
                "preparing" => Stage::Preparing,
                "hold" => Stage::Hold,
                "scratch" => Stage::Scratch,
                "screen" => Stage::Screen,
                "restart" => Stage::Restart,
                "record" => Stage::Record,
                "review" => Stage::Review,
                _ => Stage::Microphone,
            },
            detail: match detail {
                "talking" => PreviewDetail::Talking,
                "passed" => PreviewDetail::Passed,
                "retry" => PreviewDetail::Retry,
                "unheard" => PreviewDetail::Unheard,
                "cleared" => PreviewDetail::Cleared,
                "denied" => PreviewDetail::Denied,
                "waiting" => PreviewDetail::Waiting,
                "live" => PreviewDetail::Live,
                "empty" => PreviewDetail::Empty,
                _ => PreviewDetail::Plain,
            },
        }
    }
}

/// One keycap per key of a stored binding like `option+space`.
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

/// `CGEventFlags` bits for the modifier a keycap shows, or `None` for any other key.
pub fn modifier_mask(label: &str) -> Option<u64> {
    match label {
        "⇧" => Some(0x0002_0000),
        "⌃" => Some(0x0004_0000),
        "⌥" => Some(0x0008_0000),
        "⌘" => Some(0x0010_0000),
        _ => None,
    }
}

/// Picks the model a new user practices with: the first recommended one that speaks their
/// language, or the first recommended one.
pub fn pick_starter<T>(recommended: &[T], speaks: impl Fn(&T) -> bool) -> Option<&T> {
    recommended
        .iter()
        .find(|model| speaks(model))
        .or_else(|| recommended.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_on_the_first_missing_permission() {
        let tour = TourProgress::Dictate;
        assert_eq!(
            first_stage(tour, false, false, true, true),
            Stage::Microphone
        );
        assert_eq!(
            first_stage(tour, true, false, true, true),
            Stage::Accessibility
        );
    }

    #[test]
    fn dictation_waits_for_a_model() {
        let tour = TourProgress::Dictate;
        assert_eq!(
            first_stage(tour, true, true, false, false),
            Stage::Preparing
        );
        assert_eq!(first_stage(tour, true, true, true, false), Stage::Hold);
    }

    #[test]
    fn a_relaunch_resumes_at_annotate_once_the_screen_is_usable() {
        let tour = TourProgress::Annotate;
        assert_eq!(first_stage(tour, true, true, true, false), Stage::Screen);
        assert_eq!(first_stage(tour, true, true, true, true), Stage::Record);
    }

    #[test]
    fn a_lost_permission_comes_before_a_resumed_step() {
        assert_eq!(
            first_stage(TourProgress::Annotate, true, false, true, true),
            Stage::Accessibility
        );
    }

    #[test]
    fn a_saved_lesson_or_review_resumes_there() {
        assert_eq!(
            first_stage(TourProgress::Scratch, true, true, true, false),
            Stage::Scratch
        );
        assert_eq!(
            first_stage(TourProgress::Review, true, true, false, false),
            Stage::Review
        );
    }

    #[test]
    fn every_lesson_and_annotate_step_saves_its_place() {
        assert_eq!(checkpoint(Stage::Hold), Some(TourProgress::Dictate));
        assert_eq!(checkpoint(Stage::Scratch), Some(TourProgress::Scratch));
        assert_eq!(checkpoint(Stage::Restart), Some(TourProgress::Annotate));
        assert_eq!(checkpoint(Stage::Review), Some(TourProgress::Review));
        assert_eq!(checkpoint(Stage::Preparing), None);
    }

    #[test]
    fn preparing_waits_selects_or_opens_the_picker() {
        let starter = |busy, on_disk| Some(Starter { busy, on_disk });
        assert_eq!(model_action(true, None), ModelAction::Practice);
        assert_eq!(model_action(false, starter(true, false)), ModelAction::Wait);
        assert_eq!(model_action(false, starter(true, true)), ModelAction::Wait);
        assert_eq!(
            model_action(false, starter(false, true)),
            ModelAction::Select
        );
        assert_eq!(
            model_action(false, starter(false, false)),
            ModelAction::OpenPicker
        );
        assert_eq!(model_action(false, None), ModelAction::OpenPicker);
    }

    #[test]
    fn an_attempt_follows_the_recording() {
        let now = Instant::now();
        assert_eq!(
            next_attempt(&Attempt::Ready, true, now),
            Some(Attempt::Talking)
        );
        assert_eq!(next_attempt(&Attempt::Ready, false, now), None);
        assert_eq!(
            next_attempt(&Attempt::Talking, false, now),
            Some(Attempt::Working(now))
        );
        assert_eq!(next_attempt(&Attempt::Talking, true, now), None);
    }

    #[test]
    fn a_take_that_never_comes_times_out() {
        let started = Instant::now();
        let working = Attempt::Working(started);
        assert_eq!(next_attempt(&working, false, started), None);
        assert_eq!(
            next_attempt(
                &working,
                false,
                started + EMPTY_RECORDING_RETRY_AFTER + Duration::from_secs(1)
            ),
            Some(Attempt::Retry(None))
        );
    }

    #[test]
    fn scratch_passes_only_when_a_cut_ran() {
        assert_eq!(
            judge_take(Stage::Scratch, "Lunch is at noon.", true),
            Attempt::Passed("Lunch is at noon.".into())
        );
        assert_eq!(
            judge_take(Stage::Scratch, "Lunch. The budget.", false),
            Attempt::Retry(Some("Lunch. The budget.".into()))
        );
    }

    #[test]
    fn a_cut_that_removes_everything_still_passes() {
        assert_eq!(
            judge_take(Stage::Scratch, "  ", true),
            Attempt::Passed(String::new())
        );
    }

    #[test]
    fn hold_passes_on_any_words() {
        assert_eq!(
            judge_take(Stage::Hold, "hello", false),
            Attempt::Passed("hello".into())
        );
        assert_eq!(judge_take(Stage::Hold, " ", false), Attempt::Retry(None));
    }

    #[test]
    fn scratch_needs_the_feature_and_a_keyword() {
        assert!(scratch_available(true, "scratch that"));
        assert!(!scratch_available(false, "scratch that"));
        assert!(!scratch_available(true, "  "));
    }

    #[test]
    fn a_screen_grant_may_need_a_restart() {
        assert_eq!(stage_after_screen(false, false), None);
        assert_eq!(stage_after_screen(true, true), Some(Stage::Restart));
        assert_eq!(stage_after_screen(true, false), Some(Stage::Record));
    }

    #[test]
    fn summary_lines_keep_only_keyframes() {
        let summary =
            "[Bespoke annotation 0:07]\nCropped to: x\n- #1 (0:01) a\n- #2 (0:02) b\n- #3 (0:03) c";
        assert_eq!(
            keyframe_lines(summary, 2),
            ["- #1 (0:01) a", "- #2 (0:02) b"]
        );
    }

    #[test]
    fn a_preview_id_names_a_stage_and_detail() {
        assert_eq!(
            CoachPreview::parse("hold-talking"),
            CoachPreview {
                stage: Stage::Hold,
                detail: PreviewDetail::Talking
            }
        );
        assert_eq!(
            CoachPreview::parse("review"),
            CoachPreview {
                stage: Stage::Review,
                detail: PreviewDetail::Plain
            }
        );
    }

    #[test]
    fn labels_each_key() {
        assert_eq!(key_labels("option+space"), ["⌥", "Space"]);
        assert_eq!(key_labels("option+shift+s"), ["⌥", "⇧", "S"]);
        assert_eq!(key_labels("command_left+f5"), ["⌘", "F5"]);
    }

    #[test]
    fn starter_prefers_the_users_language() {
        let models = ["en-only", "many"];
        assert_eq!(pick_starter(&models, |m| *m == "many"), Some(&"many"));
        assert_eq!(pick_starter(&models, |_| false), Some(&"en-only"));
        assert_eq!(pick_starter::<&str>(&[], |_| true), None);
    }
}
