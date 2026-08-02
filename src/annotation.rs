//! Platform-independent vector annotations, object editing, and raster flattening.

use std::{
    collections::HashMap,
    fs, mem,
    path::Path as FilePath,
    sync::{Arc, Mutex, OnceLock},
};

use fontdue::{Font, FontSettings, Metrics};
use thiserror::Error;
use tiny_skia::{
    Color as SkiaColor, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke,
    Transform,
};

use crate::frame::{Frame, FrameError};

const TEXT_PADDING: f32 = 5.0;
const MIN_TEXT_SIZE: f32 = 10.0;
const MAX_TEXT_SIZE: f32 = 128.0;
const DEFAULT_PIXEL_BLOCK: u8 = 12;

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

    /// Translates this object in place. Document callers should normally use
    /// [`AnnotationDocument::translate`] so the change is undoable.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
    }
}

/// Axis-aligned annotation bounds in frame-local pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounds {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Bounds {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Result<Self, AnnotationError> {
        let bounds = Self {
            left: left.min(right),
            top: top.min(bottom),
            right: left.max(right),
            bottom: top.max(bottom),
        };
        if [bounds.left, bounds.top, bounds.right, bounds.bottom]
            .into_iter()
            .all(f32::is_finite)
        {
            Ok(bounds)
        } else {
            Err(AnnotationError::InvalidBounds(bounds))
        }
    }

    #[must_use]
    pub const fn left(self) -> f32 {
        self.left
    }

    #[must_use]
    pub const fn top(self) -> f32 {
        self.top
    }

    #[must_use]
    pub const fn right(self) -> f32 {
        self.right
    }

    #[must_use]
    pub const fn bottom(self) -> f32 {
        self.bottom
    }

    #[must_use]
    pub fn width(self) -> f32 {
        self.right - self.left
    }

    #[must_use]
    pub fn height(self) -> f32 {
        self.bottom - self.top
    }

    #[must_use]
    pub fn center(self) -> Point {
        Point::new(
            (self.left + self.right) * 0.5,
            (self.top + self.bottom) * 0.5,
        )
    }

    #[must_use]
    pub fn contains(self, point: Point, tolerance: f32) -> bool {
        point.x >= self.left - tolerance
            && point.y >= self.top - tolerance
            && point.x <= self.right + tolerance
            && point.y <= self.bottom + tolerance
    }

    #[must_use]
    pub fn expanded(self, amount: f32) -> Self {
        Self {
            left: self.left - amount,
            top: self.top - amount,
            right: self.right + amount,
            bottom: self.bottom + amount,
        }
    }

    /// Translates this object in place without recording document history.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.left += dx;
        self.right += dx;
        self.top += dy;
        self.bottom += dy;
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
    pub const WHITE: Self = Self::rgba(255, 255, 255, 255);
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

#[derive(Clone, Debug, PartialEq)]
pub struct TextBox {
    position: Point,
    text: String,
    color: Color,
    size: f32,
    callout_anchor: Option<Point>,
}

