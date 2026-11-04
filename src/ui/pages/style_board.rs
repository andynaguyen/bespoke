use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{
    canvas, div, img, prelude::*, px, relative, rems, AnyElement, App, Bounds, Context, Entity,
    FontWeight, Global, Image, ImageFormat, Pixels, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::apps::InstalledApp;
use crate::i18n::t;
use crate::settings::{AppStyleRule, WritingStyle};
use crate::shortcut;
use crate::ui::components::{
    icon, width_tracker, Dropdown, DropdownOption, RowDirection, SegmentedControl, TextField,
    Typography,
};
use crate::ui::hover_card::{self, Card};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, Alpha};
use crate::utils::RunningApp;

use super::rows::set;

const STYLES: [WritingStyle; 3] = [
    WritingStyle::Formal,
    WritingStyle::Casual,
    WritingStyle::VeryCasual,
];
const MIN_WIDTH_FOR_COLUMNS: f32 = 520.;
const SHELF_MAX_HEIGHT: f32 = 196.;
const ICON_PIXELS: f64 = 64.;
const REMOVE: &str = "remove";

fn set_default_style(style: WritingStyle, cx: &mut App) {
    set(
        cx,
        "default_style",
        move |s| s.default_style = style,
        move |app| shortcut::update_default_style(app, style),
    );
}

fn set_app_styles(rules: Vec<AppStyleRule>, cx: &mut App) {
    let stored = rules.clone();
    set(
        cx,
        "app_styles",
        move |s| s.app_styles = stored,
        move |app| shortcut::update_app_styles(app, rules),
    );
}

fn style_key(style: WritingStyle) -> &'static str {
    match style {
        WritingStyle::Formal => "formal",
        WritingStyle::Casual => "casual",
        WritingStyle::VeryCasual => "veryCasual",
    }
}

fn style_from_key(key: &str) -> Option<WritingStyle> {
    STYLES.into_iter().find(|style| style_key(*style) == key)
}

fn style_name(style: WritingStyle) -> SharedString {
    t(&format!("settings.style.styles.{}.name", style_key(style)))
}

fn specimen(style: WritingStyle) -> &'static str {
    match style {
        WritingStyle::Formal => "Aa.",
        WritingStyle::Casual => "Aa",
        WritingStyle::VeryCasual => "aa",
    }
}

fn with_rule(
    rules: &[AppStyleRule],
    bundle_id: &str,
    name: &str,
    style: WritingStyle,
) -> Vec<AppStyleRule> {
    let mut next = rules.to_vec();
    match next.iter_mut().find(|rule| rule.bundle_id == bundle_id) {
        Some(rule) => rule.style = style,
        None => next.push(AppStyleRule {
            bundle_id: bundle_id.to_string(),
            app_name: name.to_string(),
            style,
        }),
    }
    next
}

