//! Owned RGBA screenshot pixels and physical-desktop geometry.

use thiserror::Error;

const RGBA_CHANNELS: usize = 4;

/// A point in physical desktop pixels.
///
/// Desktop coordinates can be negative when a monitor is positioned to the
/// left of, or above, the primary monitor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PhysicalPoint {
    x: i32,
    y: i32,
}

impl PhysicalPoint {
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn x(self) -> i32 {
        self.x
    }

    #[must_use]
    pub const fn y(self) -> i32 {
        self.y
    }
}

/// A non-empty, half-open rectangle in physical desktop pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalRect {
    origin: PhysicalPoint,
    width: u32,
    height: u32,
}

impl PhysicalRect {
    /// Creates a rectangle, rejecting empty or unrepresentable geometry.
    pub fn new(origin: PhysicalPoint, width: u32, height: u32) -> Result<Self, FrameError> {
        validate_geometry(origin, width, height)?;
        Ok(Self {
            origin,
            width,
            height,
        })
    }

    #[must_use]
    pub const fn origin(self) -> PhysicalPoint {
        self.origin
    }

    #[must_use]
    pub const fn x(self) -> i32 {
        self.origin.x
    }

    #[must_use]
    pub const fn y(self) -> i32 {
        self.origin.y
    }

    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn right(self) -> i64 {
        i64::from(self.origin.x) + i64::from(self.width)
    }

    #[must_use]
    pub fn bottom(self) -> i64 {
        i64::from(self.origin.y) + i64::from(self.height)
    }

    #[must_use]
    pub fn contains(self, other: Self) -> bool {
        i64::from(other.x()) >= i64::from(self.x())
            && i64::from(other.y()) >= i64::from(self.y())
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }
}

