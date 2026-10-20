//! Colors and type scale, ported from `src/styles/theme.css` and `App.css`.
//!
//! The React app sized everything in rems against a 15px root, so windows set
//! their rem size to [`REM`] and components use GPUI's Tailwind-style helpers
//! (`p_4`, `rounded_lg`, `text_sm`) to get the same pixel values.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{px, rgb, rgba, App, Global, Hsla, Pixels, Rgba, Window, WindowAppearance};

use crate::settings::Theme as ThemeSetting;

/// Root font size of the React app (`:root { font-size: 15px }`).
pub const REM: Pixels = px(15.);

/// The serif stack used for page titles (`--font-serif`).
pub const SERIF_FONT: &str = if cfg!(target_os = "macos") {
    ".New York"
} else {
    "Georgia"
};
pub const SERIF_FALLBACKS: &[&str] = &["Iowan Old Style", "Georgia", "Times New Roman"];
pub const MONO_FONT: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(target_os = "windows") {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// The active palette. Every color in the UI comes from here.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub is_dark: bool,
    pub text: Hsla,
    pub background: Hsla,
    pub logo_primary: Hsla,
    pub shell: Hsla,
    pub card: Hsla,
    pub accent: Hsla,
    pub accent_contrast: Hsla,
    pub data: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub mid_gray: Hsla,
    pub background_ui: Hsla,
    /// `--color-log-surface`: the background mixed toward black.
    pub log_surface: Hsla,
}

impl Palette {
    pub fn light() -> Self {
        let background = hex(0xfcfbf9);
        Self {
            is_dark: false,
            text: hex(0x1c1a17),
            background,
            logo_primary: hex(0xfaa2ca),
            shell: hex(0xf3f1ed),
            card: hex(0xf4f2ee),
            accent: hex(0x1f1d1a),
            accent_contrast: hex(0xffffff),
            data: hex(0x2f6360),
            warning: hex(0xd97706),
            error: hex(0xdc2626),
            mid_gray: hex(0x8a857c),
            background_ui: hex(0xda5893),
            log_surface: mix_black(background, 0.06),
        }
    }

    pub fn dark() -> Self {
        let background = hex(0x252422);
        Self {
            is_dark: true,
            text: hex(0xf3f1ec),
            background,
            logo_primary: hex(0xf28cbb),
            shell: hex(0x1c1b19),
            card: hex(0x2d2c29),
            accent: hex(0xece9e3),
            accent_contrast: hex(0x1c1b19),
            data: hex(0x6fb3aa),
            warning: hex(0xfbbf24),
            error: hex(0xf87171),
            mid_gray: hex(0x8a857c),
            background_ui: hex(0xda5893),
            log_surface: mix_black(background, 0.15),
        }
    }
}

fn mix_black(color: Hsla, amount: f32) -> Hsla {
    let rgba: Rgba = color.into();
    Rgba {
        r: rgba.r * (1. - amount),
        g: rgba.g * (1. - amount),
        b: rgba.b * (1. - amount),
        a: rgba.a,
    }
    .into()
}

/// Tailwind palette colors the React app used directly.
pub mod tw {
    use super::hex;
    use gpui::Hsla;

