mod coach;
mod models;
mod permissions;
pub mod tour;

pub use coach::KeycapCoach;
pub use models::ModelOnboarding;
pub use permissions::AccessibilityOnboarding;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnboardingPreview {
    Accessibility,
    Model,
    Coach,
}

fn top_padding() -> gpui::Pixels {
    gpui::px(22.5 + crate::ui::shell::TITLEBAR_HEIGHT)
}
