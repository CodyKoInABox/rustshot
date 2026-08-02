#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "windows")]
mod app;
#[cfg(target_os = "windows")]
mod overlay;
#[cfg(target_os = "windows")]
mod settings;

#[cfg(target_os = "windows")]
fn main() {
    install_panic_dialog();
    if let Err(error) = app::run() {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Rustshot could not start")
            .set_description(error.to_string())
            .show();
        std::process::exit(1);
    }
}

#[cfg(target_os = "windows")]
fn install_panic_dialog() {
    std::panic::set_hook(Box::new(|panic| {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Rustshot encountered an unexpected error")
            .set_description(panic.to_string())
            .show();
    }));
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Rustshot's first release currently supports Windows only.");
}
