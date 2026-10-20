//! Post-processing provider and prompt settings (`post-processing/` and
//! `PostProcessingSettingsApi/`), shown in the Experimental tab.

use std::time::Duration;

use gpui::{
    anchored, canvas, deferred, div, point, prelude::*, px, rems, Animation, AnimationExt, App,
    Bounds, Context, Entity, Focusable, MouseButton, Pixels, SharedString, Subscription, Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::app_handle::AppHandle;
use crate::i18n::t;
use crate::settings::{AppSettings, APPLE_INTELLIGENCE_PROVIDER_ID};
use crate::shortcut;
use crate::ui::components::{
    error_alert, icon, Button, ButtonVariant, Dropdown, DropdownOption, KeyboardFocus,
    RowDirection, SettingContainer, SettingLayout, TextArea, TextField, Tooltip, Typography,
};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, shadow, Alpha};

fn selected_provider_id(s: &AppSettings) -> String {
    s.post_process_providers
        .iter()
        .find(|p| p.id == s.post_process_provider_id)
        .or_else(|| s.post_process_providers.first())
        .map(|p| p.id.clone())
        .unwrap_or_else(|| "openai".into())
}

/// Provider, base URL, API key and model rows.
pub struct PostProcessingApi {
    store: Entity<SettingsStore>,
    base_url: Entity<InputState>,
    api_key: Entity<InputState>,
    model_query: Entity<InputState>,
    model_menu_open: bool,
    model_field: Bounds<Pixels>,
    apple_unavailable: bool,
    shown_provider: String,
    _subscriptions: Vec<Subscription>,
}

