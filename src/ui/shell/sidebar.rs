use gpui::{
    App, Context, FontWeight, ImageSource, Resource, SharedString, div, img, prelude::*, px,
};

use super::{MainWindow, Section, TITLEBAR_HEIGHT};
use crate::i18n::{t, t_with};
use crate::ui::components::{
    Button, ButtonSize, KeyboardFocus, RowDirection, Tooltip, Typography, icon,
};
use crate::ui::glass::{GlassStyle, glass};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{Alpha, palette, radius};

pub const SIDEBAR_WIDTH: f32 = 220.;
const LOGO_TOP: f32 = 26.;
const LOGO_WIDTH: f32 = 120.;

pub fn logo(width: f32, cx: &App) -> impl IntoElement {
    const LEFT: f32 = 80.;
    const TOP: f32 = 50.;
    const RIGHT: f32 = 140.;
    const BOTTOM: f32 = 170.;
    let file = if palette(cx).is_dark {
        "bespoke-logo-clay-dark.png"
    } else {
        "bespoke-logo-clay-light.png"
    };
    let scale = width / 1965.;
    div()
        .relative()
        .w(px(width))
        .h(px(544. * scale))
        .flex_none()
        .child(
            img(ImageSource::Resource(Resource::Embedded(file.into())))
                .absolute()
                .left(px(-LEFT * scale))
                .top(px(-TOP * scale))
                .w(px((LEFT + 1965. + RIGHT) * scale))
                .h(px((TOP + 544. + BOTTOM) * scale)),
        )
}

fn nav_item(section: Section, active: bool, cx: &mut Context<MainWindow>) -> impl IntoElement {
    nav_row(
        SharedString::from(format!("nav-{:?}", section)),
        section.icon(),
        t(section.label_key()),
        active,
        cx,
    )
    .on_click(cx.listener(move |this, _, window, cx| this.select_section(section, window, cx)))
}

fn footer_buttons(main: &MainWindow, cx: &mut Context<MainWindow>) -> impl IntoElement {
    use crate::ui::pages::{settings::SettingsDialog, triage::TriageDialog};
    let settings_open = main.dialog::<SettingsDialog>().is_some();
    let triage_open = main.dialog::<TriageDialog>().is_some();
    div()
        .flex()
        .row_dir()
        .items_center()
        .gap(px(2.))
        .child(
            footer_button(
                "nav-settings",
                "nav-settings",
                t("sidebar.settings"),
                settings_open,
                cx,
            )
            .on_click(|_, window, cx| crate::ui::pages::settings::open(None, window, cx)),
        )
        .child(
            footer_button("nav-triage", "nav-bug", t("triage.title"), triage_open, cx)
                .on_click(|_, window, cx| crate::ui::pages::triage::open(window, cx)),
        )
}

fn footer_button(
    id: &'static str,
    icon_name: &'static str,
    label: SharedString,
    active: bool,
    cx: &mut Context<MainWindow>,
) -> gpui::Stateful<gpui::Div> {
    let p = palette(cx);
    div()
        .id(id)
        .size(px(36.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::ROW)
        .cursor_pointer()
        .when(active, |el| {
            el.bg(p.text.a(if p.is_dark { 0.12 } else { 0.08 }))
                .text_color(p.text)
        })
        .when(!active, |el| {
            el.text_color(p.text.a(0.85))
                .hover(|s| s.bg(p.text.a(0.06)).text_color(p.text))
        })
        .keyboard_focus(cx)
        .tooltip(move |_, cx| Tooltip::text(label.clone(), cx))
        .child(icon(icon_name).size(px(18.)).flex_none())
}

fn nav_row(
    id: SharedString,
    icon_name: &'static str,
    label: SharedString,
    active: bool,
    cx: &mut Context<MainWindow>,
) -> gpui::Stateful<gpui::Div> {
    let p = palette(cx);
    div()
        .id(id)
        .flex()
        .row_dir()
        .gap(px(10.))
        .items_center()
        .h(px(36.))
        .px(px(10.))
        .w_full()
        .rounded(radius::ROW)
        .cursor_pointer()
        .when(active, |el| {
            el.bg(p.text.a(if p.is_dark { 0.12 } else { 0.08 }))
                .text_color(p.text)
                .font_weight(FontWeight::SEMIBOLD)
        })
        .when(!active, |el| {
            el.text_color(p.text.a(0.85))
                .hover(|s| s.bg(p.text.a(0.06)).text_color(p.text))
        })
        .keyboard_focus(cx)
        .child(
            icon(icon_name)
                .size(px(18.))
                .flex_none()
                .text_color(if active { p.text } else { p.text.a(0.85) }),
        )
        .child(div().text_px(15.).min_w_0().truncate().child(label))
}

fn update_card(version: &str, cx: &App) -> impl IntoElement {
    let p = palette(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .mb(px(6.))
        .p(px(12.))
        .rounded(radius::ROW)
        .bg(p.accent.a(if p.is_dark { 0.16 } else { 0.08 }))
        .border_1()
        .border_color(p.accent.a(0.22))
        .child(
            div()
                .flex()
                .row_dir()
                .items_center()
                .gap(px(8.))
                .child(div().size(px(7.)).flex_none().rounded_full().bg(p.accent))
                .child(
                    div()
                        .text_px(13.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .min_w_0()
                        .truncate()
                        .child(t("sidebar.update.available")),
                ),
        )
        .child(
            div()
                .text_px(12.)
                .text_color(p.mid_gray)
                .min_w_0()
                .truncate()
                .child(t_with("sidebar.update.version", &[("version", &version)])),
        )
        .child(
            Button::new("sidebar-update")
                .size(ButtonSize::Sm)
                .w_full()
                .on_click(|_, _, _| crate::updater::check_now())
                .child(t("sidebar.update.button")),
        )
}

pub fn render(main: &MainWindow, cx: &mut Context<MainWindow>) -> impl IntoElement + use<> {
    let debug_mode = SettingsStore::get(cx).debug_mode;
    let active = main.section;

    let mut secondary = Vec::new();
    if let Some(version) = crate::updater::pending_version() {
        secondary.push(update_card(&version, cx).into_any_element());
    }
    if debug_mode {
        secondary.push(nav_item(Section::Debug, active == Section::Debug, cx).into_any_element());
    }
    secondary.push(footer_buttons(main, cx).into_any_element());
    let primary: Vec<_> = Section::PRIMARY
        .iter()
        .map(|section| nav_item(*section, active == *section, cx).into_any_element())
        .collect();

    div().flex_none().h_full().p(radius::INSET).child(
        div()
            .relative()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .pt(px(TITLEBAR_HEIGHT))
            .px(px(10.))
            .pb_3()
            .rounded(radius::PANEL)
            .child(glass("sidebar", GlassStyle::Regular, radius::PANEL))
            .child(
                div()
                    .mx(px(10.))
                    .mt(px(LOGO_TOP))
                    .mb_4()
                    .child(logo(LOGO_WIDTH, cx)),
            )
            .child(div().flex().flex_col().gap(px(2.)).children(primary))
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(secondary),
            ),
    )
}
