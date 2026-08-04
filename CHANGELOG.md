# Changelog

All notable user-visible changes to RustShot are recorded here. RustShot follows semantic
versioning for the desktop application and its persisted configuration, not for the unpublished
internal Rust library crate.

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

[1.0.0]: https://github.com/CodyKoInABox/rustshot/releases/tag/v1.0.0