impl TextBox {
    #[must_use]
    pub const fn position(&self) -> Point {
        self.position
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn color(&self) -> Color {
        self.color
    }

    #[must_use]
    pub const fn size(&self) -> f32 {
        self.size
    }

    #[must_use]
    pub const fn callout_anchor(&self) -> Option<Point> {
        self.callout_anchor
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionEffect {
    first_corner: Point,
    opposite_corner: Point,
    block_size: u8,
}

impl RegionEffect {
    #[must_use]
    pub const fn first_corner(self) -> Point {
        self.first_corner
    }

    #[must_use]
    pub const fn opposite_corner(self) -> Point {
        self.opposite_corner
    }

    #[must_use]
    pub const fn block_size(self) -> u8 {
        self.block_size
    }
}

/// A vector or destructive annotation. Coordinates are local to the frame's top-left pixel.
#[derive(Clone, Debug, PartialEq)]
pub enum Annotation {
    Pen(Polyline),
    Highlighter(Polyline),
    Line(Segment),
    Arrow(Segment),
    Rectangle(Rectangle),
    Ellipse(Rectangle),
    Text(TextBox),
    Redact(RegionEffect),
    Pixelate(RegionEffect),
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
        validate_segment(first_corner, opposite_corner)?;
        Ok(Self::Rectangle(Rectangle {
            first_corner,
            opposite_corner,
            style,
        }))
    }

    pub fn ellipse(
        first_corner: Point,
        opposite_corner: Point,
        style: StrokeStyle,
    ) -> Result<Self, AnnotationError> {
        validate_segment(first_corner, opposite_corner)?;
        Ok(Self::Ellipse(Rectangle {
            first_corner,
            opposite_corner,
            style,
        }))
    }

    pub fn text(
        position: Point,
        text: String,
        color: Color,
        size: f32,
        callout_anchor: Option<Point>,
    ) -> Result<Self, AnnotationError> {
        validate_point(position)?;
        if let Some(anchor) = callout_anchor {
            validate_point(anchor)?;
        }
        if text.trim().is_empty() {
            return Err(AnnotationError::EmptyText);
        }
        if !size.is_finite() || !(MIN_TEXT_SIZE..=MAX_TEXT_SIZE).contains(&size) {
            return Err(AnnotationError::InvalidTextSize(size));
        }
        Ok(Self::Text(TextBox {
            position,
            text,
            color,
            size,
            callout_anchor,
        }))
    }

    pub fn redact(first_corner: Point, opposite_corner: Point) -> Result<Self, AnnotationError> {
        validate_segment(first_corner, opposite_corner)?;
        Ok(Self::Redact(RegionEffect {
            first_corner,
            opposite_corner,
            block_size: 1,
        }))
    }

    pub fn pixelate(
        first_corner: Point,
        opposite_corner: Point,
        block_size: u8,
    ) -> Result<Self, AnnotationError> {
        validate_segment(first_corner, opposite_corner)?;
        if block_size < 2 {
            return Err(AnnotationError::InvalidPixelBlock(block_size));
        }
        Ok(Self::Pixelate(RegionEffect {
            first_corner,
            opposite_corner,
            block_size,
        }))
    }

    #[must_use]
    pub fn bounds(&self) -> Bounds {
        match self {
            Self::Pen(stroke) | Self::Highlighter(stroke) => {
                points_bounds(&stroke.points).expanded(stroke.style.width * 0.5)
            }
            Self::Line(segment) | Self::Arrow(segment) => Bounds::new(
                segment.start.x,
                segment.start.y,
                segment.end.x,
                segment.end.y,
            )
            .expect("validated segment bounds")
            .expanded(segment.style.width.mul_add(2.0, 6.0)),
            Self::Rectangle(rectangle) | Self::Ellipse(rectangle) => Bounds::new(
                rectangle.first_corner.x,
                rectangle.first_corner.y,
                rectangle.opposite_corner.x,
                rectangle.opposite_corner.y,
            )
            .expect("validated rectangle bounds")
            .expanded(rectangle.style.width * 0.5),
            Self::Text(text) => {
                let (width, height) = measure_text(&text.text, text.size);
                let mut bounds = Bounds::new(
                    text.position.x - TEXT_PADDING,
                    text.position.y - TEXT_PADDING,
                    text.position.x + width as f32 + TEXT_PADDING,
                    text.position.y + height as f32 + TEXT_PADDING,
                )
                .expect("validated text bounds");
                if let Some(anchor) = text.callout_anchor {
                    bounds.left = bounds.left.min(anchor.x);
                    bounds.top = bounds.top.min(anchor.y);
                    bounds.right = bounds.right.max(anchor.x);
                    bounds.bottom = bounds.bottom.max(anchor.y);
                }
                bounds
            }
            Self::Redact(effect) | Self::Pixelate(effect) => Bounds::new(
                effect.first_corner.x,
                effect.first_corner.y,
                effect.opposite_corner.x,
                effect.opposite_corner.y,
            )
            .expect("validated effect bounds"),
        }
    }

    #[must_use]
    pub fn style(&self) -> Option<StrokeStyle> {
        match self {
            Self::Pen(value) | Self::Highlighter(value) => Some(value.style),
            Self::Line(value) | Self::Arrow(value) => Some(value.style),
            Self::Rectangle(value) | Self::Ellipse(value) => Some(value.style),
            Self::Text(value) => StrokeStyle::new(value.color, value.size).ok(),
            Self::Redact(_) | Self::Pixelate(_) => None,
        }
    }

    #[must_use]
    pub fn hit_test(&self, point: Point, tolerance: f32) -> bool {
        if !self.bounds().contains(point, tolerance) {
            return false;
        }
        match self {
            Self::Pen(stroke) | Self::Highlighter(stroke) => stroke.points.windows(2).any(|pair| {
                distance_to_segment(point, pair[0], pair[1]) <= tolerance + stroke.style.width * 0.5
            }),
            Self::Line(segment) | Self::Arrow(segment) => {
                distance_to_segment(point, segment.start, segment.end)
                    <= tolerance + segment.style.width * 0.5
            }
            Self::Rectangle(_)
            | Self::Ellipse(_)
            | Self::Text(_)
            | Self::Redact(_)
            | Self::Pixelate(_) => true,
        }
    }

    /// Returns a translated copy without modifying document history.
    #[must_use]
    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        let mut annotation = self.clone();
        annotation.translate(dx, dy);
        annotation
    }

    /// Returns a resized copy without modifying document history.
    #[must_use]
    pub fn resized(&self, from: Bounds, to: Bounds) -> Self {
        let mut annotation = self.clone();
        annotation.resize(from, to);
        annotation
    }

    /// Translates this object in place without recording document history.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        match self {
            Self::Pen(stroke) | Self::Highlighter(stroke) => {
                for point in &mut stroke.points {
                    point.translate(dx, dy);
                }
            }
            Self::Line(segment) | Self::Arrow(segment) => {
                segment.start.translate(dx, dy);
                segment.end.translate(dx, dy);
            }
            Self::Rectangle(rectangle) | Self::Ellipse(rectangle) => {
                rectangle.first_corner.translate(dx, dy);
                rectangle.opposite_corner.translate(dx, dy);
            }
            Self::Text(text) => {
                text.position.translate(dx, dy);
                if let Some(anchor) = &mut text.callout_anchor {
                    anchor.translate(dx, dy);
                }
            }
            Self::Redact(effect) | Self::Pixelate(effect) => {
                effect.first_corner.translate(dx, dy);
                effect.opposite_corner.translate(dx, dy);
            }
        }
    }

    /// Resizes this object in place. Document callers should normally use
    /// [`AnnotationDocument::resize`] so the change is undoable.
    pub fn resize(&mut self, from: Bounds, to: Bounds) {
        fn map(point: &mut Point, from: Bounds, to: Bounds) {
            let source_width = from.width().max(0.001);
            let source_height = from.height().max(0.001);
            point.x = to.left + (point.x - from.left) / source_width * to.width();
            point.y = to.top + (point.y - from.top) / source_height * to.height();
        }

        match self {
            Self::Pen(stroke) | Self::Highlighter(stroke) => {
                for point in &mut stroke.points {
                    map(point, from, to);
                }
            }
            Self::Line(segment) | Self::Arrow(segment) => {
                map(&mut segment.start, from, to);
                map(&mut segment.end, from, to);
            }
            Self::Rectangle(rectangle) | Self::Ellipse(rectangle) => {
                map(&mut rectangle.first_corner, from, to);
                map(&mut rectangle.opposite_corner, from, to);
            }
            Self::Text(text) => {
                map(&mut text.position, from, to);
                if let Some(anchor) = &mut text.callout_anchor {
                    map(anchor, from, to);
                }
                let scale = ((to.width() / from.width().max(0.001))
                    + (to.height() / from.height().max(0.001)))
                    * 0.5;
                text.size = (text.size * scale).clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE);
            }
            Self::Redact(effect) | Self::Pixelate(effect) => {
                map(&mut effect.first_corner, from, to);
                map(&mut effect.opposite_corner, from, to);
            }
        }
    }

    fn set_style(&mut self, style: StrokeStyle) {
        match self {
            Self::Pen(value) | Self::Highlighter(value) => value.style = style,
            Self::Line(value) | Self::Arrow(value) => value.style = style,
            Self::Rectangle(value) | Self::Ellipse(value) => value.style = style,
            Self::Text(value) => {
                value.color = style.color;
                value.size = style.width.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE);
            }
            Self::Redact(_) | Self::Pixelate(_) => {}
        }
    }
}