/// A tightly packed, row-major image in straight-alpha RGBA8 format.
///
/// `origin` locates the top-left pixel in physical desktop coordinates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    origin: PhysicalPoint,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Frame {
    /// Creates a frame after checking all geometry and buffer invariants.
    pub fn new(
        origin: PhysicalPoint,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<Self, FrameError> {
        validate_geometry(origin, width, height)?;
        let expected = expected_len(width, height)?;
        if rgba.len() != expected {
            return Err(FrameError::InvalidBufferLength {
                expected,
                actual: rgba.len(),
            });
        }

        Ok(Self {
            origin,
            width,
            height,
            rgba,
        })
    }

    #[must_use]
    pub const fn origin(&self) -> PhysicalPoint {
        self.origin
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn bounds(&self) -> PhysicalRect {
        // Frame construction already validated this geometry.
        PhysicalRect {
            origin: self.origin,
            width: self.width,
            height: self.height,
        }
    }

    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    #[must_use]
    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }

    #[must_use]
    pub fn stride(&self) -> usize {
        usize::try_from(self.width).expect("validated frame width fits usize") * RGBA_CHANNELS
    }

    /// Crops using physical desktop coordinates.
    ///
    /// The returned frame keeps the requested rectangle's physical origin.
    pub fn crop(&self, rect: PhysicalRect) -> Result<Self, FrameError> {
        if !self.bounds().contains(rect) {
            return Err(FrameError::CropOutsideFrame {
                crop: rect,
                frame: self.bounds(),
            });
        }

        let x_offset = usize::try_from(i64::from(rect.x()) - i64::from(self.origin.x))
            .expect("contained crop has a non-negative x offset");
        let y_offset = usize::try_from(i64::from(rect.y()) - i64::from(self.origin.y))
            .expect("contained crop has a non-negative y offset");
        let crop_stride =
            usize::try_from(rect.width).expect("validated crop width fits usize") * RGBA_CHANNELS;
        let source_stride = self.stride();
        let crop_height = usize::try_from(rect.height).expect("validated crop height fits usize");
        let mut rgba = Vec::with_capacity(expected_len(rect.width, rect.height)?);

        for row in 0..crop_height {
            let start = (y_offset + row)
                .checked_mul(source_stride)
                .and_then(|offset| {
                    x_offset
                        .checked_mul(RGBA_CHANNELS)
                        .and_then(|x| offset.checked_add(x))
                })
                .ok_or(FrameError::BufferSizeOverflow {
                    width: self.width,
                    height: self.height,
                })?;
            let end = start
                .checked_add(crop_stride)
                .ok_or(FrameError::BufferSizeOverflow {
                    width: rect.width,
                    height: rect.height,
                })?;
            rgba.extend_from_slice(&self.rgba[start..end]);
        }

        Self::new(rect.origin, rect.width, rect.height, rgba)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum FrameError {
    #[error("a frame or rectangle cannot be empty ({width}x{height})")]
    EmptyDimensions { width: u32, height: u32 },

    #[error("RGBA buffer size overflows this platform for {width}x{height}")]
    BufferSizeOverflow { width: u32, height: u32 },

    #[error("physical rectangle extends beyond the representable desktop coordinates")]
    CoordinateOverflow,

    #[error("invalid RGBA buffer length: expected {expected} bytes, got {actual}")]
    InvalidBufferLength { expected: usize, actual: usize },

    #[error("crop {crop:?} is outside frame bounds {frame:?}")]
    CropOutsideFrame {
        crop: PhysicalRect,
        frame: PhysicalRect,
    },
}

fn validate_geometry(origin: PhysicalPoint, width: u32, height: u32) -> Result<(), FrameError> {
    if width == 0 || height == 0 {
        return Err(FrameError::EmptyDimensions { width, height });
    }
    expected_len(width, height)?;

    // A half-open end may be one beyond i32::MAX, while every actual pixel
    // coordinate must still fit i32.
    let maximum_end = i64::from(i32::MAX) + 1;
    let right = i64::from(origin.x) + i64::from(width);
    let bottom = i64::from(origin.y) + i64::from(height);
    if right > maximum_end || bottom > maximum_end {
        return Err(FrameError::CoordinateOverflow);
    }

    Ok(())
}

fn expected_len(width: u32, height: u32) -> Result<usize, FrameError> {
    let pixels = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(RGBA_CHANNELS));
    pixels.ok_or(FrameError::BufferSizeOverflow { width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterned_frame() -> Frame {
        let rgba = (0_u8..24).collect();
        Frame::new(PhysicalPoint::new(-2, 5), 3, 2, rgba).unwrap()
    }

    #[test]
    fn rejects_empty_and_mismatched_frames() {
        assert_eq!(
            Frame::new(PhysicalPoint::default(), 0, 1, Vec::new()),
            Err(FrameError::EmptyDimensions {
                width: 0,
                height: 1
            })
        );
        assert_eq!(
            Frame::new(PhysicalPoint::default(), 2, 2, vec![0; 15]),
            Err(FrameError::InvalidBufferLength {
                expected: 16,
                actual: 15
            })
        );
    }

    #[test]
    fn rejects_unrepresentable_physical_bounds() {
        assert_eq!(
            PhysicalRect::new(PhysicalPoint::new(i32::MAX, 0), 2, 1),
            Err(FrameError::CoordinateOverflow)
        );
    }

    #[test]
    fn crops_in_physical_coordinates_and_preserves_origin() {
        let frame = patterned_frame();
        let crop = frame
            .crop(PhysicalRect::new(PhysicalPoint::new(-1, 5), 2, 2).unwrap())
            .unwrap();

        assert_eq!(crop.origin(), PhysicalPoint::new(-1, 5));
        assert_eq!((crop.width(), crop.height()), (2, 2));
        assert_eq!(
            crop.rgba(),
            &[4, 5, 6, 7, 8, 9, 10, 11, 16, 17, 18, 19, 20, 21, 22, 23]
        );
    }

    #[test]
    fn rejects_a_crop_outside_the_frame() {
        let frame = patterned_frame();
        let rect = PhysicalRect::new(PhysicalPoint::new(-3, 5), 1, 1).unwrap();
        assert!(matches!(
            frame.crop(rect),
            Err(FrameError::CropOutsideFrame { .. })
        ));
    }
}
