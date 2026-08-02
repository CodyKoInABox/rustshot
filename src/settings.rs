//! Native, accessible Windows settings dialog.

#![allow(unsafe_code)]

use std::{
    ffi::OsString,
    fmt,
    num::NonZeroIsize,
    os::windows::ffi::{OsStrExt as _, OsStringExt as _},
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::{
        atomic::{AtomicIsize, Ordering},
        Arc, Mutex,
    },
};

use anyhow::{bail, Context as _, Result};
use rfd::FileDialog;
use rustshot::config::{
    Config, ConfigValidationError, MonitorScope, PngCompression, ScreenshotFormat, Shortcut,
    ShortcutBinding,
};
use windows::{
    core::{Error as WindowsError, PCWSTR},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{CheckDlgButton, IsDlgButtonChecked, BST_CHECKED, BST_UNCHECKED, EM_SETSEL},
            Input::KeyboardAndMouse::{EnableWindow, GetFocus, GetKeyState, SetFocus},
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{
                DialogBoxParamW, EndDialog, GetDlgItem, GetWindowLongPtrW, GetWindowTextLengthW,
                GetWindowTextW, MessageBoxW, PostMessageW, SendDlgItemMessageW, SendMessageW,
                SetDlgItemTextW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, CBN_SELCHANGE,
                CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, DLGC_WANTALLKEYS, GWLP_USERDATA, IDYES,
                MB_DEFBUTTON2, MB_ICONERROR, MB_ICONWARNING, MB_OK, MB_YESNO, SW_RESTORE, WM_APP,
                WM_CHAR, WM_CLOSE, WM_COMMAND, WM_GETDLGCODE, WM_INITDIALOG, WM_KEYDOWN, WM_KEYUP,
                WM_KILLFOCUS, WM_NCDESTROY, WM_SETFOCUS, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP,
            },
        },
    },
};
use winit::raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
    Win32WindowHandle, WindowHandle,
};

const IDD_SETTINGS: usize = 101;
const IDOK: i32 = 1;
const IDCANCEL: i32 = 2;
const IDC_FULLSCREEN_SHORTCUT: i32 = 1001;
const IDC_REGION_SHORTCUT: i32 = 1002;
const IDC_AUTOSAVE_DIRECTORY: i32 = 1003;
const IDC_BROWSE_DIRECTORY: i32 = 1004;
const IDC_IMAGE_FORMAT: i32 = 1005;
const IDC_JPEG_QUALITY: i32 = 1006;
const IDC_PNG_COMPRESSION: i32 = 1007;
const IDC_MONITOR_SCOPE: i32 = 1008;
const IDC_INCLUDE_CURSOR: i32 = 1009;
const IDC_RESTORE_DEFAULTS: i32 = 1010;
const IDC_SETTINGS_STATUS: i32 = 1011;
const IDC_JPEG_QUALITY_LABEL: i32 = 1012;
const IDC_PNG_COMPRESSION_LABEL: i32 = 1013;
const IDC_PNG_HELP: i32 = 1014;

const WM_SETTINGS_RESULT: u32 = WM_APP + 41;
const WM_SETTINGS_FOCUS: u32 = WM_APP + 42;
const DIALOG_PANIC_RESULT: isize = -2;
const KEY_WAS_DOWN_MASK: isize = 1 << 30;
const EXTENDED_KEY_MASK: isize = 1 << 24;

const VK_BACK: u32 = 0x08;
const VK_TAB: u32 = 0x09;
const VK_RETURN: u32 = 0x0d;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_PAUSE: u32 = 0x13;
const VK_CAPITAL: u32 = 0x14;
const VK_ESCAPE: u32 = 0x1b;
const VK_SPACE: u32 = 0x20;
const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_SNAPSHOT: u32 = 0x2c;
const VK_INSERT: u32 = 0x2d;
const VK_DELETE: u32 = 0x2e;
const VK_LWIN: u32 = 0x5b;
const VK_RWIN: u32 = 0x5c;
const VK_NUMPAD0: u32 = 0x60;
const VK_NUMPAD9: u32 = 0x69;
const VK_MULTIPLY: u32 = 0x6a;
const VK_ADD: u32 = 0x6b;
const VK_SUBTRACT: u32 = 0x6d;
const VK_DECIMAL: u32 = 0x6e;
const VK_DIVIDE: u32 = 0x6f;
const VK_F1: u32 = 0x70;
const VK_F4: u32 = 0x73;
const VK_F24: u32 = 0x87;
const VK_NUMLOCK: u32 = 0x90;
const VK_SCROLL: u32 = 0x91;
const VK_LSHIFT: u32 = 0xa0;
const VK_RSHIFT: u32 = 0xa1;
const VK_LCONTROL: u32 = 0xa2;
const VK_RCONTROL: u32 = 0xa3;
const VK_LMENU: u32 = 0xa4;
const VK_RMENU: u32 = 0xa5;
const VK_VOLUME_MUTE: u32 = 0xad;
const VK_VOLUME_DOWN: u32 = 0xae;
const VK_VOLUME_UP: u32 = 0xaf;
const VK_MEDIA_NEXT_TRACK: u32 = 0xb0;
const VK_MEDIA_PREV_TRACK: u32 = 0xb1;
const VK_MEDIA_STOP: u32 = 0xb2;
const VK_MEDIA_PLAY_PAUSE: u32 = 0xb3;
const VK_OEM_1: u32 = 0xba;
const VK_OEM_PLUS: u32 = 0xbb;
const VK_OEM_COMMA: u32 = 0xbc;
const VK_OEM_MINUS: u32 = 0xbd;
const VK_OEM_PERIOD: u32 = 0xbe;
const VK_OEM_2: u32 = 0xbf;
const VK_OEM_3: u32 = 0xc0;
const VK_OEM_4: u32 = 0xdb;
const VK_OEM_5: u32 = 0xdc;
const VK_OEM_6: u32 = 0xdd;
const VK_OEM_7: u32 = 0xde;
const VK_PLAY: u32 = 0xfa;