impl PostProcessingApi {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        let base_url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.postProcessing.api.baseUrl.placeholder"))
        });
        let api_key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.postProcessing.api.apiKey.placeholder"))
                .masked(true)
        });
        let model_query = cx.new(|cx| InputState::new(window, cx));

        let subscriptions = vec![
            cx.observe_in(&store, window, |this, _, window, cx| {
                this.sync_fields(window, cx)
            }),
            cx.subscribe_in(&base_url, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Blur) {
                    this.commit_base_url(cx);
                }
            }),
            cx.subscribe_in(&api_key, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Blur) {
                    this.commit_api_key(cx);
                }
            }),
            cx.subscribe_in(
                &model_query,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Focus => {
                        this.model_menu_open = true;
                        cx.notify();
                    }
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => {
                        // react-select commits its focused option: the first
                        // match, with "Use ..." listed last.
                        let typed = this.model_query.read(cx).value().trim().to_string();
                        let lowered = typed.to_lowercase();
                        let pick = this
                            .model_options(cx)
                            .into_iter()
                            .find(|o| o.to_lowercase().contains(&lowered))
                            .or_else(|| (!typed.is_empty()).then_some(typed));
                        if let Some(model) = pick {
                            this.set_model(model, window, cx);
                        }
                    }
                    _ => {}
                },
            ),
        ];

        let mut this = Self {
            store,
            base_url,
            api_key,
            model_query,
            model_menu_open: false,
            model_field: Bounds::default(),
            apple_unavailable: false,
            shown_provider: String::new(),
            _subscriptions: subscriptions,
        };
        this.sync_fields(window, cx);
        this
    }

    /// Reloads the inputs when the provider changes or the stored values move
    /// under an unfocused field.
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.store.read(cx).settings.clone();
        let provider_id = selected_provider_id(&settings);
        let provider_changed = provider_id != self.shown_provider;
        self.shown_provider = provider_id.clone();

        let base_url = settings
            .post_process_providers
            .iter()
            .find(|p| p.id == provider_id)
            .map(|p| p.base_url.clone())
            .unwrap_or_default();
        let api_key = settings
            .post_process_api_keys
            .get(&provider_id)
            .cloned()
            .unwrap_or_default();

        for (input, value) in [(&self.base_url, base_url), (&self.api_key, api_key)] {
            let focused = input.read(cx).focus_handle(cx).is_focused(window);
            if (provider_changed || !focused) && input.read(cx).value().as_ref() != value.as_str() {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
        cx.notify();
    }

    fn settings<'a>(&self, cx: &'a App) -> &'a AppSettings {
        &self.store.read(cx).settings
    }

    fn select_provider(&mut self, provider_id: String, cx: &mut Context<Self>) {
        self.apple_unavailable = false;
        let settings = self.settings(cx).clone();
        if provider_id == selected_provider_id(&settings) {
            return;
        }
        if provider_id == APPLE_INTELLIGENCE_PROVIDER_ID
            && !crate::commands::check_apple_intelligence_available()
        {
            // The provider is still selected so the dropdown shows it; the
            // backend handles an unavailable Apple Intelligence.
            self.apple_unavailable = true;
        }

        let id = provider_id.clone();
        self.store.update(cx, |store, cx| {
            store.post_process_models.remove(&id);
            let stored = id.clone();
            store.update(
                "post_process_provider_id",
                move |s| s.post_process_provider_id = stored,
                move |app| shortcut::set_post_process_provider(app, id),
                cx,
            );
        });

        // Fetch the provider's models when it is configured, so a stale model
        // from another provider is easy to replace.
        if provider_id != APPLE_INTELLIGENCE_PROVIDER_ID {
            let provider = settings
                .post_process_providers
                .iter()
                .find(|p| p.id == provider_id);
            let has_key = settings
                .post_process_api_keys
                .get(&provider_id)
                .is_some_and(|k| !k.trim().is_empty());
            let configured = match provider {
                Some(p) if p.id == "custom" => !p.base_url.trim().is_empty(),
                Some(_) => has_key,
                None => false,
            };
            if configured {
                self.store.update(cx, |store, cx| {
                    store.fetch_post_process_models(provider_id, cx)
                });
            }
        }
        cx.notify();
    }

    fn commit_base_url(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings(cx).clone();
        let provider_id = selected_provider_id(&settings);
        if provider_id != "custom" {
            return;
        }
        let current = settings
            .post_process_providers
            .iter()
            .find(|p| p.id == provider_id)
            .map(|p| p.base_url.clone())
            .unwrap_or_default();
        let value = self.base_url.read(cx).value().trim().to_string();
        if value.is_empty() || value == current {
            return;
        }
        // A new endpoint invalidates the stored model and the cached list.
        let key = format!("post_process_base_url:{provider_id}");
        let app = AppHandle::global().clone();
        self.store.update(cx, |store, cx| {
            store.set_updating(key.clone(), true, cx);
            store.post_process_models.remove(&provider_id);
            cx.spawn(async move |store, cx| {
                let id = provider_id.clone();
                let result = crate::runtime::spawn_blocking(move || {
                    shortcut::change_post_process_base_url_setting(app.clone(), id.clone(), value)?;
                    shortcut::change_post_process_model_setting(app, id, String::new())
                })
                .await;
                if let Ok(Err(e)) = result {
                    log::error!("Failed to update base URL: {e}");
                }
                let _ = store.update(cx, |store, cx| {
                    store.set_updating(key, false, cx);
                    store.post_process_models.remove(&provider_id);
                    store.refresh(cx);
                });
            })
            .detach();
        });
    }

    fn commit_api_key(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings(cx).clone();
        let provider_id = selected_provider_id(&settings);
        let current = settings
            .post_process_api_keys
            .get(&provider_id)
            .cloned()
            .unwrap_or_default();
        let value = self.api_key.read(cx).value().trim().to_string();
        if value == current {
            return;
        }
        self.store.update(cx, |store, cx| {
            store.post_process_models.remove(&provider_id);
            let key: &'static str = "post_process_api_key";
            let id = provider_id.clone();
            let stored = value.clone();
            store.update(
                key,
                move |s| {
                    s.post_process_api_keys.insert(id, stored);
                },
                move |app| shortcut::change_post_process_api_key_setting(app, provider_id, value),
                cx,
            );
        });
    }

    fn set_model(&mut self, model: String, window: &mut Window, cx: &mut Context<Self>) {
        let provider_id = selected_provider_id(self.settings(cx));
        self.model_menu_open = false;
        self.model_query
            .update(cx, |q, cx| q.set_value("", window, cx));
        window.blur();
        self.store.update(cx, |store, cx| {
            let id = provider_id.clone();
            let stored = model.clone();
            store.update(
                "post_process_model",
                move |s| {
                    s.post_process_models.insert(id, stored);
                },
                move |app| shortcut::change_post_process_model_setting(app, provider_id, model),
                cx,
            );
        });
        cx.notify();
    }

    fn model_options(&self, cx: &App) -> Vec<String> {
        let settings = self.settings(cx);
        let provider_id = selected_provider_id(settings);
        let current = settings
            .post_process_models
            .get(&provider_id)
            .cloned()
            .unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        self.store
            .read(cx)
            .post_process_models
            .get(&provider_id)
            .into_iter()
            .flatten()
            .chain(std::iter::once(&current))
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty() && seen.insert(m.clone()))
            .collect()
    }

    fn render_model_select(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = palette(cx);
        let settings = self.settings(cx).clone();
        let provider_id = selected_provider_id(&settings);
        let current = settings
            .post_process_models
            .get(&provider_id)
            .cloned()
            .unwrap_or_default();
        let options = self.model_options(cx);
        let store = self.store.read(cx);
        let fetching = store.is_updating(&format!("post_process_models_fetch:{provider_id}"));
        let updating = store.is_updating("post_process_model");
        let focused = self
            .model_query
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let query = self.model_query.read(cx).value().trim().to_string();

        let placeholder: SharedString = if options.is_empty() {
            t("settings.postProcessing.api.model.placeholderNoOptions")
        } else {
            t("settings.postProcessing.api.model.placeholderWithOptions")
        };
        // The stored model draws as the value (react-select's singleValue)
        // until the user types; the placeholder shows only without one.
        let shown_placeholder: SharedString = if current.is_empty() {
            placeholder
        } else {
            SharedString::default()
        };
        self.model_query.update(cx, |q, cx| {
            q.set_placeholder(shown_placeholder, window, cx);
        });

        let filtered: Vec<String> = options
            .iter()
            .filter(|o| o.to_lowercase().contains(&query.to_lowercase()))
            .cloned()
            .collect();
        let create = (!query.is_empty() && !options.contains(&query)).then(|| query.clone());

        let open = self.model_menu_open && focused;
        let field = self.model_field;
        let entity = cx.entity().downgrade();
        let menu = open.then(|| {
            let mut menu = div()
                .id("model-options")
                .occlude()
                .w(field.size.width)
                .max_h(px(300.))
                .overflow_y_scroll()
                .p_1()
                .bg(p.background)
                .border_1()
                .border_color(p.mid_gray.a(0.2))
                .rounded(px(12.))
                .shadow(vec![shadow(0., 10., 30., 0., 0.12)])
                .text_color(p.text)
                .on_mouse_down_out(cx.listener(
                    move |this, event: &gpui::MouseDownEvent, _, cx| {
                        if !field.contains(&event.position) {
                            this.model_menu_open = false;
                            cx.notify();
                        }
                    },
                ));
            for option in filtered {
                let selected = option == current;
                let value = option.clone();
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("model-option-{option}")))
                        .px_3()
                        .py_2()
                        .rounded(px(8.))
                        .sm_text()
                        .cursor_pointer()
                        .when(selected, |el| el.bg(p.mid_gray.a(0.18)))
                        .hover(|s| s.bg(p.mid_gray.a(0.1)))
                        .keyboard_focus(cx)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_model(value.clone(), window, cx)
                        }))
                        .child(option),
                );
            }
            if let Some(typed) = create {
                let label = format!("Use \"{typed}\"");
                menu = menu.child(
                    div()
                        .id("model-option-create")
                        .px_3()
                        .py_2()
                        .rounded(px(8.))
                        .sm_text()
                        .cursor_pointer()
                        .hover(|s| s.bg(p.mid_gray.a(0.1)))
                        .keyboard_focus(cx)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_model(typed.clone(), window, cx)
                        }))
                        .child(label),
                );
            }
            deferred(
                anchored()
                    .position(point(field.left(), field.bottom() + px(4.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(menu),
            )
            .with_priority(1)
        });

        // react-select's indicators: loading dots, clear, a separator and
        // the chevron, each in an 8px-padded 20px box.
        let has_value = !current.is_empty();
        let indicator = |id: &'static str, glyph: &'static str, color: gpui::Hsla| {
            div()
                .id(id)
                .flex()
                .row_dir()
                .flex_none()
                .p(px(8.))
                .cursor_pointer()
                .text_color(color)
                .hover(|s| s.text_color(p.text))
                .child(icon(glyph).size(px(20.)))
        };
        let loading = fetching.then(|| {
            div()
                .flex()
                .row_dir()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .p(px(8.))
                .children((0..3).map(|i| {
                    div()
                        .size(px(4.))
                        .rounded_full()
                        .bg(p.mid_gray)
                        .with_animation(
                            SharedString::from(format!("model-loading-{i}")),
                            Animation::new(Duration::from_secs(1)).repeat(),
                            move |dot, delta| {
                                let phase = (delta - i as f32 * 0.16).rem_euclid(1.);
                                dot.opacity(if phase < 0.5 { 1. - phase } else { phase })
                            },
                        )
                }))
        });
        let clear = (has_value && !updating).then(|| {
            indicator("model-clear", "select-cross", p.text.a(0.4)).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.set_model(String::new(), window, cx);
                }),
            )
        });
        let query_focus = self.model_query.read(cx).focus_handle(cx);
        let chevron = indicator("model-chevron", "select-chevron", p.text.a(0.55)).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if this.model_menu_open && query_focus.is_focused(window) {
                    this.model_menu_open = false;
                    window.blur();
                } else {
                    this.model_menu_open = true;
                    window.focus(&query_focus);
                }
                cx.notify();
            }),
        );
        let indicators = div()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .flex()
            .row_dir()
            .items_center()
            .children(loading)
            .children(clear)
            .child(div().w(px(1.)).h(px(20.)).bg(gpui::rgb(0xcccccc)))
            .child(chevron);
        let indicator_width = px(36. + 1. + 36.)
            + if has_value { px(36.) } else { px(0.) }
            + if fetching { px(36.) } else { px(0.) };
        let value_overlay = (has_value && query.is_empty()).then(|| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(10.))
                .right(indicator_width)
                .flex()
                .row_dir()
                .items_center()
                .sm_text()
                .text_color(p.text)
                .child(div().truncate().child(current.clone()))
        });

        div()
            .flex()
            .row_dir()
            .items_center()
            .gap_2()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(380.))
                    .child(
                        TextField::new(&self.model_query)
                            .regular_weight()
                            .disabled(updating)
                            .min_h(px(36.))
                            .pl(px(10.))
                            .pr(indicator_width),
                    )
                    .children(value_overlay)
                    .child(indicators)
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                let _ = entity.update(cx, |this, _| this.model_field = bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .children(menu),
            )
            .child(
                div()
                    .id("refresh-models")
                    .flex()
                    .row_dir()
                    .size(rems(2.5))
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .when(fetching, |el| el.opacity(0.5).cursor_not_allowed())
                    .when(!fetching, |el| {
                        el.cursor_pointer()
                            .text_color(p.text.a(0.6))
                            .hover(|s| s.bg(p.mid_gray.a(0.15)).text_color(p.text))
                            .keyboard_focus(cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = selected_provider_id(this.settings(cx));
                                if id != APPLE_INTELLIGENCE_PROVIDER_ID {
                                    this.store
                                        .update(cx, |s, cx| s.fetch_post_process_models(id, cx));
                                }
                            }))
                    })
                    .tooltip(|window, cx| {
                        crate::ui::components::Tooltip::text(
                            t("settings.postProcessing.api.model.refreshModels"),
                            window,
                            cx,
                        )
                    })
                    .child(if fetching {
                        crate::ui::components::Spinner::loader(rems(1.), p.text.a(0.6))
                            .into_any_element()
                    } else {
                        icon("refresh-ccw").size(rems(1.)).into_any_element()
                    }),
            )
    }
}

