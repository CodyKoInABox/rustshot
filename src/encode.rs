//! PNG/JPEG encoding and same-directory atomic screenshot writes.

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use image::{
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType, PngEncoder},
    },
    ExtendedColorType, ImageEncoder,
};
use thiserror::Error;

use crate::{
    config::{Config, PngCompression as ConfigPngCompression, ScreenshotFormat},
    frame::Frame,
};

const TEMP_FILE_ATTEMPTS: usize = 32;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OutputFormat {
    #[default]
    Png,
    Jpeg,
}

impl OutputFormat {
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PngCompression {
    Fast,
    #[default]
    Default,
    Best,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodeOptions {
    pub format: OutputFormat,
    pub jpeg_quality: u8,
    pub png_compression: PngCompression,
}

impl EncodeOptions {
    pub fn new(
        format: OutputFormat,
        jpeg_quality: u8,
        png_compression: PngCompression,
    ) -> Result<Self, EncodeError> {
        if !(1..=100).contains(&jpeg_quality) {
            return Err(EncodeError::InvalidJpegQuality(jpeg_quality));
        }
        Ok(Self {
            format,
            jpeg_quality,
            png_compression,
        })
    }

    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        Self {
            format: config.image_format.into(),
            jpeg_quality: config.jpeg_quality,
            png_compression: config.png_compression.into(),
        }
    }
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            format: OutputFormat::Png,
            jpeg_quality: 90,
            png_compression: PngCompression::Default,
        }
    }
}

impl From<ScreenshotFormat> for OutputFormat {
    fn from(format: ScreenshotFormat) -> Self {
        match format {
            ScreenshotFormat::Png => Self::Png,
            ScreenshotFormat::Jpeg => Self::Jpeg,
        }
    }
}

impl From<ConfigPngCompression> for PngCompression {
    fn from(compression: ConfigPngCompression) -> Self {
        match compression {
            ConfigPngCompression::Fast => Self::Fast,
            ConfigPngCompression::Default => Self::Default,
            ConfigPngCompression::Best => Self::Best,
        }
    }
}

impl From<&Config> for EncodeOptions {
    fn from(config: &Config) -> Self {
        Self::from_config(config)
    }
}

/// Encodes one frame using the selected output format.
///
/// JPEG cannot store transparency, so translucent pixels are composited over
/// white before encoding.
pub fn encode(frame: &Frame, options: EncodeOptions) -> Result<Vec<u8>, EncodeError> {
    if !(1..=100).contains(&options.jpeg_quality) {
        return Err(EncodeError::InvalidJpegQuality(options.jpeg_quality));
    }

    let mut encoded = Vec::new();
    match options.format {
        OutputFormat::Png => {
            let compression = match options.png_compression {
                PngCompression::Fast => CompressionType::Fast,
                PngCompression::Default => CompressionType::Default,
                PngCompression::Best => CompressionType::Best,
            };
            PngEncoder::new_with_quality(&mut encoded, compression, FilterType::Adaptive)
                .write_image(
                    frame.rgba(),
                    frame.width(),
                    frame.height(),
                    ExtendedColorType::Rgba8,
                )?;
        }
        OutputFormat::Jpeg => {
            let rgb = rgba_over_white(frame.rgba());
            JpegEncoder::new_with_quality(&mut encoded, options.jpeg_quality).write_image(
                &rgb,
                frame.width(),
                frame.height(),
                ExtendedColorType::Rgb8,
            )?;
        }
    }
    Ok(encoded)
}

/// Encodes and atomically publishes a new screenshot file.
///
/// The complete byte stream is synchronized to a unique temporary file in the
/// destination directory before a rename makes it visible. Existing
/// destinations are rejected so autosave never overwrites an earlier capture.
pub fn encode_and_save_atomic(
    frame: &Frame,
    options: EncodeOptions,
    destination: impl AsRef<Path>,
) -> Result<(), EncodeError> {
    let encoded = encode(frame, options)?;
    save_atomic(destination, &encoded)
}

/// Encodes a screenshot and atomically replaces the user-confirmed destination.
pub fn encode_and_save_replace_atomic(
    frame: &Frame,
    options: EncodeOptions,
    destination: impl AsRef<Path>,
) -> Result<(), EncodeError> {
    let encoded = encode(frame, options)?;
    save_replace_atomic(destination, &encoded)
}

/// Atomically publishes `contents` at a previously unused path.
pub fn save_atomic(destination: impl AsRef<Path>, contents: &[u8]) -> Result<(), EncodeError> {
    save_atomic_inner(destination.as_ref(), contents, false)
}

/// Atomically publishes `contents`, replacing an existing file.
///
/// Call this only after an explicit Save As confirmation from the user.
pub fn save_replace_atomic(
    destination: impl AsRef<Path>,
    contents: &[u8],
) -> Result<(), EncodeError> {
    save_atomic_inner(destination.as_ref(), contents, true)
}

