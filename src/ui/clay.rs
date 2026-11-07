//! GPUI's shadows have no inset form, so clay is layered over a solid base:
//! a gradient that lightens the top and darkens the bottom, a light rim
//! along the top and left edges, a dark rim along the bottom and right
//! edges, and a soft specular spot near the top left. Outside sit a
//! soft drop shadow and a tight contact shadow. Every layer's color is the
//! base mixed toward white or black, so clay follows its color in both
//! themes.

use gpui::{
    BoxShadow, Hsla, Pixels, SharedString, div, linear_color_stop, linear_gradient, point,
    prelude::*, px,
};

use super::theme::{Alpha, mix};

const GRADIENT_ANGLE: f32 = 172.;

const MUTED: f32 = 0.45;

pub trait Clay: Styled + ParentElement + Sized {
    fn clay(self, color: Hsla, size: f32, radius: Pixels, dark: bool) -> Self {
        layers(self, color, size, radius, dark, 1.)
    }

    fn muted_clay(self, color: Hsla, size: f32, radius: Pixels, dark: bool) -> Self {
        layers(self, color, size, radius, dark, MUTED)
    }
}

impl<T: Styled + ParentElement + Sized> Clay for T {}

fn layers<E: Styled + ParentElement>(
    el: E,
    color: Hsla,
    size: f32,
    radius: Pixels,
    dark: bool,
    strength: f32,
) -> E {
    let white = gpui::white();
    let black = gpui::black();
    let light = mix(color, white, 0.3);
    let shade = mix(color, black, 0.45);
    let drop = mix(color, black, 0.6);
    let layer = || div().absolute().inset_0().rounded(radius);
    // GPUI places absolute children in their parent, so the layers need
    // no `relative()` here, and an absolute caller stays absolute.
    let el = el
        .rounded(radius)
        .bg(color)
        .shadow(vec![
            BoxShadow {
                color: drop.a(strength * if dark { 0.6 } else { 0.32 }),
                offset: point(px(size * 0.05 * strength), px(size * 0.14 * strength)),
                blur_radius: px(size * 0.3 * strength),
                spread_radius: px(-size * 0.04),
            },
            BoxShadow {
                color: drop.a(if dark { 0.5 } else { 0.22 }),
                offset: point(px(0.), px(1.)),
                blur_radius: px(1.),
                spread_radius: px(-0.5),
            },
        ])
        .child(layer().bg(linear_gradient(
            GRADIENT_ANGLE,
            linear_color_stop(light.a(0.7 * strength), 0.),
            linear_color_stop(light.a(0.), 0.55),
        )))
        .child(layer().bg(linear_gradient(
            GRADIENT_ANGLE,
            linear_color_stop(shade.a(0.), 0.5),
            linear_color_stop(shade.a(0.4 * strength), 1.),
        )))
        .child(
            layer()
                .border_t_1()
                .border_l_1()
                .border_color(mix(color, white, 0.5).a(0.6 * strength)),
        )
        .child(
            layer()
                .border_b_1()
                .border_r_1()
                .border_color(shade.a(0.25 * strength)),
        );
    if strength < 1. {
        return el;
    }
    el.child(
        div()
            .absolute()
            .left(px(size * 0.2))
            .top(px(size * 0.14))
            .w(px(size * 0.32))
            .h(px(size * 0.2))
            .rounded_full()
            .shadow(vec![BoxShadow {
                color: mix(color, white, 0.65).a(0.7),
                offset: point(px(0.), px(0.)),
                blur_radius: px(size * 0.14),
                spread_radius: px(0.),
            }]),
    )
}

pub fn keycap(
    label: impl Into<SharedString>,
    height: f32,
    color: Hsla,
    text: Hsla,
    dark: bool,
) -> impl IntoElement {
    div()
        .flex_none()
        .h(px(height))
        .min_w(px(height))
        .px(px(height * 0.3))
        .clay(color, height, px(height * 0.28), dark)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(height * 0.6))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(text)
        .whitespace_nowrap()
        .child(label.into())
}