impl Render for PostProcessingApi {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.settings(cx).clone();
        let provider_id = selected_provider_id(&settings);
        let is_apple = provider_id == APPLE_INTELLIGENCE_PROVIDER_ID;
        let is_custom = provider_id == "custom";
        let store = self.store.read(cx);
        let base_url_updating = store.is_updating(&format!("post_process_base_url:{provider_id}"));
        let api_key_updating = store.is_updating("post_process_api_key");

        let providers: Vec<DropdownOption> = settings
            .post_process_providers
            .iter()
            .map(|p| DropdownOption::new(p.id.clone(), p.label.clone()))
            .collect();

        let mut rows: Vec<gpui::AnyElement> =
            vec![
                SettingContainer::new(t("settings.postProcessing.api.provider.title"))
                    .description(t("settings.postProcessing.api.provider.description"))
                    .child(
                        div().flex().items_center().gap_2().child(
                            Dropdown::new("post-process-provider", providers)
                                .selected(Some(provider_id.clone()))
                                .flex_1()
                                .on_select({
                                    let entity = cx.entity().downgrade();
                                    move |value, _, cx| {
                                        let value = value.to_string();
                                        let _ = entity
                                            .update(cx, |this, cx| this.select_provider(value, cx));
                                    }
                                }),
                        ),
                    )
                    .into_any_element(),
            ];

