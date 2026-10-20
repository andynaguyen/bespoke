//! System clipboard access through `arboard`, with the method names of
//! tauri-plugin-clipboard-manager so the paste paths read as before.
//!
//! One `arboard::Clipboard` lives for the whole process: on X11 the owning
//! instance must stay alive for other apps to read what Handy wrote.

use std::borrow::Cow;
use std::sync::Mutex;

/// Raw RGBA clipboard image, saved and restored around a paste.
pub type ClipboardImage = arboard::ImageData<'static>;

static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

fn with_clipboard<R>(
    f: impl FnOnce(&mut arboard::Clipboard) -> Result<R, arboard::Error>,
) -> Result<R, String> {
    let mut guard = CLIPBOARD.lock().map_err(|e| e.to_string())?;
    if guard.is_none() {
        *guard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    f(guard.as_mut().expect("clipboard initialized above")).map_err(|e| e.to_string())
}

#[derive(Clone, Copy)]
pub struct Clipboard;

impl Clipboard {
    pub fn read_text(&self) -> Result<String, String> {
        with_clipboard(|c| c.get_text())
    }

    pub fn write_text(&self, text: impl AsRef<str>) -> Result<(), String> {
        let text = text.as_ref().to_string();
        with_clipboard(move |c| c.set_text(text))
    }

    pub fn read_image(&self) -> Result<ClipboardImage, String> {
        with_clipboard(|c| c.get_image()).map(|image| image.to_owned_img())
    }

    pub fn write_image(&self, image: &ClipboardImage) -> Result<(), String> {
        let image = arboard::ImageData {
            width: image.width,
            height: image.height,
            bytes: Cow::Owned(image.bytes.to_vec()),
        };
        with_clipboard(move |c| c.set_image(image))
    }

    pub fn clear(&self) -> Result<(), String> {
        with_clipboard(|c| c.clear())
    }
}
