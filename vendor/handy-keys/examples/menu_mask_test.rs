use handy_keys::{Hotkey, HotkeyManager, Key, Modifiers};

fn main() -> handy_keys::Result<()> {
    let manager = HotkeyManager::new_with_blocking()?;

    let hotkeys = [
        ("Win+Space", Hotkey::new(Modifiers::CMD, Key::Space)?),
        ("Alt+H", Hotkey::new(Modifiers::OPT, Key::H)?),
        (
            "Win+Shift",
            Hotkey::new(Modifiers::CMD | Modifiers::SHIFT, None)?,
        ),
    ];

    println!("menu_mask_test — blocking hotkeys registered:");
    let mut names = std::collections::HashMap::new();
    for (name, hk) in hotkeys {
        let id = manager.register(hk)?;
        names.insert(id, name);
        println!("  {name}");
    }
    println!("Ctrl+C to exit.\n");

    while let Ok(event) = manager.recv() {
        println!("{:?} {}", event.state, names[&event.id]);
    }
    Ok(())
}