fn without_rule(rules: &[AppStyleRule], bundle_id: &str) -> Vec<AppStyleRule> {
    rules
        .iter()
        .filter(|rule| rule.bundle_id != bundle_id)
        .cloned()
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
enum Dragged {
    Default,
    App {
        bundle_id: String,
        name: String,
        has_rule: bool,
    },
}

impl Dragged {
    fn place(&self, style: WritingStyle, cx: &mut App) {
        let settings = SettingsStore::get(cx);
        match self {
            Dragged::Default if settings.default_style != style => set_default_style(style, cx),
            Dragged::Default => {}
            Dragged::App {
                bundle_id, name, ..
            } => {
                let current = settings
                    .app_styles
                    .iter()
                    .find(|rule| &rule.bundle_id == bundle_id)
                    .map(|rule| rule.style);
                if current != Some(style) {
                    let next = with_rule(&settings.app_styles, bundle_id, name, style);
                    set_app_styles(next, cx);
                }
            }
        }
    }

    fn remove(&self, cx: &mut App) {
        if let Dragged::App {
            bundle_id,
            has_rule: true,
            ..
        } = self
        {
            let next = without_rule(&SettingsStore::get(cx).app_styles, bundle_id);
            set_app_styles(next, cx);
        }
    }

    fn removable(&self) -> bool {
        matches!(self, Dragged::App { has_rule: true, .. })
    }

    fn label(&self) -> SharedString {
        match self {
            Dragged::Default => t("settings.style.board.default"),
            Dragged::App { name, .. } => name.clone().into(),
        }
    }

    fn bundle_id(&self) -> Option<&str> {
        match self {
            Dragged::Default => None,
            Dragged::App { bundle_id, .. } => Some(bundle_id),
        }
    }
}

#[derive(Clone)]
enum Icon {
    Loading,
    Missing,
    Loaded(Arc<Image>),
}

#[derive(Default)]
struct AppIcons(HashMap<String, Icon>);

impl Global for AppIcons {}

fn request_icons(ids: Vec<String>, cx: &mut App) {
    let icons = cx.default_global::<AppIcons>();
    let ids: Vec<String> = ids
        .into_iter()
        .filter(|id| !icons.0.contains_key(id))
        .collect();
    if ids.is_empty() {
        return;
    }
    for id in &ids {
        icons.0.insert(id.clone(), Icon::Loading);
    }
    let task = cx.background_executor().spawn(async move {
        ids.into_iter()
            .map(|id| {
                let png = crate::apps::icon_png(&id, ICON_PIXELS);
                (id, png)
            })
            .collect::<Vec<_>>()
    });
    cx.spawn(async move |cx| {
        let loaded = task.await;
        let _ = cx.update(|cx| {
            let icons = cx.default_global::<AppIcons>();
            for (id, png) in loaded {
                let icon = match png {
                    Some(png) => Icon::Loaded(Arc::new(Image::from_bytes(ImageFormat::Png, png))),
                    None => Icon::Missing,
                };
                icons.0.insert(id, icon);
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

fn app_icon(dragged: &Dragged, size: Pixels, cx: &App) -> AnyElement {
    let p = palette(cx);
    let Some(bundle_id) = dragged.bundle_id() else {
        return div()
            .size(size)
            .flex_none()
            .rounded(size * 0.25)
            .bg(p.accent)
            .flex()
            .items_center()
            .justify_center()
            .child(
                icon("app-window")
                    .size(size * 0.62)
                    .text_color(p.accent_contrast),
            )
            .into_any_element();
    };
    let icon = cx
        .try_global::<AppIcons>()
        .and_then(|icons| icons.0.get(bundle_id).cloned())
        .unwrap_or(Icon::Loading);
    match icon {
        Icon::Loaded(image) => img(image).size(size).flex_none().into_any_element(),
        Icon::Loading | Icon::Missing => div()
            .size(size)
            .flex_none()
            .rounded(size * 0.25)
            .bg(p.mid_gray.a(0.25))
            .flex()
            .items_center()
            .justify_center()
            .text_size(size * 0.5)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.text.a(0.7))
            .child(
                dragged
                    .label()
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default(),
            )
            .into_any_element(),
    }
}

struct DragPreview(Dragged);

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        div()
            .flex()
            .row_dir()
            .items_center()
            .gap_2()
            .px(px(8.))
            .py(px(5.))
            .rounded(px(8.))
            .bg(p.card)
            .border_1()
            .border_color(p.accent.a(0.6))
            .shadow_lg()
            .text_px(12.5)
            .child(app_icon(&self.0, px(18.), cx))
            .child(self.0.label())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ShelfApp {
    bundle_id: String,
    name: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shelf {
    Open,
    All,
}

impl Shelf {
    fn id(self) -> &'static str {
        match self {
            Shelf::Open => "open",
            Shelf::All => "all",
        }
    }
}

pub struct StyleBoard {
    store: Entity<SettingsStore>,
    shelf: Shelf,
    search: Entity<InputState>,
    installed: Vec<InstalledApp>,
    running: Vec<RunningApp>,
    loaded: bool,
    width: Pixels,
    default_hovered: bool,
    default_bounds: Bounds<Pixels>,
    _subscriptions: Vec<Subscription>,
}

impl StyleBoard {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = SettingsStore::global(cx);
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("settings.style.shelf.search")));
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        cx.spawn(async move |this, cx| {
            let installed = crate::runtime::spawn_blocking(crate::apps::installed_apps).await;
            let _ = this.update(cx, |this, cx| {
                this.installed = installed.unwrap_or_default();
                this.loaded = true;
                cx.notify();
            });
        })
        .detach();
        Self {
            store,
            shelf: Shelf::Open,
            search,
            installed: Vec::new(),
            running: crate::utils::running_apps(),
            loaded: false,
            width: px(0.),
            default_hovered: false,
            default_bounds: Bounds::default(),
            _subscriptions: subscriptions,
        }
    }

    fn select_shelf(&mut self, id: &str, cx: &mut Context<Self>) {
        self.shelf = match id {
            "all" => Shelf::All,
            _ => Shelf::Open,
        };
        if self.shelf == Shelf::Open {
            self.running = crate::utils::running_apps();
        }
        cx.notify();
    }

    fn shelf_apps(&self, ruled: &HashSet<&str>, cx: &App) -> Vec<ShelfApp> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let apps: Vec<ShelfApp> = match self.shelf {
            Shelf::Open => self
                .running
                .iter()
                .map(|app| ShelfApp {
                    bundle_id: app.bundle_id.clone(),
                    name: app.name.clone(),
                })
                .collect(),
            Shelf::All => self
                .installed
                .iter()
                .map(|app| ShelfApp {
                    bundle_id: app.bundle_id.clone(),
                    name: app.name.clone(),
                })
                .collect(),
        };
        apps.into_iter()
            .filter(|app| !ruled.contains(app.bundle_id.as_str()))
            .filter(|app| app.bundle_id != crate::app_handle::APP_IDENTIFIER)
            .filter(|app| query.is_empty() || app.name.to_lowercase().contains(&query))
            .collect()
    }

    fn menu(
        id: SharedString,
        dragged: Dragged,
        current: Option<WritingStyle>,
        body: impl FnOnce(bool, &App) -> AnyElement + 'static,
    ) -> Dropdown {
        let mut options: Vec<DropdownOption> = STYLES
            .iter()
            .map(|style| DropdownOption::new(style_key(*style), style_name(*style)))
            .collect();
        if dragged.removable() {
            options.push(DropdownOption::new(
                REMOVE,
                t("settings.style.board.remove"),
            ));
        }
        Dropdown::new(id, options)
            .selected(current.map(style_key))
            .menu_width(rems(11.))
            .trigger(body)
            .on_select(move |value, _, cx| match style_from_key(value) {
                Some(style) => dragged.place(style, cx),
                None if value.as_ref() == REMOVE => dragged.remove(cx),
                None => {}
            })
    }

    fn chip(
        &self,
        dragged: Dragged,
        current: WritingStyle,
        entity: WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        let key = match &dragged {
            Dragged::Default => "default".to_string(),
            Dragged::App { bundle_id, .. } => bundle_id.clone(),
        };
        let is_default = dragged == Dragged::Default;
        let label = dragged.label();
        let icon_el = app_icon(&dragged, px(18.), cx);
        let preview = dragged.clone();
        div()
            .id(SharedString::from(format!("chip-{key}")))
            .w_full()
            .on_drag(preview, |dragged, _, _, cx| {
                cx.new(|_| DragPreview(dragged.clone()))
            })
            .when(is_default, |el| {
                let hover = entity.clone();
                el.relative()
                    .on_hover(move |hovered, _, cx| {
                        let hovered = *hovered;
                        let _ = hover.update(cx, |this, cx| {
                            if this.default_hovered != hovered {
                                this.default_hovered = hovered;
                                cx.notify();
                            }
                        });
                    })
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                let _ = entity.update(cx, |this, _| this.default_bounds = bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
            })
            .child(Self::menu(
                SharedString::from(format!("chip-menu-{key}")),
                dragged,
                Some(current),
                move |open, cx| {
                    let p = palette(cx);
                    div()
                        .flex()
                        .row_dir()
                        .items_center()
                        .gap_2()
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(8.))
                        .border_1()
                        .text_px(12.5)
                        .when(is_default, |el| {
                            el.bg(p.accent.a(if p.is_dark { 0.18 } else { 0.1 }))
                                .border_color(p.accent.a(0.5))
                        })
                        .when(!is_default, |el| {
                            el.bg(p.background.a(0.6)).border_color(p.line)
                        })
                        .when(open, |el| el.border_color(p.accent.a(0.7)))
                        .hover(|s| s.border_color(p.mid_gray.a(0.5)))
                        .child(icon_el)
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .when(is_default, |el| {
                            el.child(
                                icon("info")
                                    .size(px(13.))
                                    .flex_none()
                                    .text_color(p.text.a(0.45)),
                            )
                        })
                        .into_any_element()
                },
            ))
            .into_any_element()
    }

    fn column(
        &self,
        style: WritingStyle,
        wide: bool,
        entity: &WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        let p = palette(cx);
        let settings = &self.store.read(cx).settings;
        let key = style_key(style);
        let mut chips: Vec<AnyElement> = Vec::new();
        if settings.default_style == style {
            chips.push(self.chip(Dragged::Default, style, entity.clone(), cx));
        }
        for rule in settings
            .app_styles
            .iter()
            .filter(|rule| rule.style == style)
        {
            chips.push(self.chip(
                Dragged::App {
                    bundle_id: rule.bundle_id.clone(),
                    name: rule.app_name.clone(),
                    has_rule: true,
                },
                style,
                entity.clone(),
                cx,
            ));
        }
        div()
            .id(SharedString::from(format!("style-column-{key}")))
            .when(wide, |el| el.flex_1().min_w_0())
            .flex()
            .flex_col()
            .gap_2()
            .p(px(12.))
            .rounded(px(10.))
            .bg(p.card)
            .border_1()
            .border_color(p.line)
            .drag_over::<Dragged>(move |s, _, _, cx| {
                let p = palette(cx);
                s.border_color(p.accent).bg(p.accent.a(0.08))
            })
            .on_drop(move |dragged: &Dragged, _, cx| dragged.place(style, cx))
            .child(
                div()
                    .flex()
                    .row_dir()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .text_px(22.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(specimen(style)),
                    )
                    .child(
                        div()
                            .text_px(13.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(style_name(style)),
                    ),
            )
            .child(
                div()
                    .px(px(9.))
                    .py(px(6.))
                    .rounded(px(7.))
                    .bg(p.background.a(0.5))
                    .text_px(12.)
                    .text_color(p.text.a(0.75))
                    .child(t(&format!("settings.style.styles.{key}.example"))),
            )
            .child(div().flex().flex_col().gap(px(5.)).children(chips))
            .child(
                div()
                    .py(px(6.))
                    .rounded(px(8.))
                    .border_1()
                    .border_dashed()
                    .border_color(p.mid_gray.a(0.3))
                    .flex()
                    .justify_center()
                    .text_px(11.5)
                    .text_color(p.text.a(0.4))
                    .child(t("settings.style.board.dropHere")),
            )
            .into_any_element()
    }

    fn tile(&self, app: &ShelfApp, wide: bool, cx: &App) -> AnyElement {
        let p = palette(cx);
        let dragged = Dragged::App {
            bundle_id: app.bundle_id.clone(),
            name: app.name.clone(),
            has_rule: false,
        };
        let icon_el = app_icon(&dragged, px(28.), cx);
        let name: SharedString = app.name.clone().into();
        let preview = dragged.clone();
        div()
            .id(SharedString::from(format!("tile-{}", app.bundle_id)))
            .w(relative(if wide { 1. / 3. } else { 1. / 2. }))
            .p(px(2.))
            .on_drag(preview, |dragged, _, _, cx| {
                cx.new(|_| DragPreview(dragged.clone()))
            })
            .child(Self::menu(
                SharedString::from(format!("tile-menu-{}", app.bundle_id)),
                dragged,
                None,
                move |open, _| {
                    div()
                        .flex()
                        .row_dir()
                        .items_center()
                        .gap(px(9.))
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(9.))
                        .border_1()
                        .border_color(gpui::transparent_black())
                        .when(open, |el| el.bg(p.text.a(0.06)).border_color(p.line))
                        .hover(|s| s.bg(p.text.a(0.05)).border_color(p.line))
                        .child(icon_el)
                        .child(div().min_w_0().text_px(12.5).truncate().child(name))
                        .into_any_element()
                },
            ))
            .into_any_element()
    }

    fn render_shelf(&self, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let rules = &self.store.read(cx).settings.app_styles;
        let ruled: HashSet<&str> = rules.iter().map(|rule| rule.bundle_id.as_str()).collect();
        let apps = self.shelf_apps(&ruled, cx);
        let searching = !self.search.read(cx).value().trim().is_empty();
        let tiles: Vec<AnyElement> = apps.iter().map(|app| self.tile(app, wide, cx)).collect();
        let entity = cx.entity().downgrade();
        let empty = if !self.loaded && self.shelf == Shelf::All {
            Some(t("settings.style.shelf.loading"))
        } else if tiles.is_empty() && searching {
            Some(t("settings.style.shelf.noMatch"))
        } else if tiles.is_empty() {
            Some(t("settings.style.shelf.noneLeft"))
        } else {
            None
        };
        div()
            .id("style-shelf")
            .flex()
            .flex_col()
            .gap_3()
            .p(px(12.))
            .rounded(px(10.))
            .bg(p.card.a(0.5))
            .border_1()
            .border_color(p.line)
            .can_drop(|value, _, _| {
                value
                    .downcast_ref::<Dragged>()
                    .is_some_and(Dragged::removable)
            })
            .drag_over::<Dragged>(|s, dragged, _, _| {
                if dragged.removable() {
                    s.border_color(gpui::red().a(0.6)).bg(gpui::red().a(0.06))
                } else {
                    s
                }
            })
            .on_drop(|dragged: &Dragged, _, cx| dragged.remove(cx))
            .child(
                div()
                    .flex()
                    .row_dir()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_px(11.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.mid_gray)
                            .child(t("settings.style.shelf.title")),
                    )
                    .child(SegmentedControl::new(
                        "style-shelf-tabs",
                        vec![
                            ("open".into(), t("settings.style.shelf.open")),
                            ("all".into(), t("settings.style.shelf.all")),
                        ],
                        self.shelf.id(),
                        move |id, _, cx| {
                            let id = id.to_string();
                            let _ = entity.update(cx, |this, cx| this.select_shelf(&id, cx));
                        },
                    ))
                    .child(div().flex_1())
                    .child(
                        TextField::new(&self.search)
                            .regular_weight()
                            .w(px(170.))
                            .prefix(
                                icon("search")
                                    .size(px(13.))
                                    .text_color(p.mid_gray)
                                    .me(px(6.)),
                            ),
                    ),
            )
            .child(match empty {
                Some(text) => div()
                    .px(px(8.))
                    .py(px(10.))
                    .text_px(12.)
                    .text_color(p.text.a(0.45))
                    .child(text)
                    .into_any_element(),
                None => div()
                    .id("style-shelf-tiles")
                    .max_h(px(SHELF_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .flex()
                    .row_dir()
                    .flex_wrap()
                    .mx(px(-2.))
                    .children(tiles)
                    .into_any_element(),
            })
            .into_any_element()
    }

    fn visible_icon_ids(&self, cx: &App) -> Vec<String> {
        let rules = &self.store.read(cx).settings.app_styles;
        let ruled: HashSet<&str> = rules.iter().map(|rule| rule.bundle_id.as_str()).collect();
        rules
            .iter()
            .map(|rule| rule.bundle_id.clone())
            .chain(
                self.shelf_apps(&ruled, cx)
                    .into_iter()
                    .map(|app| app.bundle_id),
            )
            .collect()
    }
}

impl Render for StyleBoard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let wide = self.width >= px(MIN_WIDTH_FOR_COLUMNS);
        let card = (self.default_hovered && window.is_window_active() && !cx.has_active_drag())
            .then(|| {
                let card = Card {
                    title: t("settings.style.board.defaultTitle"),
                    rows: Vec::new(),
                    body: Some(t("settings.style.board.defaultHelp")),
                };
                let bounds = self.default_bounds;
                (
                    card,
                    Bounds::new(window.bounds().origin + bounds.origin, bounds.size),
                )
            });
        cx.defer(move |cx| match card {
            Some((card, anchor)) => hover_card::show(card, anchor, cx),
            None => hover_card::hide(cx),
        });
        let entity = cx.entity().downgrade();
        let ids = self.visible_icon_ids(cx);
        let missing = cx
            .try_global::<AppIcons>()
            .is_none_or(|icons| ids.iter().any(|id| !icons.0.contains_key(id)));
        if missing {
            cx.defer(move |cx| request_icons(ids, cx));
        }
        let columns: Vec<AnyElement> = STYLES
            .iter()
            .map(|style| self.column(*style, wide, &entity, cx))
            .collect();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .sm_text()
                    .text_color(p.text.a(0.65))
                    .child(t("settings.style.intro")),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .row_dir()
                    .when(!wide, |el| el.flex_col())
                    .gap(px(10.))
                    .children(columns)
                    .child(width_tracker(cx.entity().downgrade(), |this: &mut Self| {
                        &mut this.width
                    })),
            )
            .child(if cfg!(target_os = "macos") {
                self.render_shelf(wide, cx)
            } else {
                div()
                    .text_px(12.)
                    .text_color(p.text.a(0.55))
                    .child(t("settings.style.macOSOnly"))
                    .into_any_element()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(bundle_id: &str, style: WritingStyle) -> AppStyleRule {
        AppStyleRule {
            bundle_id: bundle_id.into(),
            app_name: bundle_id.into(),
            style,
        }
    }

    #[test]
    fn a_rule_changes_in_place_or_goes_last() {
        let rules = vec![
            rule("com.apple.MobileSMS", WritingStyle::VeryCasual),
            rule("com.apple.mail", WritingStyle::Formal),
        ];
        let changed = with_rule(&rules, "com.apple.mail", "Mail", WritingStyle::Casual);
        assert_eq!(changed[1].style, WritingStyle::Casual);
        assert_eq!(changed.len(), 2);
        let added = with_rule(&rules, "com.apple.Notes", "Notes", WritingStyle::Formal);
        assert_eq!(added.len(), 3);
        assert_eq!(added[2].app_name, "Notes");
        assert_eq!(
            without_rule(&added, "com.apple.mail"),
            vec![added[0].clone(), added[2].clone()]
        );
    }

    #[test]
    fn style_keys_round_trip() {
        for style in STYLES {
            assert_eq!(style_from_key(style_key(style)), Some(style));
        }
        assert_eq!(style_from_key(REMOVE), None);
    }
}