const INTERACTIVE_CONTROLS: [i32; 11] = [
    IDC_FULLSCREEN_SHORTCUT,
    IDC_REGION_SHORTCUT,
    IDC_AUTOSAVE_DIRECTORY,
    IDC_BROWSE_DIRECTORY,
    IDC_IMAGE_FORMAT,
    IDC_JPEG_QUALITY,
    IDC_PNG_COMPRESSION,
    IDC_MONITOR_SCOPE,
    IDC_INCLUDE_CURSOR,
    IDC_RESTORE_DEFAULTS,
    IDOK,
];

type ApplyResult = std::result::Result<(), String>;
type SubmitCallback = Arc<dyn Fn(Config, SettingsResponder) + Send>;

/// Thread-safe handle used to focus the single settings dialog instance.
#[derive(Clone, Default)]
pub struct SettingsWindowHandle {
    hwnd: Arc<AtomicIsize>,
}

impl SettingsWindowHandle {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn focus(&self) {
        let hwnd = self.hwnd.load(Ordering::Acquire);
        if hwnd != 0 {
            // SAFETY: the dialog thread owns this HWND and validates the custom
            // message. Posting does not dereference the handle on this thread.
            let _ = unsafe {
                PostMessageW(
                    Some(hwnd_from_isize(hwnd)),
                    WM_SETTINGS_FOCUS,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
        }
    }

    fn attach(&self, hwnd: HWND) {
        self.hwnd.store(hwnd_to_isize(hwnd), Ordering::Release);
    }

    fn detach(&self) {
        self.hwnd.store(0, Ordering::Release);
    }
}

/// One-shot response channel from the app event loop to the dialog thread.
pub struct SettingsResponder {
    hwnd: isize,
    result: Arc<Mutex<Option<ApplyResult>>>,
}

impl SettingsResponder {
    pub fn finish(self, result: ApplyResult) {
        let mut slot = self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(result);
        drop(slot);

        // SAFETY: the dialog disables cancellation while an application is in
        // flight, so its HWND remains live until this synchronous result message
        // is handled. SendMessage avoids a message-queue quota failure leaving
        // the form permanently disabled.
        let _ = unsafe {
            SendMessageW(
                hwnd_from_isize(self.hwnd),
                WM_SETTINGS_RESULT,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            )
        };
    }
}

/// Runs the settings window on its calling thread until the user closes it.
///
/// Valid candidates are sent to the application event loop through `submit`.
/// The dialog remains open until [`SettingsResponder::finish`] reports success.
pub fn run_settings_dialog(
    config: &Config,
    window_handle: SettingsWindowHandle,
    submit: impl Fn(Config, SettingsResponder) + Send + 'static,
) -> Result<()> {
    let feedback = Arc::new(Mutex::new(None));
    let mut state = DialogState {
        initial_config: config.clone(),
        initial_snapshot: None,
        submit: Arc::new(submit),
        feedback,
        pending: false,
        fatal_error: None,
        window_handle: window_handle.clone(),
    };
    // SAFETY: a null module name returns the module containing this executable.
    let module = unsafe { GetModuleHandleW(None) }
        .context("could not locate Rustshot's settings resources")?;
    let template = PCWSTR(IDD_SETTINGS as *const u16);
    let state_pointer = (&raw mut state).cast::<()>() as isize;

    // SAFETY: resource 101 is an embedded DIALOGEX template. `state` remains at
    // a stable stack address for the synchronous DialogBoxParamW lifetime.
    let dialog_result = unsafe {
        DialogBoxParamW(
            Some(HINSTANCE(module.0)),
            template,
            None,
            Some(settings_dialog_proc),
            LPARAM(state_pointer),
        )
    };
    window_handle.detach();

    if dialog_result == -1 {
        return Err(WindowsError::from_thread()).context("Windows could not create settings");
    }
    if dialog_result == DIALOG_PANIC_RESULT {
        bail!("the native settings window stopped after an unexpected error");
    }
    if let Some(error) = state.fatal_error {
        bail!(error);
    }
    Ok(())
}

struct DialogState {
    initial_config: Config,
    initial_snapshot: Option<ControlSnapshot>,
    submit: SubmitCallback,
    feedback: Arc<Mutex<Option<ApplyResult>>>,
    pending: bool,
    fatal_error: Option<String>,
    window_handle: SettingsWindowHandle,
}

unsafe extern "system" fn settings_dialog_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Windows invokes this callback with the dialog HWND and message
        // parameters documented for DLGPROC.
        unsafe { settings_dialog_proc_inner(hwnd, message, wparam, lparam) }
    })) {
        Ok(result) => result,
        Err(_) => {
            // SAFETY: best-effort termination prevents unwinding across FFI.
            let _ = unsafe { EndDialog(hwnd, DIALOG_PANIC_RESULT) };
            1
        }
    }
}

