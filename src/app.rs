//! Windows application lifecycle and tray event loop.

use std::{
    path::{Path, PathBuf},
    process::Command,
    thread,
};

use anyhow::{Context, Result};
use chrono::Local;
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use rustshot::{
    config::{config_path, Config, MonitorScope},
    encode::{encode_and_save_atomic, encode_and_save_replace_atomic, EncodeOptions, OutputFormat},
    frame::Frame,
    platform::windows::{
        capture::{
            capture_monitor_at_with_cursor, capture_virtual_desktop_with_cursor,
            current_cursor_position,
        },
        clipboard::copy_to_clipboard_with_owner,
        print::{print_frame, PrintOutcome},
    },
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, MouseButton as TrayMouseButton, MouseButtonState, TrayIcon, TrayIconBuilder,
    TrayIconEvent,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    window::WindowId,
};

use crate::overlay::{Overlay, OverlayOutcome};

const MENU_CAPTURE_REGION: &str = "capture-region";
const MENU_CAPTURE_FULLSCREEN: &str = "capture-fullscreen";
const MENU_OPEN_SCREENSHOTS: &str = "open-screenshots";
const MENU_OPEN_SETTINGS: &str = "open-settings";
const MENU_RELOAD_SETTINGS: &str = "reload-settings";
const MENU_QUIT: &str = "quit";

#[derive(Clone, Copy)]
enum CaptureKind {
    Fullscreen,
    Region,
}

#[derive(Clone, Copy)]
enum SaveOrigin {
    Autosave,
    Editor,
}

enum AppEvent {
    HotKey(GlobalHotKeyEvent),
    Menu(MenuEvent),
    Tray(TrayIconEvent),
    CaptureFinished {
        kind: CaptureKind,
        result: Result<Frame, String>,
    },
    SaveFinished {
        origin: SaveOrigin,
        result: Result<PathBuf, String>,
    },
    PrintFinished {
        result: Result<PrintOutcome, String>,
    },
}

pub fn run() -> Result<()> {
    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("could not create the Windows event loop")?;
    wire_event_sources(&event_loop.create_proxy());

    let config = load_initial_config();
    let mut app = RustshotApp::new(event_loop.create_proxy(), config);
    event_loop
        .run_app(&mut app)
        .context("Rustshot's event loop stopped unexpectedly")
}

fn wire_event_sources(proxy: &EventLoopProxy<AppEvent>) {
    let hotkey_proxy = proxy.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |event| {
        let _ = hotkey_proxy.send_event(AppEvent::HotKey(event));
    }));

    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = menu_proxy.send_event(AppEvent::Menu(event));
    }));

    let tray_proxy = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = tray_proxy.send_event(AppEvent::Tray(event));
    }));
}

fn load_initial_config() -> Config {
    match Config::load() {
        Ok(config) => {
            if config_path().is_ok_and(|path| !path.exists()) {
                if let Err(error) = config.save() {
                    show_error("Could not create settings", &error.to_string());
                }
            }
            config
        }
        Err(error) => {
            show_error(
                "Could not load settings",
                &format!("{error}\n\nRustshot will use safe defaults until settings are reloaded."),
            );
            Config::default()
        }
    }
}

struct RustshotApp {
    proxy: EventLoopProxy<AppEvent>,
    config: Config,
    hotkeys: Option<RegisteredHotkeys>,
    tray: Option<TrayIcon>,
    overlay: Option<Overlay>,
    busy: bool,
}

impl RustshotApp {
    fn new(proxy: EventLoopProxy<AppEvent>, config: Config) -> Self {
        Self {
            proxy,
            config,
            hotkeys: None,
            tray: None,
            overlay: None,
            busy: false,
        }
    }

    fn initialize(&mut self) {
        if self.tray.is_none() {
            match build_tray_icon() {
                Ok(tray) => self.tray = Some(tray),
                Err(error) => show_error("Could not create tray icon", &error.to_string()),
            }
        }
        if self.hotkeys.is_none() {
            match RegisteredHotkeys::new(&self.config) {
                Ok(hotkeys) => self.hotkeys = Some(hotkeys),
                Err(error) => show_error(
                    "Could not register shortcuts",
                    &format!(
                        "{error}\n\nOpen Rustshot settings from the tray and choose different shortcuts."
                    ),
                ),
            }
        }
    }

