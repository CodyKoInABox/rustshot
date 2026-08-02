# Rustshot support policy

## Supported platform

Rustshot 1.0 targets 64-bit Windows (`x86_64-pc-windows-msvc`) on Windows 10 and Windows 11.
Other architectures, Windows Server, compatibility layers, and non-Windows operating systems are
not part of the 1.0 support promise.

The Cargo package is an application implementation detail and is marked `publish = false`.
Rustshot does not currently promise a stable third-party Rust library API.

## Configuration compatibility

Rustshot 1.0 writes configuration schema `1`. Configuration files created before schema versioning
are treated as schema 1. The application preserves schema-1 settings across patch and minor
releases. A future incompatible schema must include an explicit migration before Rustshot changes
the stored version.

When a newer, unsupported schema is found, Rustshot refuses to overwrite it and starts with safe
defaults after showing an error. Back up `config.toml` before downgrading between major versions.

## Diagnostics and bug reports

Choose **Create diagnostic report** from the tray menu. Rustshot creates
`rustshot-diagnostics.txt` beside its configuration and opens that folder. The report contains the
application version, target architecture, relevant settings, paths, and the last 40 diagnostic log
lines. Review paths and shortcuts in the report before sharing it.

Rustshot keeps a local `rustshot.log` in the same directory. The log is rotated at 1 MiB, one old
log is retained, and screenshots and clipboard contents are never logged.

When reporting a bug, include:

- exact reproduction steps and the expected result;
- Windows 10 or 11 version and display/DPI arrangement;
- whether the problem affects full-screen capture, region capture, or both;
- the generated diagnostic report; and
- a sample image only if it contains no private information.

## Privacy

Rustshot performs capture, annotation, encoding, and diagnostics locally. It has no telemetry or
network upload feature. Automatic copy writes the selected image to the Windows clipboard, and
automatic save writes it to the configured folder.

## Support lifecycle

Until 1.0 is published, the `main` branch is a release candidate and may still change. After 1.0,
the latest 1.x release receives bug and security fixes. Older 1.x releases may be asked to upgrade
before a report is investigated.