        if is_apple {
            if self.apple_unavailable {
                rows.push(
                    error_alert(
                        true,
                        t("settings.postProcessing.api.appleIntelligence.unavailable"),
                    )
                    .into_any_element(),
                );
            }
        } else {
            if is_custom {
                rows.push(
                    SettingContainer::new(t("settings.postProcessing.api.baseUrl.title"))
                        .description(t("settings.postProcessing.api.baseUrl.description"))
                        .layout(SettingLayout::Stacked)
                        .child(
                            div()
                                .id("base-url-field")
                                .flex()
                                .row_dir()
                                .flex_1()
                                .min_w(px(200.))
                                .child(
                                    TextField::new(&self.base_url)
                                        .compact()
                                        .disabled(base_url_updating)
                                        .flex_1(),
                                )
                                // React's `title`, hard-coded English there too.
                                .when(base_url_updating, |el| {
                                    el.tooltip(|window, cx| {
                                        Tooltip::text(
                                            "Base URL is managed by the selected provider.",
                                            window,
                                            cx,
                                        )
                                    })
                                }),
                        )
                        .into_any_element(),
                );
            }
            rows.push(
                SettingContainer::new(t("settings.postProcessing.api.apiKey.title"))
                    .description(t("settings.postProcessing.api.apiKey.description"))
                    .layout(SettingLayout::Stacked)
                    .child(
                        TextField::new(&self.api_key)
                            .compact()
                            .disabled(api_key_updating)
                            .flex_1()
                            .min_w(px(200.)),
                    )
                    .into_any_element(),
            );
            rows.push(
                SettingContainer::new(t("settings.postProcessing.api.model.title"))
                    .description(if is_custom {
                        t("settings.postProcessing.api.model.descriptionCustom")
                    } else {
                        t("settings.postProcessing.api.model.descriptionDefault")
                    })
                    .layout(SettingLayout::Stacked)
                    .child(self.render_model_select(window, cx))
                    .into_any_element(),
            );
        }

