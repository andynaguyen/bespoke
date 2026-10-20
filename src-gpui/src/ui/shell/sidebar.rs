//! The sidebar (`Sidebar.tsx`): logo, section links, and the footer with the
//! model status, update status and version.

use gpui::{
    div, img, prelude::*, px, rems, App, Context, FontWeight, ImageSource, Resource, SharedString,
    Window,
};

use super::{MainWindow, Section};
use crate::i18n::t;
use crate::ui::components::{icon, KeyboardFocus, RowDirection, Typography};
use crate::ui::stores::SettingsStore;
use crate::ui::theme::{palette, Alpha};

/// The Handy wordmark, `width` pixels wide.
pub fn logo(width: f32, cx: &App) -> impl IntoElement {
    let file = if palette(cx).is_dark {
        "handy-logo-dark.svg"
    } else {
        "handy-logo-light.svg"
    };
    img(ImageSource::Resource(Resource::Embedded(file.into())))
        .w(px(width))
        .h(px(width * 328. / 930.))
        .flex_none()
}

fn nav_item(section: Section, active: bool, cx: &mut Context<MainWindow>) -> impl IntoElement {
    let p = palette(cx);
    let label = t(section.label_key());
    div()
        .id(SharedString::from(format!("nav-{:?}", section)))
        .flex()
        .row_dir()
        .gap_3()
        .items_center()
        .px_3()
        .py_2()
        .w_full()
        .rounded_lg()
        .cursor_pointer()
        .when(active, |el| el.bg(p.mid_gray.a(0.15)).text_color(p.text))
        .when(!active, |el| {
            el.text_color(p.text.a(0.75))
                .hover(|s| s.bg(p.mid_gray.a(0.1)).text_color(p.text))
        })
        .keyboard_focus(cx)
        .on_click(cx.listener(move |this, _, window, cx| this.select_section(section, window, cx)))
        .child(icon(section.icon()).size(px(20.)).flex_none())
        .child(
            div()
                .text_px(15.)
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
        .w(rems(13.))
        .h_full()
        .px_2()
        .pb_3()
        .flex_shrink_0()
        .child(div().mx_3().mt_4().mb_6().child(logo(96., cx)))
        .child(div().flex().flex_col().gap_1().children(primary))
        .child(
            div()
                .mt_auto()
                .flex()
                .flex_col()
                .gap_1()
                .pt_3()
                .border_t_1()
                .border_color(p.mid_gray.a(0.15))
                .children(secondary)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(rems(0.375))
                        .px_3()
                        .pt_3()
                        .xs_text()
                        .text_color(p.text.a(0.6))
                        .child(main.model_selector.clone())
                        .child(
                            div()
                                .text_color(p.text.a(0.4))
                                .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                        ),
                ),
        )
}
