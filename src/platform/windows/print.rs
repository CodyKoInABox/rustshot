//! Native Windows printing for screenshot frames.

#![allow(unsafe_code)]

use std::mem::size_of;

use anyhow::{bail, Context, Result};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{GlobalFree, HGLOBAL, HWND},
        Graphics::Gdi::{
            DeleteDC, GetDeviceCaps, SetStretchBltMode, StretchDIBits, BITMAPINFO, BI_RGB,
            DIB_RGB_COLORS, GDI_ERROR, HALFTONE, HORZRES, SRCCOPY, VERTRES,
        },
        Storage::Xps::{AbortDoc, EndDoc, EndPage, StartDocW, StartPage, DOCINFOW},
        UI::Controls::Dialogs::{
            CommDlgExtendedError, PrintDlgW, PD_NOPAGENUMS, PD_NOSELECTION, PD_RETURNDC,
            PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW,
        },
    },
};

use crate::frame::Frame;

const RGBA_CHANNELS: usize = 4;

/// Result of showing the native print dialog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrintOutcome {
    Printed,
    Cancelled,
}

/// Shows the native Windows print dialog and prints the frame on one page.
///
/// This function blocks while the printer driver spools the page. Callers should
/// arrange for the actual print operation to run away from latency-sensitive UI
/// work when their windowing architecture permits it.
pub fn print_frame(frame: &Frame, owner: Option<HWND>) -> Result<PrintOutcome> {
    let mut dialog = PRINTDLGW {
        lStructSize: u32::try_from(size_of::<PRINTDLGW>()).expect("PRINTDLGW size always fits u32"),
        hwndOwner: owner.unwrap_or_default(),
        Flags: PD_RETURNDC | PD_NOPAGENUMS | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE,
        nMinPage: 1,
        nMaxPage: 1,
        nFromPage: 1,
        nToPage: 1,
        nCopies: 1,
        ..Default::default()
    };

    // SAFETY: `dialog` is initialized with the documented structure size and all
    // pointer fields are null. The API owns its modal lifetime.
    let accepted = unsafe { PrintDlgW(&mut dialog) }.as_bool();
    if !accepted {
        // SAFETY: valid immediately after a common-dialog API failure.
        let extended_error = unsafe { CommDlgExtendedError() }.0;
        cleanup_dialog_allocations(&mut dialog);
        if extended_error == 0 {
            return Ok(PrintOutcome::Cancelled);
        }
        bail!("Windows print dialog failed with error 0x{extended_error:08x}");
    }

    let resources = PrintResources::take_from(&mut dialog);
    if resources.dc.is_invalid() {
        bail!("the selected printer did not return a device context");
    }
    print_to_dc(frame, resources.dc)?;
    Ok(PrintOutcome::Printed)
}

fn print_to_dc(frame: &Frame, dc: windows::Win32::Graphics::Gdi::HDC) -> Result<()> {
    // SAFETY: `dc` is the valid printer DC returned by PrintDlgW.
    let page_width = unsafe { GetDeviceCaps(Some(dc), HORZRES) };
    // SAFETY: same valid printer DC.
    let page_height = unsafe { GetDeviceCaps(Some(dc), VERTRES) };
    if page_width <= 0 || page_height <= 0 {
        bail!("printer reported an invalid printable area");
    }

    let (destination_width, destination_height) = aspect_fit(
        frame.width(),
        frame.height(),
        u32::try_from(page_width).context("invalid printer page width")?,
        u32::try_from(page_height).context("invalid printer page height")?,
    );
    let destination_x = (page_width
        - i32::try_from(destination_width).context("scaled print width exceeds GDI limits")?)
        / 2;
    let destination_y = (page_height
        - i32::try_from(destination_height).context("scaled print height exceeds GDI limits")?)
        / 2;
    let bitmap = print_bitmap(frame)?;
    let document_name: Vec<u16> = "Rustshot Screenshot\0".encode_utf16().collect();
    let document = DOCINFOW {
        cbSize: i32::try_from(size_of::<DOCINFOW>()).expect("DOCINFOW size always fits i32"),
        lpszDocName: PCWSTR(document_name.as_ptr()),
        ..Default::default()
    };

    // SAFETY: `dc` is valid and `document` points to a live, terminated UTF-16
    // title for this call.
    if unsafe { StartDocW(dc, &document) } <= 0 {
        return Err(windows::core::Error::from_thread()).context("failed to start the print job");
    }

    // SAFETY: a document is active on this printer DC.
    if unsafe { StartPage(dc) } <= 0 {
        abort_document(dc);
        return Err(windows::core::Error::from_thread())
            .context("failed to start the printer page");
    }

    // SAFETY: the printer DC is active. HALFTONE is a valid stretch mode.
    let previous_mode = unsafe { SetStretchBltMode(dc, HALFTONE) };
    if previous_mode == 0 {
        abort_document(dc);
        return Err(windows::core::Error::from_thread())
            .context("failed to configure high-quality printer scaling");
    }

    // SAFETY: `bitmap.pixels` contains the tightly packed 32-bit top-down DIB
    // described by `bitmap.info`, and all values fit the GDI integer API.
    let copied_lines = unsafe {
        StretchDIBits(
            dc,
            destination_x,
            destination_y,
            i32::try_from(destination_width).expect("validated destination width fits i32"),
            i32::try_from(destination_height).expect("validated destination height fits i32"),
            0,
            0,
            i32::try_from(frame.width()).context("image width exceeds GDI limit")?,
            i32::try_from(frame.height()).context("image height exceeds GDI limit")?,
            Some(bitmap.pixels.as_ptr().cast()),
            &bitmap.info,
            DIB_RGB_COLORS,
            SRCCOPY,
        )
    };
    if copied_lines == 0 || copied_lines == GDI_ERROR {
        abort_document(dc);
        bail!("printer driver rejected the screenshot bitmap");
    }

    // SAFETY: a page is active on this printer DC.
    if unsafe { EndPage(dc) } <= 0 {
        abort_document(dc);
        return Err(windows::core::Error::from_thread())
            .context("failed to finish the printer page");
    }

    // SAFETY: the page is closed and the document remains active.
    if unsafe { EndDoc(dc) } <= 0 {
        return Err(windows::core::Error::from_thread()).context("failed to finish the print job");
    }

    Ok(())
}

