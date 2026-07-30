//! Full-screen region selection and annotation window.

use std::{num::NonZeroU32, rc::Rc};

use anyhow::{Context as _, Result};
use rustshot::{
    annotation::{Annotation, AnnotationDocument, Color, Point, StrokeStyle},
    frame::{Frame, PhysicalPoint, PhysicalRect},
};
use softbuffer::{Context, Surface};
use windows::Win32::Foundation::HWND;
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::ActiveEventLoop,
    keyboard::{Key, ModifiersState, NamedKey},
    platform::windows::WindowAttributesExtWindows,
    raw_window_handle::{HasWindowHandle as _, RawWindowHandle},
    window::{CursorIcon, Window, WindowId, WindowLevel},
};

const MIN_SELECTION_SIZE: u32 = 3;
const TOOLBAR_PADDING: i32 = 4;
const TOOLBAR_GAP: i32 = 8;
const TOOLBAR_BUTTONS: usize = 12;
const DEFAULT_BUTTON_SIZE: i32 = 40;
const MIN_BUTTON_SIZE: i32 = 26;

const PALETTE: [Color; 6] = [
    Color::RED,
    Color::rgba(255, 196, 0, 255),
    Color::rgba(40, 205, 90, 255),
    Color::rgba(45, 135, 255, 255),
    Color::rgba(255, 255, 255, 255),
    Color::BLACK,
];
const STROKE_WIDTHS: [f32; 4] = [2.0, 4.0, 7.0, 11.0];

/// Result of one overlay event.
pub enum OverlayOutcome {
    None,
    Cancel,
    Save(Frame),
    Copy(Frame),
    Print(Frame),
    Error(String),
}

/// Owns the temporary screen-sized window and one active edit session.
pub struct Overlay {
    window: Rc<Window>,
    _context: Context<Rc<Window>>,
    surface: Surface<Rc<Window>, Rc<Window>>,
    source: Frame,
    cursor: PixelPoint,
    selection_anchor: Option<PixelPoint>,
    selection_cursor: Option<PixelPoint>,
    editor: Option<Editor>,
    modifiers: ModifiersState,
}

impl Overlay {
    pub fn new(event_loop: &ActiveEventLoop, source: Frame) -> Result<Self> {
        let origin = source.origin();
        let attributes = Window::default_attributes()
            .with_title("Rustshot")
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_skip_taskbar(true)
            .with_position(PhysicalPosition::new(origin.x(), origin.y()))
            .with_inner_size(PhysicalSize::new(source.width(), source.height()))
            .with_visible(false)
            .with_active(true);

        let window = Rc::new(
            event_loop
                .create_window(attributes)
                .context("could not create the region selection window")?,
        );
        window.set_cursor(CursorIcon::Crosshair);
        let context = Context::new(window.clone())
            .map_err(|error| anyhow::anyhow!("could not initialize software rendering: {error}"))?;
        let surface = Surface::new(&context, window.clone()).map_err(|error| {
            anyhow::anyhow!("could not create the region selection surface: {error}")
        })?;

        window.set_visible(true);
        window.focus_window();
        window.request_redraw();

        Ok(Self {
            window,
            _context: context,
            surface,
            source,
            cursor: PixelPoint::default(),
            selection_anchor: None,
            selection_cursor: None,
            editor: None,
            modifiers: ModifiersState::empty(),
        })
    }

