# Rustshot architecture

Rustshot's first backend is Windows. The image model, annotation commands, encoder, and
configuration are platform-independent so another capture/clipboard/printing backend can be added
without replacing the editor.

## Runtime flow

```text
Idle
 ├─ full-screen shortcut ─> Capture ─> Encode ─> Atomic autosave ─> Idle
 └─ region shortcut ──────> Capture ─> Select ─> Edit
                                                   ├─ Save As ─> Idle
                                                   ├─ Copy ────> Idle
                                                   ├─ Print ───> Idle
                                                   └─ Cancel ──> Idle
```

The winit event-loop thread owns the tray icon, global-hotkey registration, Save As dialog, and
overlay window. Screen capture, encoding, and printer selection/spooling run on workers. The native
settings dialog has its own UI thread and submits validated candidates back to the event loop;
hotkey replacement and atomic persistence complete there before the dialog reports success. The app
ignores repeated capture shortcuts until the current capture or settings session ends.

## Modules

- `config`: validated, atomically persisted user settings and shortcut parsing.
- `frame`: an owned RGBA image with a physical-pixel desktop origin and safe crop operations.
- `annotation`: vector commands, undo/redo history, and CPU rendering with tiny-skia.
- `encode`: PNG/JPEG encoding and collision-safe, atomic output writes.
- `platform/windows/capture`: monitor and virtual-desktop capture.
- `platform/windows/clipboard`: a lossless Windows DIB clipboard transfer.
- `platform/windows/print`: native printer selection and aspect-fit raster printing.
- `overlay`: the region-selection and annotation interaction state machine.
- `settings`: the native Windows settings dialog, control validation, and folder-picker bridge.
- `app`: the tray lifecycle and orchestration layer.

## Memory model

Rustshot keeps no screen-sized buffer while idle. A region session owns the captured monitor, its
selected crop, a small vector of annotation commands, and the reusable presentation surface. Once
annotations exist, the editor caches their flattened crop and refreshes it only when history
changes. Undo history stores commands rather than full-screen bitmap copies.

## Milestones

1. Tray lifecycle, validated shortcuts, monitor capture, and atomic autosave.
2. Region overlay, Save As, and clipboard.
3. Pen, highlighter, line, arrow, rectangle, undo/redo, color, and width controls.
4. Native printing, settings polish, accessibility, packaging, and performance measurements.
