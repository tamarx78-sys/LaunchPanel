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
- Group buttons visually by giving each one the standard color or one of colors 1–5 (from the right-click menu or the edit window)
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

## Upgrading from v1

v2 reads a v1 `LaunchPanel.json` as is, so in most cases you only need to replace the executable.

1. Quit v1 completely with Exit in the tray menu (v1 and v2 share the single-instance check, so v2 does not start while v1 is running).
2. To be safe, copy the v1 `LaunchPanel.exe` and `LaunchPanel.json` somewhere else.
3. Overwrite the v1 `LaunchPanel.exe` with the v2 `LaunchPanel.exe` and start it.

Items, column width, hotkey, button color, transparency, bold text, and the desktop double-click setting carry over.

Things to check:

- **Settings file location**: v1 saves `LaunchPanel.json` in the current folder at startup, v2 saves it next to the executable. They are the same if you started v1 by double-clicking the executable. If you used a shortcut with a different working folder or started v1 automatically at sign-in, `LaunchPanel.json` may be elsewhere; move it next to the executable.
- **Window size**: v1 settings do not record the window size, so the window opens at the default size the first time. Once you resize it, the size is remembered.
- **Background**: If you used a background image, it is loaded without blur or darkness, so it looks the same. If you used no background image, v2's default (blurred wallpaper) is used. For a plain background, choose None under the background setting for when no image is set.
- **Switching back to v1**: Just restore the v1 executable. v1 ignores the settings added in v2.

## Requirements

- Windows 10 / 11
- The acrylic background needs Windows 11 with Windows "Transparency effects" turned on (otherwise the blurred wallpaper is used instead).

## Download

Get `LaunchPanel-v2.2.0.zip` from [Releases](https://github.com/tamarx78-sys/LaunchPanel/releases), put `LaunchPanel.exe` in a writable folder, and run it.

## Build

Install [Rust](https://www.rust-lang.org/tools/install) (stable) and the Visual Studio C++ build tools, then run the following in the repository root.

```powershell
cargo build --release
```

The executable is written to `target/release/LaunchPanel.exe`.

## Usage

1. Drag and drop files or folders onto the window to register them.
2. Click a registered button to open its target. Drag a button to reorder it.
3. Right-click a button to edit, delete, change its color, or move it up or down.
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
- LaunchPanel records when and why the window is shown or hidden (and whether it could come to the front) in `LaunchPanel.log` next to the executable, to help track down cases where it fails to appear or hide.
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