unsafe fn settings_dialog_proc_inner(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    if message == WM_INITDIALOG {
        let state = lparam.0 as *mut DialogState;
        if state.is_null() {
            let _ = EndDialog(hwnd, IDCANCEL as isize);
            return 1;
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
        (*state).window_handle.attach(hwnd);
        let initial_config = (*state).initial_config.clone();
        match initialize_controls(hwnd, &initial_config).and_then(|()| read_snapshot(hwnd)) {
            Ok(snapshot) => (*state).initial_snapshot = Some(snapshot),
            Err(error) => {
                (*state).fatal_error = Some(format!("{error:#}"));
                let _ = EndDialog(hwnd, IDCANCEL as isize);
            }
        }
        return 1;
    }

    let state = dialog_state(hwnd);
    if state.is_null() {
        return 0;
    }
    match message {
        WM_SYSKEYDOWN => {
            if let Some((edit, control_id)) = focused_shortcut_control(hwnd) {
                if record_shortcut_key(hwnd, edit, control_id, wparam.0 as u32, lparam) {
                    return 1;
                }
            }
            0
        }
        WM_SYSCHAR | WM_SYSKEYUP => {
            if focused_shortcut_control(hwnd).is_some() {
                1
            } else {
                0
            }
        }
        WM_COMMAND => {
            let command = low_word(wparam.0);
            let notification = high_word(wparam.0);
            match command {
                IDOK => submit_settings(hwnd, state),
                IDCANCEL => request_cancel(hwnd, state),
                IDC_BROWSE_DIRECTORY => {
                    if let Err(error) = browse_for_directory(hwnd) {
                        show_native_error(hwnd, "Could not choose a folder", &format!("{error:#}"));
                    }
                    1
                }
                IDC_RESTORE_DEFAULTS => {
                    if let Err(error) = apply_config_to_controls(hwnd, &Config::default()) {
                        show_native_error(
                            hwnd,
                            "Could not restore defaults",
                            &format!("{error:#}"),
                        );
                    }
                    1
                }
                IDC_IMAGE_FORMAT if notification == CBN_SELCHANGE => {
                    if let Err(error) = normalize_hidden_quality(hwnd)
                        .and_then(|()| update_codec_control_state(hwnd))
                    {
                        show_native_error(hwnd, "Settings UI failed", &format!("{error:#}"));
                    }
                    1
                }
                _ => 0,
            }
        }
        WM_CLOSE => request_cancel(hwnd, state),
        WM_SETTINGS_RESULT => handle_apply_result(hwnd, state),
        WM_SETTINGS_FOCUS => {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
            1
        }
        _ => 0,
    }
}

unsafe fn dialog_state(hwnd: HWND) -> *mut DialogState {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DialogState
}

unsafe fn submit_settings(hwnd: HWND, state: *mut DialogState) -> isize {
    if (*state).pending {
        return 1;
    }
    let values = match read_settings_values(hwnd) {
        Ok(values) => values,
        Err(error) => {
            show_native_error(hwnd, "Invalid settings", &format!("{error:#}"));
            return 1;
        }
    };
    let candidate = match values.into_config() {
        Ok(candidate) => candidate,
        Err(error) => {
            show_native_error(hwnd, "Invalid settings", &error.to_string());
            let _ = focus_control(hwnd, error.control_id);
            return 1;
        }
    };

    (*state).pending = true;
    set_pending(hwnd, true);
    let responder = SettingsResponder {
        hwnd: hwnd_to_isize(hwnd),
        result: (*state).feedback.clone(),
    };
    let submit = (*state).submit.clone();
    submit(candidate, responder);
    1
}

unsafe fn handle_apply_result(hwnd: HWND, state: *mut DialogState) -> isize {
    let feedback = (*state).feedback.clone();
    let result = feedback
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let Some(result) = result else {
        return 1;
    };

    (*state).pending = false;
    set_pending(hwnd, false);
    match result {
        Ok(()) => {
            let _ = EndDialog(hwnd, IDOK as isize);
        }
        Err(error) => show_native_error(hwnd, "Could not save settings", &error),
    }
    1
}

unsafe fn request_cancel(hwnd: HWND, state: *mut DialogState) -> isize {
    if (*state).pending {
        return 1;
    }
    let initial_snapshot = (*state).initial_snapshot.clone();
    let dirty = read_snapshot(hwnd)
        .ok()
        .zip(initial_snapshot)
        .is_none_or(|(current, initial)| current != initial);
    if dirty && !confirm_discard(hwnd) {
        return 1;
    }
    let _ = EndDialog(hwnd, IDCANCEL as isize);
    1
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ShortcutModifiers {
    shift: bool,
    control: bool,
    alt: bool,
    super_key: bool,
}

impl ShortcutModifiers {
    fn union(self, other: Self) -> Self {
        Self {
            shift: self.shift || other.shift,
            control: self.control || other.control,
            alt: self.alt || other.alt,
            super_key: self.super_key || other.super_key,
        }
    }

    fn set_key(&mut self, virtual_key: u32, pressed: bool) -> bool {
        let modifier = match virtual_key {
            VK_SHIFT | VK_LSHIFT | VK_RSHIFT => &mut self.shift,
            VK_CONTROL | VK_LCONTROL | VK_RCONTROL => &mut self.control,
            VK_MENU | VK_LMENU | VK_RMENU => &mut self.alt,
            VK_LWIN | VK_RWIN => &mut self.super_key,
            _ => return false,
        };
        *modifier = pressed;
        true
    }
}

struct ShortcutRecorderState {
    dialog: isize,
    control_id: i32,
    held_modifiers: ShortcutModifiers,
}

unsafe extern "system" fn shortcut_edit_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    match catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: ComCtl32 invokes this callback only for the edit control on
        // which `install_shortcut_recorder` installed it.
        unsafe {
            shortcut_edit_proc_inner(hwnd, message, wparam, lparam, subclass_id, reference_data)
        }
    })) {
        Ok(result) => result,
        // SAFETY: never unwind through the Windows callback boundary.
        Err(_) => unsafe { DefSubclassProc(hwnd, message, wparam, lparam) },
    }
}

