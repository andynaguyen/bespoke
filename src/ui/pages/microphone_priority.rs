use gpui::{
    AnyElement, App, Context, CursorStyle, FontWeight, SharedString, Window, div, prelude::*, px,
    rems,
};

use crate::i18n::t;
use crate::managers::audio::first_connected;
use crate::settings::AppSettings;
use crate::ui::components::{
    Dropdown, DropdownOption, KeyboardFocus, RowDirection, SettingContainer, Typography, icon,
};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{Alpha, palette};

const KEY: &str = "microphone_priority";
const GRIP_SIZE: f32 = 14.;
const NUMBER_SIZE: f32 = 20.;
const REMOVE_SIZE: f32 = 24.;

#[derive(Clone)]
struct DraggedMicrophone {
    from: usize,
    name: SharedString,
}

struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        div()
            .flex()
            .row_dir()
            .items_center()
            .gap_2()
            .px(px(10.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(p.background)
            .border_1()
            .border_color(p.accent.a(0.6))
            .shadow_lg()
            .text_px(13.)
            .child(icon("mic").size(px(15.)).text_color(p.mid_gray))
            .child(self.0.clone())
    }
}

pub fn rows(s: &AppSettings, cx: &App) -> Vec<AnyElement> {
    let store = SettingsStore::global(cx).read(cx);
    let loaded = !store.audio_devices.is_empty();
    let connected: Vec<&str> = store
        .audio_devices
        .iter()
        .filter(|d| !d.is_default)
        .map(|d| d.name.as_str())
        .collect();
    let priority = &s.microphone_priority;
    let in_use = first_connected(priority, &connected);

    let mut rows = vec![header(priority, &connected, cx)];
    for (index, name) in priority.iter().enumerate() {
        let status = match () {
            _ if !loaded => Status::Unknown,
            _ if in_use == Some(name.as_str()) => Status::InUse,
            _ if connected.contains(&name.as_str()) => Status::Connected,
            _ => Status::Disconnected,
        };
        rows.push(microphone_row(index, name.clone().into(), status, cx));
    }
    rows.push(system_default_row(loaded && in_use.is_none(), cx));
    rows
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Unknown,
    InUse,
    Connected,
    Disconnected,
}

