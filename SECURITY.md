# RustShot security policy

## Supported versions

Security fixes target the latest available RustShot 1.x release. Development snapshots receive
best-effort fixes but are not supported release artifacts.

## Private vulnerability reporting

Vulnerabilities are reported through GitHub's private
[security advisory form](https://github.com/CodyKoInABox/rustshot/security/advisories/new), not a
public issue. Reports are most useful when they include the affected version, impact, reproduction
steps, and the smallest safe proof of concept.

Complete reports are acknowledged and assessed privately. An advisory is published when user
action is required, with reasonable time allowed for a coordinated fix before disclosure.

## Security boundaries

Security-sensitive components include screen capture, clipboard ownership, secure redaction,
image encoding, configuration paths, global shortcuts, native Windows handles, release artifacts,
and the dependency supply chain.

The project uses locked dependencies, RustSec advisory checks, source and license restrictions,
warnings for application-level unsafe code, and isolated Windows FFI modules. Automated checks
support the native-code review boundary but do not replace it.

`RUSTSEC-2026-0192` marks the transitive `ttf-parser 0.25.1` dependency as unmaintained. The
dependency is reached through `fontdue 0.9.4`, has no reported vulnerability or safe upgrade, and
is explicitly documented in `deny.toml`. The exception is limited to this maintenance advisory.
