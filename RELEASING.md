# Preparing and publishing a Rustshot release

Rustshot releases are portable 64-bit Windows ZIP archives. Pushing a semantic-version tag creates
a **draft** GitHub release; it does not publish the release automatically.

## Before creating a tag

1. Confirm `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, and the Windows manifest use the intended
   version. Keep the README status accurate for the release stage.
2. Review dependency changes, `SECURITY-AUDIT.md`, and the reason for every advisory exception,
   then run `cargo deny check`.
3. Close every running `rustshot.exe`. Windows locks a running executable, so Cargo cannot replace
   it during the release build.
4. Run the automated release gate:

   ```powershell
   cargo fmt --all -- --check
   cargo clippy --locked --all-targets -- -D warnings
   cargo test --locked --all-targets
   cargo build --release --locked --target x86_64-pc-windows-msvc
   ```

   A release gate is simply the exact set of checks that must all pass on the commit being tagged.
   Using `--locked` makes Cargo use the reviewed `Cargo.lock` instead of resolving newer packages.

5. On an unlocked interactive Windows desktop, run the opt-in native tests:

   ```powershell
   cargo test --locked --test windows_native -- --ignored --test-threads=1
   ```

   The clipboard test replaces the current clipboard image. Native printing and complete UI flows
   remain in `TESTING.md` because they require a person to operate Windows dialogs and hardware.

6. Complete the applicable Windows 10 and Windows 11 checks in `TESTING.md` and inspect a generated
   diagnostic report.

## Branding and signing work for the repository owner

Before the public 1.0 release:

1. Create `resources/rustshot.ico` containing at least 16, 24, 32, 48, 64, 128, and 256 pixel
   variants. Add `resource.set_icon("resources/rustshot.ico")` in `build.rs` and verify Explorer,
   the task switcher, and both light and dark taskbars.
2. Choose the final publisher/company name and add it to the Windows version resource. Keep the
   product name, file description, copyright, and version consistent with Cargo metadata.
3. Obtain an Authenticode code-signing identity trusted by Windows. Current certificates normally
   keep the private key in approved hardware or a managed signing service.
4. Sign the final executable after `cargo build` and before creating the ZIP. With a locally
   available Windows SDK and certificate, the shape of the command is:

   ```powershell
   signtool sign /fd SHA256 /td SHA256 /tr <CA-timestamp-url> /a rustshot.exe
   signtool verify /pa /v rustshot.exe
   ```

5. Never commit a PFX file, private key, PIN, or password. Put signing credentials in a protected
   GitHub `release` environment, require approval for that environment, and insert the provider's
   signing step in `.github/workflows/release.yml` before **Package portable Windows build**.

The certificate authority or managed signing provider supplies the exact timestamp URL and
authentication flags.

## Create and inspect the draft

1. Commit a clean release candidate and push it.
2. Create and push the tag, for example `v1.0.0`.
3. The release workflow verifies the tag against Cargo metadata, runs the gate, generates
   third-party license notices, creates a ZIP and SHA-256 file, and opens a draft GitHub release.
4. Download the draft assets on a clean Windows 10 or 11 machine. Verify the checksum, signature,
   startup, tray icon, capture, save, copy, and uninstall-by-deletion behavior.
5. Edit the generated notes, attach any final screenshots, and publish the draft manually.

If verification fails, delete the draft and tag, fix the issue on a new commit, and create the tag
again only while the release has never been publicly announced. Never silently move a published
tag; publish a patch version instead.
