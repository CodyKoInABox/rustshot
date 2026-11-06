# Changelog

All notable user-visible changes to RustShot are recorded here. RustShot follows semantic
versioning for the desktop application and its persisted configuration, not for the unpublished
internal Rust library crate.

## [1.0.1] - 2026-11-06

### Fixed

- Screenshots copied to the clipboard or saved as PNG could appear transparent or broken when the
  cursor was not composited into the capture, which is the case when "include cursor" is off
  ([#5](https://github.com/CodyKoInABox/rustshot/issues/5)). Captures and clipboard images are now
  always opaque.
- An unreadable `config.toml`, such as one written by a newer schema, is no longer overwritten when
  editor preferences change. The first explicit Settings save copies the original file to
  `config.toml.bak` before repairing it.
- The screenshot folder opens through `%SystemRoot%\explorer.exe` instead of a bare command name
  resolved through the search path.
- README shortcut table: `D` selects the eraser tool, and `Delete` removes the selected object.

## [1.0.0] - 2026-08-03

### Added

- Full-screen and region capture workflows for 64-bit Windows.
- Vector annotation, secure redaction, pixelation, cropping, clipboard export, and printing.
- Configurable global shortcuts, image encoding, automatic copy, and automatic save.
- Single-instance enforcement for each Windows login session.
- Bounded local diagnostic logging and an on-demand support report.
- A versioned configuration schema and explicit compatibility policy.

- Locked-dependency CI, dependency advisory and license checks, native integration coverage, and
  automated portable Windows release packaging.

[1.0.1]: https://github.com/CodyKoInABox/rustshot/releases/tag/v1.0.1
[1.0.0]: https://github.com/CodyKoInABox/rustshot/releases/tag/v1.0.0