        let p = palette(cx);
        div()
            .flex()
            .flex_col()
            .children(rows.into_iter().enumerate().map(|(i, row)| {
                div()
                    .flex()
                    .flex_col()
                    .when(i > 0, |el| el.border_t_1().border_color(p.mid_gray.a(0.15)))
                    .child(row)
            }))
    }
}

/// The prompt picker and editor.
pub struct PostProcessingPrompts {
    store: Entity<SettingsStore>,
    creating: bool,
    name: Entity<InputState>,
    text: Entity<InputState>,
    shown_prompt: Option<(String, String, String)>,
    _subscriptions: Vec<Subscription>,
}

impl PostProcessingPrompts {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.postProcessing.prompts.promptLabelPlaceholder"))
        });
        let text = cx.new(|cx| {
            InputState::new(window, cx).multi_line(true).placeholder(t(
                "settings.postProcessing.prompts.promptInstructionsPlaceholder",
            ))
        });
        let subscriptions = vec![
            cx.observe_in(&store, window, |this, _, window, cx| {
                this.sync_drafts(window, cx)
            }),
            cx.subscribe(&name, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify()
                }
            }),
            cx.subscribe(&text, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify()
                }
            }),
        ];
        let mut this = Self {
            store,
            creating: false,
            name,
            text,
            shown_prompt: None,
            _subscriptions: subscriptions,
        };
        this.sync_drafts(window, cx);
        this
    }

    fn selected(&self, cx: &App) -> Option<crate::settings::LLMPrompt> {
        let settings = &self.store.read(cx).settings;
        let id = settings.post_process_selected_prompt_id.as_ref()?;
        settings
            .post_process_prompts
            .iter()
            .find(|p| &p.id == id)
            .cloned()
    }

    /// Loads the selected prompt into the drafts when it changes.
    fn sync_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        let selected = self
            .selected(cx)
            .map(|p| (p.id.clone(), p.name.clone(), p.prompt.clone()));
        if selected != self.shown_prompt {
            let (name, text) = selected
                .as_ref()
                .map(|(_, n, t)| (n.clone(), t.clone()))
                .unwrap_or_default();
            self.name.update(cx, |i, cx| i.set_value(name, window, cx));
            self.text.update(cx, |i, cx| i.set_value(text, window, cx));
            self.shown_prompt = selected;
        }
        cx.notify();
    }

    fn drafts(&self, cx: &App) -> (String, String) {
        (
            self.name.read(cx).value().trim().to_string(),
            self.text.read(cx).value().trim().to_string(),
        )
    }

    fn start_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.creating = true;
        self.name.update(cx, |i, cx| i.set_value("", window, cx));
        self.text.update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    fn cancel_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.creating = false;
        self.shown_prompt = None;
        self.sync_drafts(window, cx);
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let (name, text) = self.drafts(cx);
        if name.is_empty() || text.is_empty() {
            return;
        }
        let app = AppHandle::global().clone();
        match shortcut::add_post_process_prompt(app.clone(), name, text) {
            Ok(prompt) => {
                self.creating = false;
                self.shown_prompt = None;
                let id = prompt.id.clone();
                self.store.update(cx, |store, cx| {
                    store.refresh(cx);
                    let stored = id.clone();
                    store.update(
                        "post_process_selected_prompt_id",
                        move |s| s.post_process_selected_prompt_id = Some(stored),
                        move |app| shortcut::set_post_process_selected_prompt(app, id),
                        cx,
                    );
                });
            }
            Err(e) => log::error!("Failed to create prompt: {e}"),
        }
        cx.notify();
    }

    fn update_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.selected(cx) else {
            return;
        };
        let (name, text) = self.drafts(cx);
        if name.is_empty() || text.is_empty() {
            return;
        }
        if let Err(e) =
            shortcut::update_post_process_prompt(AppHandle::global().clone(), prompt.id, name, text)
        {
            log::error!("Failed to update prompt: {e}");
        }
        self.shown_prompt = None;
        self.store.update(cx, |s, cx| s.refresh(cx));
    }

    fn delete_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.selected(cx) else {
            return;
        };
        if let Err(e) = shortcut::delete_post_process_prompt(AppHandle::global().clone(), prompt.id)
        {
            log::error!("Failed to delete prompt: {e}");
        }
        self.creating = false;
        self.shown_prompt = None;
        self.store.update(cx, |s, cx| s.refresh(cx));
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.creating = false;
        self.shown_prompt = None;
        self.store.update(cx, |store, cx| {
            let stored = id.clone();
            store.update(
                "post_process_selected_prompt_id",
                move |s| s.post_process_selected_prompt_id = Some(stored),
                move |app| shortcut::set_post_process_selected_prompt(app, id),
                cx,
            );
        });
    }

    fn editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(super::rows::field_label(t(
                        "settings.postProcessing.prompts.promptLabel",
                    )))
                    .child(TextField::new(&self.name).compact()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(super::rows::field_label(t(
                        "settings.postProcessing.prompts.promptInstructions",
                    )))
                    .child(TextArea::new(&self.text))
                    .child(prompt_tip(p.mid_gray.a(0.7))),
            )
    }
}

