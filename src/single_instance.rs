//! Process-wide single-instance coordination for the Windows tray application.

#![allow(unsafe_code)]

use anyhow::{Context, Result};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE},
        System::Threading::CreateMutexW,
    },
};

const INSTANCE_NAME: &str = r"Local\io.github.CodyKoInABox.Rustshot.SingleInstance";

/// Owns the named Windows kernel object for as long as Rustshot is running.
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    /// Acquires Rustshot's per-login-session instance marker.
    pub fn acquire() -> Result<Option<Self>> {
        Self::acquire_named(INSTANCE_NAME)
    }

    fn acquire_named(name: &str) -> Result<Option<Self>> {
        let wide_name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide_name` is NUL-terminated and remains alive for the call.
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(wide_name.as_ptr())) }
            .context("could not create Rustshot's single-instance marker")?;
        // CreateMutexW returns a usable handle and sets last-error when the named
        // object already existed. This check must immediately follow the call.
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_running {
            // SAFETY: this process owns the valid handle returned above.
            let _ = unsafe { CloseHandle(handle) };
            return Ok(None);
        }

        Ok(Some(Self { handle }))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this guard and is closed exactly once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::SingleInstance;

    static NAME_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn a_named_marker_allows_only_one_live_owner() {
        let sequence = NAME_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            r"Local\Rustshot.SingleInstance.Test.{}.{}",
            std::process::id(),
            sequence
        );

        let first = SingleInstance::acquire_named(&name)
            .expect("first acquisition should succeed")
            .expect("first acquisition should own the marker");
        assert!(SingleInstance::acquire_named(&name)
            .expect("duplicate acquisition should be detected")
            .is_none());

        drop(first);
        assert!(SingleInstance::acquire_named(&name)
            .expect("marker should become available after its owner exits")
            .is_some());
    }
}
