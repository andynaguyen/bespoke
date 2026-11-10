use std::borrow::Cow;

pub type ClipboardImage = arboard::ImageData<'static>;

fn with_clipboard<R>(
    f: impl FnOnce(&mut arboard::Clipboard) -> Result<R, arboard::Error>,
) -> Result<R, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    f(&mut clipboard).map_err(|e| e.to_string())
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

    pub fn snapshot(&self) -> ClipboardSnapshot {
        if let Some(text) = self.read_text().ok().filter(|t| !t.is_empty()) {
            ClipboardSnapshot::Text(text)
        } else if let Ok(image) = self.read_image() {
            ClipboardSnapshot::Image(image)
        } else {
            ClipboardSnapshot::Empty
        }
    }

    pub fn restore(&self, snapshot: &ClipboardSnapshot) {
        let _ = match snapshot {
            ClipboardSnapshot::Text(text) => self.write_text(text),
            ClipboardSnapshot::Image(image) => self.write_image(image),
            ClipboardSnapshot::Empty => self.clear(),
        };
    }
}

pub enum ClipboardSnapshot {
    Text(String),
    Image(ClipboardImage),
    Empty,
}
