# GPUI dependency patch

Source: the published `gpui` 0.2.2 crate from crates.io. `.cargo_vcs_info.json` records its upstream revision, and `LICENSE-APACHE` contains its license. The published crate is the reference because its metadata reports uncommitted changes in the upstream checkout.

All Rust source stays identical to the published crate. `Cargo.toml` has these changes:

- Remove Wayland and X11 from default features. Bespoke enables both in its Linux dependency declaration. macOS uses the existing Metal renderer.
- Make the macOS Naga build dependency optional. `build.rs` uses Naga only when the Blade renderer is enabled on macOS.
- Enable Naga through `macos-blade`, preserving that opt-in renderer.
- Select the raster codecs exposed by GPUI's `ImageFormat` enum, while retaining Rayon. This omits codecs such as the unused AVIF encoder and OpenEXR. Bespoke's captures and MCP thumbnails use PNG. SVG support remains in GPUI's separate SVG renderer.

The upstream per-crate Cargo.lock and Cargo installation marker are omitted. Bespoke uses the root Cargo.lock. When upgrading GPUI, compare these manifest changes with upstream and remove the patch if upstream has resolved the dependency selection.
