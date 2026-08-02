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

use crate::{
    diagnostics,
    overlay::{EditorPreferences, Overlay, OverlayOutcome},
    settings::{run_settings_dialog, SettingsResponder, SettingsWindowHandle},
};

const MENU_CAPTURE_REGION: &str = "capture-region";
const MENU_CAPTURE_FULLSCREEN: &str = "capture-fullscreen";
const MENU_OPEN_SCREENSHOTS: &str = "open-screenshots";
const MENU_OPEN_SETTINGS: &str = "open-settings";
const MENU_DIAGNOSTICS: &str = "diagnostics";
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
    SettingsApply {
        session_id: u64,
        config: Config,
        responder: SettingsResponder,
    },
    SettingsClosed {
        session_id: u64,
        result: Result<(), String>,
    },
}

pub fn run() -> Result<()> {
    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("could not create the Windows event loop")?;
    wire_event_sources(&event_loop.create_proxy());

    let config = load_initial_config();
    let open_settings_on_start = std::env::args_os()
        .skip(1)
        .any(|argument| argument == "--settings");
    let mut app = RustshotApp::new(event_loop.create_proxy(), config, open_settings_on_start);
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
                &format!(
                    "{error}\n\nRustshot will use safe defaults. Open Settings to repair the configuration."
                ),
            );
            Config::default()
        }
    }
}

struct SettingsSession {
    id: u64,
    window: SettingsWindowHandle,
}

struct RustshotApp {
    proxy: EventLoopProxy<AppEvent>,
    config: Config,
    hotkeys: Option<RegisteredHotkeys>,
    tray: Option<TrayIcon>,
    overlay: Option<Overlay>,
    settings_window: Option<SettingsSession>,
    next_settings_session_id: u64,
    open_settings_on_start: bool,
    busy: bool,
}

