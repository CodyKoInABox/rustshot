# Contributing to RustShot

Bug fixes, documentation improvements, performance work, and focused feature contributions are
welcome. Large UI changes, new platform backends, and changes to persisted configuration are best
discussed in a GitHub issue before implementation.

## Development environment

RustShot targets 64-bit Windows 10 and Windows 11. Development requires Rust 1.88 or newer and the
Visual Studio C++ build tools.

The standard validation suite is:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
```

Pull requests are expected to describe user-visible behavior, include appropriate tests, and note
any Windows configurations exercised manually. Commits and issue attachments must not contain
private screenshots, unreviewed diagnostic data, signing credentials, or private keys.

Security vulnerabilities follow the private reporting process in [SECURITY.md](SECURITY.md).
