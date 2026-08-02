//! Opt-in tests that exercise an interactive Windows desktop.
//!
//! Run with:
//! `cargo test --locked --test windows_native -- --ignored --test-threads=1`

#![cfg(target_os = "windows")]
#![allow(unsafe_code)]

use rustshot::{
    frame::{Frame, PhysicalPoint},
    platform::windows::{
        capture::{capture_cursor_monitor, capture_virtual_desktop},
        clipboard::copy_to_clipboard_with_owner,
    },
};
use windows::Win32::{
    System::{DataExchange::IsClipboardFormatAvailable, Ole::CF_DIBV5},
    UI::WindowsAndMessaging::GetDesktopWindow,
};

#[test]
#[ignore = "requires an unlocked interactive Windows desktop"]
fn cursor_monitor_capture_returns_a_nonempty_valid_frame() {
    let frame = capture_cursor_monitor().expect("cursor monitor capture should succeed");

    assert!(frame.width() > 0);
    assert!(frame.height() > 0);
    assert_eq!(
        frame.rgba().len(),
        frame.width() as usize * frame.height() as usize * 4
    );
}

#[test]
#[ignore = "requires an unlocked interactive Windows desktop"]
fn virtual_desktop_capture_returns_a_nonempty_valid_frame() {
    let frame = capture_virtual_desktop().expect("virtual desktop capture should succeed");

    assert!(frame.width() > 0);
    assert!(frame.height() > 0);
    assert_eq!(
        frame.rgba().len(),
        frame.width() as usize * frame.height() as usize * 4
    );
}

#[test]
#[ignore = "replaces the current Windows clipboard contents"]
fn clipboard_export_publishes_a_dibv5_image() {
    let frame = Frame::new(
        PhysicalPoint::default(),
        2,
        2,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    )
    .expect("test frame should be valid");
    // SAFETY: GetDesktopWindow returns a process-independent, always-valid desktop HWND.
    let owner = unsafe { GetDesktopWindow() };

    copy_to_clipboard_with_owner(&frame, owner).expect("clipboard copy should succeed");
    // SAFETY: this call only queries format availability and owns no resources.
    assert!(unsafe { IsClipboardFormatAvailable(u32::from(CF_DIBV5.0)) }.is_ok());
}
