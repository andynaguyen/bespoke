mod models;
mod permissions;

pub use models::ModelOnboarding;
pub use permissions::AccessibilityOnboarding;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnboardingPreview {
    Accessibility,
    Model,
}

fn top_padding() -> gpui::Pixels {
    gpui::px(22.5 + crate::ui::shell::TITLEBAR_HEIGHT)
}
