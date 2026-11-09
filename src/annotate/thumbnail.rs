//! Card-sized copies of captures' cover images for the Captures page. GPUI keeps every image it
//! has drawn, as decoded pixels and again as a texture, so a page of full-size keyframes would
//! hold both copies of up to 120 screenshots.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use anyhow::{Context, Result};
use image::ImageFormat;

use crate::annotate::handoff::{Capture, LIBRARY_SIZE};

const FILE: &str = "thumbnail.png";

/// Enough pixels to cover the largest card at 2x: about 231 by 132 points in the 48rem column.
const COVER_WIDTH: u32 = 480;
const COVER_HEIGHT: u32 = 270;

const MAX_WORKERS: usize = 4;

/// The capture's thumbnail in `dir`, if one has been written.
pub fn existing(dir: &Path) -> Option<PathBuf> {
    Some(dir.join(FILE)).filter(|path| path.is_file())
}

/// Writes the thumbnail of a capture that just finished, so its card never waits for one.
pub fn write_finished(dir: &Path) {
    if let Err(error) = Capture::load(dir).and_then(|capture| write_for(&capture)) {
        log::warn!(
            "Couldn't write the thumbnail in {}: {error:#}",
            dir.display()
        );
    }
}

/// Writes the thumbnails the newest captures don't have yet, for captures older than thumbnails.
pub fn fill_recent() {
    fill_missing(&crate::annotate::handoff::recent_captures(LIBRARY_SIZE));
}

pub fn fill_missing(captures: &[Capture]) {
    let missing: Vec<&Capture> = captures
        .iter()
        .filter(|capture| existing(capture.dir()).is_none())
        .collect();
    let workers = std::thread::available_parallelism()
        .map_or(1, |count| count.get())
        .min(MAX_WORKERS);
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while let Some(capture) = missing.get(next.fetch_add(1, Ordering::Relaxed)) {
                    if let Err(error) = write_for(capture) {
                        log::warn!(
                            "Couldn't write the thumbnail in {}: {error:#}",
                            capture.dir().display()
                        );
                    }
                }
            });
        }
    });
}

fn write_for(capture: &Capture) -> Result<()> {
    match capture.cover_image() {
        Some(source) => write(&source, capture.dir()),
        None => Ok(()),
    }
}

fn write(source: &Path, dir: &Path) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    let image = image::open(source)
        .with_context(|| format!("Could not read {}", source.display()))?
        .into_rgba8();
    let (width, height) = cover_size(image.width(), image.height());
    let small = image::imageops::thumbnail(&image, width, height);
    // Encoding first keeps the temp file's life short, so quitting mid-encode leaves nothing.
    let mut png = Vec::new();
    small
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .context("Could not encode the thumbnail")?;
    // Launch and the page can both write one, and a reader must never see half a file.
    let temp = dir.join(format!(
        "{FILE}.{}.tmp",
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temp, &png).with_context(|| format!("Could not write {}", temp.display()))?;
    std::fs::rename(&temp, dir.join(FILE))
        .with_context(|| format!("Could not move {} into place", temp.display()))
}

/// The smallest size that still covers a card, keeping the image's shape. Never enlarges.
fn cover_size(width: u32, height: u32) -> (u32, u32) {
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let scale = (f64::from(COVER_WIDTH) / w)
        .max(f64::from(COVER_HEIGHT) / h)
        .min(1.0);
    (
        ((w * scale).round() as u32).max(1),
        ((h * scale).round() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("annotate-thumbnail-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cover_size_fills_the_card_without_enlarging() {
        assert_eq!(cover_size(2560, 1440), (480, 270));
        assert_eq!(cover_size(2560, 1654), (480, 310));
        assert_eq!(cover_size(1600, 497), (869, 270));
        assert_eq!(cover_size(1000, 2000), (480, 960));
        assert_eq!(cover_size(300, 200), (300, 200));
        assert_eq!(cover_size(0, 0), (1, 1));
    }

    #[test]
    fn write_leaves_only_a_small_png() {
        let dir = temp_dir("write");
        let source = dir.join("frame.png");
        image::RgbaImage::new(2560, 1440).save(&source).unwrap();

        write(&source, &dir).unwrap();

        let thumbnail = existing(&dir).unwrap();
        assert_eq!(image::image_dimensions(&thumbnail).unwrap(), (480, 270));
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_is_none_before_a_write() {
        let dir = temp_dir("none");
        assert_eq!(existing(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