fn abort_document(dc: windows::Win32::Graphics::Gdi::HDC) {
    // SAFETY: this helper is called only while a document is active. AbortDoc is
    // best effort because the original printer error is more useful to the user.
    let _ = unsafe { AbortDoc(dc) };
}

fn print_bitmap(frame: &Frame) -> Result<PrintBitmap> {
    let width = i32::try_from(frame.width()).context("image width exceeds the GDI limit")?;
    let height = i32::try_from(frame.height()).context("image height exceeds the GDI limit")?;
    let mut pixels = Vec::with_capacity(frame.rgba().len());
    for rgba in frame.rgba().chunks_exact(RGBA_CHANNELS) {
        // BI_RGB's fourth byte is unused. Zero avoids printer-driver alpha quirks.
        pixels.extend_from_slice(&[rgba[2], rgba[1], rgba[0], 0]);
    }
    if pixels.len() != frame.rgba().len() {
        bail!("invalid RGBA frame buffer");
    }

    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize =
        u32::try_from(size_of_val(&info.bmiHeader)).expect("BITMAPINFOHEADER size always fits u32");
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;
    info.bmiHeader.biSizeImage =
        u32::try_from(pixels.len()).context("print bitmap is too large for GDI")?;

    Ok(PrintBitmap { info, pixels })
}

fn aspect_fit(
    image_width: u32,
    image_height: u32,
    page_width: u32,
    page_height: u32,
) -> (u32, u32) {
    let image_width_64 = u64::from(image_width);
    let image_height_64 = u64::from(image_height);
    let page_width_64 = u64::from(page_width);
    let page_height_64 = u64::from(page_height);

    if page_width_64 * image_height_64 <= page_height_64 * image_width_64 {
        let height = (image_height_64 * page_width_64 / image_width_64).max(1);
        (page_width, u32::try_from(height).unwrap_or(page_height))
    } else {
        let width = (image_width_64 * page_height_64 / image_height_64).max(1);
        (u32::try_from(width).unwrap_or(page_width), page_height)
    }
}

fn cleanup_dialog_allocations(dialog: &mut PRINTDLGW) {
    if !dialog.hDC.is_invalid() {
        // SAFETY: PrintDlgW created this DC and ownership is still with the caller.
        let _ = unsafe { DeleteDC(dialog.hDC) };
        dialog.hDC = Default::default();
    }
    free_global(&mut dialog.hDevMode);
    free_global(&mut dialog.hDevNames);
}

fn free_global(handle: &mut HGLOBAL) {
    if !handle.is_invalid() {
        // SAFETY: PrintDlgW allocated this movable global block for the caller.
        let _ = unsafe { GlobalFree(Some(*handle)) };
        *handle = Default::default();
    }
}

struct PrintResources {
    dc: windows::Win32::Graphics::Gdi::HDC,
    dev_mode: HGLOBAL,
    dev_names: HGLOBAL,
}

impl PrintResources {
    fn take_from(dialog: &mut PRINTDLGW) -> Self {
        let resources = Self {
            dc: dialog.hDC,
            dev_mode: dialog.hDevMode,
            dev_names: dialog.hDevNames,
        };
        dialog.hDC = Default::default();
        dialog.hDevMode = Default::default();
        dialog.hDevNames = Default::default();
        resources
    }
}

impl Drop for PrintResources {
    fn drop(&mut self) {
        if !self.dc.is_invalid() {
            // SAFETY: the printer DC is uniquely owned by this guard.
            let _ = unsafe { DeleteDC(self.dc) };
        }
        free_global(&mut self.dev_mode);
        free_global(&mut self.dev_names);
    }
}

struct PrintBitmap {
    info: BITMAPINFO,
    pixels: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::aspect_fit;

    #[test]
    fn fits_landscape_image_on_portrait_page() {
        assert_eq!(aspect_fit(1920, 1080, 600, 800), (600, 337));
    }

    #[test]
    fn fits_portrait_image_on_landscape_page() {
        assert_eq!(aspect_fit(1080, 1920, 800, 600), (337, 600));
    }
}
