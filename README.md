# Rustshot

Rustshot is an open-source, Windows-first screenshot tool written in Rust. It is designed to feel
like Lightshot while staying responsive, native, and inexpensive when it is waiting in the tray.

> [!NOTE]
> Rustshot is under active development. The `main` branch currently targets the first usable MVP.

## MVP

- Two independent, system-wide shortcuts:
  - **Full screen** captures and immediately saves a monitor or the virtual desktop.
  - **Region** freezes the screen and lets you drag out the exact area to keep.
- A lightweight region editor with pen, highlighter, line, arrow, and rectangle tools.
- Undo/redo, color, and stroke-width controls.
- Save As, copy to the Windows clipboard, and native printing.
- PNG or JPEG output, including JPEG quality and PNG compression settings.
- A native tray process and TOML configuration—no browser or webview runtime.

The initial defaults are `Ctrl+Shift+F11` for full-screen autosave and `Ctrl+Shift+F10` for region
capture. They avoid taking over the Print Screen key before the user asks Rustshot to do so.

## Build from source

Requirements:

- Windows 10 or newer
- Rust 1.88 or later
- Visual Studio Build Tools with the **Desktop development with C++** workload

```powershell
cargo build --release
.\target\release\rustshot.exe
```

Rustshot creates its settings file on first launch under the current user's application
configuration directory. Choose **Open settings** from the tray menu, edit the TOML file, and then
choose **Reload settings**.

## Region editor

Drag to select a region, then use the toolbar or these keyboard shortcuts:

| Key | Action |
| --- | --- |
| `P`, `H`, `L`, `A`, `R` | Pen, highlighter, line, arrow, rectangle |
| `U` / `Ctrl+Z` | Undo |
| `Y` / `Ctrl+Y` | Redo |
| `X` | Clear annotations |
| `S`, `C`, `O` | Save As, copy, print |
| `Enter` | Copy |
| `Escape` | Cancel the active stroke, then cancel the capture |

## Configuration

```toml
fullscreen_shortcut = "Ctrl+Shift+F11"
region_shortcut = "Ctrl+Shift+F10"
autosave_directory = "C:\\Users\\you\\Pictures\\Rustshot"
image_format = "png"
jpeg_quality = 90
png_compression = "default"
monitor_scope = "cursor_monitor"
include_cursor = true
```

`jpeg_quality` accepts `1..=100`. PNG is lossless, so its setting controls compression speed and
file size rather than visual quality. A shortcut must contain exactly one non-modifier key; Rustshot
reports duplicate shortcuts and shortcuts already reserved by Windows.

`monitor_scope` accepts `cursor_monitor` or `virtual_desktop` and controls the immediate full-screen
workflow. Region capture starts on the monitor under the pointer. `include_cursor` controls whether
the native pointer is included in captured pixels.

Modifiers must come before the key. Accepted modifier names are `Ctrl`/`Control`, `Shift`, `Alt`,
and `Super`. Common key names include `A`–`Z`, `0`–`9`, `F1`–`F24`, `PrintScreen`, `Space`,
`Enter`, and the arrow-key names. For example:

```toml
fullscreen_shortcut = "Ctrl+Alt+PrintScreen"
region_shortcut = "Ctrl+Shift+KeyR"
```

## Design goals

- Do no work while idle beyond waiting for native events.
- Capture before opening the overlay so Rustshot never captures its own UI.
- Cache committed annotations and avoid allocating a new image for idle pointer movement.
- Keep capture, encoding, and printer spooling off the window event loop.
- Keep OS-specific behavior behind a small platform layer.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the module boundaries and development milestones.
Native release checks are listed in [TESTING.md](TESTING.md).

## License

[MIT](LICENSE)
