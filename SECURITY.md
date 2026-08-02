# Security policy

## Reporting a vulnerability

Do not open a public issue for a vulnerability or attach a sensitive screenshot to an issue.
Use GitHub's **Report a vulnerability** private security-advisory form for this repository. Include
the affected Rustshot version, reproduction steps, impact, and the smallest safe proof of concept.

Please allow reasonable time for investigation and a coordinated fix before public disclosure.
Rustshot maintainers will acknowledge a complete report, assess affected versions, and publish an
advisory when user action is required.

## Scope

Security-sensitive areas include screen capture, clipboard ownership, secure redaction, image
encoding, configuration paths, global shortcuts, native Windows handles, release artifacts, and
dependency supply chain behavior.

The project checks `Cargo.lock` against RustSec advisories, restricts dependency sources and
licenses with `cargo-deny`, and reviews the explicitly allowed Windows `unsafe` modules as part of
the release checklist. Automated checks support but do not replace manual review.

Only the latest published Rustshot 1.x release is guaranteed to receive a security fix. The current
unreleased code is supported on a best-effort basis while 1.0 is being prepared.