/// An annotation list with allocation-conscious command history.
#[derive(Clone, Debug, Default)]
pub struct AnnotationDocument {
    annotations: Vec<Annotation>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

#[derive(Clone, Debug)]
enum Edit {
    Add {
        index: usize,
        held: Option<Annotation>,
    },
    Delete {
        index: usize,
        held: Option<Annotation>,
    },
    Clear(Vec<Annotation>),
    Translate {
        index: usize,
        dx: f32,
        dy: f32,
    },
    Resize {
        index: usize,
        from: Bounds,
        to: Bounds,
    },
    Style {
        index: usize,
        before: StrokeStyle,
        after: StrokeStyle,
    },
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
    pub fn annotation(&self, index: usize) -> Option<&Annotation> {
        self.annotations.get(index)
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

    #[must_use]
    pub fn hit_test(&self, point: Point, tolerance: f32) -> Option<usize> {
        self.annotations
            .iter()
            .rposition(|annotation| annotation.hit_test(point, tolerance))
    }

    pub fn add(&mut self, annotation: Annotation) {
        let index = self.annotations.len();
        self.annotations.push(annotation);
        self.undo.push(Edit::Add { index, held: None });
        self.redo.clear();
    }

    pub fn delete(&mut self, index: usize) -> bool {
        if index >= self.annotations.len() {
            return false;
        }
        let annotation = self.annotations.remove(index);
        self.undo.push(Edit::Delete {
            index,
            held: Some(annotation),
        });
        self.redo.clear();
        true
    }

    pub fn translate(&mut self, index: usize, dx: f32, dy: f32) -> bool {
        if !dx.is_finite() || !dy.is_finite() || (dx == 0.0 && dy == 0.0) {
            return false;
        }
        let Some(annotation) = self.annotations.get_mut(index) else {
            return false;
        };
        annotation.translate(dx, dy);
        self.undo.push(Edit::Translate { index, dx, dy });
        self.redo.clear();
        true
    }

    pub fn resize(&mut self, index: usize, from: Bounds, to: Bounds) -> bool {
        if from == to || to.width() < 1.0 || to.height() < 1.0 {
            return false;
        }
        let Some(annotation) = self.annotations.get_mut(index) else {
            return false;
        };
        annotation.resize(from, to);
        self.undo.push(Edit::Resize { index, from, to });
        self.redo.clear();
        true
    }

    pub fn restyle(&mut self, index: usize, style: StrokeStyle) -> bool {
        let Some(annotation) = self.annotations.get_mut(index) else {
            return false;
        };
        let Some(before) = annotation.style() else {
            return false;
        };
        if before == style {
            return false;
        }
        annotation.set_style(style);
        self.undo.push(Edit::Style {
            index,
            before,
            after: style,
        });
        self.redo.clear();
        true
    }

    /// Clears all annotations as one undoable edit without duplicating their storage.
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
        let Some(mut edit) = self.undo.pop() else {
            return false;
        };
        edit.undo(&mut self.annotations);
        self.redo.push(edit);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(mut edit) = self.redo.pop() else {
            return false;
        };
        edit.redo(&mut self.annotations);
        self.undo.push(edit);
        true
    }