    fn start_capture(&mut self, kind: CaptureKind) {
        if self.busy || self.overlay.is_some() {
            return;
        }
        let scope = self.config.monitor_scope;
        let sampled_cursor = if matches!(kind, CaptureKind::Region)
            || matches!(scope, MonitorScope::CursorMonitor)
        {
            match current_cursor_position() {
                Ok(position) => Some(position),
                Err(error) => {
                    show_error("Screenshot failed", &error.to_string());
                    return;
                }
            }
        } else {
            None
        };
        self.busy = true;
        let include_cursor = self.config.include_cursor;
        let proxy = self.proxy.clone();
        thread::spawn(move || {
            let result = match kind {
                CaptureKind::Region => {
                    let cursor = sampled_cursor.expect("region capture samples the cursor");
                    capture_monitor_at_with_cursor(cursor.x(), cursor.y(), include_cursor)
                }
                CaptureKind::Fullscreen => match scope {
                    MonitorScope::CursorMonitor => {
                        let cursor =
                            sampled_cursor.expect("cursor-monitor capture samples the cursor");
                        capture_monitor_at_with_cursor(cursor.x(), cursor.y(), include_cursor)
                    }
                    MonitorScope::VirtualDesktop => {
                        capture_virtual_desktop_with_cursor(include_cursor)
                    }
                },
            }
            .map_err(|error| error.to_string());
            let _ = proxy.send_event(AppEvent::CaptureFinished { kind, result });
        });
    }

    fn handle_capture_finished(
        &mut self,
        event_loop: &ActiveEventLoop,
        kind: CaptureKind,
        result: Result<Frame, String>,
    ) {
        let frame = match result {
            Ok(frame) => frame,
            Err(error) => {
                self.busy = false;
                show_error("Screenshot failed", &error);
                return;
            }
        };

        match kind {
            CaptureKind::Region => match Overlay::new(event_loop, frame) {
                Ok(overlay) => self.overlay = Some(overlay),
                Err(error) => {
                    self.busy = false;
                    show_error("Could not open the region editor", &error.to_string());
                }
            },
            CaptureKind::Fullscreen => {
                let destination = next_autosave_path(&self.config);
                self.spawn_save(frame, destination, SaveOrigin::Autosave, false);
            }
        }
    }

    fn spawn_save(
        &self,
        frame: Frame,
        destination: PathBuf,
        origin: SaveOrigin,
        replace_existing: bool,
    ) {
        let proxy = self.proxy.clone();
        let options = EncodeOptions::from(&self.config);
        thread::spawn(move || {
            let save_result = if replace_existing {
                encode_and_save_replace_atomic(&frame, options, &destination)
            } else {
                encode_and_save_atomic(&frame, options, &destination)
            };
            let result = save_result
                .map(|()| destination)
                .map_err(|error| error.to_string());
            let _ = proxy.send_event(AppEvent::SaveFinished { origin, result });
        });
    }

    fn spawn_print(&self, frame: Frame) {
        let proxy = self.proxy.clone();
        thread::spawn(move || {
            let result = print_frame(&frame, None).map_err(|error| error.to_string());
            let _ = proxy.send_event(AppEvent::PrintFinished { result });
        });
    }

    fn save_as(&mut self, frame: Frame) {
        self.set_overlay_visible(false);
        let options = EncodeOptions::from(&self.config);
        let (filter_name, extension) = match options.format {
            OutputFormat::Png => ("PNG image", "png"),
            OutputFormat::Jpeg => ("JPEG image", "jpg"),
        };
        let file_name = format!("{}.{}", screenshot_stem(), extension);
        let selected = FileDialog::new()
            .set_title("Save Rustshot screenshot")
            .set_directory(&self.config.autosave_directory)
            .set_file_name(file_name)
            .add_filter(filter_name, &[extension])
            .save_file();

        let Some(selected) = selected else {
            self.set_overlay_visible(true);
            return;
        };
        let destination = ensure_extension(selected.clone(), extension);
        let replace_existing = destination.exists();
        if destination != selected
            && replace_existing
            && MessageDialog::new()
                .set_level(MessageLevel::Warning)
                .set_title("Replace existing screenshot?")
                .set_description(format!(
                    "Rustshot will encode this image as .{extension}.\n\nReplace `{}`?",
                    destination.display()
                ))
                .set_buttons(MessageButtons::YesNo)
                .show()
                != MessageDialogResult::Yes
        {
            self.set_overlay_visible(true);
            return;
        }
        self.spawn_save(frame, destination, SaveOrigin::Editor, replace_existing);
    }