    #[must_use]
    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }

    pub fn owner_hwnd(&self) -> Result<HWND> {
        let handle = self
            .window
            .window_handle()
            .context("could not access the region editor window handle")?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Ok(HWND(handle.hwnd.get() as *mut _)),
            _ => anyhow::bail!("region editor is not backed by a Windows window"),
        }
    }

    pub fn set_visible(&self, visible: bool) {
        self.window.set_visible(visible);
        if visible {
            self.window.focus_window();
            self.window.request_redraw();
        }
    }

    /// Handles input and returns an export action when the session is done.
    pub fn handle_event(&mut self, event: WindowEvent) -> OverlayOutcome {
        match event {
            WindowEvent::CloseRequested => OverlayOutcome::Cancel,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.window.request_redraw();
                OverlayOutcome::None
            }
            WindowEvent::RedrawRequested => match self.draw() {
                Ok(()) => OverlayOutcome::None,
                Err(error) => OverlayOutcome::Error(error.to_string()),
            },
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                OverlayOutcome::None
            }
            WindowEvent::CursorMoved { position, .. } => {
                let pointer = PixelPoint::from_position(position);
                self.cursor = if self.editor.is_some() {
                    pointer.clamp_to_pixels(&self.source)
                } else {
                    pointer.clamp_to_edges(&self.source)
                };
                self.update_pointer_cursor();
                let changed = if self.editor.is_some() {
                    self.update_gesture()
                } else if self.selection_anchor.is_some() {
                    let changed = self.selection_cursor != Some(self.cursor);
                    self.selection_cursor = Some(self.cursor);
                    changed
                } else {
                    false
                };
                if changed {
                    self.window.request_redraw();
                }
                OverlayOutcome::None
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => self.handle_left_button(state),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed => self.handle_key(&event.logical_key),
            _ => OverlayOutcome::None,
        }
    }

    fn handle_left_button(&mut self, state: ElementState) -> OverlayOutcome {
        if self.editor.is_none() {
            return self.handle_selection_button(state);
        }

        match state {
            ElementState::Pressed => {
                if let Some(button) = self.toolbar_button_at(self.cursor) {
                    return self.activate_toolbar(button);
                }
                match self.begin_gesture() {
                    Ok(changed) => {
                        if changed {
                            self.window.request_redraw();
                        }
                        OverlayOutcome::None
                    }
                    Err(error) => OverlayOutcome::Error(error.to_string()),
                }
            }
            ElementState::Released => match self.commit_gesture() {
                Ok(()) => OverlayOutcome::None,
                Err(error) => OverlayOutcome::Error(error.to_string()),
            },
        }
    }

    fn handle_selection_button(&mut self, state: ElementState) -> OverlayOutcome {
        match state {
            ElementState::Pressed => {
                self.selection_anchor = Some(self.cursor);
                self.selection_cursor = Some(self.cursor);
                self.window.request_redraw();
                OverlayOutcome::None
            }
            ElementState::Released => {
                let Some(anchor) = self.selection_anchor.take() else {
                    return OverlayOutcome::None;
                };
                let Some(selection) = PixelRect::between(anchor, self.cursor) else {
                    self.selection_cursor = None;
                    self.window.request_redraw();
                    return OverlayOutcome::None;
                };
                if selection.width < MIN_SELECTION_SIZE || selection.height < MIN_SELECTION_SIZE {
                    self.selection_cursor = None;
                    self.window.request_redraw();
                    return OverlayOutcome::None;
                }

                match self.create_editor(selection) {
                    Ok(editor) => {
                        self.editor = Some(editor);
                        self.selection_cursor = None;
                        self.window.request_redraw();
                        OverlayOutcome::None
                    }
                    Err(error) => OverlayOutcome::Error(error.to_string()),
                }
            }
        }
    }

    fn create_editor(&self, selection: PixelRect) -> Result<Editor> {
        let source_origin = self.source.origin();
        let x = source_origin
            .x()
            .checked_add(selection.x)
            .context("selection x coordinate overflowed")?;
        let y = source_origin
            .y()
            .checked_add(selection.y)
            .context("selection y coordinate overflowed")?;
        let crop_rect =
            PhysicalRect::new(PhysicalPoint::new(x, y), selection.width, selection.height)
                .context("selection rectangle is invalid")?;
        let frame = self
            .source
            .crop(crop_rect)
            .context("could not crop the selected pixels")?;

        Ok(Editor {
            selection,
            frame,
            document: AnnotationDocument::new(),
            tool: Tool::Pen,
            color_index: 0,
            width_index: 1,
            gesture: None,
            committed_preview: None,
        })
    }

    fn handle_key(&mut self, key: &Key) -> OverlayOutcome {
        if matches!(key, Key::Named(NamedKey::Escape)) {
            if let Some(editor) = &mut self.editor {
                if editor.gesture.take().is_some() {
                    self.window.request_redraw();
                    return OverlayOutcome::None;
                }
            }
            return OverlayOutcome::Cancel;
        }

        if self.editor.is_none() {
            return OverlayOutcome::None;
        }

        if self.modifiers.control_key() {
            let edit = if character_is(key, "z") {
                self.editor.as_mut().map(Editor::undo)
            } else if character_is(key, "y") {
                self.editor.as_mut().map(Editor::redo)
            } else {
                None
            };
            if let Some(edit) = edit {
                return self.finish_document_edit(edit);
            }
            return OverlayOutcome::None;
        }

        if character_is(key, "p") {
            self.editor.as_mut().expect("editor checked above").tool = Tool::Pen;
        } else if character_is(key, "h") {
            self.editor.as_mut().expect("editor checked above").tool = Tool::Highlighter;
        } else if character_is(key, "l") {
            self.editor.as_mut().expect("editor checked above").tool = Tool::Line;
        } else if character_is(key, "a") {
            self.editor.as_mut().expect("editor checked above").tool = Tool::Arrow;
        } else if character_is(key, "r") {
            self.editor.as_mut().expect("editor checked above").tool = Tool::Rectangle;
        } else if character_is(key, "u") {
            let edit = self.editor.as_mut().expect("editor checked above").undo();
            return self.finish_document_edit(edit);
        } else if character_is(key, "y") {
            let edit = self.editor.as_mut().expect("editor checked above").redo();
            return self.finish_document_edit(edit);
        } else if character_is(key, "x") {
            let edit = self.editor.as_mut().expect("editor checked above").clear();
            return self.finish_document_edit(edit);
        } else if character_is(key, "s") {
            return self.export(ExportKind::Save);
        } else if character_is(key, "c") {
            return self.export(ExportKind::Copy);
        } else if character_is(key, "o") {
            return self.export(ExportKind::Print);
        } else if matches!(key, Key::Named(NamedKey::Enter)) {
            return self.export(ExportKind::Copy);
        } else {
            return OverlayOutcome::None;
        }

        self.window.request_redraw();
        OverlayOutcome::None
    }

    fn begin_gesture(&mut self) -> Result<bool> {
        let Some(editor) = &mut self.editor else {
            return Ok(false);
        };
        if !editor.selection.contains(self.cursor) {
            return Ok(false);
        }
        let point = editor.selection.to_annotation_point(self.cursor);
        let tool = editor.tool;
        let style = editor.style()?;
        let shape = match tool {
            Tool::Pen | Tool::Highlighter => GestureShape::Path(vec![point]),
            Tool::Line | Tool::Arrow | Tool::Rectangle => GestureShape::Segment {
                start: point,
                current: point,
            },
        };
        editor.gesture = Some(Gesture { tool, style, shape });
        Ok(true)
    }

    fn update_gesture(&mut self) -> bool {
        let Some(editor) = &mut self.editor else {
            return false;
        };
        let point = editor.selection.to_annotation_point_clamped(self.cursor);
        editor
            .gesture
            .as_mut()
            .is_some_and(|gesture| gesture.update(point))
    }

    fn commit_gesture(&mut self) -> Result<()> {
        let Some(editor) = &mut self.editor else {
            return Ok(());
        };
        if editor.commit_active_gesture()? {
            self.window.request_redraw();
        }
        Ok(())
    }

    fn finish_document_edit(&mut self, edit: Result<bool>) -> OverlayOutcome {
        match edit {
            Ok(true) => {
                self.window.request_redraw();
                OverlayOutcome::None
            }
            Ok(false) => OverlayOutcome::None,
            Err(error) => OverlayOutcome::Error(error.to_string()),
        }
    }

    fn toolbar_button_at(&self, point: PixelPoint) -> Option<ToolbarButton> {
        let editor = self.editor.as_ref()?;
        editor
            .toolbar_layout(
                self.source.width(),
                self.source.height(),
                self.window.scale_factor(),
            )
            .button_at(point)
    }

    fn activate_toolbar(&mut self, button: ToolbarButton) -> OverlayOutcome {
        let export = match button {
            ToolbarButton::Save => Some(ExportKind::Save),
            ToolbarButton::Copy => Some(ExportKind::Copy),
            ToolbarButton::Print => Some(ExportKind::Print),
            _ => None,
        };
        if let Some(export) = export {
            return self.export(export);
        }

        let Some(editor) = &mut self.editor else {
            return OverlayOutcome::None;
        };
        match button {
            ToolbarButton::Pen => editor.tool = Tool::Pen,
            ToolbarButton::Highlighter => editor.tool = Tool::Highlighter,
            ToolbarButton::Line => editor.tool = Tool::Line,
            ToolbarButton::Arrow => editor.tool = Tool::Arrow,
            ToolbarButton::Rectangle => editor.tool = Tool::Rectangle,
            ToolbarButton::Undo => {
                let edit = editor.undo();
                return self.finish_document_edit(edit);
            }
            ToolbarButton::Redo => {
                let edit = editor.redo();
                return self.finish_document_edit(edit);
            }
            ToolbarButton::Color => {
                editor.color_index = (editor.color_index + 1) % PALETTE.len();
            }
            ToolbarButton::Width => {
                editor.width_index = (editor.width_index + 1) % STROKE_WIDTHS.len();
            }
            ToolbarButton::Save | ToolbarButton::Copy | ToolbarButton::Print => {
                unreachable!("export buttons were handled above")
            }
        }
        self.window.request_redraw();
        OverlayOutcome::None
    }

    fn export(&mut self, kind: ExportKind) -> OverlayOutcome {
        let Some(editor) = &mut self.editor else {
            return OverlayOutcome::None;
        };
        let frame = match editor.export_frame() {
            Ok(frame) => frame,
            Err(error) => return OverlayOutcome::Error(error.to_string()),
        };
        match kind {
            ExportKind::Save => OverlayOutcome::Save(frame),
            ExportKind::Copy => OverlayOutcome::Copy(frame),
            ExportKind::Print => OverlayOutcome::Print(frame),
        }
    }

    fn update_pointer_cursor(&self) {
        let icon = if self.editor.is_some() && self.toolbar_button_at(self.cursor).is_some() {
            CursorIcon::Pointer
        } else {
            CursorIcon::Crosshair
        };
        self.window.set_cursor(icon);
    }

    fn draw(&mut self) -> Result<()> {
        let width = self.source.width();
        let height = self.source.height();
        self.surface
            .resize(
                NonZeroU32::new(width).context("overlay width is zero")?,
                NonZeroU32::new(height).context("overlay height is zero")?,
            )
            .map_err(|error| anyhow::anyhow!("could not resize the overlay surface: {error}"))?;

        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|error| anyhow::anyhow!("could not access the overlay buffer: {error}"))?;
        draw_source_dimmed(&mut buffer, &self.source, 42);

        if let Some(editor) = &self.editor {
            blit_frame(
                &mut buffer,
                width,
                height,
                editor.committed_frame(),
                editor.selection.x,
                editor.selection.y,
            );
            if let Some(gesture) = &editor.gesture {
                draw_gesture_preview(&mut buffer, width, height, editor.selection, gesture);
            }
            draw_border(&mut buffer, width, height, editor.selection, 0x00ffffff);
            draw_toolbar(
                &mut buffer,
                width,
                height,
                editor,
                editor.toolbar_layout(width, height, self.window.scale_factor()),
            );
        } else if let (Some(anchor), Some(cursor)) = (self.selection_anchor, self.selection_cursor)
        {
            if let Some(selection) = PixelRect::between(anchor, cursor) {
                blit_region_undimmed(&mut buffer, &self.source, selection);
                draw_border(&mut buffer, width, height, selection, 0x00ffffff);
                draw_dimensions(&mut buffer, width, height, selection);
            }
        }

        buffer
            .present()
            .map_err(|error| anyhow::anyhow!("could not present the overlay: {error}"))?;
        Ok(())
    }
}

