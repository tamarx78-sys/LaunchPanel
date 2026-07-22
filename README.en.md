# LaunchPanel

English | [日本語](README.md)

LaunchPanel is a Windows launcher for quickly opening frequently used applications, files, folders, and websites.

![LaunchPanel in a narrow, single-column layout](Screenshot1.png)

![LaunchPanel with a background image and multiple columns](Screenshot2.png)

## Features

- Add files and folders by dragging and dropping them onto the window
- Display icons obtained from registered targets
- Adjust the window width to arrange items in multiple columns
- Customize the background image, button color, transparency, text color, and font weight
- Use a global hotkey (default: `Ctrl + Alt + Shift + M`)
- Pin the window or automatically hide it when it loses focus
- Keep the app available in the system tray and prevent multiple instances
- Save settings automatically

## Requirements

- Windows 10 or Windows 11

## Building

Install [Rust](https://www.rust-lang.org/tools/install) 1.85 or later, then run the following command from the repository root:

```powershell
cargo build --release
```

The executable will be created at `target/release/LaunchPanel.exe`. Place it in any writable folder and run it from there.

## Usage

1. Drag files or folders onto the window to register them.
2. Click a registered button to open its target.
3. Use the gear button to customize the appearance and hotkey.
4. Close the window to hide it, then reopen it from the system tray or with the hotkey.

Settings are saved as `LaunchPanel.json` in the application's current working directory. If a legacy `launcher.json` file is present, LaunchPanel migrates it automatically.

## Notes

- Background images must be smaller than 2000 pixels in both width and height.
- Hotkey changes take effect the next time LaunchPanel starts.
- LaunchPanel currently supports Windows only.

## Development

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

## License

[MIT License](LICENSE)