    fn finish_overlay(&mut self, outcome: OverlayOutcome) {
        match outcome {
            OverlayOutcome::None => {}
            OverlayOutcome::Cancel => {
                self.overlay = None;
                self.busy = false;
            }
            OverlayOutcome::Save(frame) => {
                self.save_as(frame);
            }
            OverlayOutcome::Copy(frame) => {
                self.set_overlay_visible(false);
                let result = self
                    .overlay
                    .as_ref()
                    .context("region editor window no longer exists")
                    .and_then(|overlay| overlay.owner_hwnd())
                    .and_then(|owner| copy_to_clipboard_with_owner(&frame, owner));
                match result {
                    Ok(()) => {
                        self.overlay = None;
                        self.busy = false;
                    }
                    Err(error) => {
                        show_error("Could not copy screenshot", &error.to_string());
                        self.set_overlay_visible(true);
                    }
                }
            }
            OverlayOutcome::Print(frame) => {
                self.set_overlay_visible(false);
                self.spawn_print(frame);
            }
            OverlayOutcome::Error(error) => {
                self.overlay = None;
                self.busy = false;
                show_error("Region editor failed", &error);
            }
        }
    }

    fn set_overlay_visible(&self, visible: bool) {
        if let Some(overlay) = &self.overlay {
            overlay.set_visible(visible);
        }
    }