struct Editor {
    selection: PixelRect,
    frame: Frame,
    document: AnnotationDocument,
    tool: Tool,
    color_index: usize,
    width_index: usize,
    gesture: Option<Gesture>,
    /// Exact flattening of committed commands. `None` means the document is
    /// empty and `frame` itself is the committed image.
    committed_preview: Option<Frame>,
}

impl Editor {
    fn style(&self) -> Result<StrokeStyle> {
        let base = PALETTE[self.color_index];
        let (color, width) = if self.tool == Tool::Highlighter {
            (
                Color::rgba(base.red(), base.green(), base.blue(), 96),
                STROKE_WIDTHS[self.width_index] * 3.5,
            )
        } else {
            (base, STROKE_WIDTHS[self.width_index])
        };
        StrokeStyle::new(color, width).context("invalid annotation style")
    }

    fn annotation_for_gesture(gesture: &Gesture) -> Result<Annotation> {
        let annotation = match (gesture.tool, &gesture.shape) {
            (Tool::Pen, GestureShape::Path(points)) => {
                Annotation::pen(points.clone(), gesture.style)
            }
            (Tool::Highlighter, GestureShape::Path(points)) => {
                Annotation::highlighter(points.clone(), gesture.style)
            }
            (Tool::Line, GestureShape::Segment { start, current }) => {
                Annotation::line(*start, *current, gesture.style)
            }
            (Tool::Arrow, GestureShape::Segment { start, current }) => {
                Annotation::arrow(*start, *current, gesture.style)
            }
            (Tool::Rectangle, GestureShape::Segment { start, current }) => {
                Annotation::rectangle(*start, *current, gesture.style)
            }
            _ => return Err(anyhow::anyhow!("annotation gesture did not match its tool")),
        };
        annotation.context("could not create annotation")
    }

    fn committed_frame(&self) -> &Frame {
        self.committed_preview.as_ref().unwrap_or(&self.frame)
    }

    fn refresh_committed_preview(&mut self) -> Result<()> {
        self.committed_preview = if self.document.is_empty() {
            None
        } else {
            Some(
                self.document
                    .flatten(&self.frame)
                    .context("could not render committed annotations")?,
            )
        };
        Ok(())
    }

    fn commit_active_gesture(&mut self) -> Result<bool> {
        let Some(gesture) = self.gesture.take() else {
            return Ok(false);
        };
        let annotation = Self::annotation_for_gesture(&gesture)?;
        self.document.add(annotation);
        self.refresh_committed_preview()?;
        Ok(true)
    }

    fn undo(&mut self) -> Result<bool> {
        let changed = self.document.undo();
        if changed {
            self.refresh_committed_preview()?;
        }
        Ok(changed)
    }

