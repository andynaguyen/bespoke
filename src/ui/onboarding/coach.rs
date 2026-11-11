mod view;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, Context, Entity, SharedString, Subscription, Task, Window};

use super::tour::{self, Attempt, CoachPreview, ModelAction, PreviewDetail, Stage, Starter};
use crate::annotate::permissions::{self, Permission};
use crate::app_handle::{AppEvent, AppHandle};
use crate::i18n::t;
use crate::managers::audio::AudioRecordingManager;
use crate::settings::TourProgress;
use crate::try_it::Take;
use crate::ui::WindowCallback;
use crate::ui::event_bus::event_bus;
use crate::ui::stores::{ModelStore, SettingsStore};

const TICK: Duration = Duration::from_millis(50);
const PERMISSION_EVERY: u32 = 10;
const SCREEN_EVERY: u32 = 30;
const REVIEW_LINES: usize = 3;

#[derive(Clone)]
struct Review {
    sheet: Option<PathBuf>,
    lines: Vec<SharedString>,
}

pub struct KeycapCoach {
    preview: bool,
    stage: Stage,
    tour: TourProgress,
    models: Entity<ModelStore>,
    on_done: WindowCallback,
    on_pick_model: WindowCallback,
    starter: Option<String>,
    selecting: bool,
    microphone_denied: bool,
    accessibility_asked: bool,
    screen_asked: bool,
    reading_grants: bool,
    attempt: Attempt,
    flags: u64,
    dictating: bool,
    capturing: bool,
    review: Option<Review>,
    ticks: u32,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn initialize_input() {
    let app = AppHandle::global().clone();
    std::thread::spawn(move || {
        if let Err(e) = crate::commands::initialize_enigo(app.clone()) {
            log::warn!("Failed to initialize after permission grant: {e}");
        }
        if let Err(e) = crate::commands::initialize_shortcuts(app) {
            log::warn!("Failed to initialize after permission grant: {e}");
        }
    });
}

fn binding(id: &str, cx: &App) -> Vec<String> {
    SettingsStore::get(cx)
        .bindings
        .get(id)
        .map(|binding| tour::key_labels(&binding.current_binding))
        .unwrap_or_default()
}

fn scratch_keyword(cx: &App) -> String {
    SettingsStore::get(cx).scratch_keyword.clone()
}

fn scratch_on(cx: &App) -> bool {
    let settings = SettingsStore::get(cx);
    tour::scratch_available(settings.scratch_enabled, &settings.scratch_keyword)
}

fn save_tour(tour: TourProgress) {
    crate::settings::update(AppHandle::global(), |settings| {
        settings.onboarding_tour = tour;
    });
}

impl KeycapCoach {
    pub fn new(
        tour: TourProgress,
        on_done: impl Fn(&mut Window, &mut App) + 'static,
        on_pick_model: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let models = ModelStore::global(cx);
        let microphone = crate::ui::platform::microphone_granted();
        let accessibility = crate::ui::platform::accessibility_granted();
        if accessibility {
            initialize_input();
        }
        let stage = tour::first_stage(
            tour,
            microphone,
            accessibility,
            Self::model_ready(&models, cx),
            crate::annotate::capture::screen_capture_usable(false),
        );
        let subscriptions = vec![
            cx.subscribe(&event_bus(cx), |this, _, event: &AppEvent, cx| {
                this.handle_event(event, cx)
            }),
            cx.observe_in(&models, window, |this, _, window, cx| {
                this.advance_preparing(window, cx);
                cx.notify();
            }),
        ];
        log::info!("Guided tour opens at {stage:?}, tour {tour:?}");
        let mut this = Self::blank(stage, tour, false, on_done, on_pick_model, cx);
        this._subscriptions = subscriptions;
        this.start_download(cx);
        if stage == Stage::Review {
            this.show_review(cx);
        }
        this._ticker = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this
                    .update_in(cx, |this, window, cx| this.tick(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        this
    }

    fn blank(
        stage: Stage,
        tour: TourProgress,
        preview: bool,
        on_done: impl Fn(&mut Window, &mut App) + 'static,
        on_pick_model: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            preview,
            stage,
            tour,
            models: ModelStore::global(cx),
            on_done: Rc::new(on_done),
            on_pick_model: Rc::new(on_pick_model),
            starter: None,
            selecting: false,
            microphone_denied: false,
            accessibility_asked: false,
            screen_asked: false,
            reading_grants: false,
            attempt: Attempt::Ready,
            flags: 0,
            dictating: false,
            capturing: false,
            review: None,
            ticks: 0,
            _ticker: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn preview(preview: CoachPreview, cx: &mut Context<Self>) -> Self {
        let CoachPreview { stage, detail } = preview;
        let mut this = Self::blank(stage, TourProgress::Dictate, true, |_, _| {}, |_, _| {}, cx);
        let preview_text = if stage == Stage::Scratch {
            t("onboarding.coach.preview.scratched").to_string()
        } else {
            t("onboarding.coach.hold.line").to_string()
        };
        match detail {
            PreviewDetail::Talking => {
                this.attempt = Attempt::Talking;
                this.dictating = true;
            }
            PreviewDetail::Passed => this.attempt = Attempt::Passed(preview_text),
            PreviewDetail::Retry => this.attempt = Attempt::Retry(Some(preview_text)),
            PreviewDetail::Unheard => this.attempt = Attempt::Retry(None),
            PreviewDetail::Cleared => this.attempt = Attempt::Passed(String::new()),
            PreviewDetail::Denied => this.microphone_denied = true,
            PreviewDetail::Waiting => {
                this.accessibility_asked = true;
                this.screen_asked = true;
            }
            PreviewDetail::Live => this.capturing = true,
            PreviewDetail::Plain | PreviewDetail::Empty => {}
        }
        if stage == Stage::Preparing {
            this.starter = this
                .models
                .read(cx)
                .starter_model()
                .map(|model| model.id.clone());
        }
        if stage == Stage::Review && detail != PreviewDetail::Empty {
            this.review = Some(load_review().unwrap_or(Review {
                sheet: None,
                lines: vec![t("onboarding.coach.preview.reviewLine")],
            }));
        }
        this
    }

    fn model_ready(models: &Entity<ModelStore>, cx: &App) -> bool {
        let store = models.read(cx);
        !store.current_model.is_empty()
            && store
                .model(&store.current_model)
                .is_some_and(|model| model.is_downloaded)
    }

    fn start_download(&mut self, cx: &mut Context<Self>) {
        if !tour::needs_model(self.tour) || Self::model_ready(&self.models, cx) {
            return;
        }
        let Some(model) = self.models.read(cx).starter_model().cloned() else {
            return;
        };
        self.starter = Some(model.id.clone());
        let busy = self.models.read(cx).downloading.contains(&model.id);
        if model.is_downloaded || busy {
            return;
        }
        log::info!("Guided tour: downloading starter model {}", model.id);
        self.models.update(cx, |store, cx| {
            store.download_model(model.id, |_, _| {}, cx);
        });
    }

    fn advance_preparing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview || self.stage != Stage::Preparing || self.selecting {
            return;
        }
        let store = self.models.read(cx);
        let starter = self.starter.as_deref().map(|id| Starter {
            busy: store.downloading.contains(id) || store.verifying.contains(id),
            on_disk: store.model(id).is_some_and(|model| model.is_downloaded),
        });
        match tour::model_action(Self::model_ready(&self.models, cx), starter) {
            ModelAction::Practice => self.enter(tour::practice_stage(self.tour), cx),
            ModelAction::Wait => {}
            ModelAction::OpenPicker => self.replace_starter_with_picker(window, cx),
            ModelAction::Select => self.select_starter(window, cx),
        }
    }

    fn select_starter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.starter.clone() else {
            return;
        };
        self.selecting = true;
        let weak = cx.entity().downgrade();
        let handle = window.window_handle();
        let pick = self.on_pick_model.clone();
        self.models.update(cx, |store, cx| {
            store.select_model(
                id,
                move |ok, cx| {
                    if ok {
                        let _ = weak.update(cx, |this, cx| {
                            this.selecting = false;
                            this.enter(tour::practice_stage(this.tour), cx);
                        });
                    } else {
                        let _ = handle.update(cx, |_, window, cx| pick(window, cx));
                    }
                },
                cx,
            )
        });
    }

    fn enter(&mut self, stage: Stage, cx: &mut Context<Self>) {
        self.stage = stage;
        self.attempt = Attempt::Ready;
        if !stage.is_practicing_dictation() {
            crate::try_it::set_dictionary_active(false);
        }
        if let Some(tour) = tour::checkpoint(stage)
            && tour != self.tour
        {
            self.tour = tour;
            save_tour(tour);
        }
        cx.notify();
    }

    fn after_permissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        SettingsStore::global(cx).update(cx, |store, cx| {
            store.refresh_audio_devices(cx);
            store.refresh_output_devices(cx);
        });
        let next = tour::stage_after_permissions(
            self.tour,
            Self::model_ready(&self.models, cx),
            crate::annotate::capture::screen_capture_usable(false),
        );
        self.enter(next, cx);
        self.advance_preparing(window, cx);
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ticks = self.ticks.wrapping_add(1);
        let flags = crate::annotate::capture::modifier_flags();
        let dictating = AppHandle::global()
            .state::<Arc<AudioRecordingManager>>()
            .is_recording();
        let capturing = crate::annotate::coordinator::capturing();
        if (flags, dictating, capturing) != (self.flags, self.dictating, self.capturing) {
            (self.flags, self.dictating, self.capturing) = (flags, dictating, capturing);
            cx.notify();
        }
        match self.stage {
            Stage::Microphone if self.ticks.is_multiple_of(PERMISSION_EVERY) => {
                self.poll_microphone(window, cx)
            }
            Stage::Accessibility if self.ticks.is_multiple_of(PERMISSION_EVERY) => {
                if crate::ui::platform::accessibility_granted() {
                    initialize_input();
                    self.after_permissions(window, cx);
                }
            }
            Stage::Preparing if self.ticks.is_multiple_of(PERMISSION_EVERY) => {
                self.advance_preparing(window, cx)
            }
            Stage::Hold | Stage::Scratch => {
                crate::try_it::set_dictionary_active(window.is_window_active());
                self.follow_attempt(cx);
            }
            Stage::Screen if self.ticks.is_multiple_of(SCREEN_EVERY) => {
                self.poll_screen(window, cx)
            }
            _ => {}
        }
    }

