//! Embedded assets served to GPUI: Lucide icons, the logo and release-note
//! images. Paths match the files under `assets/` (`icons/check.svg`), and
//! release-note images keep their `/release-notes/...` URL paths.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let bytes = ASSETS
            .iter()
            .chain(crate::ui::whats_new::RELEASE_NOTE_IMAGES)
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes));
        Ok(bytes)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(ASSETS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::new_static(name))
            .collect())
    }
}
