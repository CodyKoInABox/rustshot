# Security and dependency review

Last reviewed: 2026-08-02

## Automated results

`cargo deny check` passes for the 64-bit Windows dependency graph:

- no known RustSec vulnerability advisory affects the locked graph;
- dependencies resolve only from the crates.io registry;
- every encountered dependency license is in the reviewed permissive/MPL allowlist; and
- wildcard dependency requirements are denied.

Duplicate transitive versions are warnings rather than failures because the native UI crates
currently require different compatible generations of `windows-sys`, `syn`, and TOML support
crates. Review this list when direct dependencies are upgraded.

## Accepted maintenance advisory

`RUSTSEC-2026-0192` reports that `ttf-parser 0.25.1` is unmaintained. It is used transitively by
`fontdue 0.9.4` for text rendering. The advisory describes maintenance status, not a known
vulnerability, and reports no safe upgrade.

The advisory is explicitly ignored in `deny.toml` so that the reason is visible in every scan.
Before publishing 1.0, reassess whether `fontdue` has moved to a maintained parser or whether
Rustshot should move its font rasterization to a maintained alternative such as the one referenced
by the advisory. Remove the exception as soon as the dependency path is replaced.

## Native unsafe-code review boundary

Rustshot's crate-level lint warns on unsafe code. Exceptions are confined to native Windows FFI in
settings, capture, clipboard, printing, atomic file replacement, the color picker, the
single-instance handle, and the native integration test. Each block documents handle ownership,
pointer lifetime, buffer bounds, or the API invariant it relies on.

Before a public release, manually re-review changes in those modules and run Clippy, unit tests,
native integration tests, and the Windows smoke matrix. Automated linting is not a memory-safety
proof for FFI code.
