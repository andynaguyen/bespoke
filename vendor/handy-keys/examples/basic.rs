use handy_keys::{Hotkey, HotkeyManager, HotkeyState, Key, Modifiers, Result};
use std::io::Write;

fn log(msg: &str) {
    let log_path = std::env::temp_dir().join("keyboard-test.log");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .unwrap();
    writeln!(file, "{}", msg).unwrap();
    println!("{}", msg);
}

fn main() -> Result<()> {
    log("=== Starting keyboard test ===");
    #[cfg(target_os = "macos")]
    {
        use handy_keys::{check_accessibility, open_accessibility_settings};

        let has_access = check_accessibility();
        log(&format!("Accessibility permission check: {}", has_access));

        if !has_access {
            log("Accessibility permission not granted!");
            log("Opening System Settings...");
            if let Err(e) = open_accessibility_settings() {
                log(&format!("Failed to open settings: {}", e));
            }
            log("Please grant permission and restart.");
            std::process::exit(1);
        }
    }

    log("Creating hotkey manager...");
    let manager = match HotkeyManager::new() {
        Ok(m) => {
            log("Hotkey manager created successfully");
            m
        }
        Err(e) => {
            log(&format!("Failed to create hotkey manager: {}", e));
            return Err(e);
        }
    };

    let hotkey1 = Hotkey::new(Modifiers::CMD | Modifiers::SHIFT, Key::K)?;
    let id1 = manager.register(hotkey1)?;
    log(&format!("Registered: {} (id: {:?})", hotkey1, id1));

    let hotkey2 = Hotkey::new(Modifiers::CTRL | Modifiers::OPT, Key::Space)?;
    let id2 = manager.register(hotkey2)?;
    log(&format!("Registered: {} (id: {:?})", hotkey2, id2));

    let hotkey3 = Hotkey::new(Modifiers::CMD | Modifiers::SHIFT, None)?;
    let id3 = manager.register(hotkey3)?;
    log(&format!("Registered: {} (id: {:?})", hotkey3, id3));

    let hotkey4 = Hotkey::new(Modifiers::FN, None)?;
    let id4 = manager.register(hotkey4)?;
    log(&format!("Registered: {} (id: {:?})", hotkey4, id4));

    let hotkey5: Hotkey = "Ctrl+Alt+Delete".parse()?;
    let id5 = manager.register(hotkey5)?;
    log(&format!("Registered (parsed): {} (id: {:?})", hotkey5, id5));

    let hotkey6: Hotkey = "CmdRight+L".parse()?;
    let id6 = manager.register(hotkey6)?;
    log(&format!(
        "Registered (side-specific): {} (id: {:?})",
        hotkey6, id6
    ));

    log("Listening for hotkeys... Press Ctrl+C to exit.");

    while let Ok(event) = manager.recv() {
        let state_str = match event.state {
            HotkeyState::Pressed => "PRESSED",
            HotkeyState::Released => "RELEASED",
        };
        if let Some(hotkey) = manager.get_hotkey(event.id) {
            log(&format!("[{}] {} (id: {:?})", state_str, hotkey, event.id));
        } else {
            log(&format!("[{}] {:?}", state_str, event.id));
        }
    }

    Ok(())
}
