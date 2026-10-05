# LaunchPanel

English | [日本語](README.md)

LaunchPanel is a Windows launcher for quickly opening frequently used applications, files, folders, and websites.

Version 2 is a complete rewrite in Rust + Win32 + Direct2D / DirectWrite. It ships as a single executable under 1 MB and needs no runtime or installation.

![LaunchPanel main window with a blurred wallpaper background in two columns](Screenshot1.png)

![Settings window that works entirely with the mouse](Screenshot2.png)

The screenshots show the Japanese UI.

## Features

- Single executable under 1 MB. No .NET or other runtime, no installer, fast startup
- Register files and folders by drag and drop, with icons taken from the targets
- Reorder buttons by dragging (other buttons move smoothly out of the way)
- Borderless window with multiple columns, resizing from every edge and corner, and snapping to whole columns
- Background: image, blurred wallpaper, or acrylic (windows behind show through), with adjustable blur strength and darkness
- Window shadow with adjustable strength
- Customizable button color, transparency, text color, and bold text
- Settings window that works entirely with the mouse (sliders, switches, a color picker, drag and drop)
- Global hotkey (default: `Ctrl + Alt + Shift + M`)
- Double-click empty desktop space to show the window there (experimental)
- Pinning, and automatic hiding when the window loses focus
- Runs in the system tray and prevents multiple instances
- Settings are saved automatically

## Main changes from v1

- The GUI moved from egui (OpenGL) to custom drawing with the built-in Direct2D / DirectWrite, shrinking the executable from about 4.9 MB to under 1 MB.
- The settings file is now stored next to the executable (v1 used the current folder at startup), so the same settings are used however LaunchPanel is started.
- The settings window was redesigned. It has no text or number input fields and works entirely with the mouse.
- Added an item edit window (rename items, and change the path by dropping a file, folder, or browser link).
- Added blurred wallpaper and acrylic backgrounds, the window shadow, and drag-and-drop reordering.

A v1 `LaunchPanel.json` can be read as is. Replace the v1 executable in the same folder to keep your items and settings. v1 ignores the settings added in v2, so you can also switch back.

## Requirements

- Windows 10 / 11
- The acrylic background needs Windows 11 with Windows "Transparency effects" turned on (otherwise the blurred wallpaper is used instead).

## Download

Get `LaunchPanel-v2.0.0.zip` from [Releases](https://github.com/tamarx78-sys/LaunchPanel/releases), put `LaunchPanel.exe` in a writable folder, and run it.

## Build

Install [Rust](https://www.rust-lang.org/tools/install) (stable) and the Visual Studio C++ build tools, then run the following in the repository root.

```powershell
cargo build --release
```

The executable is written to `target/release/LaunchPanel.exe`.

## Usage

1. Drag and drop files or folders onto the window to register them.
2. Click a registered button to open its target. Drag a button to reorder it.
3. Right-click a button to edit, delete, or move it up or down.
4. Use the gear button to change the appearance, background, and hotkey.
5. Drag the empty part of the top bar to move the window. Drag an edge or corner to resize it.
6. Closing the window hides it. Show it again from the system tray or with the hotkey. To quit completely, choose Exit from the tray menu.

Settings are saved as `LaunchPanel.json` in the same folder as the executable. An older `launcher.json` is migrated automatically.

## Notes

- Background images must be smaller than 2000 pixels in both width and height.
- Hotkey changes take effect the next time LaunchPanel starts.
- Put the executable in a writable folder (settings cannot be saved in folders such as `C:\Program Files`).
- Desktop double-click detection uses a low-level mouse hook only while the feature is enabled (input is always passed on, never consumed). Desktop icon positions are read with UI Automation.
- Because the executable is new and unsigned, some antivirus products may report a false positive based on machine-learning detection.
- LaunchPanel currently supports Windows only.

## Development

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Checks that use the real desktop and wallpaper are run manually.

```powershell
cargo test --release -- --ignored --nocapture
```

The product specification is in [docs/SPECIFICATION.md](docs/SPECIFICATION.md) (Japanese).

## License

[MIT License](LICENSE)