    fn poll_microphone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::ui::platform::microphone_granted() {
            if crate::ui::platform::accessibility_granted() {
                initialize_input();
                self.after_permissions(window, cx);
            } else {
                self.enter(Stage::Accessibility, cx);
            }
            return;
        }
        let denied = tour::microphone_blocked(crate::ui::platform::microphone_authorization());
        if denied != self.microphone_denied {
            self.microphone_denied = denied;
            cx.notify();
        }
    }

    fn follow_attempt(&mut self, cx: &mut Context<Self>) {
        let next = tour::next_attempt(&self.attempt, self.dictating, Instant::now());
        if let Some(next) = next {
            self.attempt = next;
            cx.notify();
        }
    }

    fn poll_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading_grants {
            return;
        }
        self.reading_grants = true;
        cx.spawn_in(window, async move |this, cx| {
            let grants = crate::runtime::spawn_blocking(permissions::read_grants)
                .await
                .ok()
                .flatten();
            let _ = this.update(cx, |this, cx| {
                this.reading_grants = false;
                let Some(grants) = grants else {
                    return;
                };
                let next = tour::stage_after_screen(
                    grants.granted(Permission::ScreenRecording),
                    grants.screen_needs_relaunch,
                );
                if let Some(next) = next.filter(|_| this.stage == Stage::Screen) {
                    this.enter(next, cx);
                }
            });
        })
        .detach();
    }

    fn handle_event(&mut self, event: &AppEvent, cx: &mut Context<Self>) {
        match event.name.as_str() {
            crate::try_it::TAKE_EVENT if self.stage.is_practicing_dictation() => {
                if let Some(take) = event.parse::<Take>() {
                    self.attempt = tour::judge_take(self.stage, &take.text, take.cut_by_voice);
                    cx.notify();
                }
            }
            "transcription-error" | "recording-error" if self.stage.is_practicing_dictation() => {
                self.attempt = Attempt::Retry(None);
                cx.notify();
            }
            crate::annotate::CAPTURE_FINISHED if self.stage == Stage::Record => {
                self.show_review(cx);
            }
            _ => {}
        }
    }

    fn show_review(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let review = crate::runtime::spawn_blocking(load_review)
                .await
                .ok()
                .flatten();
            let _ = this.update(cx, |this, cx| {
                this.review = review;
                this.enter(Stage::Review, cx);
            });
        })
        .detach();
    }

    fn ask_microphone(&mut self, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        if self.microphone_denied {
            permissions::open_permission_settings(Permission::Microphone);
        } else {
            crate::ui::platform::request_microphone();
        }
        cx.notify();
    }

    fn ask_accessibility(&mut self, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        permissions::open_permission_settings(Permission::Accessibility);
        self.accessibility_asked = true;
        cx.notify();
    }

    fn ask_screen(&mut self, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        // Asking adds Bespoke to the list, and macOS prompts only once per app.
        let first = !self.screen_asked;
        self.screen_asked = true;
        std::thread::spawn(move || {
            if first {
                permissions::request_screen_capture();
            }
            permissions::open_permission_settings(Permission::ScreenRecording);
        });
        cx.notify();
    }

    fn start_annotate(&mut self, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        let usable = crate::annotate::capture::screen_capture_usable(false);
        self.enter(if usable { Stage::Record } else { Stage::Screen }, cx);
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview {
            return;
        }
        save_tour(TourProgress::Done);
        (self.on_done.clone())(window, cx);
    }

    fn relaunch(&mut self) {
        if !self.preview {
            AppHandle::global().restart();
        }
    }

    fn pick_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.preview {
            self.replace_starter_with_picker(window, cx);
        }
    }

    fn replace_starter_with_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.starter.clone() {
            self.models
                .update(cx, |store, cx| store.abandon_download(id, cx));
        }
        (self.on_pick_model.clone())(window, cx);
    }
}

fn newest_capture_in_memory_or_on_disk() -> Option<crate::annotate::handoff::Finished> {
    crate::annotate::newest_capture().or_else(crate::annotate::handoff::newest_capture)
}

fn load_review() -> Option<Review> {
    let finished = newest_capture_in_memory_or_on_disk()?;
    let capture = crate::annotate::handoff::Capture::load(&finished.dir).ok()?;
    Some(Review {
        sheet: capture.contact_sheet(false).ok(),
        lines: tour::keyframe_lines(&capture.summary(), REVIEW_LINES)
            .into_iter()
            .map(SharedString::from)
            .collect(),
    })
}

impl Drop for KeycapCoach {
    fn drop(&mut self) {
        if !self.preview {
            crate::try_it::set_dictionary_active(false);
        }
    }
}
