# RustShot architecture

RustShot's first backend is Windows. The image model, annotation commands, encoder, and
configuration are platform-independent so another capture, clipboard, or printing backend can be
added without replacing the editor.

## Runtime flow

```text
Idle
 |-- full-screen shortcut --> Capture --> Encode --> Atomic autosave --> Idle
 `-- region shortcut -------> Capture --> Select
                                          |-- automatic copy/save --> Idle
                                          `-- Edit
                                               |-- Save As --> Idle
                                               |-- Copy ----> Idle
                                               |-- Print ---> Idle
                                               `-- Cancel --> Idle
```

The winit event-loop thread owns the tray icon, global-hotkey registration, Save As dialog, and
overlay window. Screen capture, encoding, and printer selection/spooling run on workers. The native
settings dialog has its own UI thread and submits validated candidates back to the event loop;
hotkey replacement and atomic persistence complete before the dialog reports success. The app
ignores repeated capture shortcuts until the current capture or settings session ends.

A named Windows mutex prevents more than one RustShot tray process from running in the same login
session. The process also maintains a bounded local diagnostic log. On request, the tray generates a
plain-text support report from version, architecture, non-secret settings, paths, and recent errors;
captured pixels and clipboard contents are never logged.

## Modules

- `config`: validated, atomically persisted settings, editor preferences, and shortcut parsing.
- `frame`: an owned RGBA image with a physical-pixel desktop origin and safe crop operations.
- `annotation`: editable vector objects, inverse-command history, hit testing, system-font text,
  secure raster effects, and flattening.
- `encode`: PNG/JPEG encoding and collision-safe, atomic output writes.
- `platform/windows/capture`: monitor and virtual-desktop capture.
- `platform/windows/clipboard`: a lossless Windows DIB clipboard transfer.
- `platform/windows/print`: native printer selection and aspect-fit raster printing.
- `overlay`: region selection, handles, annotation gestures, toolbar, and object manipulation.
- `settings`: native Windows settings dialog, control validation, and folder-picker bridge.
- `app`: tray lifecycle, workers, automatic region actions, preference persistence, and exports.
- `single_instance`: per-session named-mutex ownership for the tray process.
- `diagnostics`: bounded local error logging and on-demand support-report generation.

## Compatibility boundaries

The persisted configuration carries an explicit schema version. Unversioned pre-1.0 files map to
schema 1, while unsupported future schemas are rejected before any overwrite. The desktop workflow
and schema follow the compatibility policy in `SUPPORT.md`.

The library target exists to separate and test internal modules. The Cargo package is not published,
so its Rust module API is not part of the 1.x compatibility promise.

## Annotation and history model

Annotations remain editable vector objects until export. Select-mode hit testing walks them from
front to back, while move, resize, style, add, delete, and clear operations record compact inverse
commands. Undo/redo therefore moves owned objects between history entries instead of cloning the
whole document or storing screen-sized bitmap snapshots.

Flattening preserves object order. Vector runs are rasterized together; text, solid redaction, and
pixelation flush those runs before changing pixels. This makes secure redaction part of the final
pixel data and prevents later export paths from accidentally revealing the covered source.

Crop applies to the original captured monitor once and rebases all remaining objects, avoiding
quality loss from repeatedly resampling an already-cropped bitmap.

## Memory and rendering model

RustShot keeps no screen-sized buffer while idle. A region session owns the captured monitor, its
selected crop, a small vector of objects, and the reusable presentation surface. The editor caches
the committed flattened crop and refreshes it only when history changes.

Object dragging temporarily replaces that cache with one base frame that excludes the manipulated
object plus one small vector preview. It does not retain a third region-sized frame. Pointer moves
mutate the preview object in place, so idle movement and dragging do not clone the document.

System glyph rasterization is cached in a bounded 2,048-entry cache. Capture, encoding, and printer
work stay off the event loop, while the overlay uses a software buffer sized to the active monitor.