    /// Changes the frame-local origin after a crop without recording a user edit.
    pub fn rebase(&mut self, dx: f32, dy: f32) {
        for annotation in &mut self.annotations {
            annotation.translate(dx, dy);
        }
        for edit in self.undo.iter_mut().chain(&mut self.redo) {
            edit.rebase(dx, dy);
        }
    }

    /// Renders the current annotation state onto a cloned frame.
    pub fn flatten(&self, frame: &Frame) -> Result<Frame, AnnotationError> {
        flatten(frame, &self.annotations)
    }

    /// Renders all annotations except one object, used for allocation-stable drag previews.
    pub fn flatten_excluding(
        &self,
        frame: &Frame,
        excluded: usize,
    ) -> Result<Frame, AnnotationError> {
        flatten_filtered(frame, &self.annotations, Some(excluded))
    }
}

impl Edit {
    fn undo(&mut self, annotations: &mut Vec<Annotation>) {
        match self {
            Self::Add { index, held } => {
                *held = Some(annotations.remove(*index));
            }
            Self::Delete { index, held } => {
                annotations.insert(
                    *index,
                    held.take().expect("applied delete holds its object"),
                );
            }
            Self::Clear(held) => mem::swap(annotations, held),
            Self::Translate { index, dx, dy } => annotations[*index].translate(-*dx, -*dy),
            Self::Resize { index, from, to } => annotations[*index].resize(*to, *from),
            Self::Style { index, before, .. } => annotations[*index].set_style(*before),
        }
    }

    fn redo(&mut self, annotations: &mut Vec<Annotation>) {
        match self {
            Self::Add { index, held } => {
                annotations.insert(*index, held.take().expect("undone add holds its object"));
            }
            Self::Delete { index, held } => *held = Some(annotations.remove(*index)),
            Self::Clear(held) => mem::swap(annotations, held),
            Self::Translate { index, dx, dy } => annotations[*index].translate(*dx, *dy),
            Self::Resize { index, from, to } => annotations[*index].resize(*from, *to),
            Self::Style { index, after, .. } => annotations[*index].set_style(*after),
        }
    }

