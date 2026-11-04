#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledApp {
    pub bundle_id: String,
    pub name: String,
}

#[cfg(target_os = "macos")]
pub fn installed_apps() -> Vec<InstalledApp> {
    use objc2_foundation::{NSBundle, NSString};
    use std::path::PathBuf;

    let mut folders: Vec<PathBuf> = [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
        "/System/Applications/Utilities",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = std::env::var_os("HOME") {
        folders.push(PathBuf::from(home).join("Applications"));
    }

    let mut apps: Vec<InstalledApp> = folders
        .iter()
        .filter_map(|folder| std::fs::read_dir(folder).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
        .filter_map(|path| {
            let bundle = NSBundle::bundleWithPath(&NSString::from_str(path.to_str()?))?;
            let bundle_id = bundle.bundleIdentifier()?.to_string();
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some(InstalledApp { bundle_id, name })
        })
        .filter(|app| app.bundle_id != crate::app_handle::APP_IDENTIFIER)
        .collect();
    apps.sort_by_cached_key(|app| app.name.to_lowercase());
    apps.dedup_by(|a, b| a.bundle_id == b.bundle_id);
    apps
}

#[cfg(not(target_os = "macos"))]
pub fn installed_apps() -> Vec<InstalledApp> {
    Vec::new()
}

#[cfg(target_os = "macos")]
pub fn icon_png(bundle_id: &str, pixels: f64) -> Option<Vec<u8>> {
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

    let workspace = NSWorkspace::sharedWorkspace();
    let url = workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
    let path = url.path()?;
    let image = workspace.iconForFile(&path);
    let mut rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(pixels, pixels));
    // SAFETY: `rect` outlives the call, and no context or hints are passed.
    let cg_image = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
    // SAFETY: an empty properties dictionary is valid for PNG.
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(data.to_vec())
}

#[cfg(not(target_os = "macos"))]
pub fn icon_png(_bundle_id: &str, _pixels: f64) -> Option<Vec<u8>> {
    None
}
