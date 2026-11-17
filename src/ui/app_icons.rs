use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyElement, App, FontWeight, Image, ImageFormat, Pixels, div, img, prelude::*};

use crate::ui::theme::{Alpha, palette};

const LARGEST_ICON_POINTS: f64 = 36.;
const RETINA_SCALE: f64 = 2.;
const ICON_PIXELS: f64 = LARGEST_ICON_POINTS * RETINA_SCALE;

#[derive(Clone)]
enum Icon {
    Loading,
    Missing,
    Loaded(Arc<Image>),
}

thread_local! {
    static ICONS: RefCell<HashMap<String, Icon>> = RefCell::new(HashMap::new());
}

fn icon_or_start_load(bundle_id: &str, cx: &App) -> Icon {
    if let Some(icon) = ICONS.with(|icons| icons.borrow().get(bundle_id).cloned()) {
        return icon;
    }
    ICONS.with(|icons| {
        icons
            .borrow_mut()
            .insert(bundle_id.to_string(), Icon::Loading)
    });
    let id = bundle_id.to_string();
    let png = cx.background_executor().spawn({
        let id = id.clone();
        async move { crate::apps::icon_png(&id, ICON_PIXELS) }
    });
    cx.spawn(async move |cx| {
        let icon = match png.await {
            Some(png) => Icon::Loaded(Arc::new(Image::from_bytes(ImageFormat::Png, png))),
            None => Icon::Missing,
        };
        ICONS.with(|icons| icons.borrow_mut().insert(id, icon));
        let _ = cx.update(|cx| cx.refresh_windows());
    })
    .detach();
    Icon::Loading
}

/// The app's icon at `size`. While it loads, an empty box holds its place. With no bundle ID,
/// or when macOS has no icon, a rounded tile shows the first letter of `label`, at the visible
/// size of an icon, which leaves about a tenth of its box clear on each side.
pub fn app_icon(bundle_id: Option<&str>, label: &str, size: Pixels, cx: &App) -> AnyElement {
    let icon = match bundle_id.filter(|id| !id.is_empty()) {
        Some(id) => icon_or_start_load(id, cx),
        None => Icon::Missing,
    };
    match icon {
        Icon::Loaded(image) => img(image).size(size).flex_none().into_any_element(),
        Icon::Loading => div().size(size).flex_none().into_any_element(),
        Icon::Missing => {
            let p = palette(cx);
            let tile = size * 0.8;
            div()
                .size(size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(tile)
                        .flex_none()
                        .rounded(size * 0.18)
                        .bg(p.mid_gray.a(0.25))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(size * 0.5)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text.a(0.7))
                        .child(
                            label
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase().to_string())
                                .unwrap_or_default(),
                        ),
                )
                .into_any_element()
        }
    }
}