impl RustshotApp {
    fn new(proxy: EventLoopProxy<AppEvent>, config: Config, open_settings_on_start: bool) -> Self {
        Self {
            proxy,
            config,
            hotkeys: None,
            tray: None,
            overlay: None,
            settings_window: None,
            next_settings_session_id: 1,
            open_settings_on_start,
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
        if self.open_settings_on_start {
            self.open_settings_on_start = false;
            self.open_settings_window();
        }
    }

    fn start_capture(&mut self, kind: CaptureKind) {
        if self.busy || self.overlay.is_some() || self.settings_window.is_some() {
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
            CaptureKind::Region => match Overlay::new(
                event_loop,
                frame,
                EditorPreferences {
                    tool: self.config.last_editor_tool,
                    color: self.config.editor_color,
                    stroke_width: self.config.editor_stroke_width,
                },
                self.config.region_auto_copy || self.config.region_autosave,
            ) {
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
        self.spawn_save_with_options(
            frame,
            destination,
            origin,
            replace_existing,
            EncodeOptions::from(&self.config),
        );
    }

    fn spawn_save_with_options(
        &self,
        frame: Frame,
        destination: PathBuf,
        origin: SaveOrigin,
        replace_existing: bool,
        options: EncodeOptions,
    ) {
        let proxy = self.proxy.clone();
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
        let default_options = EncodeOptions::from(&self.config);
        let extension = default_options.format.extension();
        let file_name = format!("{}.{}", screenshot_stem(), extension);
        let dialog = FileDialog::new()
            .set_title("Save Rustshot screenshot")
            .set_directory(&self.config.autosave_directory)
            .set_file_name(file_name);
        let dialog = match default_options.format {
            OutputFormat::Png => dialog
                .add_filter("PNG image", &["png"])
                .add_filter("JPEG image", &["jpg", "jpeg"]),
            OutputFormat::Jpeg => dialog
                .add_filter("JPEG image", &["jpg", "jpeg"])
                .add_filter("PNG image", &["png"]),
        };
        let selected = dialog.save_file();

        let Some(selected) = selected else {
            self.set_overlay_visible(true);
            return;
        };
        let selected_format = output_format_from_path(&selected).unwrap_or(default_options.format);
        let mut options = default_options;
        options.format = selected_format;
        let destination = if output_format_from_path(&selected).is_some() {
            selected.clone()
        } else {
            ensure_extension(selected.clone(), selected_format.extension())
        };
        let replace_existing = destination.exists();
        if destination != selected
            && replace_existing
            && MessageDialog::new()
                .set_level(MessageLevel::Warning)
                .set_title("Replace existing screenshot?")
                .set_description(format!(
                    "Rustshot will encode this image as .{}.\n\nReplace `{}`?",
                    selected_format.extension(),
                    destination.display()
                ))
                .set_buttons(MessageButtons::YesNo)
                .show()
                != MessageDialogResult::Yes
        {
            self.set_overlay_visible(true);
            return;
        }
        self.spawn_save_with_options(
            frame,
            destination,
            SaveOrigin::Editor,
            replace_existing,
            options,
        );
    }

    fn finish_overlay(&mut self, outcome: OverlayOutcome) {
        match outcome {
            OverlayOutcome::None => {}
            OverlayOutcome::Cancel => {
                self.remember_overlay_preferences();
                self.overlay = None;
                self.busy = false;
            }
            OverlayOutcome::Finish(frame) => {
                self.finish_region_automatically(frame);
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
                        self.remember_overlay_preferences();
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

    fn finish_region_automatically(&mut self, frame: Frame) {
        if self.config.region_auto_copy {
            let result = self
                .overlay
                .as_ref()
                .context("region selection window no longer exists")
                .and_then(|overlay| overlay.owner_hwnd())
                .and_then(|owner| copy_to_clipboard_with_owner(&frame, owner));
            if let Err(error) = result {
                show_error("Could not copy screenshot", &error.to_string());
                return;
            }
        }

        self.remember_overlay_preferences();
        self.overlay = None;
        if self.config.region_autosave {
            let destination = next_autosave_path(&self.config);
            self.spawn_save(frame, destination, SaveOrigin::Autosave, false);
        } else {
            self.busy = false;
        }
    }

    fn remember_overlay_preferences(&mut self) {
        let Some(preferences) = self.overlay.as_ref().map(Overlay::editor_preferences) else {
            return;
        };
        if self.config.last_editor_tool == preferences.tool
            && self.config.editor_color == preferences.color
            && self.config.editor_stroke_width == preferences.stroke_width
        {
            return;
        }
        self.config.last_editor_tool = preferences.tool;
        self.config.editor_color = preferences.color;
        self.config.editor_stroke_width = preferences.stroke_width;
        if let Err(error) = self.config.save() {
            show_error("Could not remember editor preferences", &error.to_string());
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
            MENU_OPEN_SETTINGS => self.open_settings_window(),
            MENU_DIAGNOSTICS => match diagnostics::write_report(&self.config) {
                Ok(report) => {
                    let directory = report.parent().unwrap_or_else(|| Path::new("."));
                    if let Err(error) = open_path(directory) {
                        show_error("Could not open diagnostics", &error.to_string());
                    } else {
                        MessageDialog::new()
                            .set_level(MessageLevel::Info)
                            .set_title("Diagnostic report created")
                            .set_description(format!(
                                "Rustshot created `{}`. Review it before attaching it to a support request.",
                                report.display()
                            ))
                            .show();
                    }
                }
                Err(error) => show_error("Could not create diagnostics", &error.to_string()),
            },
            MENU_QUIT => {
                if self.busy || self.settings_window.is_some() {
                    show_error(
                        "Rustshot is busy",
                        "Finish or cancel the current capture, settings, save, or print operation before quitting.",
                    );
                } else {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn open_settings_window(&mut self) {
        if let Some(session) = &self.settings_window {
            session.window.focus();
            return;
        }
        if self.busy || self.overlay.is_some() {
            show_error(
                "Rustshot is busy",
                "Finish or cancel the current capture, save, or print operation before opening settings.",
            );
            return;
        }

        let window_handle = SettingsWindowHandle::new();
        let session_id = self.next_settings_session_id;
        self.next_settings_session_id = self.next_settings_session_id.wrapping_add(1);
        self.settings_window = Some(SettingsSession {
            id: session_id,
            window: window_handle.clone(),
        });
        let config = self.config.clone();
        let proxy = self.proxy.clone();
        let spawn_result = thread::Builder::new()
            .name("rustshot-settings".to_owned())
            .spawn(move || {
                let submit_proxy = proxy.clone();
                let result =
                    run_settings_dialog(&config, window_handle, move |config, responder| {
                        if let Err(error) = submit_proxy.send_event(AppEvent::SettingsApply {
                            session_id,
                            config,
                            responder,
                        }) {
                            if let AppEvent::SettingsApply { responder, .. } = error.0 {
                                responder.finish(Err(
                                    "Rustshot's event loop is no longer available".to_owned(),
                                ));
                            }
                        }
                    })
                    .map_err(|error| format!("{error:#}"));
                let _ = proxy.send_event(AppEvent::SettingsClosed { session_id, result });
            });
        if let Err(error) = spawn_result {
            self.settings_window = None;
            show_error(
                "Could not open settings",
                &format!("Could not start the settings window: {error}"),
            );
        }
    }

    fn apply_settings(&mut self, new_config: Config) -> Result<()> {
        let previous_config = self.config.clone();
        let committed_config = commit_settings(self, &previous_config, new_config)?;
        self.config = committed_config;
        Ok(())
    }

    fn restore_hotkeys(&mut self, config: &Config, should_be_registered: bool) -> Result<()> {
        if !should_be_registered {
            self.hotkeys = None;
            return Ok(());
        }

        let replacement_error = self
            .hotkeys
            .as_mut()
            .and_then(|hotkeys| hotkeys.replace(config).err());
        if replacement_error.is_none() && self.hotkeys.is_some() {
            return Ok(());
        }

        self.hotkeys = None;
        match RegisteredHotkeys::new(config) {
            Ok(hotkeys) => {
                self.hotkeys = Some(hotkeys);
                Ok(())
            }
            Err(recreate_error) => match replacement_error {
                Some(replacement_error) => Err(anyhow::anyhow!(
                    "shortcut rollback failed ({replacement_error}); recreating the previous shortcuts also failed ({recreate_error})"
                )),
                None => Err(recreate_error).context("could not recreate the previous shortcuts"),
            },
        }
    }
}

trait SettingsCommitBackend {
    fn shortcuts_registered(&self) -> bool;
    fn activate_shortcuts(&mut self, config: &Config) -> Result<()>;
    fn persist_settings(&mut self, config: &Config) -> Result<()>;
    fn restore_shortcuts(&mut self, config: &Config, should_be_registered: bool) -> Result<()>;
}

impl SettingsCommitBackend for RustshotApp {
    fn shortcuts_registered(&self) -> bool {
        self.hotkeys
            .as_ref()
            .is_some_and(|hotkeys| hotkeys.registered)
    }

    fn activate_shortcuts(&mut self, config: &Config) -> Result<()> {
        if let Some(hotkeys) = self.hotkeys.as_mut() {
            hotkeys.replace(config)
        } else {
            self.hotkeys = Some(RegisteredHotkeys::new(config)?);
            Ok(())
        }
    }

    fn persist_settings(&mut self, config: &Config) -> Result<()> {
        config.save().map_err(anyhow::Error::new)
    }

    fn restore_shortcuts(&mut self, config: &Config, should_be_registered: bool) -> Result<()> {
        self.restore_hotkeys(config, should_be_registered)
    }
}

fn commit_settings(
    backend: &mut impl SettingsCommitBackend,
    previous_config: &Config,
    new_config: Config,
) -> Result<Config> {
    new_config.validate().context("settings are invalid")?;
    let previous_shortcuts_registered = backend.shortcuts_registered();

    if let Err(apply_error) = backend.activate_shortcuts(&new_config) {
        return match backend.restore_shortcuts(previous_config, previous_shortcuts_registered) {
            Ok(()) => Err(apply_error).context(
                "could not activate the new shortcuts; the previous shortcut state was restored",
            ),
            Err(rollback_error) => Err(anyhow::anyhow!(
                "could not activate the new shortcuts ({apply_error}); restoring the previous shortcut state also failed ({rollback_error})"
            )),
        };
    }

    if let Err(save_error) = backend.persist_settings(&new_config) {
        return match backend.restore_shortcuts(previous_config, previous_shortcuts_registered) {
            Ok(()) => Err(save_error)
                .context("could not save settings; the previous shortcut state was restored"),
            Err(rollback_error) => Err(anyhow::anyhow!(
                "could not save settings ({save_error}); restoring the previous shortcut state also failed ({rollback_error})"
            )),
        };
    }

    Ok(new_config)
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
                    self.remember_overlay_preferences();
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
                    self.remember_overlay_preferences();
                    self.overlay = None;
                    self.busy = false;
                }
                Ok(PrintOutcome::Cancelled) => self.set_overlay_visible(true),
                Err(error) => {
                    show_error("Could not print screenshot", &error);
                    self.set_overlay_visible(true);
                }
            },
            AppEvent::SettingsApply {
                session_id,
                config,
                responder,
            } => {
                if self
                    .settings_window
                    .as_ref()
                    .is_some_and(|session| session.id == session_id)
                {
                    responder.finish(
                        self.apply_settings(config)
                            .map_err(|error| format!("{error:#}")),
                    );
                }
            }
            AppEvent::SettingsClosed { session_id, result } => {
                if self
                    .settings_window
                    .as_ref()
                    .is_some_and(|session| session.id == session_id)
                {
                    self.settings_window = None;
                    if let Err(error) = result {
                        show_error("Settings window failed", &error);
                    }
                }
            }
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
    let settings = MenuItem::with_id(MENU_OPEN_SETTINGS, "&Settings...", true, None);
    let diagnostics = MenuItem::with_id(MENU_DIAGNOSTICS, "Create &diagnostic report", true, None);
    let capture_separator = PredefinedMenuItem::separator();
    let quit_separator = PredefinedMenuItem::separator();
    let quit = MenuItem::with_id(MENU_QUIT, "&Quit Rustshot", true, None);
    menu.append_items(&[
        &region,
        &fullscreen,
        &capture_separator,
        &screenshots,
        &settings,
        &diagnostics,
        &quit_separator,
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

fn output_format_from_path(path: &Path) -> Option<OutputFormat> {
    let extension = path.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("png") {
        Some(OutputFormat::Png)
    } else if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") {
        Some(OutputFormat::Jpeg)
    } else {
        None
    }
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
    diagnostics::record_error(title, description);
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(title)
        .set_description(description)
        .show();
}

#[cfg(test)]
mod tests {
    use super::{
        commit_settings, ensure_extension, output_format_from_path, OutputFormat,
        SettingsCommitBackend,
    };
    use anyhow::{bail, Result};
    use rustshot::config::Config;
    use std::path::PathBuf;

    #[derive(Default)]
    struct FakeSettingsBackend {
        registered: bool,
        fail_activate: bool,
        fail_persist: bool,
        fail_restore: bool,
        operations: Vec<&'static str>,
        restored_registration_state: Option<bool>,
    }

    impl SettingsCommitBackend for FakeSettingsBackend {
        fn shortcuts_registered(&self) -> bool {
            self.registered
        }

        fn activate_shortcuts(&mut self, _config: &Config) -> Result<()> {
            self.operations.push("activate");
            if self.fail_activate {
                bail!("activation failed");
            }
            self.registered = true;
            Ok(())
        }

        fn persist_settings(&mut self, _config: &Config) -> Result<()> {
            self.operations.push("persist");
            if self.fail_persist {
                bail!("persistence failed");
            }
            Ok(())
        }

        fn restore_shortcuts(
            &mut self,
            _config: &Config,
            should_be_registered: bool,
        ) -> Result<()> {
            self.operations.push("restore");
            self.restored_registration_state = Some(should_be_registered);
            if self.fail_restore {
                bail!("rollback failed");
            }
            self.registered = should_be_registered;
            Ok(())
        }
    }

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

    #[test]
    fn save_format_follows_the_extension_selected_in_the_dialog() {
        assert_eq!(
            output_format_from_path(std::path::Path::new("capture.png")),
            Some(OutputFormat::Png)
        );
        assert_eq!(
            output_format_from_path(std::path::Path::new("capture.JPEG")),
            Some(OutputFormat::Jpeg)
        );
        assert_eq!(
            output_format_from_path(std::path::Path::new("capture.unsupported")),
            None
        );
    }

    #[test]
    fn settings_commit_validates_before_side_effects() {
        let previous = Config::default();
        let mut invalid = previous.clone();
        invalid.jpeg_quality = 0;
        let mut backend = FakeSettingsBackend::default();

        assert!(commit_settings(&mut backend, &previous, invalid).is_err());
        assert!(backend.operations.is_empty());
    }

    #[test]
    fn settings_commit_activates_then_persists() {
        let previous = Config::default();
        let mut candidate = previous.clone();
        candidate.jpeg_quality = 77;
        let mut backend = FakeSettingsBackend {
            registered: true,
            ..Default::default()
        };

        let committed = commit_settings(&mut backend, &previous, candidate.clone())
            .expect("valid settings should commit");

        assert_eq!(committed, candidate);
        assert_eq!(backend.operations, ["activate", "persist"]);
    }

    #[test]
    fn activation_failure_restores_previous_shortcuts() {
        let previous = Config::default();
        let mut backend = FakeSettingsBackend {
            registered: true,
            fail_activate: true,
            ..Default::default()
        };

        let error = commit_settings(&mut backend, &previous, previous.clone())
            .expect_err("activation failure should abort");

        assert_eq!(backend.operations, ["activate", "restore"]);
        assert_eq!(backend.restored_registration_state, Some(true));
        assert!(error
            .to_string()
            .contains("previous shortcut state was restored"));
    }

    #[test]
    fn persistence_failure_restores_an_initially_unregistered_state() {
        let previous = Config::default();
        let mut backend = FakeSettingsBackend {
            fail_persist: true,
            ..Default::default()
        };

        let error = commit_settings(&mut backend, &previous, previous.clone())
            .expect_err("persistence failure should abort");

        assert_eq!(backend.operations, ["activate", "persist", "restore"]);
        assert_eq!(backend.restored_registration_state, Some(false));
        assert!(!backend.registered);
        assert!(error
            .to_string()
            .contains("previous shortcut state was restored"));
    }

    #[test]
    fn rollback_failure_is_reported_with_the_original_failure() {
        let previous = Config::default();
        let mut backend = FakeSettingsBackend {
            registered: true,
            fail_persist: true,
            fail_restore: true,
            ..Default::default()
        };

        let error = commit_settings(&mut backend, &previous, previous.clone())
            .expect_err("double failure should abort");
        let description = error.to_string();

        assert!(description.contains("persistence failed"));
        assert!(description.contains("rollback failed"));
    }
}