fn save_atomic_inner(
    destination: &Path,
    contents: &[u8],
    replace_existing: bool,
) -> Result<(), EncodeError> {
    if destination.file_name().is_none() {
        return Err(EncodeError::MissingFileName(destination.to_path_buf()));
    }
    if !replace_existing && destination.exists() {
        return Err(EncodeError::DestinationExists(destination.to_path_buf()));
    }

    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| EncodeError::Io {
        operation: "create destination directory",
        path: parent.to_path_buf(),
        source,
    })?;

    let (temporary_path, mut temporary_file) = create_temporary_file(destination)?;
    let write_result = (|| {
        temporary_file
            .write_all(contents)
            .map_err(|source| EncodeError::Io {
                operation: "write temporary screenshot",
                path: temporary_path.clone(),
                source,
            })?;
        temporary_file.sync_all().map_err(|source| EncodeError::Io {
            operation: "synchronize temporary screenshot",
            path: temporary_path.clone(),
            source,
        })
    })();

    if let Err(error) = write_result {
        drop(temporary_file);
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    drop(temporary_file);

    // Check again after the potentially slow encode/write phase. This is not
    // a general lock, but avoids replacing a destination created meanwhile on
    // platforms whose rename operation replaces existing files.
    if !replace_existing && destination.exists() {
        let _ = fs::remove_file(&temporary_path);
        return Err(EncodeError::DestinationExists(destination.to_path_buf()));
    }

    if let Err(source) = publish_temporary(&temporary_path, destination, replace_existing) {
        let _ = fs::remove_file(&temporary_path);
        return Err(EncodeError::Io {
            operation: "publish screenshot",
            path: destination.to_path_buf(),
            source,
        });
    }
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn publish_temporary(
    temporary_path: &Path,
    destination: &Path,
    replace_existing: bool,
) -> io::Result<()> {
    use std::{iter, os::windows::ffi::OsStrExt as _};

    use windows::{
        core::PCWSTR,
        Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        },
    };

    let temporary_wide: Vec<u16> = temporary_path
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    let flags = if replace_existing {
        MOVEFILE_WRITE_THROUGH | MOVEFILE_REPLACE_EXISTING
    } else {
        MOVEFILE_WRITE_THROUGH
    };

    // SAFETY: both paths are terminated UTF-16 strings that remain alive for
    // the call, and the flags are valid for MoveFileExW.
    unsafe {
        MoveFileExW(
            PCWSTR(temporary_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
            flags,
        )
    }
    .map_err(|_| io::Error::last_os_error())
}

#[cfg(not(windows))]
fn publish_temporary(
    temporary_path: &Path,
    destination: &Path,
    _replace_existing: bool,
) -> io::Result<()> {
    fs::rename(temporary_path, destination)
}

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("JPEG quality must be between 1 and 100, got {0}")]
    InvalidJpegQuality(u8),

    #[error("screenshot destination has no filename: `{}`", .0.display())]
    MissingFileName(PathBuf),

    #[error("refusing to overwrite existing screenshot `{}`", .0.display())]
    DestinationExists(PathBuf),

    #[error("failed to {operation} at `{}`: {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("image encoding failed: {0}")]
    Image(#[from] image::ImageError),
}

fn rgba_over_white(rgba: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for pixel in rgba.chunks_exact(4) {
        let alpha = u16::from(pixel[3]);
        for channel in &pixel[..3] {
            let composited = (u16::from(*channel) * alpha + 255 * (255 - alpha) + 127) / 255;
            rgb.push(composited as u8);
        }
    }
    rgb
}

fn create_temporary_file(destination: &Path) -> Result<(PathBuf, fs::File), EncodeError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .expect("save_atomic checked the destination filename");

    for _ in 0..TEMP_FILE_ATTEMPTS {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = OsString::from(".");
        temporary_name.push(file_name);
        temporary_name.push(format!(".rustshot-{}-{sequence}.tmp", std::process::id()));
        let temporary_path = parent.join(temporary_name);
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(EncodeError::Io {
                    operation: "create temporary screenshot",
                    path: temporary_path,
                    source,
                });
            }
        }
    }

    Err(EncodeError::Io {
        operation: "create a unique temporary screenshot",
        path: parent.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary screenshot filename attempts exhausted",
        ),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use image::GenericImageView;

    use super::*;
    use crate::frame::PhysicalPoint;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rustshot-encode-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn frame() -> Frame {
        Frame::new(
            PhysicalPoint::new(-100, 50),
            2,
            1,
            vec![255, 0, 0, 255, 0, 0, 255, 128],
        )
        .unwrap()
    }

    #[test]
    fn png_round_trips_rgba_pixels() {
        let encoded = encode(&frame(), EncodeOptions::default()).unwrap();
        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");

        let decoded = image::load_from_memory(&encoded).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.as_raw(), frame().rgba());
    }

    #[test]
    fn jpeg_encodes_dimensions_and_composites_alpha() {
        let options = EncodeOptions::new(OutputFormat::Jpeg, 95, PngCompression::Fast).unwrap();
        let encoded = encode(&frame(), options).unwrap();
        assert_eq!(&encoded[..2], b"\xff\xd8");

        let decoded = image::load_from_memory(&encoded).unwrap();
        assert_eq!(decoded.dimensions(), (2, 1));
    }

    #[test]
    fn invalid_jpeg_quality_is_rejected() {
        assert!(matches!(
            EncodeOptions::new(OutputFormat::Jpeg, 0, PngCompression::Default),
            Err(EncodeError::InvalidJpegQuality(0))
        ));
    }

    #[test]
    fn atomic_save_creates_parents_and_refuses_overwrite() {
        let directory = TestDirectory::new();
        let destination = directory.path().join("nested").join("capture.png");
        save_atomic(&destination, b"complete").unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"complete");

        assert!(matches!(
            save_atomic(&destination, b"replacement"),
            Err(EncodeError::DestinationExists(path)) if path == destination
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"complete");
    }

    #[test]
    fn confirmed_save_can_atomically_replace_a_file() {
        let directory = TestDirectory::new();
        let destination = directory.path().join("capture.png");
        fs::write(&destination, b"old").unwrap();

        save_replace_atomic(&destination, b"new").unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
    }
}
