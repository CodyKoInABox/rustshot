#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "windows")]
mod app;
#[cfg(target_os = "windows")]
mod diagnostics;
#[cfg(target_os = "windows")]
mod overlay;
#[cfg(target_os = "windows")]
mod settings;
#[cfg(target_os = "windows")]
mod single_instance;

#[cfg(target_os = "windows")]
fn main() {
    diagnostics::initialize();
    install_panic_dialog();
    let _single_instance = match single_instance::SingleInstance::acquire() {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Info)
                .set_title("Rustshot is already running")
                .set_description(
                    "Rustshot is already running in this Windows session. Use its tray icon or configured shortcuts.",
                )
                .show();
            return;
        }
        Err(error) => {
            diagnostics::record_error("Single-instance check failed", &error.to_string());
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Rustshot could not start")
                .set_description(error.to_string())
                .show();
            std::process::exit(1);
        }
    };
    if let Err(error) = app::run() {
        diagnostics::record_error("Rustshot could not start", &error.to_string());
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
        diagnostics::record_error("Unexpected panic", &panic.to_string());
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