    fn handle_menu(&mut self, event_loop: &ActiveEventLoop, event: &MenuEvent) {
        match event.id.as_ref() {
            MENU_CAPTURE_REGION => self.start_capture(CaptureKind::Region),
            MENU_CAPTURE_FULLSCREEN => self.start_capture(CaptureKind::Fullscreen),
            MENU_OPEN_SCREENSHOTS => {
                if let Err(error) = open_path(&self.config.autosave_directory) {
                    show_error("Could not open screenshot folder", &error.to_string());
                }
            }
            MENU_OPEN_SETTINGS => {
                if let Err(error) = open_settings(&self.config) {
                    show_error("Could not open settings", &error.to_string());
                }
            }
            MENU_RELOAD_SETTINGS => self.reload_settings(),
            MENU_QUIT => {
                if self.busy {
                    show_error(
                        "Rustshot is busy",
                        "Finish or cancel the current capture, save, or print operation before quitting.",
                    );
                } else {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn reload_settings(&mut self) {
        let new_config = match Config::load() {
            Ok(config) => config,
            Err(error) => {
                show_error("Could not reload settings", &error.to_string());
                return;
            }
        };
        let replacement = if let Some(hotkeys) = &mut self.hotkeys {
            hotkeys.replace(&new_config)
        } else {
            RegisteredHotkeys::new(&new_config).map(|hotkeys| {
                self.hotkeys = Some(hotkeys);
            })
        };
        if let Err(error) = replacement {
            show_error(
                "Could not register new shortcuts",
                &format!("{error}\n\nRustshot tried to restore the previous shortcuts."),
            );
            return;
        }
        self.config = new_config;
    }
}

impl ApplicationHandler<AppEvent> for RustshotApp {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        self.initialize();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::HotKey(event) if event.state == HotKeyState::Pressed => {
                let registered = self.hotkeys.as_ref().filter(|hotkeys| hotkeys.registered);
                let fullscreen_id = registered.map(|hotkeys| hotkeys.fullscreen.id());
                let region_id = registered.map(|hotkeys| hotkeys.region.id());
                if Some(event.id) == fullscreen_id {
                    self.start_capture(CaptureKind::Fullscreen);
                } else if Some(event.id) == region_id {
                    self.start_capture(CaptureKind::Region);
                }
            }
            AppEvent::HotKey(_) => {}
            AppEvent::Menu(event) => self.handle_menu(event_loop, &event),
            AppEvent::Tray(TrayIconEvent::DoubleClick {
                button: TrayMouseButton::Left,
                ..
            })
            | AppEvent::Tray(TrayIconEvent::Click {
                button: TrayMouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) => self.start_capture(CaptureKind::Region),
            AppEvent::Tray(_) => {}
            AppEvent::CaptureFinished { kind, result } => {
                self.handle_capture_finished(event_loop, kind, result);
            }
            AppEvent::SaveFinished { origin, result } => match (origin, result) {
                (SaveOrigin::Autosave, Ok(_)) => self.busy = false,
                (SaveOrigin::Autosave, Err(error)) => {
                    self.busy = false;
                    show_error("Could not save screenshot", &error);
                }
                (SaveOrigin::Editor, Ok(_)) => {
                    self.overlay = None;
                    self.busy = false;
                }
                (SaveOrigin::Editor, Err(error)) => {
                    show_error("Could not save screenshot", &error);
                    self.set_overlay_visible(true);
                }
            },
            AppEvent::PrintFinished { result } => match result {
                Ok(PrintOutcome::Printed) => {
                    self.overlay = None;
                    self.busy = false;
                }
                Ok(PrintOutcome::Cancelled) => self.set_overlay_visible(true),
                Err(error) => {
                    show_error("Could not print screenshot", &error);
                    self.set_overlay_visible(true);
                }
            },
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let outcome = self
            .overlay
            .as_mut()
            .filter(|overlay| overlay.window_id() == window_id)
            .map_or(OverlayOutcome::None, |overlay| overlay.handle_event(event));
        self.finish_overlay(outcome);
    }
}

struct RegisteredHotkeys {
    manager: GlobalHotKeyManager,
    fullscreen: HotKey,
    region: HotKey,
    registered: bool,
}

impl RegisteredHotkeys {
    fn new(config: &Config) -> Result<Self> {
        let manager = GlobalHotKeyManager::new().context("could not initialize global hotkeys")?;
        let fullscreen = config
            .fullscreen_shortcut
            .as_hotkey()
            .context("invalid full-screen shortcut")?;
        let region = config
            .region_shortcut
            .as_hotkey()
            .context("invalid region shortcut")?;
        manager
            .register_all(&[fullscreen, region])
            .with_context(|| {
                format!(
                    "Windows rejected `{}` or `{}` because another application may already use it",
                    config.fullscreen_shortcut, config.region_shortcut
                )
            })?;
        Ok(Self {
            manager,
            fullscreen,
            region,
            registered: true,
        })
    }

    fn replace(&mut self, config: &Config) -> Result<()> {
        let fullscreen = config
            .fullscreen_shortcut
            .as_hotkey()
            .context("invalid full-screen shortcut")?;
        let region = config
            .region_shortcut
            .as_hotkey()
            .context("invalid region shortcut")?;
        if self.registered && fullscreen == self.fullscreen && region == self.region {
            return Ok(());
        }

        let previous = [self.fullscreen, self.region];
        let replacement_manager =
            GlobalHotKeyManager::new().context("could not reinitialize global hotkeys")?;
        let previous_manager = std::mem::replace(&mut self.manager, replacement_manager);
        // Destroying the old manager's hidden window atomically releases all
        // hotkeys associated with it, avoiding partial sequential unregisters.
        drop(previous_manager);
        self.registered = false;

        if let Err(error) = self.manager.register_all(&[fullscreen, region]) {
            // `register_all` can fail after registering its first item. Best
            // effort cleanup prevents that partial replacement from lingering.
            let _ = self.manager.unregister_all(&[fullscreen, region]);
            let rollback = self.manager.register_all(&previous);
            return match rollback {
                Ok(()) => {
                    self.registered = true;
                    Err(error).with_context(|| {
                        format!(
                            "Windows rejected `{}` or `{}`",
                            config.fullscreen_shortcut, config.region_shortcut
                        )
                    })
                }
                Err(rollback_error) => Err(anyhow::anyhow!(
                    "Windows rejected the new shortcuts ({error}) and restoring the previous shortcuts also failed ({rollback_error})"
                )),
            };
        }

        self.fullscreen = fullscreen;
        self.region = region;
        self.registered = true;
        Ok(())
    }
}

fn build_tray_icon() -> Result<TrayIcon> {
    let menu = Menu::new();
    let region = MenuItem::with_id(MENU_CAPTURE_REGION, "Capture &region", true, None);
    let fullscreen = MenuItem::with_id(MENU_CAPTURE_FULLSCREEN, "Capture &full screen", true, None);
    let screenshots =
        MenuItem::with_id(MENU_OPEN_SCREENSHOTS, "Open screenshot &folder", true, None);
    let settings = MenuItem::with_id(MENU_OPEN_SETTINGS, "Open &settings", true, None);
    let reload = MenuItem::with_id(MENU_RELOAD_SETTINGS, "&Reload settings", true, None);
    let separator = PredefinedMenuItem::separator();
    let quit = MenuItem::with_id(MENU_QUIT, "&Quit Rustshot", true, None);
    menu.append_items(&[
        &region,
        &fullscreen,
        &separator,
        &screenshots,
        &settings,
        &reload,
        &quit,
    ])?;

    TrayIconBuilder::new()
        .with_tooltip("Rustshot — click to capture a region")
        .with_menu(Box::new(menu))
        .with_icon(rustshot_icon()?)
        .build()
        .context("Windows rejected the Rustshot tray icon")
}

fn rustshot_icon() -> Result<Icon> {
    const SIZE: u32 = 32;
    let mut rgba = vec![0_u8; (SIZE * SIZE * 4) as usize];
    for y in 3..29 {
        for x in 3..29 {
            let index = ((y * SIZE + x) * 4) as usize;
            rgba[index] = 38;
            rgba[index + 1] = 119;
            rgba[index + 2] = 222;
            rgba[index + 3] = 255;
        }
    }
    for y in 9..23 {
        for x in 8..24 {
            let border = !(11..21).contains(&x) || !(12..20).contains(&y);
            if border {
                let index = ((y * SIZE + x) * 4) as usize;
                rgba[index..index + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).context("generated an invalid tray icon")
}

fn next_autosave_path(config: &Config) -> PathBuf {
    let extension = config.image_format.extension();
    let stem = screenshot_stem();
    for suffix in 0_u32..10_000 {
        let name = if suffix == 0 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem}_{suffix}.{extension}")
        };
        let path = config.autosave_directory.join(name);
        if !path.exists() {
            return path;
        }
    }
    config
        .autosave_directory
        .join(format!("{stem}_{}.{}", std::process::id(), extension))
}

fn screenshot_stem() -> String {
    Local::now()
        .format("Rustshot_%Y-%m-%d_%H-%M-%S_%3f")
        .to_string()
}

fn ensure_extension(mut path: PathBuf, extension: &str) -> PathBuf {
    let matches = path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension));
    if !matches {
        path.set_extension(extension);
    }
    path
}

fn open_settings(config: &Config) -> Result<()> {
    let path = config_path().context("could not determine the settings path")?;
    if !path.exists() {
        config
            .save()
            .context("could not create the settings file")?;
    }
    Command::new("notepad.exe")
        .arg(&path)
        .spawn()
        .with_context(|| format!("could not open `{}`", path.display()))?;
    Ok(())
}

fn open_path(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("could not create `{}`", path.display()))?;
    Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .with_context(|| format!("could not open `{}`", path.display()))?;
    Ok(())
}

fn show_error(title: &str, description: &str) {
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(title)
        .set_description(description)
        .show();
}

#[cfg(test)]
mod tests {
    use super::ensure_extension;
    use std::path::PathBuf;

    #[test]
    fn save_extension_matches_the_selected_encoder() {
        assert_eq!(
            ensure_extension(PathBuf::from("capture.png"), "jpg"),
            PathBuf::from("capture.jpg")
        );
        assert_eq!(
            ensure_extension(PathBuf::from("capture"), "png"),
            PathBuf::from("capture.png")
        );
        assert_eq!(
            ensure_extension(PathBuf::from("capture.PNG"), "png"),
            PathBuf::from("capture.PNG")
        );
    }
}