    fn rebase(&mut self, dx: f32, dy: f32) {
        match self {
            Self::Add { held, .. } | Self::Delete { held, .. } => {
                if let Some(annotation) = held {
                    annotation.translate(dx, dy);
                }
            }
            Self::Clear(annotations) => {
                for annotation in annotations {
                    annotation.translate(dx, dy);
                }
            }
            Self::Resize { from, to, .. } => {
                from.translate(dx, dy);
                to.translate(dx, dy);
            }
            Self::Translate { .. } | Self::Style { .. } => {}
        }
    }
}

/// Renders annotations and returns a new straight-alpha RGBA frame.
pub fn flatten(frame: &Frame, annotations: &[Annotation]) -> Result<Frame, AnnotationError> {
    flatten_filtered(frame, annotations, None)
}

fn flatten_filtered(
    frame: &Frame,
    annotations: &[Annotation],
    excluded: Option<usize>,
) -> Result<Frame, AnnotationError> {
    if annotations.is_empty() || (annotations.len() == 1 && excluded == Some(0)) {
        return Ok(frame.clone());
    }

    let mut rgba = frame.rgba().to_vec();
    let mut overlay =
        Pixmap::new(frame.width(), frame.height()).ok_or(AnnotationError::PixmapAllocation {
            width: frame.width(),
            height: frame.height(),
        })?;
    let mut overlay_dirty = false;

    for (index, annotation) in annotations.iter().enumerate() {
        if excluded == Some(index) {
            continue;
        }
        match annotation {
            Annotation::Redact(effect) => {
                flush_overlay(&mut rgba, &mut overlay, &mut overlay_dirty);
                fill_effect(
                    &mut rgba,
                    frame.width(),
                    frame.height(),
                    *effect,
                    Color::BLACK,
                );
            }
            Annotation::Pixelate(effect) => {
                flush_overlay(&mut rgba, &mut overlay, &mut overlay_dirty);
                pixelate_effect(&mut rgba, frame.width(), frame.height(), *effect);
            }
            Annotation::Text(text) => {
                flush_overlay(&mut rgba, &mut overlay, &mut overlay_dirty);
                draw_text_box_background(&mut overlay, text);
                overlay_dirty = true;
                flush_overlay(&mut rgba, &mut overlay, &mut overlay_dirty);
                draw_text_rgba(
                    &mut rgba,
                    frame.width(),
                    frame.height(),
                    text.position,
                    &text.text,
                    text.color,
                    text.size,
                );
            }
            _ => {
                draw_vector_annotation(&mut overlay, annotation)?;
                overlay_dirty = true;
            }
        }
    }
    flush_overlay(&mut rgba, &mut overlay, &mut overlay_dirty);
    Frame::new(frame.origin(), frame.width(), frame.height(), rgba).map_err(AnnotationError::Frame)
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum AnnotationError {
    #[error("stroke width must be finite and greater than zero, got {0}")]
    InvalidStrokeWidth(f32),
    #[error("text size must be between 10 and 128 pixels, got {0}")]
    InvalidTextSize(f32),
    #[error("pixelation block must be at least 2 pixels, got {0}")]
    InvalidPixelBlock(u8),
    #[error("a pen or highlighter stroke needs at least one point")]
    EmptyStroke,
    #[error("a text annotation cannot be empty")]
    EmptyText,
    #[error("annotation point must contain finite coordinates: {0:?}")]
    InvalidPoint(Point),
    #[error("annotation bounds must contain finite coordinates: {0:?}")]
    InvalidBounds(Bounds),
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

fn validate_segment(first: Point, second: Point) -> Result<(), AnnotationError> {
    validate_point(first)?;
    validate_point(second)
}

fn validate_point(point: Point) -> Result<(), AnnotationError> {
    if point.x.is_finite() && point.y.is_finite() {
        Ok(())
    } else {
        Err(AnnotationError::InvalidPoint(point))
    }
}

fn points_bounds(points: &[Point]) -> Bounds {
    let first = points[0];
    let mut bounds = Bounds {
        left: first.x,
        top: first.y,
        right: first.x,
        bottom: first.y,
    };
    for point in &points[1..] {
        bounds.left = bounds.left.min(point.x);
        bounds.top = bounds.top.min(point.y);
        bounds.right = bounds.right.max(point.x);
        bounds.bottom = bounds.bottom.max(point.y);
    }
    bounds
}

fn distance_to_segment(point: Point, start: Point, end: Point) -> f32 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx.mul_add(dx, dy * dy);
    if length_squared <= f32::EPSILON {
        return (point.x - start.x).hypot(point.y - start.y);
    }
    let t =
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0);
    (point.x - (start.x + t * dx)).hypot(point.y - (start.y + t * dy))
}

fn draw_vector_annotation(
    pixmap: &mut Pixmap,
    annotation: &Annotation,
) -> Result<(), AnnotationError> {
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
            stroke_path(pixmap, &segment_path(*segment, false), segment.style);
        }
        Annotation::Arrow(segment) => {
            stroke_path(pixmap, &segment_path(*segment, true), segment.style);
        }
        Annotation::Rectangle(rectangle) => {
            if let Some(path) = rectangle_path(*rectangle, false) {
                stroke_path(pixmap, &path, rectangle.style);
            }
        }
        Annotation::Ellipse(ellipse) => {
            if let Some(path) = rectangle_path(*ellipse, true) {
                stroke_path(pixmap, &path, ellipse.style);
            }
        }
        Annotation::Text(_) | Annotation::Redact(_) | Annotation::Pixelate(_) => {}
    }
    Ok(())
}

