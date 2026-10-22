//! The sidebar (`Sidebar.tsx`): logo, section links, and the footer with the
//! model status.

use gpui::{
    div, img, prelude::*, px, App, Context, FontWeight, ImageSource, Resource, SharedString, Window,
};

use super::{MainWindow, Section, TITLEBAR_HEIGHT};
use crate::i18n::t;
use crate::ui::components::{icon, KeyboardFocus, RowDirection, Typography};
use crate::ui::glass::{glass, GlassStyle};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, radius, Alpha};

/// The sidebar panel's width, without the inset around it.
pub const SIDEBAR_WIDTH: f32 = 220.;

/// The Bespoke wordmark, `width` pixels wide.
pub fn logo(width: f32, cx: &App) -> impl IntoElement {
    let file = if palette(cx).is_dark {
        "bespoke-logo-dark.svg"
    } else {
        "bespoke-logo-light.svg"
    };
    img(ImageSource::Resource(Resource::Embedded(file.into())))
        .w(px(width))
        .h(px(width * 544. / 1965.))
        .flex_none()
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
                .text_color(if active { p.accent } else { p.mid_gray }),
        )
        .child(div().text_px(15.).min_w_0().truncate().child(label))
}

pub fn render(
    main: &MainWindow,
    _window: &mut Window,
    cx: &mut Context<MainWindow>,
) -> impl IntoElement {
    let p = palette(cx);
    let debug_mode = SettingsStore::get(cx).debug_mode;
    let active = main.section;

    let mut secondary = Vec::new();
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
            .child(div().mx(px(10.)).mt_3().mb_4().child(logo(92., cx)))
            .child(div().flex().flex_col().gap(px(2.)).children(primary))
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(secondary)
                    .child(
                        div()
                            .pt_2()
                            .text_px(13.)
                            .line_height(px(18.))
                            .text_color(p.mid_gray)
                            .child(main.model_selector.clone()),
                    ),
            ),
    )
}
