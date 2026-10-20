//! First-run screens (`components/onboarding`): the permission requests,
//! then the model picker. The debug page can preview both without touching
//! permissions or the active model.

mod models;
mod permissions;

pub use models::ModelOnboarding;
pub use permissions::AccessibilityOnboarding;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnboardingPreview {
    Accessibility,
    Model,
}

/// `p-6 pt-[calc(1.5rem+var(--titlebar-height))]`: the top padding clears
/// the macOS title bar.
fn top_padding() -> gpui::Pixels {
    gpui::px(22.5 + crate::ui::shell::TITLEBAR_HEIGHT)
}