fn rectangle_path(rectangle: Rectangle, oval: bool) -> Option<Path> {
    let bounds = Bounds::new(
        rectangle.first_corner.x,
        rectangle.first_corner.y,
        rectangle.opposite_corner.x,
        rectangle.opposite_corner.y,
    )
    .ok()?;
    let rect = Rect::from_xywh(
        bounds.left,
        bounds.top,
        bounds.width().max(0.01),
        bounds.height().max(0.01),
    )?;
    if oval {
        PathBuilder::from_oval(rect)
    } else {
        Some(PathBuilder::from_rect(rect))
    }
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

fn draw_text_box_background(pixmap: &mut Pixmap, text: &TextBox) {
    let (width, height) = measure_text(&text.text, text.size);
    let bounds = Rect::from_xywh(
        text.position.x - TEXT_PADDING,
        text.position.y - TEXT_PADDING,
        width as f32 + TEXT_PADDING * 2.0,
        height as f32 + TEXT_PADDING * 2.0,
    );
    if let Some(bounds) = bounds {
        let mut paint = Paint::default();
        paint.set_color_rgba8(18, 20, 26, 210);
        pixmap.fill_rect(bounds, &paint, Transform::identity(), None);
    }
    if let Some(anchor) = text.callout_anchor {
        let target = Point::new(
            text.position.x + width as f32 * 0.5,
            text.position.y + height as f32 * 0.5,
        );
        let style = StrokeStyle::new(text.color, 3.0).expect("callout style is valid");
        stroke_path(
            pixmap,
            &segment_path(
                Segment {
                    start: anchor,
                    end: target,
                    style,
                },
                true,
            ),
            style,
        );
    }
}

fn flush_overlay(rgba: &mut [u8], overlay: &mut Pixmap, dirty: &mut bool) {
    if !*dirty {
        return;
    }
    blend_premultiplied_over_straight(rgba, overlay.data());
    overlay.fill(SkiaColor::TRANSPARENT);
    *dirty = false;
}

fn effect_bounds(effect: RegionEffect, width: u32, height: u32) -> (usize, usize, usize, usize) {
    let bounds = Bounds::new(
        effect.first_corner.x,
        effect.first_corner.y,
        effect.opposite_corner.x,
        effect.opposite_corner.y,
    )
    .expect("validated effect bounds");
    let left = bounds.left.floor().max(0.0).min(width as f32) as usize;
    let top = bounds.top.floor().max(0.0).min(height as f32) as usize;
    let right = bounds.right.ceil().max(0.0).min(width as f32) as usize;
    let bottom = bounds.bottom.ceil().max(0.0).min(height as f32) as usize;
    (left, top, right, bottom)
}

fn fill_effect(rgba: &mut [u8], width: u32, height: u32, effect: RegionEffect, color: Color) {
    let stride = width as usize;
    let (left, top, right, bottom) = effect_bounds(effect, width, height);
    for y in top..bottom {
        for x in left..right {
            let offset = (y * stride + x) * 4;
            rgba[offset..offset + 4].copy_from_slice(&[
                color.red,
                color.green,
                color.blue,
                color.alpha,
            ]);
        }
    }
}

fn pixelate_effect(rgba: &mut [u8], width: u32, height: u32, effect: RegionEffect) {
    let stride = width as usize;
    let (left, top, right, bottom) = effect_bounds(effect, width, height);
    let block = usize::from(effect.block_size.max(DEFAULT_PIXEL_BLOCK));
    for block_y in (top..bottom).step_by(block) {
        for block_x in (left..right).step_by(block) {
            let end_y = (block_y + block).min(bottom);
            let end_x = (block_x + block).min(right);
            let mut totals = [0_u64; 4];
            let mut count = 0_u64;
            for y in block_y..end_y {
                for x in block_x..end_x {
                    let offset = (y * stride + x) * 4;
                    for channel in 0..4 {
                        totals[channel] += u64::from(rgba[offset + channel]);
                    }
                    count += 1;
                }
            }
            let average = totals.map(|total| (total / count) as u8);
            for y in block_y..end_y {
                for x in block_x..end_x {
                    let offset = (y * stride + x) * 4;
                    rgba[offset..offset + 4].copy_from_slice(&average);
                }
            }
        }
    }
}

static UI_FONT: OnceLock<Option<Font>> = OnceLock::new();
static GLYPH_CACHE: OnceLock<Mutex<HashMap<(char, u16), CachedGlyph>>> = OnceLock::new();
const MAX_CACHED_GLYPHS: usize = 2048;

#[derive(Clone)]
struct CachedGlyph {
    metrics: Metrics,
    bitmap: Arc<[u8]>,
}

fn rasterized_glyph(font: &Font, character: char, size: f32) -> CachedGlyph {
    let key = (
        character,
        (size * 10.0).round().clamp(0.0, u16::MAX as f32) as u16,
    );
    let cache = GLYPH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(glyph) = cache.get(&key) {
        return glyph.clone();
    }
    let (metrics, bitmap) = font.rasterize(character, size);
    let glyph = CachedGlyph {
        metrics,
        bitmap: Arc::from(bitmap),
    };
    if cache.len() < MAX_CACHED_GLYPHS {
        cache.insert(key, glyph.clone());
    }
    glyph
}

fn ui_font() -> Option<&'static Font> {
    UI_FONT
        .get_or_init(|| {
            let candidates: &[&FilePath] = if cfg!(windows) {
                &[
                    FilePath::new(r"C:\Windows\Fonts\segoeui.ttf"),
                    FilePath::new(r"C:\Windows\Fonts\arial.ttf"),
                ]
            } else {
                &[
                    FilePath::new("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
                    FilePath::new("/System/Library/Fonts/Helvetica.ttc"),
                ]
            };
            candidates.iter().find_map(|path| {
                fs::read(path)
                    .ok()
                    .and_then(|bytes| Font::from_bytes(bytes, FontSettings::default()).ok())
            })
        })
        .as_ref()
}

/// Measures text using the lazily loaded system UI font.
#[must_use]
pub fn measure_text(text: &str, size: f32) -> (u32, u32) {
    let size = size.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE);
    let line_height = (size * 1.25).ceil().max(1.0) as u32;
    let Some(font) = ui_font() else {
        let width = text
            .lines()
            .map(|line| line.chars().count() as f32 * size * 0.62)
            .fold(0.0_f32, f32::max)
            .ceil() as u32;
        return (
            width.max(1),
            line_height * text.lines().count().max(1) as u32,
        );
    };
    let mut maximum = 0.0_f32;
    for line in text.lines() {
        let mut width = 0.0_f32;
        let mut previous = None;
        for character in line.chars() {
            if let Some(previous) = previous {
                width += font
                    .horizontal_kern(previous, character, size)
                    .unwrap_or(0.0);
            }
            width += font.metrics(character, size).advance_width;
            previous = Some(character);
        }
        maximum = maximum.max(width);
    }
    (
        maximum.ceil().max(1.0) as u32,
        line_height * text.lines().count().max(1) as u32,
    )
}

