# Rustshot manual smoke matrix

Run this checklist on Windows 10 and Windows 11 before publishing a release. Desktop capture,
global hotkeys, clipboard interoperability, and printer drivers require an interactive session and
cannot be validated reliably by headless CI.

## Startup and configuration

- Start `rustshot.exe`; verify there is one tray icon and no console or taskbar window.
- Open **Settings...** and verify the saved shortcuts, autosave folder, image options, capture
  scope, cursor inclusion, automatic copy, and automatic save values. Launch once with
  `rustshot.exe --settings` and verify the same window.
- Open Settings again while it is visible. Verify Rustshot focuses the existing window and capture
  shortcuts do nothing until it closes.
- Change a field and choose **Cancel** or press Escape. Verify active and persisted settings remain
  unchanged. Verify **Restore defaults** changes only the draft until saved.
- Use **Browse...** with a folder containing spaces and non-ASCII characters. Save, restart, and
  verify every field round-trips.
- Record modifier-only keys, Tab, Shift+Tab, Windows-key chords, `Ctrl+Shift+Backspace`, and
  `Ctrl+Shift+Alt+Backspace`. Verify navigation still works and every held modifier is preserved.
- Save both shortcuts. Verify old shortcuts stop working and new shortcuts work without restart.
- Try invalid, duplicate, and reserved shortcuts. Verify the draft remains open and the previous
  shortcut pair remains active.
- Toggle PNG/JPEG and verify only the relevant quality control is enabled. Check JPEG qualities 1
  and 100, all PNG compression choices, and rejection of 0, 101, and blank quality values.
- Move the settings window between displays with different scale factors and operate every control
  using only the keyboard.

## Immediate capture and automatic region actions

- Trigger full-screen autosave for `cursor_monitor` and `virtual_desktop`, with cursor inclusion on
  and off. Verify dimensions, format, extension, cursor pixels, and collision-safe filenames.
- Enable region auto-copy only. Select a region, verify the editor is skipped, then paste into Paint.
- Enable region autosave only. Select a region and verify one file appears in the configured folder.
- Enable both automatic options. Verify one selection is both copied and saved.
- Disable both options and verify region selection opens the editor normally.
- Attempt to quit while a large capture is encoding; verify Rustshot waits for the operation.

## Region selection and editor

- Drag a selection in all four directions and to the monitor's right and bottom edges.
- Move the selection and resize it from every corner and edge handle before editing.
- On high-density and narrow displays, verify both toolbar rows, hover tooltips, and the bottom
  shortcut hint remain readable and reachable.
- Exercise pen, highlighter, line, arrow, rectangle, ellipse, text, multiline text, callout, secure
  redaction, pixelation, eraser, crop, eyedropper, palette color, custom color, and every width.
- With Select, hit objects in overlapping areas, move them, resize from all eight handles, recolor,
  restyle, and delete. Verify undo/redo returns each object and style exactly.
- Crop after adding annotations. Verify objects rebase to the new origin and exports match the crop.
- Export a solid redaction and inspect the pixels in an image editor; verify the covered area is
  opaque black rather than translucent or recoverable. Repeat for pixelation.
- Change tools during a held drag and export during a held drag. Verify the original stroke remains.
- Cancel Save As, cancel Print, and provoke a failed copy/save. Verify annotations remain and a
  second attempt exports the same pixels.
- End a session after changing tool, color, and width. Open another region editor and verify those
  three choices are restored.

## Outputs and performance

- In Save As, choose PNG, `.jpg`, and `.jpeg` regardless of the configured default. Decode each
  file and verify its actual format matches the selected extension.
- Confirm overwrite in the dialog and enter an unsupported/no extension. Verify Rustshot applies
  the configured default extension without format mismatch.
- Copy and paste into Paint and an Office application. Verify dimensions and colors.
- Open Print, cancel, retry, and print to Microsoft Print to PDF and a physical driver. Verify
  aspect-fit placement and no cropped edges.
- Measure the tray process after startup and several idle minutes; verify it performs no periodic
  capture/render work and retains no screen-sized buffer.
- During an object drag, watch private memory across repeated pointer moves. Verify it remains
  bounded and drops back after the editor closes.

## Automated release gate

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
```