/// "Tip: Use `${output}` to insert…", with the placeholder in code style.
fn prompt_tip(color: gpui::Hsla) -> impl IntoElement {
    let template = t("settings.postProcessing.prompts.promptTip");
    let (before, rest) = template.split_once("<code>").unwrap_or((&template, ""));
    let (code, after) = rest.split_once("</code>").unwrap_or(("", rest));
    div()
        .xs_text()
        .text_color(color)
        .flex()
        .row_dir()
        .flex_wrap()
        .child(before.to_string())
        .child(
            div()
                .font_family(crate::ui::theme::MONO_FONT)
                .child(code.to_string()),
        )
        .child(after.to_string())
}

impl Render for PostProcessingPrompts {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let settings = self.store.read(cx).settings.clone();
        let store = self.store.read(cx);
        let selecting = store.is_updating("post_process_selected_prompt_id");
        let prompts = settings.post_process_prompts.clone();
        let selected = self.selected(cx);
        let (name, text) = self.drafts(cx);
        let has_prompts = !prompts.is_empty();
        let dirty = selected
            .as_ref()
            .is_some_and(|p| name != p.name || text != p.prompt.trim());

        let picker = div()
            .flex()
            .row_dir()
            .gap_2()
            .min_w_0()
            .child(
                Dropdown::new(
                    "post-process-prompt",
                    prompts
                        .iter()
                        .map(|p| DropdownOption::new(p.id.clone(), p.name.clone()))
                        .collect(),
                )
                .selected(settings.post_process_selected_prompt_id.clone())
                .placeholder(if has_prompts {
                    t("settings.postProcessing.prompts.selectPrompt")
                } else {
                    t("settings.postProcessing.prompts.noPrompts")
                })
                .disabled(selecting || self.creating)
                .flex_1()
                .min_w_0()
                .on_select({
                    let entity = cx.entity().downgrade();
                    move |value, _, cx| {
                        let value = value.to_string();
                        let _ = entity.update(cx, |this, cx| this.select(value, cx));
                    }
                }),
            )
            .child(
                Button::new("create-prompt")
                    .flex_shrink_0()
                    .disabled(self.creating)
                    .child(t("settings.postProcessing.prompts.createNew"))
                    .on_click(cx.listener(|this, _, window, cx| this.start_create(window, cx))),
            );

