use std::cell::Cell;
use std::rc::{Rc, Weak};

use gpui::{
    AnyElement, App, Context, CursorStyle, FontWeight, Global, Pixels, SharedString, Window,
    canvas, div, prelude::*, px, rems,
};

use crate::i18n::t;
use crate::managers::audio::first_connected;
use crate::settings::AppSettings;
use crate::ui::components::{
    Dropdown, DropdownOption, KeyboardFocus, RowDirection, SettingContainer, Typography,
    divided_rows, icon,
};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{Alpha, palette};

const KEY: &str = "microphone_priority";
const GRIP_SIZE: f32 = 14.;
const NUMBER_SIZE: f32 = 20.;
const REMOVE_SIZE: f32 = 24.;

struct Reorder {
    from: usize,
    to: Cell<usize>,
    measured_row_width: Cell<Pixels>,
}

#[derive(Clone)]
struct DraggedMicrophone {
    name: SharedString,
    status: Status,
    reorder: Rc<Reorder>,
}

struct ActiveReorder(Weak<Reorder>);

impl Global for ActiveReorder {}

fn active_reorder(cx: &App) -> Option<Rc<Reorder>> {
    if !cx.has_active_drag() {
        return None;
    }
    cx.try_global::<ActiveReorder>()
        .and_then(|active| active.0.upgrade())
}

struct DragPreview {
    name: SharedString,
    status: Status,
    reorder: Weak<Reorder>,
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let Some(reorder) = self.reorder.upgrade() else {
            return div();
        };
        row_frame()
            .w(reorder.measured_row_width.get())
            .rounded(px(8.))
            .bg(p.clay)
            .border_1()
            .border_color(p.accent.a(0.6))
            .shadow_lg()
            .text_color(p.text)
            .children(row_content(
                reorder.to.get(),
                self.name.clone(),
                self.status,
                cx,
            ))
            .child(div().w(px(REMOVE_SIZE)).flex_none())
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

    let mut order: Vec<usize> = (0..priority.len()).collect();
    let lifted = active_reorder(cx)
        .map(|reorder| (reorder.from, reorder.to.get()))
        .filter(|(from, _)| *from < order.len());
    if let Some((from, to)) = lifted {
        let item = order.remove(from);
        order.insert(to.min(order.len()), item);
    }

    let mut rows = Vec::new();
    for (slot, &index) in order.iter().enumerate() {
        if lifted.is_some_and(|(from, _)| from == index) {
            rows.push(placeholder_row(slot, cx));
            continue;
        }
        let name = &priority[index];
        let status = match () {
            _ if !loaded => Status::Unknown,
            _ if in_use == Some(name.as_str()) => Status::InUse,
            _ if connected.contains(&name.as_str()) => Status::Connected,
            _ => Status::Disconnected,
        };
        rows.push(microphone_row(index, slot, name.clone().into(), status, cx));
    }
    let last = priority.len().saturating_sub(1);
    rows.push(system_default_row(last, loaded && in_use.is_none(), cx));
    vec![header(priority, &connected, cx), drop_zone(rows, cx)]
}

fn drop_zone(rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let p = palette(cx);
    div()
        .w_full()
        .flex()
        .flex_col()
        .on_drag_move::<DraggedMicrophone>(|event, window, cx| {
            cx.set_active_drag_cursor_style(CursorStyle::ClosedHand, window);
            if !event.bounds.contains(&event.event.position) {
                let reorder = &event.drag(cx).reorder;
                reorder.to.set(reorder.from);
            }
        })
        .on_drop(|dragged: &DraggedMicrophone, _, cx| {
            move_to(dragged.reorder.from, dragged.reorder.to.get(), cx)
        })
        .children(divided_rows(rows, p.line))
        .into_any_element()
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

fn microphone_row(
    saved_index: usize,
    shown_slot: usize,
    name: SharedString,
    status: Status,
    cx: &App,
) -> AnyElement {
    let reorder = Rc::new(Reorder {
        from: saved_index,
        to: Cell::new(saved_index),
        measured_row_width: Cell::new(px(0.)),
    });
    let measured = reorder.clone();
    let dragged = DraggedMicrophone {
        name: name.clone(),
        status,
        reorder,
    };
    track_slot(row_frame().id(("microphone", saved_index)), shown_slot)
        .relative()
        .cursor(CursorStyle::OpenHand)
        .on_drag(dragged, |dragged, _, _, cx| {
            cx.set_global(ActiveReorder(Rc::downgrade(&dragged.reorder)));
            let preview = DragPreview {
                name: dragged.name.clone(),
                status: dragged.status,
                reorder: Rc::downgrade(&dragged.reorder),
            };
            cx.new(|_| preview)
        })
        .child(
            canvas(
                move |bounds, _, _| measured.measured_row_width.set(bounds.size.width),
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .children(row_content(shown_slot, name.clone(), status, cx))
        .child(remove_button(saved_index, name, cx))
        .into_any_element()
}

fn placeholder_row(slot: usize, cx: &App) -> AnyElement {
    let p = palette(cx);
    track_slot(row_frame().id(("microphone-slot", slot)), slot)
        .child(
            div()
                .flex_1()
                .h(px(30.))
                .rounded(px(8.))
                .bg(p.accent.a(0.08))
                .border_1()
                .border_color(p.accent.a(0.35)),
        )
        .into_any_element()
}

fn track_slot<E: InteractiveElement>(row: E, slot: usize) -> E {
    row.on_drag_move::<DraggedMicrophone>(move |event, _, cx| {
        if event.bounds.contains(&event.event.position) {
            event.drag(cx).reorder.to.set(slot);
        }
    })
}

fn row_content(slot: usize, name: SharedString, status: Status, cx: &App) -> Vec<AnyElement> {
    let p = palette(cx);
    let dim = status == Status::Disconnected;
    let mut content = vec![
        icon("grip-vertical")
            .size(px(GRIP_SIZE))
            .flex_none()
            .text_color(p.mid_gray)
            .into_any_element(),
        number(slot + 1, cx),
        icon("mic")
            .size(px(15.))
            .flex_none()
            .text_color(p.mid_gray.a(if dim { 0.5 } else { 1. }))
            .into_any_element(),
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_px(13.)
            .when(dim, |el| el.text_color(p.text.a(0.45)))
            .child(name)
            .into_any_element(),
    ];
    content.extend(match status {
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
    });
    content
}

fn system_default_row(last: usize, in_use: bool, cx: &App) -> AnyElement {
    let p = palette(cx);
    let gap = |width: f32| div().w(px(width)).flex_none();
    track_slot(row_frame().id("microphone-system-default"), last)
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
