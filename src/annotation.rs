//! Platform-independent vector annotations and raster flattening.

use std::mem;

use thiserror::Error;
use tiny_skia::{LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

use crate::frame::{Frame, FrameError};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    x: f32,
    y: f32,
}

impl Point {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn x(self) -> f32 {
        self.x
    }

    #[must_use]
    pub const fn y(self) -> f32 {
        self.y
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Color {
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
}

impl Color {
    pub const BLACK: Self = Self::rgba(0, 0, 0, 255);
    pub const RED: Self = Self::rgba(255, 0, 0, 255);
    pub const HIGHLIGHT_YELLOW: Self = Self::rgba(255, 230, 0, 96);

    #[must_use]
    pub const fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    #[must_use]
    pub const fn red(self) -> u8 {
        self.red
    }

    #[must_use]
    pub const fn green(self) -> u8 {
        self.green
    }

    #[must_use]
    pub const fn blue(self) -> u8 {
        self.blue
    }

    #[must_use]
    pub const fn alpha(self) -> u8 {
        self.alpha
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeStyle {
    color: Color,
    width: f32,
}

impl StrokeStyle {
    pub fn new(color: Color, width: f32) -> Result<Self, AnnotationError> {
        if !width.is_finite() || width <= 0.0 {
            return Err(AnnotationError::InvalidStrokeWidth(width));
        }
        Ok(Self { color, width })
    }

    #[must_use]
    pub const fn color(self) -> Color {
        self.color
    }

    #[must_use]
    pub const fn width(self) -> f32 {
        self.width
    }
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            color: Color::RED,
            width: 3.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Polyline {
    points: Vec<Point>,
    style: StrokeStyle,
}

impl Polyline {
    #[must_use]
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    #[must_use]
    pub const fn style(&self) -> StrokeStyle {
        self.style
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    start: Point,
    end: Point,
    style: StrokeStyle,
}

impl Segment {
    #[must_use]
    pub const fn start(self) -> Point {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> Point {
        self.end
    }

    #[must_use]
    pub const fn style(self) -> StrokeStyle {
        self.style
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rectangle {
    first_corner: Point,
    opposite_corner: Point,
    style: StrokeStyle,
}

impl Rectangle {
    #[must_use]
    pub const fn first_corner(self) -> Point {
        self.first_corner
    }

    #[must_use]
    pub const fn opposite_corner(self) -> Point {
        self.opposite_corner
    }

    #[must_use]
    pub const fn style(self) -> StrokeStyle {
        self.style
    }
}

/// A vector annotation. Coordinates are local to the frame's top-left pixel.
#[derive(Clone, Debug, PartialEq)]
pub enum Annotation {
    Pen(Polyline),
    Highlighter(Polyline),
    Line(Segment),
    Arrow(Segment),
    Rectangle(Rectangle),
}

impl Annotation {
    pub fn pen(points: Vec<Point>, style: StrokeStyle) -> Result<Self, AnnotationError> {
        validate_points(&points)?;
        Ok(Self::Pen(Polyline { points, style }))
    }

    pub fn highlighter(points: Vec<Point>, style: StrokeStyle) -> Result<Self, AnnotationError> {
        validate_points(&points)?;
        Ok(Self::Highlighter(Polyline { points, style }))
    }

    pub fn line(start: Point, end: Point, style: StrokeStyle) -> Result<Self, AnnotationError> {
        validate_point(start)?;
        validate_point(end)?;
        Ok(Self::Line(Segment { start, end, style }))
    }

    pub fn arrow(start: Point, end: Point, style: StrokeStyle) -> Result<Self, AnnotationError> {
        validate_point(start)?;
        validate_point(end)?;
        Ok(Self::Arrow(Segment { start, end, style }))
    }

    pub fn rectangle(
        first_corner: Point,
        opposite_corner: Point,
        style: StrokeStyle,
    ) -> Result<Self, AnnotationError> {
        validate_point(first_corner)?;
        validate_point(opposite_corner)?;
        Ok(Self::Rectangle(Rectangle {
            first_corner,
            opposite_corner,
            style,
        }))
    }
}

/// An annotation list with command-style undo, redo, and undoable clear.
#[derive(Clone, Debug, Default)]
pub struct AnnotationDocument {
    annotations: Vec<Annotation>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

#[derive(Clone, Debug)]
enum Edit {
    Add(Annotation),
    Clear(Vec<Annotation>),
}

impl AnnotationDocument {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            annotations: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    #[must_use]
    pub fn annotations(&self) -> &[Annotation] {
        &self.annotations
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.annotations.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.annotations.is_empty()
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn add(&mut self, annotation: Annotation) {
        self.annotations.push(annotation.clone());
        self.undo.push(Edit::Add(annotation));
        self.redo.clear();
    }

    /// Clears all annotations as one undoable edit.
    pub fn clear(&mut self) -> bool {
        if self.annotations.is_empty() {
            return false;
        }
        let removed = mem::take(&mut self.annotations);
        self.undo.push(Edit::Clear(removed));
        self.redo.clear();
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo.pop() else {
            return false;
        };
        match edit {
            Edit::Add(annotation) => {
                let removed = self
                    .annotations
                    .pop()
                    .expect("an applied add always has an annotation");
                debug_assert_eq!(removed, annotation);
                self.redo.push(Edit::Add(removed));
            }
            Edit::Clear(previous) => {
                debug_assert!(self.annotations.is_empty());
                self.annotations = previous;
                self.redo.push(Edit::Clear(Vec::new()));
            }
        }
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo.pop() else {
            return false;
        };
        match edit {
            Edit::Add(annotation) => {
                self.annotations.push(annotation.clone());
                self.undo.push(Edit::Add(annotation));
            }
            Edit::Clear(_) => {
                let removed = mem::take(&mut self.annotations);
                self.undo.push(Edit::Clear(removed));
            }
        }
        true
    }

    /// Renders the current vector state onto a cloned frame.
    pub fn flatten(&self, frame: &Frame) -> Result<Frame, AnnotationError> {
        flatten(frame, &self.annotations)
    }
}

/// Renders vector annotations and returns a new straight-alpha RGBA frame.
pub fn flatten(frame: &Frame, annotations: &[Annotation]) -> Result<Frame, AnnotationError> {
    if annotations.is_empty() {
        return Ok(frame.clone());
    }

    let mut overlay =
        Pixmap::new(frame.width(), frame.height()).ok_or(AnnotationError::PixmapAllocation {
            width: frame.width(),
            height: frame.height(),
        })?;

    for annotation in annotations {
        draw_annotation(&mut overlay, annotation)?;
    }

    let mut rgba = frame.rgba().to_vec();
    blend_premultiplied_over_straight(&mut rgba, overlay.data());
    Frame::new(frame.origin(), frame.width(), frame.height(), rgba).map_err(AnnotationError::Frame)
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum AnnotationError {
    #[error("stroke width must be finite and greater than zero, got {0}")]
    InvalidStrokeWidth(f32),

    #[error("a pen or highlighter stroke needs at least one point")]
    EmptyStroke,

    #[error("annotation point must contain finite coordinates: {0:?}")]
    InvalidPoint(Point),

    #[error("could not allocate a {width}x{height} annotation pixmap")]
    PixmapAllocation { width: u32, height: u32 },

    #[error(transparent)]
    Frame(#[from] FrameError),
}

fn validate_points(points: &[Point]) -> Result<(), AnnotationError> {
    if points.is_empty() {
        return Err(AnnotationError::EmptyStroke);
    }
    points.iter().try_for_each(|point| validate_point(*point))
}

fn validate_point(point: Point) -> Result<(), AnnotationError> {
    if point.x.is_finite() && point.y.is_finite() {
        Ok(())
    } else {
        Err(AnnotationError::InvalidPoint(point))
    }
}

fn draw_annotation(pixmap: &mut Pixmap, annotation: &Annotation) -> Result<(), AnnotationError> {
    match annotation {
        Annotation::Pen(stroke) | Annotation::Highlighter(stroke) => {
            let mut builder = PathBuilder::new();
            let first = stroke.points[0];
            builder.move_to(first.x, first.y);
            if stroke.points.len() == 1 {
                builder.line_to(first.x + 0.01, first.y);
            } else {
                for point in &stroke.points[1..] {
                    builder.line_to(point.x, point.y);
                }
            }
            if let Some(path) = builder.finish() {
                stroke_path(pixmap, &path, stroke.style);
            }
        }
        Annotation::Line(segment) => {
            let path = segment_path(*segment, false);
            stroke_path(pixmap, &path, segment.style);
        }
        Annotation::Arrow(segment) => {
            let path = segment_path(*segment, true);
            stroke_path(pixmap, &path, segment.style);
        }
        Annotation::Rectangle(rectangle) => {
            let first = rectangle.first_corner;
            let opposite = rectangle.opposite_corner;
            let left = first.x.min(opposite.x);
            let top = first.y.min(opposite.y);
            let right = first.x.max(opposite.x);
            let bottom = first.y.max(opposite.y);
            let mut builder = PathBuilder::new();
            builder.move_to(left, top);
            builder.line_to(right, top);
            builder.line_to(right, bottom);
            builder.line_to(left, bottom);
            builder.close();
            if let Some(path) = builder.finish() {
                stroke_path(pixmap, &path, rectangle.style);
            }
        }
    }
    Ok(())
}

fn segment_path(segment: Segment, arrow: bool) -> Path {
    let mut builder = PathBuilder::new();
    builder.move_to(segment.start.x, segment.start.y);
    builder.line_to(segment.end.x, segment.end.y);

    if arrow {
        let dx = segment.end.x - segment.start.x;
        let dy = segment.end.y - segment.start.y;
        let length = dx.hypot(dy);
        if length > f32::EPSILON {
            let head_length = (segment.style.width * 4.0 + 6.0).min(length * 0.45);
            let angle = dy.atan2(dx);
            let spread = std::f32::consts::FRAC_PI_6;
            for head_angle in [angle + spread, angle - spread] {
                builder.move_to(segment.end.x, segment.end.y);
                builder.line_to(
                    segment.end.x - head_length * head_angle.cos(),
                    segment.end.y - head_length * head_angle.sin(),
                );
            }
        }
    }

    builder
        .finish()
        .expect("a segment path always contains at least one line")
}

fn stroke_path(pixmap: &mut Pixmap, path: &Path, style: StrokeStyle) {
    let color = style.color;
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.red, color.green, color.blue, color.alpha);
    paint.anti_alias = true;
    let stroke = Stroke {
        width: style.width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(path, &paint, &stroke, Transform::identity(), None);
}

fn blend_premultiplied_over_straight(destination: &mut [u8], source: &[u8]) {
    debug_assert_eq!(destination.len(), source.len());
    for (destination, source) in destination.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
        let source_alpha = u32::from(source[3]);
        if source_alpha == 0 {
            continue;
        }
        let destination_alpha = u32::from(destination[3]);
        let inverse_source_alpha = 255 - source_alpha;
        let output_alpha = source_alpha + (destination_alpha * inverse_source_alpha + 127) / 255;

        for channel in 0..3 {
            let destination_premultiplied =
                (u32::from(destination[channel]) * destination_alpha + 127) / 255;
            let output_premultiplied = u32::from(source[channel])
                + (destination_premultiplied * inverse_source_alpha + 127) / 255;
            destination[channel] =
                ((output_premultiplied * 255 + output_alpha / 2) / output_alpha).min(255) as u8;
        }
        destination[3] = output_alpha as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::PhysicalPoint;

    fn line() -> Annotation {
        Annotation::line(
            Point::new(1.0, 1.0),
            Point::new(6.0, 6.0),
            StrokeStyle::new(Color::RED, 2.0).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn undo_redo_and_clear_are_consistent() {
        let mut document = AnnotationDocument::new();
        document.add(line());
        document.add(line());
        assert_eq!(document.len(), 2);

        assert!(document.clear());
        assert!(document.is_empty());
        assert!(document.undo());
        assert_eq!(document.len(), 2);
        assert!(document.redo());
        assert!(document.is_empty());
        assert!(document.undo());
        assert!(document.undo());
        assert_eq!(document.len(), 1);
    }

    #[test]
    fn a_new_edit_invalidates_redo() {
        let mut document = AnnotationDocument::new();
        document.add(line());
        assert!(document.undo());
        assert!(document.can_redo());
        document.add(line());
        assert!(!document.can_redo());
    }

    #[test]
    fn constructors_reject_invalid_vectors() {
        assert_eq!(
            Annotation::pen(Vec::new(), StrokeStyle::default()),
            Err(AnnotationError::EmptyStroke)
        );
        assert!(matches!(
            StrokeStyle::new(Color::BLACK, f32::NAN),
            Err(AnnotationError::InvalidStrokeWidth(_))
        ));
        assert!(matches!(
            Annotation::line(
                Point::new(f32::INFINITY, 0.0),
                Point::default(),
                StrokeStyle::default()
            ),
            Err(AnnotationError::InvalidPoint(_))
        ));
    }

    #[test]
    fn flatten_draws_without_changing_frame_geometry() {
        let frame = Frame::new(PhysicalPoint::new(-10, 20), 8, 8, vec![255; 8 * 8 * 4]).unwrap();
        let flattened = flatten(&frame, &[line()]).unwrap();

        assert_eq!(flattened.origin(), frame.origin());
        assert_eq!((flattened.width(), flattened.height()), (8, 8));
        assert_ne!(flattened.rgba(), frame.rgba());
        assert!(flattened
            .rgba()
            .chunks_exact(4)
            .any(|pixel| pixel[0] == 255 && pixel[1] < 255 && pixel[2] < 255));
    }
}