        let body = if self.creating {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(self.editor(cx))
                .child(
                    div()
                        .flex()
                        .row_dir()
                        .gap_2()
                        .pt_2()
                        .child(
                            Button::new("create-prompt-confirm")
                                .disabled(name.is_empty() || text.is_empty())
                                .child(t("settings.postProcessing.prompts.createPrompt"))
                                .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                        )
                        .child(
                            Button::new("create-prompt-cancel")
                                .variant(ButtonVariant::Secondary)
                                .child(t("settings.postProcessing.prompts.cancel"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel_create(window, cx)
                                })),
                        ),
                )
                .into_any_element()
        } else if selected.is_some() {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(self.editor(cx))
                .child(
                    div()
                        .flex()
                        .row_dir()
                        .gap_2()
                        .pt_2()
                        .child(
                            Button::new("update-prompt")
                                .disabled(name.is_empty() || text.is_empty() || !dirty)
                                .child(t("settings.postProcessing.prompts.updatePrompt"))
                                .on_click(cx.listener(|this, _, _, cx| this.update_prompt(cx))),
                        )
                        .child(
                            Button::new("delete-prompt")
                                .variant(ButtonVariant::Secondary)
                                .disabled(prompts.len() <= 1)
                                .child(t("settings.postProcessing.prompts.deletePrompt"))
                                .on_click(cx.listener(|this, _, _, cx| this.delete_prompt(cx))),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .p_3()
                .bg(p.mid_gray.a(0.05))
                .rounded_md()
                .border_1()
                .border_color(p.mid_gray.a(0.2))
                .child(
                    div()
                        .sm_text()
                        .text_color(p.mid_gray)
                        .child(if has_prompts {
                            t("settings.postProcessing.prompts.selectToEdit")
                        } else {
                            t("settings.postProcessing.prompts.createFirst")
                        }),
                )
                .into_any_element()
        };

        SettingContainer::new(t("settings.postProcessing.prompts.selectedPrompt.title"))
            .description(t(
                "settings.postProcessing.prompts.selectedPrompt.description",
            ))
            .layout(SettingLayout::Stacked)
            .child(div().flex().flex_col().gap_3().child(picker).child(body))
    }
}
