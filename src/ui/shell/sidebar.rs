//! The sidebar (`Sidebar.tsx`): logo, section links, and the footer with the
//! model status, update status and version.

use gpui::{
    div, img, prelude::*, px, App, Context, FontWeight, ImageSource, Resource, SharedString, Window,
};

use super::{MainWindow, Section, TITLEBAR_HEIGHT};
use crate::i18n::t;
use crate::ui::components::{icon, KeyboardFocus, RowDirection, Typography};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, shadow, Alpha};

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
        .gap(px(9.))
        .items_center()
        .h(px(28.))
        .px_2()
        .w_full()
        .rounded(px(6.))
        .cursor_pointer()
        .when(active, |el| {
            el.bg(p.raised)
                .text_color(p.text)
                .shadow(vec![shadow(0., 1., 2., 0., 0.25)])
        })
        .when(!active, |el| {
            el.text_color(p.text.a(0.8))
                .hover(|s| s.bg(p.mid_gray.a(0.12)).text_color(p.text))
        })
        .keyboard_focus(cx)
        .on_click(cx.listener(move |this, _, window, cx| this.select_section(section, window, cx)))
        .child(icon(section.icon()).size(px(15.)).flex_none())
        .child(
            div()
                .text_px(13.)
                .font_weight(FontWeight::MEDIUM)
                .min_w_0()
                .truncate()
                .child(label),
        )
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

    div()
        .flex()
        .flex_col()
        .w(px(196.))
        .h_full()
        .pt(px(TITLEBAR_HEIGHT))
        .px(px(10.))
        .pb_3()
        .flex_shrink_0()
        .bg(p.shell)
        .when(crate::i18n::is_rtl(), |el| el.border_l_1())
        .when(!crate::i18n::is_rtl(), |el| el.border_r_1())
        .border_color(p.line)
        .child(div().mx_2().mt_3().mb_4().child(logo(80., cx)))
        .child(div().flex().flex_col().gap(px(1.)).children(primary))
        .child(
            div()
                .mt_auto()
                .flex()
                .flex_col()
                .gap(px(1.))
                .pt_2()
                .border_t_1()
                .border_color(p.line)
                .children(secondary)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .px_2()
                        .pt_2()
                        .text_px(12.)
                        .line_height(px(16.))
                        .text_color(p.mid_gray)
                        .child(main.model_selector.clone())
                        .child(
                            div()
                                .text_color(p.mid_gray.a(0.7))
                                .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                        ),
                ),
        )
}
