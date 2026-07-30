//! Windows clipboard export using the color-managed `CF_DIBV5` format.

#![allow(unsafe_code)]

use std::{mem::size_of, ptr, slice, thread, time::Duration};

use anyhow::{bail, Context, Result};
use windows::{
    core::Error as WindowsError,
    Win32::{
        Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
        Graphics::Gdi::{BITMAPV5HEADER, BI_BITFIELDS},
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_DIBV5,
        },
    },
};

use crate::frame::Frame;

const RGBA_CHANNELS: usize = 4;
const OPEN_ATTEMPTS: usize = 10;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(10);
const LCS_SRGB: u32 = 0x7352_4742;
const LCS_GM_IMAGES: u32 = 4;

/// Copies a frame to the Windows clipboard using a real Rustshot owner window.
pub fn copy_to_clipboard_with_owner(frame: &Frame, owner: HWND) -> Result<()> {
    if owner.is_invalid() {
        bail!("clipboard owner window is invalid");
    }
    let mut dib = create_dibv5(frame)?;
    let _clipboard = ClipboardGuard::open(owner)?;

    // SAFETY: the guard proves that this thread currently has the clipboard open.
    unsafe { EmptyClipboard() }.context("failed to empty the Windows clipboard")?;

    let handle = dib
        .handle
        .context("clipboard DIB memory was released unexpectedly")?;
    // SAFETY: `handle` is movable global memory containing a valid CF_DIBV5
    // payload. On success Windows assumes ownership, and `release` prevents a
    // second free from Rust.
    unsafe { SetClipboardData(u32::from(CF_DIBV5.0), Some(HANDLE(handle.0))) }
        .context("failed to place the screenshot on the Windows clipboard")?;
    dib.release();
    Ok(())
}

fn create_dibv5(frame: &Frame) -> Result<OwnedGlobalMemory> {
    let width =
        i32::try_from(frame.width()).context("clipboard image width exceeds the DIB limit")?;
    let height =
        i32::try_from(frame.height()).context("clipboard image height exceeds the DIB limit")?;
    let image_bytes = frame.rgba().len();
    let image_size =
        u32::try_from(image_bytes).context("clipboard image is too large for a DIB")?;
    let header_size = size_of::<BITMAPV5HEADER>();
    let allocation_size = header_size
        .checked_add(image_bytes)
        .context("clipboard allocation size overflow")?;

    // SAFETY: allocation size is checked and the returned handle is managed by
    // `OwnedGlobalMemory` until Windows takes ownership.
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, allocation_size) }
        .context("failed to allocate clipboard memory")?;
    let memory = OwnedGlobalMemory::new(memory);

    // SAFETY: the handle is valid movable global memory allocated above.
    let base = unsafe { GlobalLock(memory.handle()) }.cast::<u8>();
    if base.is_null() {
        return Err(WindowsError::from_thread()).context("failed to lock clipboard memory");
    }

    let header = BITMAPV5HEADER {
        bV5Size: u32::try_from(header_size).expect("BITMAPV5HEADER size always fits u32"),
        bV5Width: width,
        // A negative height makes the DIB top-down, matching Frame's row order.
        bV5Height: -height,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: image_size,
        bV5RedMask: 0x00ff_0000,
        bV5GreenMask: 0x0000_ff00,
        bV5BlueMask: 0x0000_00ff,
        bV5AlphaMask: 0xff00_0000,
        bV5CSType: LCS_SRGB,
        bV5Intent: LCS_GM_IMAGES,
        ..Default::default()
    };

    // SAFETY: `base` addresses `allocation_size` writable bytes. The header and
    // following pixel slice are non-overlapping and exactly fill that allocation.
    unsafe {
        ptr::write(base.cast::<BITMAPV5HEADER>(), header);
        let output = slice::from_raw_parts_mut(base.add(header_size), image_bytes);
        rgba_to_bgra(frame.rgba(), output)?;
    }

    // GlobalUnlock reports zero both for a successful final unlock and for an
    // error, so the windows-rs Result is intentionally ignored here.
    // SAFETY: this balances the successful `GlobalLock` above.
    let _ = unsafe { GlobalUnlock(memory.handle()) };
    Ok(memory)
}

fn rgba_to_bgra(source: &[u8], destination: &mut [u8]) -> Result<()> {
    if source.len() != destination.len() || !source.len().is_multiple_of(RGBA_CHANNELS) {
        bail!("invalid RGBA clipboard buffer length");
    }

    for (rgba, bgra) in source
        .chunks_exact(RGBA_CHANNELS)
        .zip(destination.chunks_exact_mut(RGBA_CHANNELS))
    {
        bgra.copy_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
    }
    Ok(())
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open(owner: HWND) -> Result<Self> {
        let mut last_error = None;
        for attempt in 0..OPEN_ATTEMPTS {
            // SAFETY: `owner` is the live Rustshot overlay HWND.
            match unsafe { OpenClipboard(Some(owner)) } {
                Ok(()) => return Ok(Self),
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < OPEN_ATTEMPTS {
                thread::sleep(OPEN_RETRY_DELAY);
            }
        }

        let error = last_error.unwrap_or_else(WindowsError::from_thread);
        Err(error).context("the Windows clipboard is busy")
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: the guard is created only after OpenClipboard succeeds and is
        // neither cloneable nor movable across threads.
        let _ = unsafe { CloseClipboard() };
    }
}

struct OwnedGlobalMemory {
    handle: Option<HGLOBAL>,
}

impl OwnedGlobalMemory {
    const fn new(handle: HGLOBAL) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    fn handle(&self) -> HGLOBAL {
        self.handle
            .expect("global memory handle must exist while owned")
    }

    fn release(&mut self) {
        self.handle = None;
    }
}

impl Drop for OwnedGlobalMemory {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: this object has unique ownership unless `release` was called.
            let _ = unsafe { GlobalFree(Some(handle)) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::PhysicalPoint;

    #[test]
    fn converts_rgba_to_bgra() {
        let mut output = [0_u8; 8];
        rgba_to_bgra(&[10, 20, 30, 40, 50, 60, 70, 80], &mut output).unwrap();
        assert_eq!(output, [30, 20, 10, 40, 70, 60, 50, 80]);
    }

    #[test]
    fn rejects_a_null_owner_without_touching_the_clipboard() {
        let frame = Frame::new(PhysicalPoint::default(), 1, 1, vec![0, 0, 0, 255]).unwrap();
        let error = copy_to_clipboard_with_owner(&frame, HWND::default()).unwrap_err();
        assert!(error.to_string().contains("owner window is invalid"));
    }
}
