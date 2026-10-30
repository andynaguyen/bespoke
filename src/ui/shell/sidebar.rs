//! The sidebar (`Sidebar.tsx`): the logo and the section links.

use gpui::{
    div, img, prelude::*, px, App, Context, FontWeight, ImageSource, Resource, SharedString, Window,
};

use super::{MainWindow, Section, TITLEBAR_HEIGHT};
use crate::i18n::{t, t_with};
use crate::ui::components::{
    icon, Button, ButtonSize, ButtonVariant, KeyboardFocus, RowDirection, Typography,
};
use crate::ui::glass::{glass, GlassStyle};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, radius, Alpha};

/// The sidebar panel's width, without the inset around it.
pub const SIDEBAR_WIDTH: f32 = 220.;
/// The gap between the title bar, where the traffic lights sit, and the logo.
const LOGO_TOP: f32 = 26.;
/// The sidebar wordmark's width. The clay PNGs are drawn at twice this size,
/// with a bevel and shadow tuned for it.
const LOGO_WIDTH: f32 = 120.;

/// The clay Bespoke wordmark, `width` pixels wide: grey on dark, charcoal on
/// light. Each PNG pads the 1965 by 544 wordmark so its drop shadow isn't
/// clipped, so the image spills past the element's bounds by that padding.
/// The PNGs are pre-rendered from `bespoke-logo-dark.svg` and
/// `bespoke-logo-light.svg`, since resvg's blur filters take about 0.3s at
/// launch.
pub fn logo(width: f32, cx: &App) -> impl IntoElement {
    // The padding, in the wordmark's 1965 by 544 units.
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
    let p = palette(cx);
    let label = t(section.label_key());
    div()
        .id(SharedString::from(format!("nav-{:?}", section)))
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
        .on_click(cx.listener(move |this, _, window, cx| this.select_section(section, window, cx)))
        .child(
            icon(section.icon())
                .size(px(18.))
                .flex_none()
                .text_color(if active { p.text } else { p.text.a(0.85) }),
        )
        .child(div().text_px(15.).min_w_0().truncate().child(label))
}

/// Offers the update a scheduled check found (`updater::pending_version`).
/// Update opens Sparkle's window, which downloads, installs and relaunches.
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
                .variant(ButtonVariant::Primary)
                .size(ButtonSize::Sm)
                .w_full()
                .on_click(|_, _, _| crate::updater::check_now())
                .child(t("sidebar.update.button")),
        )
}

pub fn render(
    main: &MainWindow,
    _window: &mut Window,
    cx: &mut Context<MainWindow>,
) -> impl IntoElement {
    let debug_mode = SettingsStore::get(cx).debug_mode;
    let active = main.section;

    let mut secondary = Vec::new();
    if let Some(version) = crate::updater::pending_version() {
        secondary.push(update_card(&version, cx).into_any_element());
    }
    if debug_mode {
        secondary.push(nav_item(Section::Debug, active == Section::Debug, cx).into_any_element());
    }
    secondary.push(nav_item(Section::Settings, active == Section::Settings, cx).into_any_element());
    let primary: Vec<_> = Section::PRIMARY
        .iter()
        .map(|section| nav_item(*section, active == *section, cx).into_any_element())
        .collect();

    // The panel floats in the window, inset on every side, so its corners
    // nest inside the window's.
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
