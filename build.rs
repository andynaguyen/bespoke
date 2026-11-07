use std::path::{Path, PathBuf};

fn main() {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    build_apple_intelligence_bridge();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_annotate_helper();
        link_swift_runtime();
    }

    embed_locales();
    embed_release_notes();
    embed_assets();
}

/// Builds Annotate's Swift helper (`swift/annotate-helper/`), which transcribes with Apple
/// Speech, reads frames from the recording and shows the permission panel, into
/// `target/<profile>/scrawl-helper`. `scripts/bundle-app.sh` copies it into the app. The file
/// keeps its name and bundle id so the macOS permissions it was granted carry over.
fn build_annotate_helper() {
    use std::process::Command;

    let helper_dir = manifest_dir().join("swift/annotate-helper");
    println!("cargo:rerun-if-changed={}", helper_dir.display());
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&helper_dir)
        .expect("swift/annotate-helper is missing")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "swift"))
        .collect();
    sources.sort();
    for source in &sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    let plist = helper_dir.join("Info.plist");
    println!("cargo:rerun-if-changed={}", plist.display());
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("Unexpected OUT_DIR layout");
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x86_64",
        _ => "arm64",
    };
    let built = out_dir.join("scrawl-helper");
    let status = Command::new("xcrun")
        .args(["swiftc", "-O", "-swift-version", "5", "-target"])
        .arg(format!("{arch}-apple-macos26.0"))
        .args(&sources)
        .arg("-o")
        .arg(&built)
        .args([
            "-Xlinker",
            "-sectcreate",
            "-Xlinker",
            "__TEXT",
            "-Xlinker",
            "__info_plist",
            "-Xlinker",
        ])
        .arg(&plist)
        .status()
        .expect("xcrun swiftc is required to build Annotate's helper");
    assert!(
        status.success(),
        "Annotate's helper failed to compile (needs the macOS 26 SDK)"
    );
    let dest = profile_dir.join("scrawl-helper");
    let _ = std::fs::remove_file(&dest);
    std::fs::copy(&built, dest).expect("Could not place Annotate's helper");
}

/// screencapturekit's Swift bridge needs the Swift runtime, which macOS ships in
/// `/usr/lib/swift`. Debug builds also look in Xcode's toolchain for its back-deployment
/// libraries.
fn link_swift_runtime() {
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    if std::env::var("PROFILE").as_deref() == Ok("debug") {
        link_build_machine_xcode_runtime_in_debug_builds();
    }
}

fn link_build_machine_xcode_runtime_in_debug_builds() {
    use std::process::Command;

    if let Ok(output) = Command::new("xcode-select").arg("-p").output()
        && output.status.success()
        && let Ok(developer_dir) = String::from_utf8(output.stdout)
    {
        for runtime in ["swift-5.5/macosx", "swift/macosx"] {
            let swift_runtime = format!(
                "{}/Toolchains/XcodeDefault.xctoolchain/usr/lib/{runtime}",
                developer_dir.trim()
            );
            println!("cargo:rustc-link-arg=-Wl,-rpath,{swift_runtime}");
        }
    }
}

fn embed_assets() {
    let assets_dir = manifest_dir().join("assets");
    println!("cargo:rerun-if-changed={}", assets_dir.display());

    let mut files = Vec::new();
    collect_files(&assets_dir, &mut files);
    files.sort();

    let mut out = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for (_, path) in &files {
        let rel = path
            .strip_prefix(&assets_dir)
            .expect("asset under assets/")
            .to_string_lossy()
            .replace('\\', "/");
        out.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            rel,
            path.canonicalize().expect("canonical asset path")
        ));
    }
    out.push_str("];\n");
    write_out("assets.rs", &out);
}

fn embed_locales() {
    let locales_dir = manifest_dir().join("locales");
    println!("cargo:rerun-if-changed={}", locales_dir.display());

    let mut entries: Vec<(String, PathBuf)> = std::fs::read_dir(&locales_dir)
        .expect("read locales dir")
        .flatten()
        .filter(|entry| entry.path().join("translation.json").is_file())
        .map(|entry| {
            let code = entry.file_name().to_string_lossy().to_string();
            (code, entry.path().join("translation.json"))
        })
        .collect();
    entries.sort();

    let mut out = String::from("pub static LOCALES: &[(&str, &str)] = &[\n");
    for (code, path) in &entries {
        println!("cargo:rerun-if-changed={}", path.display());
        out.push_str(&format!(
            "    ({:?}, include_str!({:?})),\n",
            code,
            path.canonicalize().expect("canonical locale path")
        ));
    }
    out.push_str("];\n");
    write_out("locales.rs", &out);
}

