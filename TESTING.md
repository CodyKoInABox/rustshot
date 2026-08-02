# Rustshot manual smoke matrix

Run this checklist on Windows 10 and Windows 11 before publishing a release. Desktop capture,
global hotkeys, clipboard interoperability, and printer drivers require an interactive session and
cannot be validated reliably by headless CI.

## Startup and configuration

- Start `rustshot.exe`; verify there is one tray icon and no console or taskbar window.
- Open **Settings...** from the tray and verify a single native window appears with the saved values
  for both shortcuts, autosave folder, image format, JPEG quality, PNG compression, full-screen
  scope, and cursor inclusion. Launch once with `rustshot.exe --settings` and verify the same window.
- Open Settings a second time while it is already visible. Verify Rustshot focuses the existing
  window instead of creating another one, and verify capture shortcuts do nothing until it closes.
- Change a field and choose **Cancel** or press Escape. Decline and then accept the discard prompt;
  verify the active and persisted settings did not change.
- Choose **Restore defaults**, verify it changes only the draft, then cancel. Repeat and save to
  confirm all defaults are applied.
- Use **Browse...** with a folder containing spaces and non-ASCII characters. Save, restart
  Rustshot, and verify the exact path and every other field round-trip.
- Focus each shortcut field, press a new combination, and verify the displayed value is replaced
  rather than typed character-by-character. Confirm modifier-only presses do not replace it, Tab
  and Shift+Tab still navigate, and combinations using Ctrl, Shift, Alt, and the Windows key record.
  In particular, hold Ctrl+Shift+Backspace and verify all three keys appear in the recorded shortcut.
- Save both shortcuts. Verify the old shortcuts stop working immediately and the new shortcuts work
  without restarting.
- Try invalid, duplicate, and already-reserved shortcuts. Verify the window reports the error,
  retains the draft, and keeps the previous shortcut pair active.
- Select PNG and verify JPEG quality is disabled; select JPEG and verify PNG compression is
  disabled. Verify JPEG qualities `1` and `100`, and all three PNG compression choices.
- Verify invalid JPEG values (`0`, `101`, and blank) are rejected without closing the window.
- Move the settings window between displays with different scale factors and operate every control
  using only Tab, Shift+Tab, arrow keys, Space, Enter, and Escape.

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