    pub fn red_300() -> Hsla {
        hex(0xffa2a2)
    }
    pub fn red_400() -> Hsla {
        hex(0xff6467)
    }
    pub fn red_500() -> Hsla {
        hex(0xfb2c36)
    }
    pub fn red_600() -> Hsla {
        hex(0xe7000b)
    }
    pub fn red_700() -> Hsla {
        hex(0xc10007)
    }
    pub fn yellow_400() -> Hsla {
        hex(0xfdc700)
    }
    pub fn green_400() -> Hsla {
        hex(0x05df72)
    }
    pub fn emerald_400() -> Hsla {
        hex(0x00d492)
    }
    pub fn emerald_500() -> Hsla {
        hex(0x00bc7d)
    }
    pub fn emerald_600() -> Hsla {
        hex(0x009966)
    }
    pub fn orange_400() -> Hsla {
        hex(0xff8904)
    }
    pub fn sky_400() -> Hsla {
        hex(0x00bcff)
    }
    pub fn sky_600() -> Hsla {
        hex(0x0084d1)
    }
    pub fn amber_300() -> Hsla {
        hex(0xffd236)
    }
    pub fn amber_400() -> Hsla {
        hex(0xffb900)
    }
    pub fn amber_600() -> Hsla {
        hex(0xe17100)
    }
    pub fn amber_700() -> Hsla {
        hex(0xbb4d00)
    }
    pub fn red_50() -> Hsla {
        hex(0xfef2f2)
    }
    pub fn red_200() -> Hsla {
        hex(0xffc9c9)
    }
}

/// `color/NN` in Tailwind: the same color at `alpha` opacity.
pub trait Alpha {
    fn a(self, alpha: f32) -> Hsla;
}

impl Alpha for Hsla {
    fn a(self, alpha: f32) -> Hsla {
        Hsla {
            a: self.a * alpha,
            ..self
        }
    }
}

/// The current palette plus the appearance inputs it was derived from.
pub struct BespokeTheme {
    pub palette: Palette,
    pub setting: ThemeSetting,
    pub system: WindowAppearance,
}

impl Global for BespokeTheme {}

static EFFECTIVE_DARK: AtomicBool = AtomicBool::new(false);

/// Whether the UI is currently dark. Readable from any thread (the tray
/// picks icons with it).
pub fn effective_is_dark() -> bool {
    EFFECTIVE_DARK.load(Ordering::Relaxed)
}

fn appearance_is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

pub fn palette(cx: &App) -> Palette {
    cx.global::<BespokeTheme>().palette
}