fn embed_release_notes() {
    let notes_dir = manifest_dir().join("release-notes");
    let images_dir = notes_dir.join("images");
    println!("cargo:rerun-if-changed={}", notes_dir.display());
    println!("cargo:rerun-if-changed={}", images_dir.display());

    let mut notes: Vec<(String, PathBuf)> = std::fs::read_dir(&notes_dir)
        .map(|dir| {
            dir.flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
                .filter_map(|path| {
                    let version = path.file_stem()?.to_string_lossy().to_string();
                    version
                        .split('.')
                        .all(|part| part.parse::<u32>().is_ok())
                        .then_some((version, path))
                })
                .collect()
        })
        .unwrap_or_default();
    notes.sort();

    let mut images: Vec<(String, PathBuf)> = Vec::new();
    collect_files(&images_dir, &mut images);
    images.sort();

    let mut out = String::from("pub static RELEASE_NOTES: &[(&str, &str)] = &[\n");
    for (version, path) in &notes {
        out.push_str(&format!(
            "    ({:?}, include_str!({:?})),\n",
            version,
            path.canonicalize().expect("canonical note path")
        ));
    }
    out.push_str("];\n\npub static RELEASE_NOTE_IMAGES: &[(&str, &[u8])] = &[\n");
    for (_, path) in &images {
        let rel = path
            .strip_prefix(&images_dir)
            .expect("image under release-notes")
            .to_string_lossy()
            .replace('\\', "/");
        out.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            format!("/release-notes/images/{rel}"),
            path.canonicalize().expect("canonical image path")
        ));
    }
    out.push_str("];\n");
    write_out("release_notes.rs", &out);
}

fn collect_files(dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push((path.to_string_lossy().to_string(), path));
        }
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
}