    fn redo(&mut self) -> Result<bool> {
        let changed = self.document.redo();
        if changed {
            self.refresh_committed_preview()?;
        }
        Ok(changed)
    }

    fn clear(&mut self) -> Result<bool> {
        let changed = self.document.clear();
        if changed {
            self.refresh_committed_preview()?;
        }
        Ok(changed)
    }

    fn export_frame(&mut self) -> Result<Frame> {
        self.commit_active_gesture()?;
        // Output can be cancelled or fail, in which case the same editor is
        // shown again. Preserve the cache so retries keep every annotation.
        Ok(self.committed_frame().clone())
    }

    fn toolbar_layout(
        &self,
        source_width: u32,
        source_height: u32,
        scale_factor: f64,
    ) -> ToolbarLayout {
        ToolbarLayout::for_selection(self.selection, source_width, source_height, scale_factor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tool {
    Pen,
    Highlighter,
    Line,
    Arrow,
    Rectangle,
}

struct Gesture {
    tool: Tool,
    style: StrokeStyle,
    shape: GestureShape,
}

impl Gesture {
    fn update(&mut self, point: Point) -> bool {
        match &mut self.shape {
            GestureShape::Path(points) => {
                let should_add = points.last().is_none_or(|previous| {
                    let dx = previous.x() - point.x();
                    let dy = previous.y() - point.y();
                    dx.mul_add(dx, dy * dy) >= 1.5_f32.powi(2)
                });
                if should_add {
                    points.push(point);
                    true
                } else {
                    false
                }
            }
            GestureShape::Segment { current, .. } => {
                let changed = *current != point;
                *current = point;
                changed
            }
        }
    }
}

enum GestureShape {
    Path(Vec<Point>),
    Segment { start: Point, current: Point },
}

#[derive(Clone, Copy)]
enum ExportKind {
    Save,
    Copy,
    Print,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolbarButton {
    Pen,
    Highlighter,
    Line,
    Arrow,
    Rectangle,
    Undo,
    Redo,
    Color,
    Width,
    Save,
    Copy,
    Print,
}

impl ToolbarButton {
    const ALL: [Self; TOOLBAR_BUTTONS] = [
        Self::Pen,
        Self::Highlighter,
        Self::Line,
        Self::Arrow,
        Self::Rectangle,
        Self::Undo,
        Self::Redo,
        Self::Color,
        Self::Width,
        Self::Save,
        Self::Copy,
        Self::Print,
    ];

    const fn tool(self) -> Option<Tool> {
        match self {
            Self::Pen => Some(Tool::Pen),
            Self::Highlighter => Some(Tool::Highlighter),
            Self::Line => Some(Tool::Line),
            Self::Arrow => Some(Tool::Arrow),
            Self::Rectangle => Some(Tool::Rectangle),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct ToolbarLayout {
    x: i32,
    y: i32,
    button_size: i32,
}

impl ToolbarLayout {
    fn for_selection(
        selection: PixelRect,
        source_width: u32,
        source_height: u32,
        scale_factor: f64,
    ) -> Self {
        let screen_width = i32::try_from(source_width).unwrap_or(i32::MAX);
        let screen_height = i32::try_from(source_height).unwrap_or(i32::MAX);
        let padding = scaled_ui_size(TOOLBAR_PADDING, scale_factor);
        let gap = scaled_ui_size(TOOLBAR_GAP, scale_factor);
        let default_button_size = scaled_ui_size(DEFAULT_BUTTON_SIZE, scale_factor);
        let minimum_button_size = scaled_ui_size(MIN_BUTTON_SIZE, scale_factor);
        let button_count = i32::try_from(TOOLBAR_BUTTONS).unwrap_or(1);
        let available = (screen_width - padding * 2).max(1);
        let maximum_fitting_button = (available / button_count).max(1);
        let button_size = if maximum_fitting_button >= minimum_button_size {
            default_button_size.min(maximum_fitting_button)
        } else {
            maximum_fitting_button
        };
        let toolbar_width = button_size * i32::try_from(TOOLBAR_BUTTONS).unwrap_or_default();
        let maximum_x = (screen_width - toolbar_width - padding).max(0);
        let minimum_x = padding.min(maximum_x);
        let x = selection.x.clamp(minimum_x, maximum_x);

        let below = selection.bottom() + gap;
        let above = selection.y - gap - button_size;
        let y = if below + button_size + padding <= screen_height {
            below
        } else if above >= padding {
            above
        } else {
            (screen_height - button_size - padding).max(0)
        };
        Self { x, y, button_size }
    }

    fn button_at(self, point: PixelPoint) -> Option<ToolbarButton> {
        let width = self.button_size * i32::try_from(TOOLBAR_BUTTONS).unwrap_or_default();
        if point.x < self.x
            || point.y < self.y
            || point.x >= self.x + width
            || point.y >= self.y + self.button_size
        {
            return None;
        }
        let index = usize::try_from((point.x - self.x) / self.button_size).ok()?;
        ToolbarButton::ALL.get(index).copied()
    }
}

fn scaled_ui_size(base: i32, scale_factor: f64) -> i32 {
    let scale_factor = if scale_factor.is_finite() {
        scale_factor.clamp(1.0, 8.0)
    } else {
        1.0
    };
    #[allow(clippy::cast_possible_truncation)]
    let scaled = (f64::from(base) * scale_factor).round() as i32;
    scaled.max(1)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PixelPoint {
    x: i32,
    y: i32,
}

impl PixelPoint {
    fn from_position(position: PhysicalPosition<f64>) -> Self {
        #[allow(clippy::cast_possible_truncation)]
        Self {
            x: position.x.floor() as i32,
            y: position.y.floor() as i32,
        }
    }

    /// Clamps to real pixel coordinates for annotation and toolbar input.
    fn clamp_to_pixels(self, frame: &Frame) -> Self {
        let max_x = i32::try_from(frame.width().saturating_sub(1)).unwrap_or(i32::MAX);
        let max_y = i32::try_from(frame.height().saturating_sub(1)).unwrap_or(i32::MAX);
        Self {
            x: self.x.clamp(0, max_x),
            y: self.y.clamp(0, max_y),
        }
    }

    /// Clamps to half-open image edges for selection geometry.
    ///
    /// Unlike a drawable pixel, the right and bottom selection edges may equal
    /// the frame dimensions. This lets a drag reaching the window boundary
    /// include the final source column or row.
    fn clamp_to_edges(self, frame: &Frame) -> Self {
        let max_x = i32::try_from(frame.width()).unwrap_or(i32::MAX);
        let max_y = i32::try_from(frame.height()).unwrap_or(i32::MAX);
        Self {
            x: self.x.clamp(0, max_x),
            y: self.y.clamp(0, max_y),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PixelRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl PixelRect {
    fn between(first: PixelPoint, second: PixelPoint) -> Option<Self> {
        let x = first.x.min(second.x);
        let y = first.y.min(second.y);
        let width = u32::try_from((i64::from(first.x) - i64::from(second.x)).abs()).ok()?;
        let height = u32::try_from((i64::from(first.y) - i64::from(second.y)).abs()).ok()?;
        (width > 0 && height > 0).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }

    fn right(self) -> i32 {
        self.x
            .saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX))
    }

    fn bottom(self) -> i32 {
        self.y
            .saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX))
    }

    fn contains(self, point: PixelPoint) -> bool {
        point.x >= self.x && point.y >= self.y && point.x < self.right() && point.y < self.bottom()
    }

    fn to_annotation_point(self, point: PixelPoint) -> Point {
        Point::new((point.x - self.x) as f32, (point.y - self.y) as f32)
    }

    fn to_annotation_point_clamped(self, point: PixelPoint) -> Point {
        let x = point.x.clamp(self.x, self.right().saturating_sub(1));
        let y = point.y.clamp(self.y, self.bottom().saturating_sub(1));
        self.to_annotation_point(PixelPoint { x, y })
    }
}

fn character_is(key: &Key, expected: &str) -> bool {
    matches!(key, Key::Character(value) if value.eq_ignore_ascii_case(expected))
}

fn draw_source_dimmed(buffer: &mut [u32], source: &Frame, brightness: u8) {
    for (destination, pixel) in buffer.iter_mut().zip(source.rgba().chunks_exact(4)) {
        let red = u32::from(pixel[0]) * u32::from(brightness) / 100;
        let green = u32::from(pixel[1]) * u32::from(brightness) / 100;
        let blue = u32::from(pixel[2]) * u32::from(brightness) / 100;
        *destination = (red << 16) | (green << 8) | blue;
    }
}

fn blit_region_undimmed(buffer: &mut [u32], source: &Frame, region: PixelRect) {
    let source_width = usize::try_from(source.width()).unwrap_or_default();
    let x_start = usize::try_from(region.x.max(0)).unwrap_or_default();
    let y_start = usize::try_from(region.y.max(0)).unwrap_or_default();
    let x_end = usize::try_from(region.right().max(0))
        .unwrap_or_default()
        .min(source_width);
    let y_end = usize::try_from(region.bottom().max(0))
        .unwrap_or_default()
        .min(usize::try_from(source.height()).unwrap_or_default());

    for y in y_start..y_end {
        for x in x_start..x_end {
            let index = y * source_width + x;
            let offset = index * 4;
            let pixel = &source.rgba()[offset..offset + 4];
            buffer[index] =
                (u32::from(pixel[0]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[2]);
        }
    }
}

fn blit_frame(
    buffer: &mut [u32],
    target_width: u32,
    target_height: u32,
    source: &Frame,
    target_x: i32,
    target_y: i32,
) {
    let target_width = usize::try_from(target_width).unwrap_or_default();
    let target_height = usize::try_from(target_height).unwrap_or_default();
    let source_width = usize::try_from(source.width()).unwrap_or_default();
    let source_height = usize::try_from(source.height()).unwrap_or_default();
    let target_x = usize::try_from(target_x.max(0)).unwrap_or_default();
    let target_y = usize::try_from(target_y.max(0)).unwrap_or_default();

    let copy_width = source_width.min(target_width.saturating_sub(target_x));
    let copy_height = source_height.min(target_height.saturating_sub(target_y));
    for row in 0..copy_height {
        for column in 0..copy_width {
            let source_index = row * source_width + column;
            let destination_index = (target_y + row) * target_width + target_x + column;
            let offset = source_index * 4;
            let pixel = &source.rgba()[offset..offset + 4];
            buffer[destination_index] =
                (u32::from(pixel[0]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[2]);
        }
    }
}

fn draw_border(buffer: &mut [u32], width: u32, height: u32, rect: PixelRect, color: u32) {
    let left = rect.x;
    let top = rect.y;
    let right = rect.right().saturating_sub(1);
    let bottom = rect.bottom().saturating_sub(1);
    draw_line(buffer, width, height, left, top, right, top, color, 1);
    draw_line(buffer, width, height, right, top, right, bottom, color, 1);
    draw_line(buffer, width, height, right, bottom, left, bottom, color, 1);
    draw_line(buffer, width, height, left, bottom, left, top, color, 1);
}

fn draw_dimensions(buffer: &mut [u32], width: u32, height: u32, rect: PixelRect) {
    let label_width = 82;
    let label_height = 20;
    let x = rect.right().saturating_sub(label_width).max(rect.x).max(0);
    let y = rect.y.max(0);
    fill_rect(
        buffer,
        width,
        height,
        x,
        y,
        label_width,
        label_height,
        0x00_15171d,
    );
    let text = format!("{}x{}", rect.width, rect.height);
    draw_text(buffer, width, height, x + 6, y + 6, &text, 0x00ffffff, 1);
}

fn draw_toolbar(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    editor: &Editor,
    layout: ToolbarLayout,
) {
    let toolbar_width = layout.button_size * i32::try_from(TOOLBAR_BUTTONS).unwrap_or_default();
    fill_rect(
        buffer,
        width,
        height,
        layout.x - TOOLBAR_PADDING,
        layout.y - TOOLBAR_PADDING,
        toolbar_width + TOOLBAR_PADDING * 2,
        layout.button_size + TOOLBAR_PADDING * 2,
        0x00_171a22,
    );

    for (index, button) in ToolbarButton::ALL.iter().copied().enumerate() {
        let x = layout.x + i32::try_from(index).unwrap_or_default() * layout.button_size;
        let selected = button.tool().is_some_and(|tool| tool == editor.tool);
        let background = if selected { 0x00_2d74da } else { 0x00_242833 };
        fill_rect(
            buffer,
            width,
            height,
            x + 1,
            layout.y + 1,
            layout.button_size - 2,
            layout.button_size - 2,
            background,
        );
        draw_toolbar_icon(
            buffer,
            width,
            height,
            button,
            x,
            layout.y,
            layout.button_size,
            editor,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_toolbar_icon(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    button: ToolbarButton,
    x: i32,
    y: i32,
    size: i32,
    editor: &Editor,
) {
    let center_x = x + size / 2;
    let center_y = y + size / 2;
    let inset = (size / 4).max(5);
    let left = x + inset;
    let right = x + size - inset;
    let top = y + inset;
    let bottom = y + size - inset;
    let white = 0x00ffffff;

    match button {
        ToolbarButton::Pen => {
            draw_line(buffer, width, height, left, bottom, right, top, white, 3);
            draw_line(
                buffer,
                width,
                height,
                left,
                bottom,
                left + 5,
                bottom - 1,
                white,
                2,
            );
        }
        ToolbarButton::Highlighter => {
            draw_line(
                buffer,
                width,
                height,
                left,
                bottom - 2,
                right,
                top + 2,
                0x00ffe35a,
                6,
            );
        }
        ToolbarButton::Line => {
            draw_line(buffer, width, height, left, bottom, right, top, white, 2);
        }
        ToolbarButton::Arrow => {
            draw_line(buffer, width, height, left, bottom, right, top, white, 2);
            draw_line(buffer, width, height, right, top, right - 7, top, white, 2);
            draw_line(buffer, width, height, right, top, right, top + 7, white, 2);
        }
        ToolbarButton::Rectangle => {
            draw_rect(
                buffer,
                width,
                height,
                left,
                top,
                right - left,
                bottom - top,
                white,
                2,
            );
        }
        ToolbarButton::Undo => {
            draw_text(
                buffer,
                width,
                height,
                center_x - 4,
                center_y - 4,
                "U",
                white,
                1,
            );
        }
        ToolbarButton::Redo => {
            draw_text(
                buffer,
                width,
                height,
                center_x - 4,
                center_y - 4,
                "Y",
                white,
                1,
            );
        }
        ToolbarButton::Color => {
            let color = PALETTE[editor.color_index];
            let value = (u32::from(color.red()) << 16)
                | (u32::from(color.green()) << 8)
                | u32::from(color.blue());
            fill_rect(
                buffer,
                width,
                height,
                center_x - 7,
                center_y - 7,
                14,
                14,
                value,
            );
            draw_rect(
                buffer,
                width,
                height,
                center_x - 8,
                center_y - 8,
                16,
                16,
                white,
                1,
            );
        }
        ToolbarButton::Width => {
            let thickness = i32::try_from(editor.width_index + 2)
                .unwrap_or(3)
                .clamp(2, 5);
            draw_line(
                buffer, width, height, left, center_y, right, center_y, white, thickness,
            );
        }
        ToolbarButton::Save => {
            draw_rect(
                buffer,
                width,
                height,
                left,
                top,
                right - left,
                bottom - top,
                white,
                2,
            );
            fill_rect(buffer, width, height, center_x - 5, top, 10, 7, white);
            draw_rect(
                buffer,
                width,
                height,
                center_x - 6,
                center_y + 2,
                12,
                7,
                white,
                1,
            );
        }
        ToolbarButton::Copy => {
            draw_rect(
                buffer,
                width,
                height,
                left + 4,
                top,
                right - left - 4,
                bottom - top - 4,
                white,
                1,
            );
            draw_rect(
                buffer,
                width,
                height,
                left,
                top + 4,
                right - left - 4,
                bottom - top - 4,
                white,
                2,
            );
        }
        ToolbarButton::Print => {
            draw_rect(
                buffer,
                width,
                height,
                left + 4,
                top,
                right - left - 8,
                7,
                white,
                1,
            );
            fill_rect(
                buffer,
                width,
                height,
                left,
                center_y - 4,
                right - left,
                10,
                white,
            );
            draw_rect(
                buffer,
                width,
                height,
                left + 4,
                center_y + 2,
                right - left - 8,
                8,
                0x00_171a22,
                1,
            );
        }
    }
}

fn draw_gesture_preview(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    selection: PixelRect,
    gesture: &Gesture,
) {
    let color = gesture.style.color();
    #[allow(clippy::cast_possible_truncation)]
    let thickness = gesture.style.width().round().max(1.0) as i32;

    match &gesture.shape {
        GestureShape::Path(points) => {
            if let [point] = points.as_slice() {
                let (x, y) = annotation_pixel(selection, *point);
                draw_preview_line(buffer, width, height, x, y, x, y, color, thickness);
            }
            for segment in points.windows(2) {
                let (x0, y0) = annotation_pixel(selection, segment[0]);
                let (x1, y1) = annotation_pixel(selection, segment[1]);
                draw_preview_line(buffer, width, height, x0, y0, x1, y1, color, thickness);
            }
        }
        GestureShape::Segment { start, current } => {
            let (x0, y0) = annotation_pixel(selection, *start);
            let (x1, y1) = annotation_pixel(selection, *current);
            match gesture.tool {
                Tool::Line => {
                    draw_preview_line(buffer, width, height, x0, y0, x1, y1, color, thickness)
                }
                Tool::Arrow => {
                    draw_preview_line(buffer, width, height, x0, y0, x1, y1, color, thickness);
                    draw_preview_arrow_head(
                        buffer,
                        width,
                        height,
                        selection,
                        *start,
                        *current,
                        gesture.style,
                    );
                }
                Tool::Rectangle => {
                    draw_preview_line(buffer, width, height, x0, y0, x1, y0, color, thickness);
                    draw_preview_line(buffer, width, height, x1, y0, x1, y1, color, thickness);
                    draw_preview_line(buffer, width, height, x1, y1, x0, y1, color, thickness);
                    draw_preview_line(buffer, width, height, x0, y1, x0, y0, color, thickness);
                }
                Tool::Pen | Tool::Highlighter => {}
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_preview_arrow_head(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    selection: PixelRect,
    start: Point,
    end: Point,
    style: StrokeStyle,
) {
    let dx = end.x() - start.x();
    let dy = end.y() - start.y();
    let length = dx.hypot(dy);
    if length <= f32::EPSILON {
        return;
    }

    let head_length = (style.width() * 4.0 + 6.0).min(length * 0.45);
    let angle = dy.atan2(dx);
    let spread = std::f32::consts::FRAC_PI_6;
    let (end_x, end_y) = annotation_pixel(selection, end);
    #[allow(clippy::cast_possible_truncation)]
    let thickness = style.width().round().max(1.0) as i32;
    for head_angle in [angle + spread, angle - spread] {
        let head = Point::new(
            end.x() - head_length * head_angle.cos(),
            end.y() - head_length * head_angle.sin(),
        );
        let (head_x, head_y) = annotation_pixel(selection, head);
        draw_preview_line(
            buffer,
            width,
            height,
            end_x,
            end_y,
            head_x,
            head_y,
            style.color(),
            thickness,
        );
    }
}

#[allow(clippy::cast_possible_truncation)]
fn annotation_pixel(selection: PixelRect, point: Point) -> (i32, i32) {
    (
        selection.x.saturating_add(point.x().round() as i32),
        selection.y.saturating_add(point.y().round() as i32),
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_preview_line(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: Color,
    thickness: i32,
) {
    let dx = (x1 - x0).abs();
    let step_x = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let step_y = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;

    loop {
        blend_preview_brush(buffer, width, height, x0, y0, color, thickness);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let doubled = error * 2;
        if doubled >= dy {
            error += dy;
            x0 += step_x;
        }
        if doubled <= dx {
            error += dx;
            y0 += step_y;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blend_preview_brush(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    color: Color,
    thickness: i32,
) {
    let before = thickness.max(1) / 2;
    let after = (thickness.max(1) - 1) / 2;
    for brush_y in y.saturating_sub(before)..=y.saturating_add(after) {
        for brush_x in x.saturating_sub(before)..=x.saturating_add(after) {
            blend_preview_pixel(buffer, width, height, brush_x, brush_y, color);
        }
    }
}

fn blend_preview_pixel(buffer: &mut [u32], width: u32, height: u32, x: i32, y: i32, color: Color) {
    let Ok(x) = u32::try_from(x) else {
        return;
    };
    let Ok(y) = u32::try_from(y) else {
        return;
    };
    if x >= width || y >= height {
        return;
    }
    let Ok(index) = usize::try_from(u64::from(y) * u64::from(width) + u64::from(x)) else {
        return;
    };
    let Some(destination) = buffer.get_mut(index) else {
        return;
    };

    let alpha = u32::from(color.alpha());
    let inverse = 255 - alpha;
    let red =
        (u32::from(color.red()) * alpha + ((*destination >> 16) & 0xff) * inverse + 127) / 255;
    let green =
        (u32::from(color.green()) * alpha + ((*destination >> 8) & 0xff) * inverse + 127) / 255;
    let blue = (u32::from(color.blue()) * alpha + (*destination & 0xff) * inverse + 127) / 255;
    *destination = (red << 16) | (green << 8) | blue;
}

#[allow(clippy::too_many_arguments)]
fn fill_rect(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rect_width: i32,
    rect_height: i32,
    color: u32,
) {
    if rect_width <= 0 || rect_height <= 0 {
        return;
    }
    let width_usize = usize::try_from(width).unwrap_or_default();
    let x_start = usize::try_from(x.max(0)).unwrap_or_default();
    let y_start = usize::try_from(y.max(0)).unwrap_or_default();
    let x_end = usize::try_from(x.saturating_add(rect_width).max(0))
        .unwrap_or_default()
        .min(width_usize);
    let y_end = usize::try_from(y.saturating_add(rect_height).max(0))
        .unwrap_or_default()
        .min(usize::try_from(height).unwrap_or_default());
    for row in y_start..y_end {
        let start = row * width_usize + x_start;
        let end = row * width_usize + x_end;
        buffer[start..end].fill(color);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_rect(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rect_width: i32,
    rect_height: i32,
    color: u32,
    thickness: i32,
) {
    draw_line(
        buffer,
        width,
        height,
        x,
        y,
        x + rect_width,
        y,
        color,
        thickness,
    );
    draw_line(
        buffer,
        width,
        height,
        x + rect_width,
        y,
        x + rect_width,
        y + rect_height,
        color,
        thickness,
    );
    draw_line(
        buffer,
        width,
        height,
        x + rect_width,
        y + rect_height,
        x,
        y + rect_height,
        color,
        thickness,
    );
    draw_line(
        buffer,
        width,
        height,
        x,
        y + rect_height,
        x,
        y,
        color,
        thickness,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_line(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
    thickness: i32,
) {
    let dx = (x1 - x0).abs();
    let step_x = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let step_y = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;

    loop {
        let radius = (thickness.max(1) - 1) / 2;
        fill_rect(
            buffer,
            width,
            height,
            x0 - radius,
            y0 - radius,
            radius * 2 + 1,
            radius * 2 + 1,
            color,
        );
        if x0 == x1 && y0 == y1 {
            break;
        }
        let doubled = error * 2;
        if doubled >= dy {
            error += dy;
            x0 += step_x;
        }
        if doubled <= dx {
            error += dx;
            y0 += step_y;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    mut x: i32,
    y: i32,
    text: &str,
    color: u32,
    scale: i32,
) {
    for character in text.chars() {
        let rows = glyph(character);
        for (row, bits) in rows.into_iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    fill_rect(
                        buffer,
                        width,
                        height,
                        x + column * scale,
                        y + i32::try_from(row).unwrap_or_default() * scale,
                        scale,
                        scale,
                        color,
                    );
                }
            }
        }
        x += 6 * scale;
    }
}

fn glyph(character: char) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1e, 0x01, 0x01, 0x0e, 0x01, 0x01, 0x1e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x10, 0x1e, 0x01, 0x01, 0x1e],
        '6' => [0x0e, 0x10, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x01, 0x0e],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        _ => [0; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustshot::annotation::flatten;

    fn test_editor() -> Editor {
        let frame = Frame::new(PhysicalPoint::new(-20, 30), 8, 8, vec![255; 8 * 8 * 4])
            .expect("test frame is valid");
        Editor {
            selection: PixelRect {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            },
            frame,
            document: AnnotationDocument::new(),
            tool: Tool::Pen,
            color_index: 0,
            width_index: 1,
            gesture: None,
            committed_preview: None,
        }
    }

    fn line_gesture(tool: Tool, style: StrokeStyle) -> Gesture {
        Gesture {
            tool,
            style,
            shape: GestureShape::Segment {
                start: Point::new(1.0, 1.0),
                current: Point::new(6.0, 6.0),
            },
        }
    }

    #[test]
    fn selection_normalizes_drag_direction() {
        let selection =
            PixelRect::between(PixelPoint { x: 90, y: 70 }, PixelPoint { x: 10, y: 20 })
                .expect("non-empty selection");
        assert_eq!(
            selection,
            PixelRect {
                x: 10,
                y: 20,
                width: 80,
                height: 50,
            }
        );
    }

    #[test]
    fn annotation_points_are_local_and_clamped() {
        let selection = PixelRect {
            x: 100,
            y: 50,
            width: 40,
            height: 20,
        };
        assert_eq!(
            selection.to_annotation_point(PixelPoint { x: 110, y: 55 }),
            Point::new(10.0, 5.0)
        );
        assert_eq!(
            selection.to_annotation_point_clamped(PixelPoint { x: 999, y: -20 }),
            Point::new(39.0, 0.0)
        );
    }

    #[test]
    fn selection_edges_can_include_the_last_source_pixel() {
        let frame = test_editor().frame;
        let edge = PixelPoint { x: 99, y: 99 }.clamp_to_edges(&frame);
        assert_eq!(edge, PixelPoint { x: 8, y: 8 });
        assert_eq!(
            PixelPoint { x: 99, y: 99 }.clamp_to_pixels(&frame),
            PixelPoint { x: 7, y: 7 }
        );

        let selection =
            PixelRect::between(PixelPoint { x: 0, y: 0 }, edge).expect("selection is non-empty");
        assert_eq!((selection.width, selection.height), (8, 8));
    }

    #[test]
    fn gesture_preview_draws_directly_into_the_presentation_buffer() {
        let mut buffer = vec![0x00_102030; 8 * 8];
        let selection = PixelRect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        };
        let gesture = line_gesture(
            Tool::Line,
            StrokeStyle::new(Color::rgba(255, 230, 0, 96), 2.0).expect("valid style"),
        );

        draw_gesture_preview(&mut buffer, 8, 8, selection, &gesture);

        assert!(buffer.iter().any(|pixel| *pixel != 0x00_102030));
        assert!(
            buffer.iter().all(|pixel| *pixel != 0x00_ffe600),
            "translucent preview should blend instead of overwriting"
        );
    }

    #[test]
    fn committed_cache_refreshes_only_when_document_changes() {
        let mut editor = test_editor();
        let original = editor.frame.clone();
        editor.gesture = Some(line_gesture(
            Tool::Line,
            StrokeStyle::new(Color::RED, 2.0).expect("valid style"),
        ));

        assert!(editor.commit_active_gesture().expect("gesture commits"));
        let committed = editor
            .document
            .flatten(&original)
            .expect("document flattens");
        assert_eq!(editor.committed_frame(), &committed);

        assert!(editor.undo().expect("undo succeeds"));
        assert_eq!(editor.committed_frame(), &original);
        assert!(editor.redo().expect("redo succeeds"));
        assert_eq!(editor.committed_frame(), &committed);
        assert!(editor.clear().expect("clear succeeds"));
        assert_eq!(editor.committed_frame(), &original);
    }

    #[test]
    fn gesture_keeps_its_original_tool_and_style_and_exports_exactly() {
        let mut editor = test_editor();
        let style = StrokeStyle::new(Color::rgba(12, 34, 56, 255), 3.0).expect("valid style");
        let gesture = Gesture {
            tool: Tool::Pen,
            style,
            shape: GestureShape::Path(vec![
                Point::new(1.0, 1.0),
                Point::new(3.0, 3.0),
                Point::new(6.0, 2.0),
            ]),
        };
        let annotation =
            Editor::annotation_for_gesture(&gesture).expect("gesture creates an annotation");
        let expected = flatten(&editor.frame, &[annotation]).expect("expected image flattens");
        editor.gesture = Some(gesture);

        // A keyboard tool change during the drag must not reinterpret its shape.
        editor.tool = Tool::Rectangle;
        editor.color_index = 5;
        editor.width_index = 3;

        let exported = editor.export_frame().expect("active gesture exports");
        assert_eq!(exported, expected);
        let retried = editor.export_frame().expect("output can be retried");
        assert_eq!(retried, expected);
        assert_eq!(editor.committed_frame(), &expected);
        assert!(matches!(
            editor.document.annotations(),
            [Annotation::Pen(_)]
        ));
        assert!(editor.gesture.is_none());
    }

    #[test]
    fn gesture_update_reports_only_visible_changes() {
        let style = StrokeStyle::default();
        let mut gesture = line_gesture(Tool::Line, style);
        assert!(!gesture.update(Point::new(6.0, 6.0)));
        assert!(gesture.update(Point::new(5.0, 6.0)));
        assert!(!gesture.update(Point::new(5.0, 6.0)));

        let mut path = Gesture {
            tool: Tool::Pen,
            style,
            shape: GestureShape::Path(vec![Point::new(1.0, 1.0)]),
        };
        assert!(!path.update(Point::new(1.5, 1.5)));
        assert!(path.update(Point::new(3.0, 1.0)));
    }

    #[test]
    fn toolbar_layout_does_not_panic_on_a_narrow_capture() {
        let selection = PixelRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let layout = ToolbarLayout::for_selection(selection, 8, 8, 1.0);
        assert!(layout.button_size > 0);
        assert!(layout.x >= 0);
        assert!(layout.y >= 0);
    }

    #[test]
    fn toolbar_scales_for_high_density_displays() {
        let selection = PixelRect {
            x: 100,
            y: 100,
            width: 500,
            height: 300,
        };
        let normal = ToolbarLayout::for_selection(selection, 1920, 1080, 1.0);
        let high_density = ToolbarLayout::for_selection(selection, 3840, 2160, 2.0);

        assert_eq!(normal.button_size, DEFAULT_BUTTON_SIZE);
        assert_eq!(high_density.button_size, DEFAULT_BUTTON_SIZE * 2);
    }
}
