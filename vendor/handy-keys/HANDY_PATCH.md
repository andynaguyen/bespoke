# Mouse buttons 6 through 8

This directory contains handy-keys 0.3.4 from crates.io, licensed under MIT.
The published crate checksum is `1a007b6c921d3273fd88aac45b516eee01e5a9dc2d76bb6f1a981500cc0d818a`.

Local changes add `Mouse6`, `Mouse7`, and `Mouse8` key names, macOS button numbers 5 through 7, and Linux evdev button mappings. Windows low-level mouse hooks only expose the first five buttons, so Handy rejects higher button bindings there.

The patch keeps extended buttons in the existing native recorder and hotkey manager. Remove this vendored copy when an upstream release supplies equivalent support.
