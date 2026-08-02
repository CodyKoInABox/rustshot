# Contributing to Rustshot

Rustshot currently accepts focused bug fixes and release-hardening improvements for the Windows
1.0 milestone. Discuss broad feature or platform work in an issue before investing in an
implementation.

## Development checks

Use Rust 1.88 or newer on 64-bit Windows with the Visual Studio C++ build tools, then run:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

Changes to native capture or clipboard behavior should also run the opt-in tests documented in
`RELEASING.md`. Pull requests should explain user-visible behavior, tests performed, and any manual
Windows coverage.

Do not include captured private information, diagnostic reports without reviewing their paths, or
signing credentials in commits or issues. Report vulnerabilities using `SECURITY.md`.