fn write_out(name: &str, contents: &str) {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out_dir.join(name), contents).expect("write generated file");
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn build_apple_intelligence_bridge() {
    use std::env;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const REAL_SWIFT_FILE: &str = "swift/apple_intelligence.swift";
    const STUB_SWIFT_FILE: &str = "swift/apple_intelligence_stub.swift";
    const BRIDGE_HEADER: &str = "swift/apple_intelligence_bridge.h";

    println!("cargo:rerun-if-changed={REAL_SWIFT_FILE}");
    println!("cargo:rerun-if-changed={STUB_SWIFT_FILE}");
    println!("cargo:rerun-if-changed={BRIDGE_HEADER}");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let object_path = out_dir.join("apple_intelligence.o");
    let static_lib_path = out_dir.join("libapple_intelligence.a");

    // SDKROOT/SWIFTC env-var overrides let non-Xcode toolchains (e.g. nixpkgs
    // with apple-sdk_* + standalone swift) bypass xcrun, which is Xcode-only.
    let sdk_path = env::var("SDKROOT").unwrap_or_else(|_| {
        String::from_utf8(
            Command::new("xcrun")
                .args(["--sdk", "macosx", "--show-sdk-path"])
                .output()
                .expect("Failed to locate macOS SDK")
                .stdout,
        )
        .expect("SDK path is not valid UTF-8")
        .trim()
        .to_string()
    });

    let framework_path =
        Path::new(&sdk_path).join("System/Library/Frameworks/FoundationModels.framework");
    let force_stub = env::var("BESPOKE_FORCE_AI_STUB").as_deref() == Ok("1");

    // Auto-detect a Command-Line-Tools-only toolchain. The CLT SDK contains
    // FoundationModels.framework, so the `framework_path.exists()` check alone
    // wrongly selects the real Swift path, which then fails to compile because
    // the CLT `swiftc` has no FoundationModelsMacros plugin (full Xcode only).
    // Detecting this lets a plain `cargo build` / `tauri dev` succeed without the
    // manual flag. Skipped when SWIFTC is overridden: that signals a custom
    // toolchain (e.g. the nixpkgs standalone-swift path supported above) whose
    // capabilities can't be inferred from `xcode-select`.
    let command_line_tools_only = env::var("SWIFTC").is_err() && is_command_line_tools_only();
    if command_line_tools_only && !force_stub {
        println!(
            "cargo:warning=Command Line Tools-only toolchain detected; Apple Intelligence \
             (FoundationModels) needs full Xcode. Falling back to stubs. Install Xcode and run \
             `sudo xcode-select -s /Applications/Xcode.app`, or set BESPOKE_FORCE_AI_STUB=1 to \
             silence this message."
        );
    }

    let has_foundation_models = framework_path.exists() && !force_stub && !command_line_tools_only;

    let source_file = if has_foundation_models {
        println!("cargo:warning=Building with Apple Intelligence support.");
        REAL_SWIFT_FILE
    } else {
        if framework_path.exists() {
            println!("cargo:warning=Building Apple Intelligence with stubs.");
        } else {
            println!("cargo:warning=Apple Intelligence SDK not found. Building with stubs.");
        }
        STUB_SWIFT_FILE
    };

    if !Path::new(source_file).exists() {
        panic!("Source file {} is missing!", source_file);
    }

    let swiftc_path = env::var("SWIFTC").unwrap_or_else(|_| {
        String::from_utf8(
            Command::new("xcrun")
                .args(["--find", "swiftc"])
                .output()
                .expect("Failed to locate swiftc")
                .stdout,
        )
        .expect("swiftc path is not valid UTF-8")
        .trim()
        .to_string()
    });

    let toolchain_swift_lib = Path::new(&swiftc_path)
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("lib/swift/macosx"))
        .expect("Unable to determine Swift toolchain lib directory");
    let sdk_swift_lib = Path::new(&sdk_path).join("usr/lib/swift");

    let status = Command::new(&swiftc_path)
        .args([
            // Without this flag swiftc treats single-file input as script
            // mode and emits its own `_main` symbol into the .o, which can
            // win the link against Rust's main under some linkers (e.g.
            // open-source ld64 used in nixpkgs' Darwin stdenv), producing a
            // binary whose main() is a 5-instruction no-op that returns 0.
            // `-parse-as-library` keeps the compilation in library mode so
            // no `_main` is emitted. See:
            //   https://forums.swift.org/t/main-in-a-single-swift-file/63079
            "-parse-as-library",
            "-target",
            "arm64-apple-macosx11.0",
            "-sdk",
            &sdk_path,
            "-O",
            "-import-objc-header",
            BRIDGE_HEADER,
            "-c",
            source_file,
            "-o",
            object_path
                .to_str()
                .expect("Failed to convert object path to string"),
        ])
        .status()
        .expect("Failed to invoke swiftc for Apple Intelligence bridge");

    if !status.success() {
        panic!("swiftc failed to compile {source_file}");
    }

    let status = Command::new("libtool")
        .args([
            "-static",
            "-o",
            static_lib_path
                .to_str()
                .expect("Failed to convert static lib path to string"),
            object_path
                .to_str()
                .expect("Failed to convert object path to string"),
        ])
        .status()
        .expect("Failed to create static library for Apple Intelligence bridge");

    if !status.success() {
        panic!("libtool failed for Apple Intelligence bridge");
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=apple_intelligence");
    println!(
        "cargo:rustc-link-search=native={}",
        toolchain_swift_lib.display()
    );
    println!("cargo:rustc-link-search=native={}", sdk_swift_lib.display());
    println!("cargo:rustc-link-lib=framework=Foundation");

    if has_foundation_models {
        println!("cargo:rustc-link-arg=-weak_framework");
        println!("cargo:rustc-link-arg=FoundationModels");
    }
}

/// Returns true when the active developer directory is the standalone Command
/// Line Tools rather than a full Xcode install.
///
/// `xcode-select -p` prints the active developer dir; the CLT install resolves
/// to a path ending in `CommandLineTools` (e.g. `/Library/Developer/CommandLineTools`),
/// whereas full Xcode resolves under `Xcode.app`. A CLT-only toolchain ships
/// FoundationModels.framework in its SDK but a `swiftc` without the
/// FoundationModelsMacros plugin, so the Apple Intelligence Swift path cannot
/// compile (issue #1448). On any error we conservatively return false so the
/// existing SDK-presence check decides.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn is_command_line_tools_only() -> bool {
    use std::process::Command;

    Command::new("xcode-select")
        .arg("-p")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|path| path.trim().ends_with("CommandLineTools"))
        .unwrap_or(false)
}
