# Rustshot

Rustshot is an open-source, Windows-first screenshot tool written in Rust. It is designed to feel
like Lightshot while staying responsive, native, and inexpensive when it is waiting in the tray.

> [!NOTE]
> Rustshot is under active development. The `main` branch currently targets the first usable MVP.

## Features

- Two independent system-wide shortcuts:
  - **Full screen** captures and immediately saves a monitor or the virtual desktop.
  - **Region** freezes the screen and lets you drag out the exact area to keep.
- Resizable region selections with eight handles and move support before export.
- A vector editor with pen, highlighter, line, arrow, rectangle, ellipse, text box, callout,
  secure redaction, pixelation, object eraser, crop, and eyedropper tools.
- Select, move, resize, recolor, and restyle existing objects, with allocation-conscious undo/redo.
- Custom Windows color picker, quick palette, stroke widths, and remembered last-used tool/style.
- Save As, lossless Windows clipboard copy, and native printing.
- PNG or JPEG chosen in each Save As dialog, plus configurable JPEG quality and PNG compression.
- Optional automatic copy and/or autosave immediately after selecting a region.
- A native settings window and tray process with no browser or webview runtime.

The initial defaults are `Ctrl+Shift+F11` for full-screen autosave and `Ctrl+Shift+F10` for region
capture. They avoid taking over the Print Screen key before the user asks Rustshot to do so.

## Build from source

Requirements:

- Windows 10 or newer
- Rust 1.88 or later
- Visual Studio Build Tools with the **Desktop development with C++** workload

```powershell
cargo build --release --locked
.\target\release\rustshot.exe
```

Choose **Settings...** from the tray menu to change Rustshot's behavior. **Save** validates the
whole form, activates both shortcuts, and persists the changes immediately; if Windows rejects a
shortcut or the file cannot be saved, the previous settings remain active. **Cancel** leaves the
current settings unchanged. You can also launch `rustshot.exe --settings` to open the same window.

To change a shortcut, focus its field, hold any supported modifier chord, press one non-modifier
key, and release it. Chords can contain more than two modifiers and can use keys such as Backspace;
for example, `Ctrl+Shift+Backspace` and `Ctrl+Shift+Alt+F10` are valid.

## Region editor

Drag to select a region. The eight boundary handles resize it and dragging inside moves it. Every
toolbar button has a hover tooltip, and the most useful shortcuts remain visible at the bottom of
the overlay.

| Key | Action |
| --- | --- |
| `V` | Select, move, resize, or restyle an object |
| `P`, `H`, `L`, `A` | Pen, highlighter, line, arrow |
| `R`, `E` | Rectangle, ellipse |
| `T`, `Q` | Text box, callout; `Shift+Enter` inserts a line break |
| `B`, `M` | Secure black redaction, pixelation |
| `D` / `Delete` | Delete the object under the pointer / selected object |
| `G` | Crop or resize the captured region |
| `I` | Pick a color from captured pixels |
| `K` | Open the native custom color picker |
| `U` / `Ctrl+Z` | Undo |
| `Y` / `Ctrl+Y` | Redo |
| `X` | Clear annotations |
| `S`, `C`, `O` | Save As, copy, print |
| `Enter` | Copy |
| `Escape` | Cancel the active edit, then cancel the capture |

Secure redaction is flattened as solid black pixels in the exported image; it does not merely draw
a translucent overlay. Pixelation is also destructively flattened into the output pixels.

## Region completion options

The settings window can automatically copy a selected region, automatically save it, or do both.
Enabling either option completes the capture immediately after selection and skips the editor.
Autosave uses the configured folder and image format. With both options enabled, the same capture
is copied and saved.

## Configuration

The settings window manages every supported workflow option. Rustshot stores the values as TOML
under the current user's application configuration directory. The editor preference fields are
updated automatically when an editor session ends.

```toml
fullscreen_shortcut = "Ctrl+Shift+F11"
region_shortcut = "Ctrl+Shift+F10"
autosave_directory = "C:\\Users\\you\\Pictures\\Rustshot"
image_format = "png"
jpeg_quality = 90
png_compression = "default"
monitor_scope = "cursor_monitor"
include_cursor = true
region_auto_copy = false
region_autosave = false
last_editor_tool = "pen"
editor_color = { red = 255, green = 64, blue = 64 }
editor_stroke_width = 1
```

`jpeg_quality` accepts `1..=100`. PNG is lossless, so its setting controls compression speed and
file size rather than visual quality. Save As uses the chosen `.png`, `.jpg`, or `.jpeg` extension
to select the encoder; the configured format is the dialog default and the autosave format.

A shortcut must contain exactly one non-modifier key. Accepted modifier names are
`Ctrl`/`Control`, `Shift`, `Alt`, and `Super`; common keys include `A`-`Z`, `0`-`9`, `F1`-`F24`,
`PrintScreen`, `Backspace`, `Space`, `Enter`, and the arrow keys. Rustshot reports duplicate
shortcuts and shortcuts already reserved by Windows.

`monitor_scope` accepts `cursor_monitor` or `virtual_desktop` for the immediate full-screen
workflow. Region capture starts on the monitor under the pointer. `include_cursor` controls whether
the native pointer is included in captured pixels.

## Design goals

- Do no work while idle beyond waiting for native events.
- Capture before opening the overlay so Rustshot never captures its own UI.
- Cache committed annotations and avoid allocating a new image for idle pointer movement.
- Store inverse edit commands instead of full bitmap snapshots for undo/redo.
- Keep capture, encoding, and printer spooling off the window event loop.
- Keep OS-specific behavior behind a small platform layer.

See [ARCHITECTURE.md](ARCHITECTURE.md) for module boundaries and [TESTING.md](TESTING.md) for the
native release checklist.

## License

[MIT](LICENSE)