/// Draws antialiased text into a straight-alpha RGBA frame buffer.
pub fn draw_text_rgba(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    position: Point,
    text: &str,
    color: Color,
    size: f32,
) {
    let Some(font) = ui_font() else {
        return;
    };
    let size = size.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE);
    let line_height = (size * 1.25).ceil() as i32;
    let stride = width as usize;
    let mut baseline_y = position.y.round() as i32 + size.ceil() as i32;
    for line in text.lines() {
        let mut cursor_x = position.x;
        let mut previous = None;
        for character in line.chars() {
            if let Some(previous) = previous {
                cursor_x += font
                    .horizontal_kern(previous, character, size)
                    .unwrap_or(0.0);
            }
            let glyph = rasterized_glyph(font, character, size);
            let metrics = glyph.metrics;
            let bitmap = &glyph.bitmap;
            let draw_x = cursor_x.round() as i32 + metrics.xmin;
            let draw_y = baseline_y - metrics.height as i32 - metrics.ymin;
            for row in 0..metrics.height {
                let y = draw_y + row as i32;
                if y < 0 || y >= height as i32 {
                    continue;
                }
                for column in 0..metrics.width {
                    let x = draw_x + column as i32;
                    if x < 0 || x >= width as i32 {
                        continue;
                    }
                    let coverage = u16::from(bitmap[row * metrics.width + column]);
                    if coverage == 0 {
                        continue;
                    }
                    let source_alpha = (coverage * u16::from(color.alpha) + 127) / 255;
                    let offset = (y as usize * stride + x as usize) * 4;
                    blend_straight_pixel(
                        &mut rgba[offset..offset + 4],
                        [color.red, color.green, color.blue, source_alpha as u8],
                    );
                }
            }
            cursor_x += metrics.advance_width;
            previous = Some(character);
        }
        baseline_y += line_height;
    }
}

/// Draws antialiased text into the overlay's packed `0x00RRGGBB` presentation buffer.
pub fn draw_text_bgrx(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    position: Point,
    text: &str,
    color: Color,
    size: f32,
) {
    let Some(font) = ui_font() else {
        return;
    };
    let size = size.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE);
    let line_height = (size * 1.25).ceil() as i32;
    let stride = width as usize;
    let mut baseline_y = position.y.round() as i32 + size.ceil() as i32;
    for line in text.lines() {
        let mut cursor_x = position.x;
        let mut previous = None;
        for character in line.chars() {
            if let Some(previous) = previous {
                cursor_x += font
                    .horizontal_kern(previous, character, size)
                    .unwrap_or(0.0);
            }
            let glyph = rasterized_glyph(font, character, size);
            let metrics = glyph.metrics;
            let bitmap = &glyph.bitmap;
            let draw_x = cursor_x.round() as i32 + metrics.xmin;
            let draw_y = baseline_y - metrics.height as i32 - metrics.ymin;
            for row in 0..metrics.height {
                let y = draw_y + row as i32;
                if y < 0 || y >= height as i32 {
                    continue;
                }
                for column in 0..metrics.width {
                    let x = draw_x + column as i32;
                    if x < 0 || x >= width as i32 {
                        continue;
                    }
                    let coverage = u32::from(bitmap[row * metrics.width + column]);
                    let alpha = coverage * u32::from(color.alpha) / 255;
                    if alpha == 0 {
                        continue;
                    }
                    let index = y as usize * stride + x as usize;
                    let destination = pixels[index];
                    let inverse = 255 - alpha;
                    let red = (u32::from(color.red) * alpha
                        + ((destination >> 16) & 0xff) * inverse
                        + 127)
                        / 255;
                    let green = (u32::from(color.green) * alpha
                        + ((destination >> 8) & 0xff) * inverse
                        + 127)
                        / 255;
                    let blue =
                        (u32::from(color.blue) * alpha + (destination & 0xff) * inverse + 127)
                            / 255;
                    pixels[index] = (red << 16) | (green << 8) | blue;
                }
            }
            cursor_x += metrics.advance_width;
            previous = Some(character);
        }
        baseline_y += line_height;
    }
}

