mod button;
mod dialog;
mod dropdown;
mod focus;
mod icon;
mod misc;
mod segmented;
mod setting;
mod slider;
mod text_field;
mod toggle;

pub use button::{Button, ButtonSize, ButtonVariant};
pub use dialog::{DIALOG_CONTEXT, Dialog, DialogFrame, DialogMaterial};
pub use dropdown::{Dropdown, DropdownOption};
pub use focus::init as focus_init;
pub use focus::{
    FocusVisible, KeyboardFocus, focus_handle_for, keyboard_modality, set_keyboard_modality,
};
pub use icon::icon;
pub use misc::*;
pub use segmented::SegmentedControl;
pub use setting::{SettingContainer, SettingLayout, SettingsGroup};
pub use slider::Slider;
pub use text_field::{TextArea, TextField};
pub use toggle::ToggleSwitchRow;

use gpui::{Styled, px, relative, rems};

/// `dir="rtl"` flips every flex row in the React app, and its logical
/// classes (`ms-*`, `ps-*`, `text-end`, `start-*`) follow the direction.
/// GPUI has no writing direction, so these helpers pick the physical side.
pub trait RowDirection: Styled + Sized {
    fn row_dir(self) -> Self {
        if crate::i18n::is_rtl() {
            self.flex_row_reverse()
        } else {
            self
        }
    }

    fn ms(self, length: impl Into<gpui::Length> + Clone) -> Self {
        if crate::i18n::is_rtl() {
            self.mr(length)
        } else {
            self.ml(length)
        }
    }

    fn me(self, length: impl Into<gpui::Length> + Clone) -> Self {
        if crate::i18n::is_rtl() {
            self.ml(length)
        } else {
            self.mr(length)
        }
    }

    fn ps(self, length: impl Into<gpui::DefiniteLength> + Clone) -> Self {
        if crate::i18n::is_rtl() {
            self.pr(length)
        } else {
            self.pl(length)
        }
    }

    fn pe(self, length: impl Into<gpui::DefiniteLength> + Clone) -> Self {
        if crate::i18n::is_rtl() {
            self.pl(length)
        } else {
            self.pr(length)
        }
    }

    fn inset_end(self, length: impl Into<gpui::Length> + Clone) -> Self {
        if crate::i18n::is_rtl() {
            self.left(length)
        } else {
            self.right(length)
        }
    }

    /// Tailwind's `justify-end` (`flex-end`). GPUI's `justify_end` packs at
    /// the physical right even in a reversed row; this follows the row's
    /// direction, so a mirrored row packs at the left.
    fn justify_flex_end(mut self) -> Self {
        self.style().justify_content = Some(gpui::JustifyContent::FlexEnd);
        self
    }
}

impl<T: Styled> RowDirection for T {}

/// Tailwind's text sizes, which set a line height along with the font size.
/// GPUI's `text_sm()` and friends set only the size.
pub trait Typography: Styled + Sized {
    fn xs_text(self) -> Self {
        self.text_size(rems(0.75)).line_height(rems(1.))
    }
    fn sm_text(self) -> Self {
        self.text_size(rems(0.875)).line_height(rems(1.25))
    }
    fn base_text(self) -> Self {
        self.text_size(rems(1.)).line_height(rems(1.5))
    }
    fn lg_text(self) -> Self {
        self.text_size(rems(1.125)).line_height(rems(1.75))
    }
    fn xl_text(self) -> Self {
        self.text_size(rems(1.25)).line_height(rems(1.75))
    }
    fn leading_snug(self) -> Self {
        self.line_height(relative(1.375))
    }
    fn leading_relaxed(self) -> Self {
        self.line_height(relative(1.625))
    }
    fn text_px(self, size: f32) -> Self {
        self.text_size(px(size))
    }
}

impl<T: Styled> Typography for T {}

/// Overlays a caller's styles on a component's defaults. Plain `refined`
/// replaces the whole text style (GPUI's `Style::text` is not refined field
/// by field), which would drop the component's font size when the caller
/// only sets a color.
pub fn merge_styles(
    base: gpui::StyleRefinement,
    overlay: gpui::StyleRefinement,
) -> gpui::StyleRefinement {
    use gpui::Refineable;
    let text = match (base.text.clone(), overlay.text.clone()) {
        (Some(mut base_text), Some(overlay_text)) => {
            base_text.refine(&overlay_text);
            Some(base_text)
        }
        (base_text, overlay_text) => overlay_text.or(base_text),
    };
    let mut merged = base.refined(overlay);
    merged.text = text;
    merged
}
