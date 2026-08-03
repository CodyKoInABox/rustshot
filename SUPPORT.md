# RustShot support

## Platform support

RustShot 1.x supports 64-bit Windows 10 and Windows 11. Other architectures, Windows Server,
compatibility layers, and non-Windows operating systems are outside the supported platform set.

RustShot is distributed as a desktop application. Its internal Rust modules are not a stable
third-party library API.

## Help and bug reports

Questions and reproducible bugs are tracked in
[GitHub Issues](https://github.com/CodyKoInABox/rustshot/issues). Useful reports include:

- the RustShot version;
- Windows version and display scaling;
- the expected and actual behavior;
- reliable reproduction steps; and
- a reviewed diagnostic report when relevant.

The **Create diagnostic report** tray command writes `rustshot-diagnostics.txt` beside the local
configuration. It contains version, architecture, relevant settings, paths, and the last 40 log
lines. The related `rustshot.log` is bounded to 1 MiB with one rotated copy.

Diagnostic text can contain local paths and configured shortcuts. Screenshots and clipboard image
contents are never logged.

## Configuration compatibility

RustShot 1.x uses configuration schema `1`. Unversioned pre-1.0 configuration is interpreted as
schema 1. Unsupported newer schemas are preserved without being overwritten, and the application
starts with safe defaults after reporting the incompatibility.

## Privacy

Capture, annotation, encoding, printing, configuration, and diagnostics run locally. RustShot has
no telemetry, analytics, account system, cloud storage, or network upload feature.