fn header(priority: &[String], connected: &[&str], cx: &App) -> AnyElement {
    let p = palette(cx);
    let addable: Vec<DropdownOption> = connected
        .iter()
        .filter(|name| !priority.iter().any(|m| m == *name))
        .map(|name| DropdownOption::new(name.to_string(), name.to_string()))
        .collect();
    let options = if addable.is_empty() {
        vec![
            DropdownOption::new("", t("settings.sound.microphone.priority.noneToAdd"))
                .disabled(true),
        ]
    } else {
        addable
    };
    SettingContainer::new(t("settings.sound.microphone.priority.title"))
        .description(t("settings.sound.microphone.priority.description"))
        .child(
            Dropdown::new("add-microphone", options)
                .menu_end_aligned(rems(16.))
                .on_open(|_, cx| {
                    SettingsStore::global(cx).update(cx, SettingsStore::refresh_audio_devices)
                })
                .on_select(|name, _, cx| {
                    if name.is_empty() {
                        return;
                    }
                    let mut list = SettingsStore::get(cx).microphone_priority.clone();
                    if !list.iter().any(|m| m == name.as_ref()) {
                        list.push(name.to_string());
                        save(list, cx);
                    }
                })
                .trigger(move |open, _| {
                    div()
                        .flex()
                        .row_dir()
                        .items_center()
                        .gap(px(6.))
                        .h(px(30.))
                        .px(px(10.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(if open { p.accent.a(0.7) } else { p.line })
                        .bg(p.clay)
                        .text_px(13.)
                        .child(icon("plus").size(px(14.)).text_color(p.mid_gray))
                        .child(t("settings.sound.microphone.priority.add"))
                        .into_any_element()
                }),
        )
        .into_any_element()
}

fn microphone_row(index: usize, name: SharedString, status: Status, cx: &App) -> AnyElement {
    let p = palette(cx);
    let dim = status == Status::Disconnected;
    let dragged = DraggedMicrophone {
        from: index,
        name: name.clone(),
    };
    row_frame()
        .id(("microphone", index))
        .cursor(CursorStyle::OpenHand)
        .on_drag(dragged, |dragged, _, _, cx| {
            cx.new(|_| DragPreview(dragged.name.clone()))
        })
        .drag_over::<DraggedMicrophone>(move |style, dragged, _, cx| {
            if dragged.from == index {
                style
            } else {
                style.bg(palette(cx).accent.a(0.08))
            }
        })
        .on_drop(move |dragged: &DraggedMicrophone, _, cx| move_to(dragged.from, index, cx))
        .child(
            icon("grip-vertical")
                .size(px(GRIP_SIZE))
                .flex_none()
                .text_color(p.mid_gray),
        )
        .child(number(index + 1, cx))
        .child(
            icon("mic")
                .size(px(15.))
                .flex_none()
                .text_color(p.mid_gray.a(if dim { 0.5 } else { 1. })),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_px(13.)
                .when(dim, |el| el.text_color(p.text.a(0.45)))
                .child(name.clone()),
        )
        .children(match status {
            Status::InUse => Some(pill(
                t("settings.sound.microphone.priority.inUse"),
                true,
                cx,
            )),
            Status::Disconnected => Some(pill(
                t("settings.sound.microphone.priority.notConnected"),
                false,
                cx,
            )),
            Status::Unknown | Status::Connected => None,
        })
        .child(remove_button(index, name, cx))
        .into_any_element()
}

fn system_default_row(in_use: bool, cx: &App) -> AnyElement {
    let p = palette(cx);
    let gap = |width: f32| div().w(px(width)).flex_none();
    row_frame()
        .id("microphone-system-default")
        .drag_over::<DraggedMicrophone>(|style, _, _, cx| style.bg(palette(cx).accent.a(0.08)))
        .on_drop(|dragged: &DraggedMicrophone, _, cx| {
            let last = SettingsStore::get(cx)
                .microphone_priority
                .len()
                .saturating_sub(1);
            move_to(dragged.from, last, cx);
        })
        .child(gap(GRIP_SIZE))
        .child(gap(NUMBER_SIZE))
        .child(
            icon("settings")
                .size(px(15.))
                .flex_none()
                .text_color(p.mid_gray),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_px(13.)
                .child(t("settings.sound.microphone.priority.systemDefault")),
        )
        .child(if in_use {
            pill(t("settings.sound.microphone.priority.inUse"), true, cx)
        } else {
            div()
                .text_px(12.)
                .text_color(p.mid_gray)
                .child(t("settings.sound.microphone.priority.alwaysLast"))
                .into_any_element()
        })
        .child(gap(REMOVE_SIZE))
        .into_any_element()
}

fn row_frame() -> gpui::Div {
    div()
        .w_full()
        .flex()
        .row_dir()
        .items_center()
        .gap(px(10.))
        .min_h(px(44.))
        .px_3()
        .py(px(6.))
}

fn number(n: usize, cx: &App) -> AnyElement {
    let p = palette(cx);
    div()
        .size(px(NUMBER_SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(p.text.a(if p.is_dark { 0.1 } else { 0.06 }))
        .text_px(11.)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.text.a(0.7))
        .child(n.to_string())
        .into_any_element()
}

fn pill(label: SharedString, in_use: bool, cx: &App) -> AnyElement {
    let p = palette(cx);
    let (fg, bg) = if in_use {
        (p.done, p.done.a(0.15))
    } else {
        (p.mid_gray, p.text.a(if p.is_dark { 0.1 } else { 0.06 }))
    };
    div()
        .flex_none()
        .px(px(8.))
        .py(px(2.))
        .rounded_full()
        .text_px(11.)
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .bg(bg)
        .child(label)
        .into_any_element()
}

fn remove_button(index: usize, name: SharedString, cx: &App) -> AnyElement {
    let p = palette(cx);
    div()
        .id(("remove-microphone", index))
        .size(px(REMOVE_SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .keyboard_focus(cx)
        .text_color(p.text.a(0.5))
        .hover(|s| s.bg(p.mid_gray.a(0.15)).text_color(p.text))
        .active(|s| s.bg(p.mid_gray.a(0.25)))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let mut list = SettingsStore::get(cx).microphone_priority.clone();
            list.retain(|m| m != name.as_ref());
            save(list, cx);
        })
        .child(icon("x").size(px(13.)))
        .into_any_element()
}

fn move_to(from: usize, to: usize, cx: &mut App) {
    let mut list = SettingsStore::get(cx).microphone_priority.clone();
    if from == to || from >= list.len() || to >= list.len() {
        return;
    }
    let item = list.remove(from);
    list.insert(to, item);
    save(list, cx);
}

fn save(priority: Vec<String>, cx: &mut App) {
    SettingsStore::global(cx).update(cx, |store, cx| {
        let app = store.app().clone();
        let stored = priority.clone();
        store.update_async(
            KEY,
            move |s| s.microphone_priority = stored,
            async move { crate::commands::audio::set_microphone_priority(app, priority).await },
            cx,
        )
    });
}