/// Makes the page-title serif available. WebKit's `ui-serif` is the hidden
/// system font New York, which GPUI cannot find by name, so its file is
/// registered directly. Other platforms fall back to Georgia.
fn load_serif_font(cx: &mut App) {
    #[cfg(target_os = "macos")]
    match std::fs::read("/System/Library/Fonts/NewYork.ttf") {
        Ok(bytes) => {
            if let Err(e) = cx
                .text_system()
                .add_fonts(vec![std::borrow::Cow::Owned(bytes)])
            {
                log::warn!("Failed to load New York: {e}");
            }
        }
        Err(e) => log::warn!("New York font not found: {e}"),
    }
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

pub fn init(setting: ThemeSetting, cx: &mut App) {
    load_serif_font(cx);
    let system = cx.window_appearance();
    cx.set_global(BespokeTheme {
        palette: Palette::light(),
        setting,
        system,
    });
    recompute(cx);
    apply_native_appearance(setting, cx);
}

/// Applies the appearance setting (System/Light/Dark) and repaints.
pub fn apply_theme_setting(setting: ThemeSetting, cx: &mut App) {
    if !cx.has_global::<BespokeTheme>() {
        return;
    }
    cx.global_mut::<BespokeTheme>().setting = setting;
    apply_native_appearance(setting, cx);
    recompute(cx);
    crate::tray::refresh_tray_icon(crate::app_handle::AppHandle::global());
}

/// Matches the Windows title bar to the effective theme; see
/// `platform::set_title_bar_dark`. Deferred, since the caller may be
/// updating the main window.
pub fn sync_title_bar(cx: &mut App) {
    if !cfg!(target_os = "windows") {
        return;
    }
    cx.defer(|cx| {
        let dark = effective_is_dark();
        if let Some(handle) = crate::ui::main_window_handle(cx) {
            let _ = handle.update(cx, |_, window, _| {
                crate::ui::platform::set_title_bar_dark(window, dark)
            });
        }
    });
}

/// Call when the OS appearance changes.
pub fn system_appearance_changed(window: &Window, cx: &mut App) {
    let appearance = window.appearance();
    if cx.global::<BespokeTheme>().system == appearance {
        return;
    }
    cx.global_mut::<BespokeTheme>().system = appearance;
    recompute(cx);
    log::info!("Theme changed to: {:?}", appearance);
    crate::tray::refresh_tray_icon(crate::app_handle::AppHandle::global());
}

fn recompute(cx: &mut App) {
    let theme = cx.global::<BespokeTheme>();
    let dark = match theme.setting {
        ThemeSetting::Light => false,
        ThemeSetting::Dark => true,
        ThemeSetting::System => appearance_is_dark(theme.system),
    };
    EFFECTIVE_DARK.store(dark, Ordering::Relaxed);
    let palette = if dark {
        Palette::dark()
    } else {
        Palette::light()
    };
    cx.global_mut::<BespokeTheme>().palette = palette;
    sync_component_theme(&palette, cx);
    cx.refresh_windows();
    sync_title_bar(cx);
}

/// Points gpui-component's theme (used by text inputs) at the Bespoke palette.
fn sync_component_theme(palette: &Palette, cx: &mut App) {
    if !cx.has_global::<gpui_component::Theme>() {
        return;
    }
    let theme = gpui_component::Theme::global_mut(cx);
    theme.foreground = palette.text;
    theme.background = palette.background;
    theme.muted_foreground = palette.text.a(0.4);
    theme.caret = palette.text;
    theme.selection = palette.accent.a(if palette.is_dark { 0.3 } else { 0.2 });
    theme.border = palette.mid_gray.a(0.25);
    theme.input = palette.mid_gray.a(0.25);
    theme.ring = palette.accent.a(0.1);
    theme.scrollbar = gpui::transparent_black();
    theme.scrollbar_thumb = palette.text.a(0.15);
    theme.scrollbar_thumb_hover = palette.text.a(0.3);
    theme.popover = palette.background;
    theme.popover_foreground = palette.text;
    theme.font_size = px(15.);
    theme.radius = px(7.5);
}

/// macOS: sets `NSApp.appearance` so native chrome (title bar, traffic
/// lights) follows the in-app theme. `System` clears the override.
fn apply_native_appearance(setting: ThemeSetting, cx: &App) {
    #[cfg(target_os = "macos")]
    {
        let dark = match setting {
            ThemeSetting::System => None,
            ThemeSetting::Light => Some(false),
            ThemeSetting::Dark => Some(true),
        };
        // Deferred: AppKit reports the appearance change back to GPUI
        // synchronously, which GPUI cannot take while the app is in use.
        cx.foreground_executor()
            .spawn(async move { crate::ui::platform::set_app_appearance(dark) })
            .detach();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (setting, cx);
}

/// Shadow helper matching Tailwind's `shadow-lg`.
pub fn shadow_lg() -> Vec<gpui::BoxShadow> {
    vec![shadow(0., 10., 15., -3., 0.1), shadow(0., 4., 6., -4., 0.1)]
}

/// Tailwind's `shadow-xl`.
pub fn shadow_xl() -> Vec<gpui::BoxShadow> {
    vec![
        shadow(0., 20., 25., -5., 0.1),
        shadow(0., 8., 10., -6., 0.1),
    ]
}

/// Tailwind's `shadow-sm`.
pub fn shadow_sm() -> Vec<gpui::BoxShadow> {
    vec![shadow(0., 1., 3., 0., 0.1), shadow(0., 1., 2., -1., 0.1)]
}

/// A black drop shadow at `alpha` opacity, in CSS px.
pub fn shadow(x: f32, y: f32, blur: f32, spread: f32, alpha: f32) -> gpui::BoxShadow {
    gpui::BoxShadow {
        color: rgba((alpha * 255.) as u32).into(),
        offset: gpui::point(px(x), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
    }
}