unsafe fn shortcut_edit_proc_inner(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    // SAFETY: `install_shortcut_recorder` owns this allocation until this
    // control receives WM_NCDESTROY and removes its subclass below.
    let state = unsafe { &mut *(reference_data as *mut ShortcutRecorderState) };
    let virtual_key = wparam.0 as u32;
    match message {
        WM_GETDLGCODE => {
            let modifiers = state.held_modifiers.union(current_shortcut_modifiers());
            if wants_all_dialog_keys(virtual_key, modifiers) {
                let default = DefSubclassProc(hwnd, message, wparam, lparam);
                return LRESULT(default.0 | DLGC_WANTALLKEYS as isize);
            }
        }
        WM_SETFOCUS => {
            state.held_modifiers = ShortcutModifiers::default();
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            let _ = SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            return result;
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if state.held_modifiers.set_key(virtual_key, true) {
                return LRESULT(0);
            }
            let modifiers = state.held_modifiers.union(current_shortcut_modifiers());
            if record_shortcut_key_with_modifiers(
                hwnd_from_isize(state.dialog),
                hwnd,
                state.control_id,
                virtual_key,
                lparam,
                modifiers,
            ) {
                return LRESULT(0);
            }
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
        WM_KEYUP | WM_SYSKEYUP => {
            let _ = state.held_modifiers.set_key(virtual_key, false);
            return LRESULT(0);
        }
        WM_CHAR | WM_SYSCHAR => return LRESULT(0),
        WM_KILLFOCUS => {
            state.held_modifiers = ShortcutModifiers::default();
        }
        WM_NCDESTROY => {
            let state = reference_data as *mut ShortcutRecorderState;
            let _ = RemoveWindowSubclass(hwnd, Some(shortcut_edit_proc), subclass_id);
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            // SAFETY: this is the single matching reclamation of the Box
            // allocated by `install_shortcut_recorder`.
            drop(unsafe { Box::from_raw(state) });
            return result;
        }
        _ => {}
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

unsafe fn focused_shortcut_control(dialog: HWND) -> Option<(HWND, i32)> {
    let focused = GetFocus();
    [IDC_FULLSCREEN_SHORTCUT, IDC_REGION_SHORTCUT]
        .into_iter()
        .find_map(|control_id| {
            let edit = GetDlgItem(Some(dialog), control_id).ok()?;
            (edit == focused).then_some((edit, control_id))
        })
}

unsafe fn record_shortcut_key(
    dialog: HWND,
    edit: HWND,
    control_id: i32,
    virtual_key: u32,
    lparam: LPARAM,
) -> bool {
    let modifiers = current_shortcut_modifiers();
    record_shortcut_key_with_modifiers(dialog, edit, control_id, virtual_key, lparam, modifiers)
}

unsafe fn record_shortcut_key_with_modifiers(
    dialog: HWND,
    edit: HWND,
    control_id: i32,
    virtual_key: u32,
    lparam: LPARAM,
    modifiers: ShortcutModifiers,
) -> bool {
    if virtual_key == VK_F4 && modifiers.alt {
        return false;
    }
    if is_modifier_key(virtual_key) || lparam.0 & KEY_WAS_DOWN_MASK != 0 {
        return true;
    }

    let extended = lparam.0 & EXTENDED_KEY_MASK != 0;
    match shortcut_from_virtual_key(virtual_key, extended, modifiers) {
        Some(shortcut) => {
            let _ = set_text(dialog, control_id, shortcut.as_str());
            let _ = set_text(dialog, IDC_SETTINGS_STATUS, "");
            let _ = SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        None => {
            let _ = set_text(
                dialog,
                IDC_SETTINGS_STATUS,
                "That key cannot be used as a shortcut.",
            );
        }
    }
    true
}

fn install_shortcut_recorder(dialog: HWND, control_id: i32) -> Result<()> {
    // SAFETY: the dialog owns this edit for the whole subclass lifetime. The
    // child removes the subclass during WM_NCDESTROY.
    let edit = unsafe { GetDlgItem(Some(dialog), control_id) }
        .with_context(|| format!("shortcut control {control_id} is missing"))?;
    let state = Box::new(ShortcutRecorderState {
        dialog: hwnd_to_isize(dialog),
        control_id,
        held_modifiers: ShortcutModifiers::default(),
    });
    let reference_data = Box::into_raw(state) as usize;
    let installed = unsafe {
        SetWindowSubclass(
            edit,
            Some(shortcut_edit_proc),
            control_id as usize,
            reference_data,
        )
    };
    if !installed.as_bool() {
        // SAFETY: the subclass did not take ownership, so reclaim it here.
        drop(unsafe { Box::from_raw(reference_data as *mut ShortcutRecorderState) });
        bail!("Windows could not initialize shortcut control {control_id}");
    }
    Ok(())
}

fn current_shortcut_modifiers() -> ShortcutModifiers {
    ShortcutModifiers {
        shift: key_is_down(VK_SHIFT) || key_is_down(VK_LSHIFT) || key_is_down(VK_RSHIFT),
        control: key_is_down(VK_CONTROL) || key_is_down(VK_LCONTROL) || key_is_down(VK_RCONTROL),
        alt: key_is_down(VK_MENU) || key_is_down(VK_LMENU) || key_is_down(VK_RMENU),
        super_key: key_is_down(VK_LWIN) || key_is_down(VK_RWIN),
    }
}

fn key_is_down(virtual_key: u32) -> bool {
    // SAFETY: GetKeyState accepts every virtual-key value. All callers pass
    // constants defined by the Windows virtual-key table.
    unsafe { GetKeyState(virtual_key as i32) < 0 }
}

fn wants_all_dialog_keys(virtual_key: u32, modifiers: ShortcutModifiers) -> bool {
    if virtual_key == VK_F4 && modifiers.alt {
        return false;
    }
    modifiers.control
        || modifiers.alt
        || modifiers.super_key
        || (modifiers.shift && virtual_key != VK_TAB)
}

fn is_modifier_key(virtual_key: u32) -> bool {
    matches!(
        virtual_key,
        VK_SHIFT
            | VK_CONTROL
            | VK_MENU
            | VK_LWIN
            | VK_RWIN
            | VK_LSHIFT
            | VK_RSHIFT
            | VK_LCONTROL
            | VK_RCONTROL
            | VK_LMENU
            | VK_RMENU
    )
}

fn shortcut_from_virtual_key(
    virtual_key: u32,
    extended: bool,
    modifiers: ShortcutModifiers,
) -> Option<Shortcut> {
    let key = virtual_key_name(virtual_key, extended)?;
    let mut shortcut = String::new();
    if modifiers.shift {
        shortcut.push_str("shift+");
    }
    if modifiers.control {
        shortcut.push_str("control+");
    }
    if modifiers.alt {
        shortcut.push_str("alt+");
    }
    if modifiers.super_key {
        shortcut.push_str("super+");
    }
    shortcut.push_str(&key);
    Shortcut::new(shortcut).ok()
}

fn virtual_key_name(virtual_key: u32, extended: bool) -> Option<String> {
    if (0x41..=0x5a).contains(&virtual_key) {
        return char::from_u32(virtual_key).map(|key| format!("Key{key}"));
    }
    if (0x30..=0x39).contains(&virtual_key) {
        return char::from_u32(virtual_key).map(|key| format!("Digit{key}"));
    }
    if (VK_F1..=VK_F24).contains(&virtual_key) {
        return Some(format!("F{}", virtual_key - VK_F1 + 1));
    }
    if (VK_NUMPAD0..=VK_NUMPAD9).contains(&virtual_key) {
        return Some(format!("Numpad{}", virtual_key - VK_NUMPAD0));
    }

    let key = match virtual_key {
        VK_OEM_PLUS => "Equal",
        VK_OEM_COMMA => "Comma",
        VK_OEM_MINUS => "Minus",
        VK_OEM_PERIOD => "Period",
        VK_OEM_1 => "Semicolon",
        VK_OEM_2 => "Slash",
        VK_OEM_3 => "Backquote",
        VK_OEM_4 => "BracketLeft",
        VK_OEM_5 => "Backslash",
        VK_OEM_6 => "BracketRight",
        VK_OEM_7 => "Quote",
        VK_BACK => "Backspace",
        VK_TAB => "Tab",
        VK_SPACE => "Space",
        VK_RETURN if extended => "NumpadEnter",
        VK_RETURN => "Enter",
        VK_CAPITAL => "CapsLock",
        VK_ESCAPE => "Escape",
        VK_PRIOR => "PageUp",
        VK_NEXT => "PageDown",
        VK_END => "End",
        VK_HOME => "Home",
        VK_LEFT => "ArrowLeft",
        VK_UP => "ArrowUp",
        VK_RIGHT => "ArrowRight",
        VK_DOWN => "ArrowDown",
        VK_SNAPSHOT => "PrintScreen",
        VK_INSERT => "Insert",
        VK_DELETE => "Delete",
        VK_NUMLOCK => "NumLock",
        VK_MULTIPLY => "NumpadMultiply",
        VK_ADD => "NumpadAdd",
        VK_SUBTRACT => "NumpadSubtract",
        VK_DECIMAL => "NumpadDecimal",
        VK_DIVIDE => "NumpadDivide",
        VK_SCROLL => "ScrollLock",
        VK_VOLUME_DOWN => "AudioVolumeDown",
        VK_VOLUME_UP => "AudioVolumeUp",
        VK_VOLUME_MUTE => "AudioVolumeMute",
        VK_PLAY => "MediaPlay",
        VK_MEDIA_PLAY_PAUSE => "MediaPlayPause",
        VK_MEDIA_STOP => "MediaStop",
        VK_MEDIA_NEXT_TRACK => "MediaTrackNext",
        VK_MEDIA_PREV_TRACK => "MediaTrackPrevious",
        VK_PAUSE => "Pause",
        _ => return None,
    };
    Some(key.to_owned())
}

fn initialize_controls(hwnd: HWND, config: &Config) -> Result<()> {
    add_combo_items(hwnd, IDC_IMAGE_FORMAT, &["PNG (lossless)", "JPEG"])?;
    add_combo_items(
        hwnd,
        IDC_PNG_COMPRESSION,
        &["Fast", "Default", "Best compression"],
    )?;
    add_combo_items(
        hwnd,
        IDC_MONITOR_SCOPE,
        &["Monitor under cursor", "All displays"],
    )?;
    install_shortcut_recorder(hwnd, IDC_FULLSCREEN_SHORTCUT)?;
    install_shortcut_recorder(hwnd, IDC_REGION_SHORTCUT)?;
    apply_config_to_controls(hwnd, config)
}

fn apply_config_to_controls(hwnd: HWND, config: &Config) -> Result<()> {
    let values = SettingsValues::from_config(config);
    set_text(hwnd, IDC_FULLSCREEN_SHORTCUT, &values.fullscreen_shortcut)?;
    set_text(hwnd, IDC_REGION_SHORTCUT, &values.region_shortcut)?;
    set_path(hwnd, IDC_AUTOSAVE_DIRECTORY, &values.autosave_directory)?;
    set_combo_selection(hwnd, IDC_IMAGE_FORMAT, values.image_format_index)?;
    set_text(hwnd, IDC_JPEG_QUALITY, &values.jpeg_quality)?;
    set_combo_selection(hwnd, IDC_PNG_COMPRESSION, values.png_compression_index)?;
    set_combo_selection(hwnd, IDC_MONITOR_SCOPE, values.monitor_scope_index)?;
    // SAFETY: the checkbox belongs to this initialized dialog.
    unsafe {
        CheckDlgButton(
            hwnd,
            IDC_INCLUDE_CURSOR,
            if values.include_cursor {
                BST_CHECKED
            } else {
                BST_UNCHECKED
            },
        )
    }
    .context("could not initialize cursor inclusion")?;
    update_codec_control_state(hwnd)
}

fn read_settings_values(hwnd: HWND) -> Result<SettingsValues> {
    let fullscreen_shortcut = read_text(hwnd, IDC_FULLSCREEN_SHORTCUT)
        .context("could not read the full-screen shortcut")?;
    let region_shortcut =
        read_text(hwnd, IDC_REGION_SHORTCUT).context("could not read the region shortcut")?;
    let autosave_directory = PathBuf::from(OsString::from_wide(&read_text_wide(
        hwnd,
        IDC_AUTOSAVE_DIRECTORY,
    )?));
    let jpeg_quality = read_text(hwnd, IDC_JPEG_QUALITY).context("could not read JPEG quality")?;
    // SAFETY: the checkbox belongs to the live settings dialog.
    let include_cursor = unsafe { IsDlgButtonChecked(hwnd, IDC_INCLUDE_CURSOR) } == BST_CHECKED.0;

    Ok(SettingsValues {
        fullscreen_shortcut,
        region_shortcut,
        autosave_directory,
        image_format_index: combo_selection(hwnd, IDC_IMAGE_FORMAT)?,
        jpeg_quality,
        png_compression_index: combo_selection(hwnd, IDC_PNG_COMPRESSION)?,
        monitor_scope_index: combo_selection(hwnd, IDC_MONITOR_SCOPE)?,
        include_cursor,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SettingsValues {
    fullscreen_shortcut: String,
    region_shortcut: String,
    autosave_directory: PathBuf,
    image_format_index: usize,
    jpeg_quality: String,
    png_compression_index: usize,
    monitor_scope_index: usize,
    include_cursor: bool,
}

impl SettingsValues {
    fn from_config(config: &Config) -> Self {
        Self {
            fullscreen_shortcut: config.fullscreen_shortcut.to_string(),
            region_shortcut: config.region_shortcut.to_string(),
            autosave_directory: config.autosave_directory.clone(),
            image_format_index: usize::from(config.image_format == ScreenshotFormat::Jpeg),
            jpeg_quality: config.jpeg_quality.to_string(),
            png_compression_index: match config.png_compression {
                PngCompression::Fast => 0,
                PngCompression::Default => 1,
                PngCompression::Best => 2,
            },
            monitor_scope_index: usize::from(config.monitor_scope == MonitorScope::VirtualDesktop),
            include_cursor: config.include_cursor,
        }
    }

    fn into_config(self) -> std::result::Result<Config, SettingsInputError> {
        let fullscreen_shortcut = Shortcut::new(&self.fullscreen_shortcut).map_err(|error| {
            SettingsInputError::new(
                IDC_FULLSCREEN_SHORTCUT,
                format!("Full-screen shortcut is invalid.\n\n{error}"),
            )
        })?;
        let region_shortcut = Shortcut::new(&self.region_shortcut).map_err(|error| {
            SettingsInputError::new(
                IDC_REGION_SHORTCUT,
                format!("Region shortcut is invalid.\n\n{error}"),
            )
        })?;
        let jpeg_quality = self.jpeg_quality.trim().parse::<u8>().map_err(|_| {
            SettingsInputError::new(
                IDC_JPEG_QUALITY,
                "JPEG quality must be a number from 1 to 100",
            )
        })?;
        let image_format = match self.image_format_index {
            0 => ScreenshotFormat::Png,
            1 => ScreenshotFormat::Jpeg,
            _ => {
                return Err(SettingsInputError::new(
                    IDC_IMAGE_FORMAT,
                    "Image format selection is invalid",
                ));
            }
        };
        let png_compression = match self.png_compression_index {
            0 => PngCompression::Fast,
            1 => PngCompression::Default,
            2 => PngCompression::Best,
            _ => {
                return Err(SettingsInputError::new(
                    IDC_PNG_COMPRESSION,
                    "PNG compression selection is invalid",
                ));
            }
        };
        let monitor_scope = match self.monitor_scope_index {
            0 => MonitorScope::CursorMonitor,
            1 => MonitorScope::VirtualDesktop,
            _ => {
                return Err(SettingsInputError::new(
                    IDC_MONITOR_SCOPE,
                    "Full-screen scope selection is invalid",
                ));
            }
        };
        let config = Config {
            fullscreen_shortcut,
            region_shortcut,
            autosave_directory: self.autosave_directory,
            image_format,
            jpeg_quality,
            png_compression,
            monitor_scope,
            include_cursor: self.include_cursor,
        };
        config.validate().map_err(|error| {
            let control_id = match &error {
                ConfigValidationError::InvalidShortcut { binding, .. } => match binding {
                    ShortcutBinding::Fullscreen => IDC_FULLSCREEN_SHORTCUT,
                    ShortcutBinding::Region => IDC_REGION_SHORTCUT,
                },
                ConfigValidationError::DuplicateShortcuts { .. } => IDC_REGION_SHORTCUT,
                ConfigValidationError::InvalidJpegQuality(_) => IDC_JPEG_QUALITY,
                ConfigValidationError::EmptyAutosaveDirectory
                | ConfigValidationError::AutosaveDirectoryIsFile(_) => IDC_AUTOSAVE_DIRECTORY,
            };
            SettingsInputError::new(control_id, format!("Settings are invalid.\n\n{error}"))
        })?;
        Ok(config)
    }
}

#[derive(Debug)]
struct SettingsInputError {
    control_id: i32,
    message: String,
}

impl SettingsInputError {
    fn new(control_id: i32, message: impl Into<String>) -> Self {
        Self {
            control_id,
            message: message.into(),
        }
    }
}

impl fmt::Display for SettingsInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SettingsInputError {}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ControlSnapshot {
    fullscreen_shortcut: Vec<u16>,
    region_shortcut: Vec<u16>,
    autosave_directory: Vec<u16>,
    jpeg_quality: Vec<u16>,
    image_format: isize,
    png_compression: isize,
    monitor_scope: isize,
    include_cursor: u32,
}

fn read_snapshot(hwnd: HWND) -> Result<ControlSnapshot> {
    // SAFETY: every queried control belongs to this live dialog.
    Ok(ControlSnapshot {
        fullscreen_shortcut: read_text_wide(hwnd, IDC_FULLSCREEN_SHORTCUT)?,
        region_shortcut: read_text_wide(hwnd, IDC_REGION_SHORTCUT)?,
        autosave_directory: read_text_wide(hwnd, IDC_AUTOSAVE_DIRECTORY)?,
        jpeg_quality: read_text_wide(hwnd, IDC_JPEG_QUALITY)?,
        image_format: unsafe {
            SendDlgItemMessageW(hwnd, IDC_IMAGE_FORMAT, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0
        },
        png_compression: unsafe {
            SendDlgItemMessageW(
                hwnd,
                IDC_PNG_COMPRESSION,
                CB_GETCURSEL,
                WPARAM(0),
                LPARAM(0),
            )
            .0
        },
        monitor_scope: unsafe {
            SendDlgItemMessageW(hwnd, IDC_MONITOR_SCOPE, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0
        },
        include_cursor: unsafe { IsDlgButtonChecked(hwnd, IDC_INCLUDE_CURSOR) },
    })
}

fn update_codec_control_state(hwnd: HWND) -> Result<()> {
    let png_selected = combo_selection(hwnd, IDC_IMAGE_FORMAT)? == 0;
    set_control_enabled(hwnd, IDC_JPEG_QUALITY, !png_selected)?;
    set_control_enabled(hwnd, IDC_JPEG_QUALITY_LABEL, !png_selected)?;
    set_control_enabled(hwnd, IDC_PNG_COMPRESSION, png_selected)?;
    set_control_enabled(hwnd, IDC_PNG_COMPRESSION_LABEL, png_selected)?;
    set_control_enabled(hwnd, IDC_PNG_HELP, png_selected)
}

fn normalize_hidden_quality(hwnd: HWND) -> Result<()> {
    if combo_selection(hwnd, IDC_IMAGE_FORMAT)? != 0 {
        return Ok(());
    }
    let quality = read_text(hwnd, IDC_JPEG_QUALITY)
        .ok()
        .and_then(|value| value.trim().parse::<u8>().ok());
    if quality.is_none_or(|quality| !(1..=100).contains(&quality)) {
        set_text(
            hwnd,
            IDC_JPEG_QUALITY,
            &Config::default().jpeg_quality.to_string(),
        )?;
    }
    Ok(())
}

unsafe fn set_pending(hwnd: HWND, pending: bool) {
    for control_id in INTERACTIVE_CONTROLS {
        let _ = set_control_enabled(hwnd, control_id, !pending);
    }
    let _ = set_control_enabled(hwnd, IDCANCEL, !pending);
    if !pending {
        let _ = update_codec_control_state(hwnd);
    }
    let _ = set_text(
        hwnd,
        IDC_SETTINGS_STATUS,
        if pending { "Saving settings..." } else { "" },
    );
}

fn set_control_enabled(hwnd: HWND, control_id: i32, enabled: bool) -> Result<()> {
    // SAFETY: the requested child HWND is used only for EnableWindow.
    let control = unsafe { GetDlgItem(Some(hwnd), control_id) }
        .with_context(|| format!("settings control {control_id} is missing"))?;
    // EnableWindow returns the previous enabled state, not success/failure.
    let _ = unsafe { EnableWindow(control, enabled) };
    Ok(())
}

fn focus_control(hwnd: HWND, control_id: i32) -> Result<()> {
    // SAFETY: the requested child belongs to the live settings dialog. EM_SETSEL
    // is ignored by non-edit controls after they receive focus.
    let control = unsafe { GetDlgItem(Some(hwnd), control_id) }
        .with_context(|| format!("settings control {control_id} is missing"))?;
    let _ = unsafe { SetFocus(Some(control)) };
    let _ = unsafe { SendMessageW(control, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1))) };
    Ok(())
}

fn browse_for_directory(hwnd: HWND) -> Result<()> {
    let current = PathBuf::from(OsString::from_wide(&read_text_wide(
        hwnd,
        IDC_AUTOSAVE_DIRECTORY,
    )?));
    let parent = DialogParent(hwnd);
    let mut picker = FileDialog::new()
        .set_parent(&parent)
        .set_title("Choose Rustshot autosave folder");
    if current.is_dir() {
        picker = picker.set_directory(&current);
    }
    if let Some(directory) = picker.pick_folder() {
        set_path(hwnd, IDC_AUTOSAVE_DIRECTORY, &directory)?;
    }
    Ok(())
}

struct DialogParent(HWND);

impl HasWindowHandle for DialogParent {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        let value = NonZeroIsize::new(hwnd_to_isize(self.0)).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(value));
        // SAFETY: this wrapper is created and used only while the dialog HWND is live.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for DialogParent {
    fn display_handle(&self) -> std::result::Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

fn add_combo_items(hwnd: HWND, control_id: i32, items: &[&str]) -> Result<()> {
    for item in items {
        let wide = wide_string(item);
        // SAFETY: the combo copies this terminated string during the synchronous message.
        let result = unsafe {
            SendDlgItemMessageW(
                hwnd,
                control_id,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide.as_ptr() as isize),
            )
        };
        if result.0 < 0 {
            bail!("could not populate settings control {control_id}");
        }
    }
    Ok(())
}

fn combo_selection(hwnd: HWND, control_id: i32) -> Result<usize> {
    // SAFETY: this synchronously queries a combo box in the live dialog.
    let result =
        unsafe { SendDlgItemMessageW(hwnd, control_id, CB_GETCURSEL, WPARAM(0), LPARAM(0)) };
    usize::try_from(result.0)
        .with_context(|| format!("settings control {control_id} has no selection"))
}

fn set_combo_selection(hwnd: HWND, control_id: i32, index: usize) -> Result<()> {
    // SAFETY: the index maps to an item inserted during dialog initialization.
    let result =
        unsafe { SendDlgItemMessageW(hwnd, control_id, CB_SETCURSEL, WPARAM(index), LPARAM(0)) };
    if result.0 < 0 {
        bail!("could not select settings control {control_id}");
    }
    Ok(())
}

fn read_text(hwnd: HWND, control_id: i32) -> Result<String> {
    String::from_utf16(&read_text_wide(hwnd, control_id)?)
        .with_context(|| format!("settings control {control_id} contains invalid text"))
}

fn read_text_wide(hwnd: HWND, control_id: i32) -> Result<Vec<u16>> {
    // SAFETY: the child HWND remains owned by the live dialog for both calls.
    let control = unsafe { GetDlgItem(Some(hwnd), control_id) }
        .with_context(|| format!("settings control {control_id} is missing"))?;
    let length = unsafe { GetWindowTextLengthW(control) };
    let capacity = usize::try_from(length)
        .context("settings text is too long")?
        .checked_add(1)
        .context("settings text length overflow")?;
    let mut buffer = vec![0_u16; capacity];
    let copied = unsafe { GetWindowTextW(control, &mut buffer) };
    let copied = usize::try_from(copied).context("settings text length is invalid")?;
    buffer.truncate(copied);
    Ok(buffer)
}

fn set_text(hwnd: HWND, control_id: i32, value: &str) -> Result<()> {
    set_text_wide(hwnd, control_id, value.encode_utf16().collect())
}

fn set_path(hwnd: HWND, control_id: i32, path: &std::path::Path) -> Result<()> {
    set_text_wide(hwnd, control_id, path.as_os_str().encode_wide().collect())
}

fn set_text_wide(hwnd: HWND, control_id: i32, mut value: Vec<u16>) -> Result<()> {
    value.push(0);
    // SAFETY: `value` is terminated and alive for the synchronous copy.
    unsafe { SetDlgItemTextW(hwnd, control_id, PCWSTR(value.as_ptr())) }
        .with_context(|| format!("could not update settings control {control_id}"))
}

unsafe fn confirm_discard(hwnd: HWND) -> bool {
    let text = wide_string("Discard unsaved settings changes?");
    let title = wide_string("Rustshot Settings");
    MessageBoxW(
        Some(hwnd),
        PCWSTR(text.as_ptr()),
        PCWSTR(title.as_ptr()),
        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
    ) == IDYES
}

unsafe fn show_native_error(hwnd: HWND, title: &str, description: &str) {
    let title = wide_string(title);
    let description = wide_string(description);
    let _ = MessageBoxW(
        Some(hwnd),
        PCWSTR(description.as_ptr()),
        PCWSTR(title.as_ptr()),
        MB_OK | MB_ICONERROR,
    );
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn low_word(value: usize) -> i32 {
    i32::from((value & 0xffff) as u16)
}

fn high_word(value: usize) -> u32 {
    u32::from(((value >> 16) & 0xffff) as u16)
}

fn hwnd_to_isize(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

fn hwnd_from_isize(value: isize) -> HWND {
    HWND(value as *mut _)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource_define(name: &str) -> i32 {
        let resource = include_str!("../resources/settings.rc");
        let value = resource
            .lines()
            .find_map(|line| {
                let mut parts = line.split_whitespace();
                (parts.next() == Some("#define") && parts.next() == Some(name))
                    .then(|| parts.next())
                    .flatten()
            })
            .unwrap_or_else(|| panic!("resource define {name} should exist"))
            .trim_end_matches('L');
        value
            .parse()
            .unwrap_or_else(|_| panic!("resource define {name} should be decimal"))
    }

    #[test]
    fn rust_and_resource_dialog_ids_stay_in_sync() {
        let expected = [
            ("IDD_SETTINGS", IDD_SETTINGS as i32),
            ("IDC_FULLSCREEN_SHORTCUT", IDC_FULLSCREEN_SHORTCUT),
            ("IDC_REGION_SHORTCUT", IDC_REGION_SHORTCUT),
            ("IDC_AUTOSAVE_DIRECTORY", IDC_AUTOSAVE_DIRECTORY),
            ("IDC_BROWSE_DIRECTORY", IDC_BROWSE_DIRECTORY),
            ("IDC_IMAGE_FORMAT", IDC_IMAGE_FORMAT),
            ("IDC_JPEG_QUALITY", IDC_JPEG_QUALITY),
            ("IDC_PNG_COMPRESSION", IDC_PNG_COMPRESSION),
            ("IDC_MONITOR_SCOPE", IDC_MONITOR_SCOPE),
            ("IDC_INCLUDE_CURSOR", IDC_INCLUDE_CURSOR),
            ("IDC_RESTORE_DEFAULTS", IDC_RESTORE_DEFAULTS),
            ("IDC_SETTINGS_STATUS", IDC_SETTINGS_STATUS),
            ("IDC_JPEG_QUALITY_LABEL", IDC_JPEG_QUALITY_LABEL),
            ("IDC_PNG_COMPRESSION_LABEL", IDC_PNG_COMPRESSION_LABEL),
            ("IDC_PNG_HELP", IDC_PNG_HELP),
        ];

        for (name, value) in expected {
            assert_eq!(resource_define(name), value, "resource ID {name} drifted");
        }
    }

    #[test]
    fn shortcut_recorder_builds_canonical_modifier_order() {
        let shortcut = shortcut_from_virtual_key(
            VK_F1 + 10,
            false,
            ShortcutModifiers {
                shift: true,
                control: true,
                ..Default::default()
            },
        )
        .expect("F11 should be recordable");

        assert_eq!(shortcut.as_str(), "shift+control+F11");
    }

    #[test]
    fn shortcut_recorder_supports_ctrl_shift_backspace() {
        let shortcut = shortcut_from_virtual_key(
            VK_BACK,
            false,
            ShortcutModifiers {
                shift: true,
                control: true,
                ..Default::default()
            },
        )
        .expect("Ctrl+Shift+Backspace should be recordable");

        assert_eq!(shortcut.as_str(), "shift+control+Backspace");
    }

    #[test]
    fn shortcut_recorder_tracks_multiple_held_modifiers() {
        let mut modifiers = ShortcutModifiers::default();

        assert!(modifiers.set_key(VK_CONTROL, true));
        assert!(modifiers.set_key(VK_SHIFT, true));
        assert_eq!(
            modifiers,
            ShortcutModifiers {
                shift: true,
                control: true,
                ..Default::default()
            }
        );

        assert!(modifiers.set_key(VK_SHIFT, false));
        assert!(!modifiers.shift);
        assert!(modifiers.control);
    }

    #[test]
    fn shortcut_recorder_supports_super_and_extended_keys() {
        let print_screen = shortcut_from_virtual_key(
            VK_SNAPSHOT,
            true,
            ShortcutModifiers {
                super_key: true,
                ..Default::default()
            },
        )
        .expect("Print Screen should be recordable");
        let numpad_enter = shortcut_from_virtual_key(
            VK_RETURN,
            true,
            ShortcutModifiers {
                control: true,
                ..Default::default()
            },
        )
        .expect("extended Enter should be recordable");

        assert_eq!(print_screen.as_str(), "super+PrintScreen");
        assert_eq!(numpad_enter.as_str(), "control+NumpadEnter");
    }

    #[test]
    fn shortcut_recorder_preserves_dialog_navigation() {
        assert!(!wants_all_dialog_keys(
            VK_TAB,
            ShortcutModifiers {
                shift: true,
                ..Default::default()
            }
        ));
        assert!(wants_all_dialog_keys(
            VK_TAB,
            ShortcutModifiers {
                control: true,
                ..Default::default()
            }
        ));
        assert!(!wants_all_dialog_keys(
            VK_F4,
            ShortcutModifiers {
                alt: true,
                ..Default::default()
            }
        ));
    }

    #[test]
    fn unsupported_virtual_keys_are_not_recorded() {
        assert!(shortcut_from_virtual_key(0xff, false, ShortcutModifiers::default()).is_none());
    }

    #[test]
    fn every_config_field_round_trips_through_settings_values() {
        let expected = Config {
            fullscreen_shortcut: Shortcut::new("alt+F8").unwrap(),
            region_shortcut: Shortcut::new("control+shift+KeyR").unwrap(),
            autosave_directory: PathBuf::from(r"C:\Capturas\área com espaços"),
            image_format: ScreenshotFormat::Jpeg,
            jpeg_quality: 73,
            png_compression: PngCompression::Best,
            monitor_scope: MonitorScope::VirtualDesktop,
            include_cursor: false,
        };

        let actual = SettingsValues::from_config(&expected)
            .into_config()
            .expect("settings values should be valid");

        assert_eq!(actual, expected);
    }

    #[test]
    fn invalid_quality_is_rejected_even_when_png_is_selected() {
        let mut values = SettingsValues::from_config(&Config::default());
        values.image_format_index = 0;
        values.jpeg_quality = "101".to_owned();

        assert!(values.into_config().is_err());
    }

    #[test]
    fn duplicate_shortcuts_are_rejected() {
        let mut values = SettingsValues::from_config(&Config::default());
        values.region_shortcut = values.fullscreen_shortcut.clone();

        assert!(values.into_config().is_err());
    }

    #[test]
    fn enum_indices_are_checked() {
        let mut values = SettingsValues::from_config(&Config::default());
        values.monitor_scope_index = usize::MAX;

        assert!(values.into_config().is_err());
    }
}
