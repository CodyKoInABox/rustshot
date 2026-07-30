# Rustshot manual smoke matrix

Run this checklist on Windows 10 and Windows 11 before publishing a release. Desktop capture,
global hotkeys, clipboard interoperability, and printer drivers require an interactive session and
cannot be validated reliably by headless CI.

## Startup and configuration

- Start `rustshot.exe`; verify there is one tray icon and no console or taskbar window.
- Open settings from the tray, change both shortcuts, and reload. Verify old shortcuts stop working.
- Try duplicate or already-reserved shortcuts. Verify Rustshot reports the error and keeps the
  previous pair active.
- Verify JPEG qualities `1` and `100`, and PNG compression values `fast`, `default`, and `best`.

## Immediate capture

- Trigger full-screen autosave for `cursor_monitor` and `virtual_desktop`.
- Verify the file has the expected dimensions, format, extension, and nonzero contents.
- Repeat with `include_cursor = true` and `false`, keeping the pointer over a static background.
- Trigger two captures in the same millisecond range and verify neither file is overwritten.
- Attempt to quit while a large capture is encoding; verify Rustshot waits for the operation.

## Region editor

- Drag in all four directions, including to the monitor's right and bottom edges.
- On a high-density display, verify the toolbar remains comfortably sized.
- Exercise pen, highlighter, line, arrow, rectangle, colors, widths, clear, undo, and redo.
- Change tools during a held drag. Export during a held drag. Verify the original stroke is retained.
- Cancel Save As, cancel Print, and provoke a failed copy/save. Verify annotations remain and a
  second attempt exports the same pixels.

## Outputs

- Save as PNG and JPEG, confirm overwrite in the dialog, and type a mismatched extension. Verify the
  encoder-selected extension and decoded file contents.
- Copy, then paste into Paint and an Office application. Verify dimensions, colors, and alpha.
- Open Print, cancel, retry, and print to both Microsoft Print to PDF and one physical driver. Verify
  aspect-fit placement and no cropped edges.