fn blend_straight_pixel(destination: &mut [u8], source: [u8; 4]) {
    let source_alpha = u32::from(source[3]);
    if source_alpha == 0 {
        return;
    }
    let destination_alpha = u32::from(destination[3]);
    let inverse_source_alpha = 255 - source_alpha;
    let output_alpha = source_alpha + (destination_alpha * inverse_source_alpha + 127) / 255;
    for channel in 0..3 {
        let source_premultiplied = u32::from(source[channel]) * source_alpha / 255;
        let destination_premultiplied =
            (u32::from(destination[channel]) * destination_alpha + 127) / 255;
        let output_premultiplied =
            source_premultiplied + (destination_premultiplied * inverse_source_alpha + 127) / 255;
        destination[channel] =
            ((output_premultiplied * 255 + output_alpha / 2) / output_alpha).min(255) as u8;
    }
    destination[3] = output_alpha as u8;
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
        assert_eq!(document.len(), 1);
        assert!(document.undo());
        assert!(document.is_empty());
        assert!(document.redo());
        assert_eq!(document.len(), 1);
        assert!(document.clear());
        assert!(document.is_empty());
        assert!(document.undo());
        assert_eq!(document.len(), 1);
        assert!(document.redo());
        assert!(document.is_empty());
    }

    #[test]
    fn a_new_edit_invalidates_redo() {
        let mut document = AnnotationDocument::new();
        document.add(line());
        document.undo();
        document.add(line());
        assert!(!document.can_redo());
    }

    #[test]
    fn constructors_reject_invalid_vectors() {
        assert!(matches!(
            Annotation::pen(Vec::new(), StrokeStyle::default()),
            Err(AnnotationError::EmptyStroke)
        ));
        assert!(matches!(
            Annotation::line(
                Point::new(f32::NAN, 0.0),
                Point::default(),
                StrokeStyle::default()
            ),
            Err(AnnotationError::InvalidPoint(_))
        ));
        assert!(matches!(
            Annotation::text(Point::default(), String::new(), Color::WHITE, 18.0, None),
            Err(AnnotationError::EmptyText)
        ));
    }

    #[test]
    fn flatten_draws_without_changing_frame_geometry() {
        let frame = Frame::new(PhysicalPoint::new(-20, 30), 8, 8, vec![255; 8 * 8 * 4]).unwrap();
        let rendered = flatten(&frame, &[line()]).unwrap();
        assert_eq!(rendered.origin(), frame.origin());
        assert_eq!((rendered.width(), rendered.height()), (8, 8));
        assert_ne!(rendered.rgba(), frame.rgba());
    }

    #[test]
    fn object_edits_round_trip_without_full_document_snapshots() {
        let mut document = AnnotationDocument::new();
        document.add(line());
        let before = document.annotation(0).unwrap().clone();
        assert!(document.translate(0, 7.0, -3.0));
        assert_ne!(document.annotation(0), Some(&before));
        assert!(document.undo());
        assert_eq!(document.annotation(0), Some(&before));
        assert!(document.redo());
        assert!(document.delete(0));
        assert!(document.is_empty());
        assert!(document.undo());
        assert_eq!(document.len(), 1);
    }

    #[test]
    fn secure_effects_modify_only_the_requested_region() {
        let frame = Frame::new(PhysicalPoint::default(), 4, 4, vec![255; 4 * 4 * 4]).unwrap();
        let redaction = Annotation::redact(Point::new(1.0, 1.0), Point::new(3.0, 3.0)).unwrap();
        let rendered = flatten(&frame, &[redaction]).unwrap();
        assert_eq!(&rendered.rgba()[0..4], &[255, 255, 255, 255]);
        let center = 5 * 4;
        assert_eq!(&rendered.rgba()[center..center + 4], &[0, 0, 0, 255]);
    }

    #[test]
    fn text_measurement_is_nonempty_and_multiline() {
        let single = measure_text("Rustshot", 18.0);
        let multiline = measure_text("Rustshot\nFast", 18.0);
        assert!(single.0 > 0 && single.1 > 0);
        assert!(multiline.1 > single.1);
    }
}
