//! Full-screen region selection and allocation-conscious annotation window.

use std::{mem::size_of, num::NonZeroU32, rc::Rc};

use anyhow::{Context as _, Result};
use rustshot::{
    annotation::{
        draw_text_bgrx, measure_text, Annotation, AnnotationDocument, Bounds, Color, Point,
        StrokeStyle,
    },
    config::{EditorTool, RgbColor},
    frame::{Frame, PhysicalPoint, PhysicalRect},
};
use softbuffer::{Context, Surface};
use windows::Win32::{
    Foundation::{COLORREF, HWND, LPARAM},
    UI::Controls::Dialogs::{ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW},
};
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
const MIN_OBJECT_SIZE: f32 = 3.0;
const HANDLE_RADIUS: i32 = 5;
const HANDLE_HIT_RADIUS: f32 = 9.0;
const TOOLBAR_PADDING: i32 = 4;
const TOOLBAR_GAP: i32 = 8;
const TOOLBAR_BUTTONS: usize = 21;
const TOOLBAR_COLUMNS: usize = 11;
const DEFAULT_BUTTON_SIZE: i32 = 38;
const MIN_BUTTON_SIZE: i32 = 24;

const PALETTE: [Color; 6] = [
    Color::rgba(255, 64, 64, 255),
    Color::rgba(255, 196, 0, 255),
    Color::rgba(40, 205, 90, 255),
    Color::rgba(45, 135, 255, 255),
    Color::WHITE,
    Color::BLACK,
];
const STROKE_WIDTHS: [f32; 4] = [2.0, 4.0, 7.0, 11.0];
const TEXT_SIZES: [f32; 4] = [14.0, 18.0, 24.0, 32.0];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorPreferences {
    pub tool: EditorTool,
    pub color: RgbColor,
    pub stroke_width: u8,
}

/// Result of one overlay event.
pub enum OverlayOutcome {
    None,
    Cancel,
    /// Region selected while an automatic post-capture action is configured.
    Finish(Frame),
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
    preferences: EditorPreferences,
    finish_after_selection: bool,
    hovered_button: Option<ToolbarButton>,
}

impl Overlay {
    pub fn new(
        event_loop: &ActiveEventLoop,
        source: Frame,
        preferences: EditorPreferences,
        finish_after_selection: bool,
    ) -> Result<Self> {
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
            preferences,
            finish_after_selection,
            hovered_button: None,
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

    #[must_use]
    pub fn editor_preferences(&self) -> EditorPreferences {
        self.editor
            .as_ref()
            .map_or(self.preferences, Editor::preferences)
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
                let previous_hover = self.hovered_button;
                self.hovered_button = self.toolbar_button_at(self.cursor);
                self.update_pointer_cursor();
                let interaction_changed = if self.editor.is_some() {
                    self.update_gesture()
                } else if self.selection_anchor.is_some() {
                    let changed = self.selection_cursor != Some(self.cursor);
                    self.selection_cursor = Some(self.cursor);
                    changed
                } else {
                    false
                };
                let changed = interaction_changed || previous_hover != self.hovered_button;
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
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => self.handle_right_button(),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed => {
                self.handle_key(&event.logical_key, event.text.as_deref())
            }
            _ => OverlayOutcome::None,
        }
    }

    fn handle_left_button(&mut self, state: ElementState) -> OverlayOutcome {
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.text_draft.is_some())
        {
            return OverlayOutcome::None;
        }
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

    fn handle_right_button(&mut self) -> OverlayOutcome {
        if self.toolbar_button_at(self.cursor) != Some(ToolbarButton::Color) {
            return OverlayOutcome::None;
        }
        match self.choose_custom_color() {
            Ok(true) => {
                self.window.request_redraw();
                OverlayOutcome::None
            }
            Ok(false) => OverlayOutcome::None,
            Err(error) => OverlayOutcome::Error(error.to_string()),
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

                if self.finish_after_selection {
                    return match self.crop_source(selection) {
                        Ok(frame) => OverlayOutcome::Finish(frame),
                        Err(error) => OverlayOutcome::Error(error.to_string()),
                    };
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

    fn crop_source(&self, selection: PixelRect) -> Result<Frame> {
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
        self.source
            .crop(crop_rect)
            .context("could not crop the selected pixels")
    }

    fn create_editor(&self, selection: PixelRect) -> Result<Editor> {
        let frame = self.crop_source(selection)?;

        Ok(Editor {
            selection,
            frame,
            document: AnnotationDocument::new(),
            tool: Tool::from_preference(self.preferences.tool),
            color: Color::rgba(
                self.preferences.color.red,
                self.preferences.color.green,
                self.preferences.color.blue,
                255,
            ),
            width_index: usize::from(self.preferences.stroke_width.min(3)),
            gesture: None,
            crop_gesture: None,
            text_draft: None,
            selected: None,
            manipulation_base: None,
            committed_preview: None,
        })
    }

    fn handle_key(&mut self, key: &Key, text: Option<&str>) -> OverlayOutcome {
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.text_draft.is_some())
        {
            return self.handle_text_key(key, text);
        }
        if matches!(key, Key::Named(NamedKey::Escape)) {
            if let Some(editor) = &mut self.editor {
                match editor.cancel_active_interaction() {
                    Ok(true) => {
                        self.window.request_redraw();
                        return OverlayOutcome::None;
                    }
                    Ok(false) => {}
                    Err(error) => return OverlayOutcome::Error(error.to_string()),
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

        let tool = Tool::ALL
            .into_iter()
            .find(|tool| character_is(key, tool.shortcut()));
        if let Some(tool) = tool {
            self.editor.as_mut().expect("editor checked above").tool = tool;
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
        } else if character_is(key, "k") {
            return match self.choose_custom_color() {
                Ok(_) => OverlayOutcome::None,
                Err(error) => OverlayOutcome::Error(error.to_string()),
            };
        } else if matches!(key, Key::Named(NamedKey::Delete)) {
            let edit = self
                .editor
                .as_mut()
                .expect("editor checked above")
                .delete_selected();
            return self.finish_document_edit(edit);
        } else if matches!(key, Key::Named(NamedKey::Enter)) {
            return self.export(ExportKind::Copy);
        } else {
            return OverlayOutcome::None;
        }

        self.window.request_redraw();
        OverlayOutcome::None
    }

    fn handle_text_key(&mut self, key: &Key, text: Option<&str>) -> OverlayOutcome {
        let editor = self.editor.as_mut().expect("text input requires an editor");
        if matches!(key, Key::Named(NamedKey::Escape)) {
            editor.text_draft = None;
            self.window.request_redraw();
            return OverlayOutcome::None;
        }
        if matches!(key, Key::Named(NamedKey::Enter)) {
            if self.modifiers.shift_key() {
                editor
                    .text_draft
                    .as_mut()
                    .expect("draft checked")
                    .text
                    .push('\n');
                self.window.request_redraw();
                return OverlayOutcome::None;
            }
            let edit = editor.commit_text_draft();
            return self.finish_document_edit(edit);
        }
        if matches!(key, Key::Named(NamedKey::Backspace)) {
            editor
                .text_draft
                .as_mut()
                .expect("draft checked")
                .text
                .pop();
            self.window.request_redraw();
            return OverlayOutcome::None;
        }
        if !self.modifiers.control_key() && !self.modifiers.super_key() {
            if let Some(text) = text.filter(|value| !value.chars().any(char::is_control)) {
                editor
                    .text_draft
                    .as_mut()
                    .expect("draft checked")
                    .text
                    .push_str(text);
                self.window.request_redraw();
            }
        }
        OverlayOutcome::None
    }

    fn begin_gesture(&mut self) -> Result<bool> {
        let Some(editor) = &mut self.editor else {
            return Ok(false);
        };
        if editor.tool == Tool::Crop {
            return Ok(editor.begin_crop(self.cursor));
        }
        if !editor.selection.contains(self.cursor) {
            return Ok(false);
        }
        let point = editor.selection.to_annotation_point(self.cursor);
        let tool = editor.tool;
        if tool == Tool::Select {
            return editor.begin_object_transform(point);
        }
        if tool == Tool::Eraser {
            let changed = editor.erase_at(point)?;
            return Ok(changed);
        }
        if tool == Tool::Eyedropper {
            let offset = (usize::try_from(self.cursor.y).unwrap_or_default()
                * usize::try_from(self.source.width()).unwrap_or_default()
                + usize::try_from(self.cursor.x).unwrap_or_default())
                * 4;
            if let Some(pixel) = self.source.rgba().get(offset..offset + 3) {
                editor.color = Color::rgba(pixel[0], pixel[1], pixel[2], 255);
                editor.tool = Tool::Pen;
                return Ok(true);
            }
            return Ok(false);
        }
        if tool == Tool::Text {
            editor.text_draft = Some(TextDraft::new(
                point,
                None,
                editor.color,
                editor.text_size(),
            ));
            return Ok(true);
        }
        let style = editor.style()?;
        let shape = match tool {
            Tool::Pen | Tool::Highlighter => GestureShape::Path(vec![point]),
            Tool::Line
            | Tool::Arrow
            | Tool::Rectangle
            | Tool::Ellipse
            | Tool::Redact
            | Tool::Pixelate
            | Tool::Callout => GestureShape::Segment {
                start: point,
                current: point,
            },
            Tool::Select | Tool::Text | Tool::Eraser | Tool::Crop | Tool::Eyedropper => {
                return Ok(false)
            }
        };
        editor.gesture = Some(Gesture { tool, style, shape });
        Ok(true)
    }

    fn update_gesture(&mut self) -> bool {
        let Some(editor) = &mut self.editor else {
            return false;
        };
        if editor.crop_gesture.is_some() {
            return editor.update_crop(self.cursor, self.source.width(), self.source.height());
        }
        let point = editor.selection.to_annotation_point_clamped(self.cursor);
        editor.update_gesture(point)
    }

    fn commit_gesture(&mut self) -> Result<()> {
        let Some(editor) = &mut self.editor else {
            return Ok(());
        };
        let changed = if editor.crop_gesture.is_some() {
            editor.commit_crop(&self.source)?
        } else {
            editor.commit_active_gesture()?
        };
        if changed {
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
            ToolbarButton::Select => editor.tool = Tool::Select,
            ToolbarButton::Pen => editor.tool = Tool::Pen,
            ToolbarButton::Highlighter => editor.tool = Tool::Highlighter,
            ToolbarButton::Line => editor.tool = Tool::Line,
            ToolbarButton::Arrow => editor.tool = Tool::Arrow,
            ToolbarButton::Rectangle => editor.tool = Tool::Rectangle,
            ToolbarButton::Ellipse => editor.tool = Tool::Ellipse,
            ToolbarButton::Text => editor.tool = Tool::Text,
            ToolbarButton::Callout => editor.tool = Tool::Callout,
            ToolbarButton::Redact => editor.tool = Tool::Redact,
            ToolbarButton::Pixelate => editor.tool = Tool::Pixelate,
            ToolbarButton::Eraser => editor.tool = Tool::Eraser,
            ToolbarButton::Crop => editor.tool = Tool::Crop,
            ToolbarButton::Eyedropper => editor.tool = Tool::Eyedropper,
            ToolbarButton::Undo => {
                let edit = editor.undo();
                return self.finish_document_edit(edit);
            }
            ToolbarButton::Redo => {
                let edit = editor.redo();
                return self.finish_document_edit(edit);
            }
            ToolbarButton::Color => {
                let index = PALETTE.iter().position(|color| *color == editor.color);
                editor.color = PALETTE[index.map_or(0, |index| (index + 1) % PALETTE.len())];
                if let Some(selected) = editor.selected {
                    let edit = editor.restyle_selected(selected);
                    return self.finish_document_edit(edit);
                }
            }
            ToolbarButton::Width => {
                editor.width_index = (editor.width_index + 1) % STROKE_WIDTHS.len();
                if let Some(selected) = editor.selected {
                    let edit = editor.restyle_selected(selected);
                    return self.finish_document_edit(edit);
                }
            }
            ToolbarButton::Save | ToolbarButton::Copy | ToolbarButton::Print => {
                unreachable!("export buttons were handled above")
            }
        }
        self.window.request_redraw();
        OverlayOutcome::None
    }

    #[allow(unsafe_code)]
    fn choose_custom_color(&mut self) -> Result<bool> {
        let owner = self.owner_hwnd()?;
        let Some(editor) = &mut self.editor else {
            return Ok(false);
        };
        let mut custom = [COLORREF(0); 16];
        let rgb = u32::from(editor.color.red())
            | (u32::from(editor.color.green()) << 8)
            | (u32::from(editor.color.blue()) << 16);
        let mut chooser = CHOOSECOLORW {
            lStructSize: u32::try_from(size_of::<CHOOSECOLORW>())
                .context("color dialog structure size overflow")?,
            hwndOwner: owner,
            rgbResult: COLORREF(rgb),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_FULLOPEN | CC_RGBINIT,
            lCustData: LPARAM(0),
            ..Default::default()
        };
        // SAFETY: the structure and custom-color buffer remain valid for the modal call.
        if !unsafe { ChooseColorW(&mut chooser) }.as_bool() {
            return Ok(false);
        }
        let value = chooser.rgbResult.0;
        editor.color = Color::rgba(
            (value & 0xff) as u8,
            ((value >> 8) & 0xff) as u8,
            ((value >> 16) & 0xff) as u8,
            255,
        );
        if let Some(selected) = editor.selected {
            editor.restyle_selected(selected)?;
        }
        self.window.request_redraw();
        Ok(true)
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
        let icon = if self.hovered_button.is_some() {
            CursorIcon::Pointer
        } else if self.editor.as_ref().is_some_and(|editor| {
            editor.tool == Tool::Select && editor.object_handle_at(self.cursor).is_some()
        }) {
            CursorIcon::Move
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
            let selection = editor
                .crop_gesture
                .as_ref()
                .map_or(editor.selection, |gesture| gesture.preview);
            if editor.crop_gesture.is_some() {
                blit_region_undimmed(&mut buffer, &self.source, selection);
            }
            blit_frame(
                &mut buffer,
                width,
                height,
                editor.presentation_frame(),
                editor.selection.x,
                editor.selection.y,
            );
            if let Some(gesture) = &editor.gesture {
                draw_gesture_preview(&mut buffer, width, height, editor.selection, gesture);
            }
            if let Some(draft) = &editor.text_draft {
                draw_text_draft(&mut buffer, width, height, editor.selection, draft);
            }
            draw_border(&mut buffer, width, height, selection, 0x00ffffff);
            if editor.tool == Tool::Crop {
                draw_resize_handles(&mut buffer, width, height, pixel_rect_bounds(selection));
            }
            let selected_bounds = editor
                .gesture
                .as_ref()
                .and_then(|gesture| match &gesture.shape {
                    GestureShape::Object { preview, .. } => Some(preview.bounds()),
                    _ => None,
                })
                .or_else(|| {
                    editor
                        .selected
                        .and_then(|index| editor.document.annotation(index))
                        .map(Annotation::bounds)
                });
            if let Some(selected_bounds) = selected_bounds {
                draw_object_selection(
                    &mut buffer,
                    width,
                    height,
                    editor.selection,
                    selected_bounds,
                );
            }
            let layout = editor.toolbar_layout(width, height, self.window.scale_factor());
            draw_toolbar(
                &mut buffer,
                width,
                height,
                editor,
                layout,
                self.hovered_button,
            );
            draw_shortcut_hints(&mut buffer, width, height);
            if let Some(button) = self.hovered_button {
                draw_tooltip(&mut buffer, width, height, layout, button, button.tooltip());
            }
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
    color: Color,
    width_index: usize,
    gesture: Option<Gesture>,
    crop_gesture: Option<CropGesture>,
    text_draft: Option<TextDraft>,
    selected: Option<usize>,
    /// Cached document without the actively transformed object; allocated once per drag.
    manipulation_base: Option<Frame>,
    /// Exact flattening of committed commands. `None` means the document is
    /// empty and `frame` itself is the committed image.
    committed_preview: Option<Frame>,
}

impl Editor {
    fn preferences(&self) -> EditorPreferences {
        EditorPreferences {
            tool: self.tool.preference(),
            color: RgbColor::new(self.color.red(), self.color.green(), self.color.blue()),
            stroke_width: u8::try_from(self.width_index).unwrap_or(1).min(3),
        }
    }

    fn style(&self) -> Result<StrokeStyle> {
        let base = self.color;
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

    fn text_size(&self) -> f32 {
        TEXT_SIZES[self.width_index]
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
            (Tool::Ellipse, GestureShape::Segment { start, current }) => {
                Annotation::ellipse(*start, *current, gesture.style)
            }
            (Tool::Redact, GestureShape::Segment { start, current }) => {
                Annotation::redact(*start, *current)
            }
            (Tool::Pixelate, GestureShape::Segment { start, current }) => {
                Annotation::pixelate(*start, *current, 12)
            }
            _ => return Err(anyhow::anyhow!("annotation gesture did not match its tool")),
        };
        annotation.context("could not create annotation")
    }

    fn committed_frame(&self) -> &Frame {
        self.committed_preview.as_ref().unwrap_or(&self.frame)
    }

    fn presentation_frame(&self) -> &Frame {
        self.manipulation_base
            .as_ref()
            .unwrap_or_else(|| self.committed_frame())
    }

    fn refresh_committed_preview(&mut self) -> Result<()> {
        if self.document.is_empty() {
            self.committed_preview = None;
            return Ok(());
        }

        // Flattening needs one destination frame and one temporary vector
        // surface. Release the stale committed frame first so a refresh never
        // peaks at three region-sized editor buffers.
        self.committed_preview = None;
        let preview = self
            .document
            .flatten(&self.frame)
            .context("could not render committed annotations")?;
        self.committed_preview = Some(preview);
        Ok(())
    }

    fn commit_active_gesture(&mut self) -> Result<bool> {
        let Some(gesture) = self.gesture.take() else {
            return Ok(false);
        };
        let changed = match &gesture.shape {
            GestureShape::Object {
                index,
                original,
                current,
                operation,
                start,
                cursor,
                ..
            } => match operation {
                ObjectOperation::Move => {
                    self.document
                        .translate(*index, cursor.x() - start.x(), cursor.y() - start.y())
                }
                ObjectOperation::Resize(_) => self.document.resize(*index, *original, *current),
            },
            GestureShape::Segment { start, current } if gesture.tool == Tool::Callout => {
                self.text_draft = Some(TextDraft::new(
                    *current,
                    Some(*start),
                    self.color,
                    self.text_size(),
                ));
                false
            }
            _ => {
                let annotation = Self::annotation_for_gesture(&gesture)?;
                self.document.add(annotation);
                true
            }
        };
        self.manipulation_base = None;
        if changed {
            self.refresh_committed_preview()?;
        }
        Ok(changed || self.text_draft.is_some())
    }

    fn update_gesture(&mut self, point: Point) -> bool {
        let Some(gesture) = &mut self.gesture else {
            return false;
        };
        match &mut gesture.shape {
            GestureShape::Object {
                original,
                current,
                operation,
                start,
                cursor,
                preview,
                ..
            } => {
                if *cursor == point {
                    return false;
                }
                let previous_cursor = *cursor;
                let previous_bounds = *current;
                *cursor = point;
                *current = match operation {
                    ObjectOperation::Move => {
                        let dx = point.x() - start.x();
                        let dy = point.y() - start.y();
                        Bounds::new(
                            original.left() + dx,
                            original.top() + dy,
                            original.right() + dx,
                            original.bottom() + dy,
                        )
                        .expect("translated object bounds are finite")
                    }
                    ObjectOperation::Resize(handle) => handle.resize_bounds(*original, point),
                };
                match operation {
                    ObjectOperation::Move => preview.translate(
                        point.x() - previous_cursor.x(),
                        point.y() - previous_cursor.y(),
                    ),
                    ObjectOperation::Resize(_) => preview.resize(previous_bounds, *current),
                }
                true
            }
            _ => gesture.update(point),
        }
    }

    fn begin_object_transform(&mut self, point: Point) -> Result<bool> {
        let handle = self.selected.and_then(|index| {
            self.document
                .annotation(index)
                .and_then(|annotation| ResizeHandle::at(annotation.bounds(), point))
                .map(|handle| (index, handle))
        });
        let (index, operation) = if let Some((index, handle)) = handle {
            (index, ObjectOperation::Resize(handle))
        } else if let Some(index) = self.document.hit_test(point, HANDLE_HIT_RADIUS) {
            self.selected = Some(index);
            (index, ObjectOperation::Move)
        } else {
            self.selected = None;
            return Ok(true);
        };
        let original = self
            .document
            .annotation(index)
            .expect("hit object exists")
            .bounds();
        let preview = self
            .document
            .annotation(index)
            .expect("hit object exists")
            .clone();
        // Drop the full committed cache before producing the exclusion cache,
        // keeping object drags at two region-sized buffers instead of three.
        self.committed_preview = None;
        self.manipulation_base = match self.document.flatten_excluding(&self.frame, index) {
            Ok(frame) => Some(frame),
            Err(error) => {
                self.refresh_committed_preview()?;
                return Err(error).context("could not prepare object drag preview");
            }
        };
        self.gesture = Some(Gesture {
            tool: Tool::Select,
            style: StrokeStyle::default(),
            shape: GestureShape::Object {
                index,
                original,
                current: original,
                operation,
                start: point,
                cursor: point,
                preview,
            },
        });
        Ok(true)
    }

    fn cancel_active_interaction(&mut self) -> Result<bool> {
        let changed = self.gesture.take().is_some() || self.crop_gesture.take().is_some();
        if !changed {
            return Ok(false);
        }
        let had_manipulation_cache = self.manipulation_base.take().is_some();
        if had_manipulation_cache && !self.document.is_empty() {
            self.refresh_committed_preview()?;
        }
        Ok(true)
    }

    fn object_handle_at(&self, point: PixelPoint) -> Option<ResizeHandle> {
        let selected = self.selected?;
        let annotation = self.document.annotation(selected)?;
        ResizeHandle::at(
            annotation.bounds(),
            self.selection.to_annotation_point(point),
        )
    }

    fn erase_at(&mut self, point: Point) -> Result<bool> {
        let Some(index) = self.document.hit_test(point, HANDLE_HIT_RADIUS) else {
            return Ok(false);
        };
        let changed = self.document.delete(index);
        self.selected = None;
        if changed {
            self.refresh_committed_preview()?;
        }
        Ok(changed)
    }

    fn delete_selected(&mut self) -> Result<bool> {
        let Some(index) = self.selected.take() else {
            return Ok(false);
        };
        let changed = self.document.delete(index);
        if changed {
            self.refresh_committed_preview()?;
        }
        Ok(changed)
    }

    fn restyle_selected(&mut self, index: usize) -> Result<bool> {
        let style = match self.document.annotation(index) {
            Some(Annotation::Highlighter(_)) => StrokeStyle::new(
                Color::rgba(self.color.red(), self.color.green(), self.color.blue(), 96),
                STROKE_WIDTHS[self.width_index] * 3.5,
            )?,
            Some(Annotation::Text(_)) => StrokeStyle::new(self.color, self.text_size())?,
            _ => StrokeStyle::new(self.color, STROKE_WIDTHS[self.width_index])?,
        };
        let changed = self.document.restyle(index, style);
        if changed {
            self.refresh_committed_preview()?;
        }
        Ok(changed)
    }

    fn commit_text_draft(&mut self) -> Result<bool> {
        let Some(draft) = self.text_draft.take() else {
            return Ok(false);
        };
        if draft.text.trim().is_empty() {
            return Ok(false);
        }
        self.document.add(Annotation::text(
            draft.position,
            draft.text,
            draft.color,
            draft.size,
            draft.callout_anchor,
        )?);
        self.refresh_committed_preview()?;
        Ok(true)
    }

    fn begin_crop(&mut self, point: PixelPoint) -> bool {
        let Some(handle) = selection_resize_handle(self.selection, point)
            .or_else(|| self.selection.contains(point).then_some(ResizeHandle::Move))
        else {
            return false;
        };
        self.crop_gesture = Some(CropGesture {
            original: self.selection,
            preview: self.selection,
            start: point,
            handle,
        });
        true
    }

    fn update_crop(&mut self, point: PixelPoint, source_width: u32, source_height: u32) -> bool {
        let Some(gesture) = &mut self.crop_gesture else {
            return false;
        };
        let preview = gesture.handle.resize_pixel_rect(
            gesture.original,
            gesture.start,
            point,
            source_width,
            source_height,
        );
        let changed = preview != gesture.preview;
        gesture.preview = preview;
        changed
    }

    fn commit_crop(&mut self, source: &Frame) -> Result<bool> {
        let Some(gesture) = self.crop_gesture.take() else {
            return Ok(false);
        };
        if gesture.preview == self.selection {
            return Ok(false);
        }
        let origin = source.origin();
        let rect = PhysicalRect::new(
            PhysicalPoint::new(
                origin.x().saturating_add(gesture.preview.x),
                origin.y().saturating_add(gesture.preview.y),
            ),
            gesture.preview.width,
            gesture.preview.height,
        )?;
        let frame = source.crop(rect)?;
        let dx = (self.selection.x - gesture.preview.x) as f32;
        let dy = (self.selection.y - gesture.preview.y) as f32;
        self.document.rebase(dx, dy);
        self.selection = gesture.preview;
        self.frame = frame;
        self.refresh_committed_preview()?;
        Ok(true)
    }

    fn undo(&mut self) -> Result<bool> {
        let changed = self.document.undo();
        if changed {
            self.selected = None;
            self.refresh_committed_preview()?;
        }
        Ok(changed)
    }

    fn redo(&mut self) -> Result<bool> {
        let changed = self.document.redo();
        if changed {
            self.selected = None;
            self.refresh_committed_preview()?;
        }
        Ok(changed)
    }

    fn clear(&mut self) -> Result<bool> {
        let changed = self.document.clear();
        if changed {
            self.selected = None;
            self.refresh_committed_preview()?;
        }
        Ok(changed)
    }

    fn export_frame(&mut self) -> Result<Frame> {
        self.commit_text_draft()?;
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
    Select,
    Pen,
    Highlighter,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Callout,
    Redact,
    Pixelate,
    Eraser,
    Crop,
    Eyedropper,
}

impl Tool {
    const ALL: [Self; 14] = [
        Self::Select,
        Self::Pen,
        Self::Highlighter,
        Self::Line,
        Self::Arrow,
        Self::Rectangle,
        Self::Ellipse,
        Self::Text,
        Self::Callout,
        Self::Redact,
        Self::Pixelate,
        Self::Eraser,
        Self::Crop,
        Self::Eyedropper,
    ];

    const fn shortcut(self) -> &'static str {
        match self {
            Self::Select => "v",
            Self::Pen => "p",
            Self::Highlighter => "h",
            Self::Line => "l",
            Self::Arrow => "a",
            Self::Rectangle => "r",
            Self::Ellipse => "e",
            Self::Text => "t",
            Self::Callout => "q",
            Self::Redact => "b",
            Self::Pixelate => "m",
            Self::Eraser => "d",
            Self::Crop => "g",
            Self::Eyedropper => "i",
        }
    }

    const fn from_preference(tool: EditorTool) -> Self {
        match tool {
            EditorTool::Select => Self::Select,
            EditorTool::Pen => Self::Pen,
            EditorTool::Highlighter => Self::Highlighter,
            EditorTool::Line => Self::Line,
            EditorTool::Arrow => Self::Arrow,
            EditorTool::Rectangle => Self::Rectangle,
            EditorTool::Ellipse => Self::Ellipse,
            EditorTool::Text => Self::Text,
            EditorTool::Callout => Self::Callout,
            EditorTool::Redact => Self::Redact,
            EditorTool::Pixelate => Self::Pixelate,
            EditorTool::Eraser => Self::Eraser,
            EditorTool::Crop => Self::Crop,
            EditorTool::Eyedropper => Self::Eyedropper,
        }
    }

    const fn preference(self) -> EditorTool {
        match self {
            Self::Select => EditorTool::Select,
            Self::Pen => EditorTool::Pen,
            Self::Highlighter => EditorTool::Highlighter,
            Self::Line => EditorTool::Line,
            Self::Arrow => EditorTool::Arrow,
            Self::Rectangle => EditorTool::Rectangle,
            Self::Ellipse => EditorTool::Ellipse,
            Self::Text => EditorTool::Text,
            Self::Callout => EditorTool::Callout,
            Self::Redact => EditorTool::Redact,
            Self::Pixelate => EditorTool::Pixelate,
            Self::Eraser => EditorTool::Eraser,
            Self::Crop => EditorTool::Crop,
            Self::Eyedropper => EditorTool::Eyedropper,
        }
    }
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
            GestureShape::Object { .. } => false,
        }
    }
}

enum GestureShape {
    Path(Vec<Point>),
    Segment {
        start: Point,
        current: Point,
    },
    Object {
        index: usize,
        original: Bounds,
        current: Bounds,
        operation: ObjectOperation,
        start: Point,
        cursor: Point,
        preview: Annotation,
    },
}

#[derive(Clone, Copy, Debug)]
enum ObjectOperation {
    Move,
    Resize(ResizeHandle),
}

#[derive(Clone, Debug)]
struct TextDraft {
    position: Point,
    callout_anchor: Option<Point>,
    text: String,
    color: Color,
    size: f32,
}

impl TextDraft {
    fn new(position: Point, callout_anchor: Option<Point>, color: Color, size: f32) -> Self {
        Self {
            position,
            callout_anchor,
            text: String::new(),
            color,
            size,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CropGesture {
    original: PixelRect,
    preview: PixelRect,
    start: PixelPoint,
    handle: ResizeHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResizeHandle {
    Move,
    NorthWest,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
}

impl ResizeHandle {
    fn at(bounds: Bounds, point: Point) -> Option<Self> {
        let centers = [
            (Self::NorthWest, Point::new(bounds.left(), bounds.top())),
            (Self::North, Point::new(bounds.center().x(), bounds.top())),
            (Self::NorthEast, Point::new(bounds.right(), bounds.top())),
            (Self::East, Point::new(bounds.right(), bounds.center().y())),
            (Self::SouthEast, Point::new(bounds.right(), bounds.bottom())),
            (
                Self::South,
                Point::new(bounds.center().x(), bounds.bottom()),
            ),
            (Self::SouthWest, Point::new(bounds.left(), bounds.bottom())),
            (Self::West, Point::new(bounds.left(), bounds.center().y())),
        ];
        centers.into_iter().find_map(|(handle, center)| {
            ((center.x() - point.x()).hypot(center.y() - point.y()) <= HANDLE_HIT_RADIUS)
                .then_some(handle)
        })
    }

    fn resize_bounds(self, original: Bounds, point: Point) -> Bounds {
        let mut left = original.left();
        let mut top = original.top();
        let mut right = original.right();
        let mut bottom = original.bottom();
        match self {
            Self::NorthWest => {
                left = point.x();
                top = point.y();
            }
            Self::North => top = point.y(),
            Self::NorthEast => {
                right = point.x();
                top = point.y();
            }
            Self::East => right = point.x(),
            Self::SouthEast => {
                right = point.x();
                bottom = point.y();
            }
            Self::South => bottom = point.y(),
            Self::SouthWest => {
                left = point.x();
                bottom = point.y();
            }
            Self::West => left = point.x(),
            Self::Move => {}
        }
        if (right - left).abs() < MIN_OBJECT_SIZE {
            if matches!(self, Self::West | Self::NorthWest | Self::SouthWest) {
                left = right - MIN_OBJECT_SIZE;
            } else {
                right = left + MIN_OBJECT_SIZE;
            }
        }
        if (bottom - top).abs() < MIN_OBJECT_SIZE {
            if matches!(self, Self::North | Self::NorthWest | Self::NorthEast) {
                top = bottom - MIN_OBJECT_SIZE;
            } else {
                bottom = top + MIN_OBJECT_SIZE;
            }
        }
        Bounds::new(left, top, right, bottom).expect("resized bounds remain finite")
    }

    fn resize_pixel_rect(
        self,
        original: PixelRect,
        start: PixelPoint,
        point: PixelPoint,
        source_width: u32,
        source_height: u32,
    ) -> PixelRect {
        let max_x = i32::try_from(source_width).unwrap_or(i32::MAX);
        let max_y = i32::try_from(source_height).unwrap_or(i32::MAX);
        if self == Self::Move {
            let dx = point.x - start.x;
            let dy = point.y - start.y;
            let width = i32::try_from(original.width).unwrap_or(i32::MAX);
            let height = i32::try_from(original.height).unwrap_or(i32::MAX);
            return PixelRect {
                x: original
                    .x
                    .saturating_add(dx)
                    .clamp(0, max_x.saturating_sub(width)),
                y: original
                    .y
                    .saturating_add(dy)
                    .clamp(0, max_y.saturating_sub(height)),
                ..original
            };
        }
        let bounds = self.resize_bounds(
            pixel_rect_bounds(original),
            Point::new(
                point.x.clamp(0, max_x) as f32,
                point.y.clamp(0, max_y) as f32,
            ),
        );
        let left = bounds.left().round().clamp(0.0, max_x as f32) as i32;
        let top = bounds.top().round().clamp(0.0, max_y as f32) as i32;
        let right = bounds.right().round().clamp(0.0, max_x as f32) as i32;
        let bottom = bounds.bottom().round().clamp(0.0, max_y as f32) as i32;
        let minimum = i32::try_from(MIN_SELECTION_SIZE).unwrap_or(3);
        let mut x = left.min(right).clamp(0, max_x.saturating_sub(minimum));
        let mut y = top.min(bottom).clamp(0, max_y.saturating_sub(minimum));
        let mut rect_right = left.max(right).clamp(x + minimum, max_x);
        let mut rect_bottom = top.max(bottom).clamp(y + minimum, max_y);
        if rect_right - x < minimum {
            x = rect_right.saturating_sub(minimum).max(0);
            rect_right = (x + minimum).min(max_x);
        }
        if rect_bottom - y < minimum {
            y = rect_bottom.saturating_sub(minimum).max(0);
            rect_bottom = (y + minimum).min(max_y);
        }
        PixelRect {
            x,
            y,
            width: u32::try_from(rect_right - x).unwrap_or(MIN_SELECTION_SIZE),
            height: u32::try_from(rect_bottom - y).unwrap_or(MIN_SELECTION_SIZE),
        }
    }
}

#[derive(Clone, Copy)]
enum ExportKind {
    Save,
    Copy,
    Print,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolbarButton {
    Select,
    Pen,
    Highlighter,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Callout,
    Redact,
    Pixelate,
    Eraser,
    Crop,
    Eyedropper,
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
        Self::Select,
        Self::Pen,
        Self::Highlighter,
        Self::Line,
        Self::Arrow,
        Self::Rectangle,
        Self::Ellipse,
        Self::Text,
        Self::Callout,
        Self::Redact,
        Self::Pixelate,
        Self::Eraser,
        Self::Crop,
        Self::Eyedropper,
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
            Self::Select => Some(Tool::Select),
            Self::Pen => Some(Tool::Pen),
            Self::Highlighter => Some(Tool::Highlighter),
            Self::Line => Some(Tool::Line),
            Self::Arrow => Some(Tool::Arrow),
            Self::Rectangle => Some(Tool::Rectangle),
            Self::Ellipse => Some(Tool::Ellipse),
            Self::Text => Some(Tool::Text),
            Self::Callout => Some(Tool::Callout),
            Self::Redact => Some(Tool::Redact),
            Self::Pixelate => Some(Tool::Pixelate),
            Self::Eraser => Some(Tool::Eraser),
            Self::Crop => Some(Tool::Crop),
            Self::Eyedropper => Some(Tool::Eyedropper),
            _ => None,
        }
    }

    const fn tooltip(self) -> &'static str {
        match self {
            Self::Select => "Select/move/resize (V)",
            Self::Pen => "Pen (P)",
            Self::Highlighter => "Highlighter (H)",
            Self::Line => "Line (L)",
            Self::Arrow => "Arrow (A)",
            Self::Rectangle => "Rectangle (R)",
            Self::Ellipse => "Ellipse (E)",
            Self::Text => "Text box (T)",
            Self::Callout => "Callout (Q)",
            Self::Redact => "Secure redact (B)",
            Self::Pixelate => "Pixelate (M)",
            Self::Eraser => "Delete object (D)",
            Self::Crop => "Crop/resize selection (G)",
            Self::Eyedropper => "Eyedropper (I)",
            Self::Undo => "Undo (Ctrl+Z)",
            Self::Redo => "Redo (Ctrl+Y)",
            Self::Color => "Color: click cycles, right-click custom (K)",
            Self::Width => "Stroke width / text size",
            Self::Save => "Save As (S)",
            Self::Copy => "Copy (C or Enter)",
            Self::Print => "Print (O)",
        }
    }
}

#[derive(Clone, Copy)]
struct ToolbarLayout {
    x: i32,
    y: i32,
    button_size: i32,
    columns: i32,
    rows: i32,
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
        let columns = i32::try_from(TOOLBAR_COLUMNS.min(TOOLBAR_BUTTONS)).unwrap_or(1);
        let rows = i32::try_from(TOOLBAR_BUTTONS.div_ceil(TOOLBAR_COLUMNS)).unwrap_or(1);
        let available = (screen_width - padding * 2).max(1);
        let maximum_fitting_button = (available / columns).max(1);
        let button_size = if maximum_fitting_button >= minimum_button_size {
            default_button_size.min(maximum_fitting_button)
        } else {
            maximum_fitting_button
        };
        let toolbar_width = button_size * columns;
        let toolbar_height = button_size * rows;
        let maximum_x = (screen_width - toolbar_width - padding).max(0);
        let minimum_x = padding.min(maximum_x);
        let x = selection.x.clamp(minimum_x, maximum_x);

        let below = selection.bottom() + gap;
        let above = selection.y - gap - toolbar_height;
        let y = if below + toolbar_height + padding <= screen_height {
            below
        } else if above >= padding {
            above
        } else {
            (screen_height - toolbar_height - padding).max(0)
        };
        Self {
            x,
            y,
            button_size,
            columns,
            rows,
        }
    }

    fn button_at(self, point: PixelPoint) -> Option<ToolbarButton> {
        let width = self.button_size * self.columns;
        let height = self.button_size * self.rows;
        if point.x < self.x
            || point.y < self.y
            || point.x >= self.x + width
            || point.y >= self.y + height
        {
            return None;
        }
        let column = (point.x - self.x) / self.button_size;
        let row = (point.y - self.y) / self.button_size;
        let index = usize::try_from(row * self.columns + column).ok()?;
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

fn pixel_rect_bounds(rect: PixelRect) -> Bounds {
    Bounds::new(
        rect.x as f32,
        rect.y as f32,
        rect.right() as f32,
        rect.bottom() as f32,
    )
    .expect("pixel rectangle bounds are finite")
}

fn selection_resize_handle(selection: PixelRect, point: PixelPoint) -> Option<ResizeHandle> {
    ResizeHandle::at(
        pixel_rect_bounds(selection),
        Point::new(point.x as f32, point.y as f32),
    )
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
    hovered: Option<ToolbarButton>,
) {
    let toolbar_width = layout.button_size * layout.columns;
    let toolbar_height = layout.button_size * layout.rows;
    fill_rect(
        buffer,
        width,
        height,
        layout.x - TOOLBAR_PADDING,
        layout.y - TOOLBAR_PADDING,
        toolbar_width + TOOLBAR_PADDING * 2,
        toolbar_height + TOOLBAR_PADDING * 2,
        0x00_171a22,
    );

    for (index, button) in ToolbarButton::ALL.iter().copied().enumerate() {
        let index = i32::try_from(index).unwrap_or_default();
        let x = layout.x + (index % layout.columns) * layout.button_size;
        let y = layout.y + (index / layout.columns) * layout.button_size;
        let selected = button.tool().is_some_and(|tool| tool == editor.tool);
        let background = if selected {
            0x00_2d74da
        } else if hovered == Some(button) {
            0x00_3a4050
        } else {
            0x00_242833
        };
        fill_rect(
            buffer,
            width,
            height,
            x + 1,
            y + 1,
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
            y,
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
    let mut icon = ToolbarIconPainter::new(buffer, width, height, x, y, size);

    match button {
        ToolbarButton::Select => {
            icon.line(3, 12, 21, 12);
            icon.line(12, 3, 12, 21);
            for segment in [
                (3, 12, 7, 8),
                (3, 12, 7, 16),
                (21, 12, 17, 8),
                (21, 12, 17, 16),
                (12, 3, 8, 7),
                (12, 3, 16, 7),
                (12, 21, 8, 17),
                (12, 21, 16, 17),
            ] {
                icon.line(segment.0, segment.1, segment.2, segment.3);
            }
        }
        ToolbarButton::Pen => {
            icon.polyline(&[(4, 20), (6, 15), (16, 5), (20, 9), (10, 19), (4, 20)]);
            icon.line(14, 7, 18, 11);
            icon.line(6, 15, 10, 19);
        }
        ToolbarButton::Highlighter => {
            icon.polyline(&[(5, 16), (15, 6), (20, 11), (10, 21), (5, 16)]);
            icon.line(13, 8, 18, 13);
            icon.line_width(4, 21, 11, 21, 3);
        }
        ToolbarButton::Line => {
            icon.line(5, 19, 19, 5);
            icon.dot(5, 19, 1, 0x00ffffff);
            icon.dot(19, 5, 1, 0x00ffffff);
        }
        ToolbarButton::Arrow => {
            icon.line(5, 19, 19, 5);
            icon.polyline(&[(12, 5), (19, 5), (19, 12)]);
        }
        ToolbarButton::Rectangle => {
            icon.rect(4, 6, 20, 18);
        }
        ToolbarButton::Ellipse => {
            icon.ellipse(4, 5, 20, 19);
        }
        ToolbarButton::Text => {
            icon.line_width(5, 5, 19, 5, 3);
            icon.line_width(12, 5, 12, 20, 3);
            icon.line(8, 20, 16, 20);
        }
        ToolbarButton::Callout => {
            icon.polyline(&[
                (4, 5),
                (20, 5),
                (20, 16),
                (12, 16),
                (7, 20),
                (8, 16),
                (4, 16),
                (4, 5),
            ]);
            for dot_x in [8, 12, 16] {
                icon.dot(dot_x, 11, 1, 0x00ffffff);
            }
        }
        ToolbarButton::Redact => {
            icon.polyline(&[
                (3, 12),
                (6, 8),
                (10, 6),
                (14, 6),
                (18, 8),
                (21, 12),
                (18, 16),
                (14, 18),
                (10, 18),
                (6, 16),
                (3, 12),
            ]);
            icon.ellipse(9, 9, 15, 15);
            icon.line_width(4, 4, 20, 20, 3);
        }
        ToolbarButton::Pixelate => {
            for (left, top, right, bottom) in [
                (4, 4, 9, 9),
                (11, 4, 15, 8),
                (17, 5, 21, 10),
                (5, 11, 9, 15),
                (11, 10, 17, 16),
                (18, 12, 21, 16),
                (4, 18, 9, 21),
                (11, 18, 15, 21),
                (17, 18, 21, 21),
            ] {
                icon.fill_box(left, top, right, bottom, 0x00ffffff);
            }
        }
        ToolbarButton::Eraser => {
            icon.polyline(&[
                (5, 15),
                (14, 6),
                (20, 12),
                (12, 20),
                (8, 20),
                (5, 17),
                (5, 15),
            ]);
            icon.line(10, 10, 16, 16);
            icon.line(8, 20, 21, 20);
        }
        ToolbarButton::Crop => {
            icon.polyline(&[(7, 4), (7, 17), (20, 17)]);
            icon.polyline(&[(4, 7), (17, 7), (17, 20)]);
        }
        ToolbarButton::Eyedropper => {
            icon.polyline(&[(14, 4), (19, 10), (16, 13), (11, 8), (14, 4)]);
            icon.polyline(&[(12, 9), (5, 16), (5, 20), (9, 20), (16, 13)]);
            icon.line(16, 5, 20, 9);
        }
        ToolbarButton::Undo | ToolbarButton::Redo => {
            let undo = button == ToolbarButton::Undo;
            let mirror = |value: i32| if undo { value } else { 24 - value };
            icon.polyline(&[(mirror(10), 5), (mirror(5), 10), (mirror(10), 15)]);
            icon.polyline(&[
                (mirror(5), 10),
                (mirror(12), 10),
                (mirror(16), 12),
                (mirror(19), 16),
                (mirror(19), 20),
            ]);
        }
        ToolbarButton::Color => {
            let color = editor.color;
            let value = (u32::from(color.red()) << 16)
                | (u32::from(color.green()) << 8)
                | u32::from(color.blue());
            icon.rect(4, 4, 16, 16);
            icon.fill_box(9, 9, 21, 21, value);
            icon.rect(9, 9, 21, 21);
        }
        ToolbarButton::Width => {
            icon.line_width(5, 6, 19, 6, 1);
            icon.line_width(5, 12, 19, 12, 2);
            let active_width = i32::try_from(editor.width_index).unwrap_or(1) + 3;
            icon.line_width(5, 19, 19, 19, active_width);
        }
        ToolbarButton::Save => {
            icon.polyline(&[(4, 3), (17, 3), (21, 7), (21, 21), (4, 21), (4, 3)]);
            icon.rect(7, 3, 17, 9);
            icon.rect(7, 14, 18, 21);
            icon.dot(15, 6, 1, 0x00ffffff);
        }
        ToolbarButton::Copy => {
            icon.rect(5, 4, 16, 17);
            icon.rect(8, 7, 20, 21);
        }
        ToolbarButton::Print => {
            icon.rect(7, 3, 17, 9);
            icon.rect(4, 9, 20, 17);
            icon.rect(7, 15, 17, 21);
            icon.dot(17, 12, 1, 0x00ffffff);
        }
    }
}

struct ToolbarIconPainter<'a> {
    buffer: &'a mut [u32],
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    extent: i32,
    stroke: i32,
}

impl<'a> ToolbarIconPainter<'a> {
    fn new(buffer: &'a mut [u32], width: u32, height: u32, x: i32, y: i32, size: i32) -> Self {
        let maximum_extent = size.saturating_sub(4).max(4);
        let extent = (size * 2 / 3).clamp(4, maximum_extent);
        Self {
            buffer,
            width,
            height,
            origin_x: x + (size - extent) / 2,
            origin_y: y + (size - extent) / 2,
            extent,
            stroke: (size / 19).clamp(1, 4),
        }
    }

    fn coordinate(origin: i32, extent: i32, value: i32) -> i32 {
        origin + (value.clamp(0, 24) * extent + 12) / 24
    }

    fn point(&self, x: i32, y: i32) -> (i32, i32) {
        (
            Self::coordinate(self.origin_x, self.extent, x),
            Self::coordinate(self.origin_y, self.extent, y),
        )
    }

    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let (x0, y0) = self.point(x0, y0);
        let (x1, y1) = self.point(x1, y1);
        draw_line(
            self.buffer,
            self.width,
            self.height,
            x0,
            y0,
            x1,
            y1,
            0x00ffffff,
            self.stroke,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn line_width(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, design_width: i32) {
        let (x0, y0) = self.point(x0, y0);
        let (x1, y1) = self.point(x1, y1);
        let thickness = ((design_width * self.extent + 12) / 24).max(1);
        draw_line(
            self.buffer,
            self.width,
            self.height,
            x0,
            y0,
            x1,
            y1,
            0x00ffffff,
            thickness,
        );
    }

    fn polyline(&mut self, points: &[(i32, i32)]) {
        for pair in points.windows(2) {
            self.line(pair[0].0, pair[0].1, pair[1].0, pair[1].1);
        }
    }

    fn rect(&mut self, left: i32, top: i32, right: i32, bottom: i32) {
        let (left, top) = self.point(left, top);
        let (right, bottom) = self.point(right, bottom);
        draw_rect(
            self.buffer,
            self.width,
            self.height,
            left,
            top,
            right - left,
            bottom - top,
            0x00ffffff,
            self.stroke,
        );
    }

    fn ellipse(&mut self, left: i32, top: i32, right: i32, bottom: i32) {
        let (left, top) = self.point(left, top);
        let (right, bottom) = self.point(right, bottom);
        draw_ellipse_preview(
            self.buffer,
            self.width,
            self.height,
            left,
            top,
            right,
            bottom,
            Color::WHITE,
            self.stroke,
        );
    }

    fn fill_box(&mut self, left: i32, top: i32, right: i32, bottom: i32, color: u32) {
        let (left, top) = self.point(left, top);
        let (right, bottom) = self.point(right, bottom);
        fill_rect(
            self.buffer,
            self.width,
            self.height,
            left,
            top,
            (right - left).max(1),
            (bottom - top).max(1),
            color,
        );
    }

    fn dot(&mut self, x: i32, y: i32, radius: i32, color: u32) {
        let (center_x, center_y) = self.point(x, y);
        let radius = ((radius * self.extent + 12) / 24).max(1);
        for offset_y in -radius..=radius {
            for offset_x in -radius..=radius {
                if offset_x * offset_x + offset_y * offset_y <= radius * radius {
                    fill_rect(
                        self.buffer,
                        self.width,
                        self.height,
                        center_x + offset_x,
                        center_y + offset_y,
                        1,
                        1,
                        color,
                    );
                }
            }
        }
    }
}

fn draw_shortcut_hints(buffer: &mut [u32], width: u32, height: u32) {
    let text = "V Select  P Pen  T Text  B Redact  G Crop  K Color  Enter Copy  S Save  Esc Cancel";
    let size = 13.0;
    let (text_width, text_height) = measure_text(text, size);
    let x = 10;
    let y = i32::try_from(height.saturating_sub(text_height + 16)).unwrap_or_default();
    fill_rect(
        buffer,
        width,
        height,
        x - 5,
        y - 5,
        i32::try_from(text_width + 10).unwrap_or(i32::MAX),
        i32::try_from(text_height + 10).unwrap_or(i32::MAX),
        0x00_171a22,
    );
    draw_text_bgrx(
        buffer,
        width,
        height,
        Point::new(x as f32, y as f32),
        text,
        Color::WHITE,
        size,
    );
}

fn draw_tooltip(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    layout: ToolbarLayout,
    button: ToolbarButton,
    text: &str,
) {
    let size = 13.0;
    let (text_width, text_height) = measure_text(text, size);
    let box_width = i32::try_from(text_width + 14).unwrap_or(i32::MAX);
    let box_height = i32::try_from(text_height + 10).unwrap_or(i32::MAX);
    let index = ToolbarButton::ALL
        .iter()
        .position(|candidate| *candidate == button)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or_default();
    let button_x = layout.x + (index % layout.columns) * layout.button_size;
    let mut x = button_x + (layout.button_size - box_width) / 2;
    let mut y = layout.y - box_height - 6;
    if y < 0 {
        y = layout.y + layout.button_size * layout.rows + 6;
    }
    x = x.clamp(
        0,
        i32::try_from(width)
            .unwrap_or(i32::MAX)
            .saturating_sub(box_width),
    );
    fill_rect(
        buffer,
        width,
        height,
        x,
        y,
        box_width,
        box_height,
        0x00_111319,
    );
    draw_rect(
        buffer,
        width,
        height,
        x,
        y,
        box_width,
        box_height,
        0x00_5d6578,
        1,
    );
    draw_text_bgrx(
        buffer,
        width,
        height,
        Point::new((x + 7) as f32, (y + 5) as f32),
        text,
        Color::WHITE,
        size,
    );
}

fn draw_text_draft(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    selection: PixelRect,
    draft: &TextDraft,
) {
    let content = if draft.text.is_empty() {
        "Type text..."
    } else {
        &draft.text
    };
    let (text_width, text_height) = measure_text(content, draft.size);
    let (x, y) = annotation_pixel(selection, draft.position);
    fill_rect(
        buffer,
        width,
        height,
        x - 5,
        y - 5,
        i32::try_from(text_width + 10).unwrap_or(i32::MAX),
        i32::try_from(text_height + 10).unwrap_or(i32::MAX),
        0x00_171a22,
    );
    if let Some(anchor) = draft.callout_anchor {
        let (anchor_x, anchor_y) = annotation_pixel(selection, anchor);
        draw_preview_line(
            buffer,
            width,
            height,
            anchor_x,
            anchor_y,
            x,
            y,
            draft.color,
            3,
        );
    }
    draw_text_bgrx(
        buffer,
        width,
        height,
        Point::new(x as f32, y as f32),
        content,
        draft.color,
        draft.size,
    );
}

fn draw_resize_handles(buffer: &mut [u32], width: u32, height: u32, bounds: Bounds) {
    let center = bounds.center();
    for point in [
        Point::new(bounds.left(), bounds.top()),
        Point::new(center.x(), bounds.top()),
        Point::new(bounds.right(), bounds.top()),
        Point::new(bounds.right(), center.y()),
        Point::new(bounds.right(), bounds.bottom()),
        Point::new(center.x(), bounds.bottom()),
        Point::new(bounds.left(), bounds.bottom()),
        Point::new(bounds.left(), center.y()),
    ] {
        let x = point.x().round() as i32;
        let y = point.y().round() as i32;
        fill_rect(
            buffer,
            width,
            height,
            x - HANDLE_RADIUS,
            y - HANDLE_RADIUS,
            HANDLE_RADIUS * 2 + 1,
            HANDLE_RADIUS * 2 + 1,
            0x00_ffffff,
        );
        draw_rect(
            buffer,
            width,
            height,
            x - HANDLE_RADIUS,
            y - HANDLE_RADIUS,
            HANDLE_RADIUS * 2,
            HANDLE_RADIUS * 2,
            0x00_2d74da,
            1,
        );
    }
}

fn draw_object_selection(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    selection: PixelRect,
    local: Bounds,
) {
    let bounds = Bounds::new(
        local.left() + selection.x as f32,
        local.top() + selection.y as f32,
        local.right() + selection.x as f32,
        local.bottom() + selection.y as f32,
    )
    .expect("screen object bounds are finite");
    draw_rect(
        buffer,
        width,
        height,
        bounds.left().round() as i32,
        bounds.top().round() as i32,
        bounds.width().round() as i32,
        bounds.height().round() as i32,
        0x00_61a0ff,
        1,
    );
    draw_resize_handles(buffer, width, height, bounds);
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
                Tool::Ellipse => {
                    draw_ellipse_preview(buffer, width, height, x0, y0, x1, y1, color, thickness)
                }
                Tool::Redact => fill_rect(
                    buffer,
                    width,
                    height,
                    x0.min(x1),
                    y0.min(y1),
                    (x1 - x0).abs(),
                    (y1 - y0).abs(),
                    0,
                ),
                Tool::Pixelate => pixelate_buffer_region(
                    buffer,
                    width,
                    height,
                    x0.min(x1),
                    y0.min(y1),
                    x0.max(x1),
                    y0.max(y1),
                    12,
                ),
                Tool::Callout => {
                    draw_preview_line(buffer, width, height, x0, y0, x1, y1, color, 3);
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
                Tool::Select
                | Tool::Pen
                | Tool::Highlighter
                | Tool::Text
                | Tool::Eraser
                | Tool::Crop
                | Tool::Eyedropper => {}
            }
        }
        GestureShape::Object { preview, .. } => {
            draw_annotation_preview(buffer, width, height, selection, preview);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_ellipse_preview(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: Color,
    thickness: i32,
) {
    let center_x = (x0 + x1) as f32 * 0.5;
    let center_y = (y0 + y1) as f32 * 0.5;
    let radius_x = (x1 - x0).abs() as f32 * 0.5;
    let radius_y = (y1 - y0).abs() as f32 * 0.5;
    let mut previous = (
        (center_x + radius_x).round() as i32,
        center_y.round() as i32,
    );
    for step in 1..=48 {
        let angle = step as f32 / 48.0 * std::f32::consts::TAU;
        let current = (
            (center_x + radius_x * angle.cos()).round() as i32,
            (center_y + radius_y * angle.sin()).round() as i32,
        );
        draw_preview_line(
            buffer, width, height, previous.0, previous.1, current.0, current.1, color, thickness,
        );
        previous = current;
    }
}

fn draw_annotation_preview(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    selection: PixelRect,
    annotation: &Annotation,
) {
    match annotation {
        Annotation::Pen(stroke) | Annotation::Highlighter(stroke) => {
            let style = stroke.style();
            let thickness = style.width().round().max(1.0) as i32;
            for segment in stroke.points().windows(2) {
                let (x0, y0) = annotation_pixel(selection, segment[0]);
                let (x1, y1) = annotation_pixel(selection, segment[1]);
                draw_preview_line(
                    buffer,
                    width,
                    height,
                    x0,
                    y0,
                    x1,
                    y1,
                    style.color(),
                    thickness,
                );
            }
        }
        Annotation::Line(segment) | Annotation::Arrow(segment) => {
            let (x0, y0) = annotation_pixel(selection, segment.start());
            let (x1, y1) = annotation_pixel(selection, segment.end());
            let style = segment.style();
            let thickness = style.width().round().max(1.0) as i32;
            draw_preview_line(
                buffer,
                width,
                height,
                x0,
                y0,
                x1,
                y1,
                style.color(),
                thickness,
            );
            if matches!(annotation, Annotation::Arrow(_)) {
                draw_preview_arrow_head(
                    buffer,
                    width,
                    height,
                    selection,
                    segment.start(),
                    segment.end(),
                    style,
                );
            }
        }
        Annotation::Rectangle(rectangle) | Annotation::Ellipse(rectangle) => {
            let (x0, y0) = annotation_pixel(selection, rectangle.first_corner());
            let (x1, y1) = annotation_pixel(selection, rectangle.opposite_corner());
            let style = rectangle.style();
            let thickness = style.width().round().max(1.0) as i32;
            if matches!(annotation, Annotation::Ellipse(_)) {
                draw_ellipse_preview(
                    buffer,
                    width,
                    height,
                    x0,
                    y0,
                    x1,
                    y1,
                    style.color(),
                    thickness,
                );
            } else {
                draw_rect(
                    buffer,
                    width,
                    height,
                    x0.min(x1),
                    y0.min(y1),
                    (x1 - x0).abs(),
                    (y1 - y0).abs(),
                    (u32::from(style.color().red()) << 16)
                        | (u32::from(style.color().green()) << 8)
                        | u32::from(style.color().blue()),
                    thickness,
                );
            }
        }
        Annotation::Text(text) => {
            let (x, y) = annotation_pixel(selection, text.position());
            let (text_width, text_height) = measure_text(text.text(), text.size());
            fill_rect(
                buffer,
                width,
                height,
                x - 5,
                y - 5,
                i32::try_from(text_width + 10).unwrap_or(i32::MAX),
                i32::try_from(text_height + 10).unwrap_or(i32::MAX),
                0x00_171a22,
            );
            if let Some(anchor) = text.callout_anchor() {
                let (anchor_x, anchor_y) = annotation_pixel(selection, anchor);
                draw_preview_line(
                    buffer,
                    width,
                    height,
                    anchor_x,
                    anchor_y,
                    x,
                    y,
                    text.color(),
                    3,
                );
            }
            draw_text_bgrx(
                buffer,
                width,
                height,
                Point::new(x as f32, y as f32),
                text.text(),
                text.color(),
                text.size(),
            );
        }
        Annotation::Redact(effect) => {
            let (x0, y0) = annotation_pixel(selection, effect.first_corner());
            let (x1, y1) = annotation_pixel(selection, effect.opposite_corner());
            fill_rect(
                buffer,
                width,
                height,
                x0.min(x1),
                y0.min(y1),
                (x1 - x0).abs(),
                (y1 - y0).abs(),
                0,
            );
        }
        Annotation::Pixelate(effect) => {
            let (x0, y0) = annotation_pixel(selection, effect.first_corner());
            let (x1, y1) = annotation_pixel(selection, effect.opposite_corner());
            pixelate_buffer_region(
                buffer,
                width,
                height,
                x0.min(x1),
                y0.min(y1),
                x0.max(x1),
                y0.max(y1),
                i32::from(effect.block_size()),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pixelate_buffer_region(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    block: i32,
) {
    let left = left.max(0);
    let top = top.max(0);
    let right = right.min(i32::try_from(width).unwrap_or(i32::MAX));
    let bottom = bottom.min(i32::try_from(height).unwrap_or(i32::MAX));
    let block = block.max(2);
    let stride = usize::try_from(width).unwrap_or_default();
    for block_y in (top..bottom).step_by(block as usize) {
        for block_x in (left..right).step_by(block as usize) {
            let end_y = (block_y + block).min(bottom);
            let end_x = (block_x + block).min(right);
            let mut red = 0_u64;
            let mut green = 0_u64;
            let mut blue = 0_u64;
            let mut count = 0_u64;
            for y in block_y..end_y {
                for x in block_x..end_x {
                    let pixel = buffer[y as usize * stride + x as usize];
                    red += u64::from((pixel >> 16) & 0xff);
                    green += u64::from((pixel >> 8) & 0xff);
                    blue += u64::from(pixel & 0xff);
                    count += 1;
                }
            }
            if count == 0 {
                continue;
            }
            let color = (((red / count) as u32) << 16)
                | (((green / count) as u32) << 8)
                | (blue / count) as u32;
            fill_rect(
                buffer,
                width,
                height,
                block_x,
                block_y,
                end_x - block_x,
                end_y - block_y,
                color,
            );
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
            color: PALETTE[0],
            width_index: 1,
            gesture: None,
            crop_gesture: None,
            text_draft: None,
            selected: None,
            manipulation_base: None,
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
        editor.color = PALETTE[5];
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

    #[test]
    fn every_toolbar_button_renders_a_visual_icon() {
        let editor = test_editor();
        for button in ToolbarButton::ALL {
            let mut buffer = vec![0_u32; 48 * 48];
            draw_toolbar_icon(&mut buffer, 48, 48, button, 8, 8, 32, &editor);
            assert!(
                buffer.iter().any(|pixel| *pixel != 0),
                "{button:?} should render visible icon pixels"
            );
        }
    }

    #[test]
    fn extended_shape_gestures_create_the_requested_annotation_types() {
        for (tool, expected) in [
            (Tool::Ellipse, "ellipse"),
            (Tool::Redact, "redact"),
            (Tool::Pixelate, "pixelate"),
        ] {
            let gesture = line_gesture(tool, StrokeStyle::default());
            let annotation = Editor::annotation_for_gesture(&gesture).expect("valid gesture");
            assert!(matches!(
                (expected, annotation),
                ("ellipse", Annotation::Ellipse(_))
                    | ("redact", Annotation::Redact(_))
                    | ("pixelate", Annotation::Pixelate(_))
            ));
        }
    }

    #[test]
    fn text_draft_commits_as_an_editable_object() {
        let mut editor = test_editor();
        editor.text_draft = Some(TextDraft {
            position: Point::new(1.0, 1.0),
            callout_anchor: Some(Point::new(0.0, 0.0)),
            text: "Fast".to_owned(),
            color: Color::WHITE,
            size: 14.0,
        });

        assert!(editor.commit_text_draft().expect("text should commit"));
        assert!(matches!(
            editor.document.annotations(),
            [Annotation::Text(_)]
        ));
        assert!(editor.text_draft.is_none());
    }

    #[test]
    fn crop_resize_is_clamped_inside_the_source() {
        let original = PixelRect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        };
        let resized = ResizeHandle::SouthEast.resize_pixel_rect(
            original,
            PixelPoint { x: 8, y: 8 },
            PixelPoint { x: 99, y: 99 },
            8,
            8,
        );
        assert!(resized.right() <= 8);
        assert!(resized.bottom() <= 8);
        assert!(resized.width >= MIN_SELECTION_SIZE);
        assert!(resized.height >= MIN_SELECTION_SIZE);
    }

    #[test]
    fn object_drag_replaces_the_committed_cache_instead_of_adding_a_third_frame() {
        let mut editor = test_editor();
        let gesture = line_gesture(Tool::Line, StrokeStyle::default());
        let annotation = Editor::annotation_for_gesture(&gesture).expect("line annotation");
        editor.document.add(annotation);
        editor
            .refresh_committed_preview()
            .expect("preview should render");
        editor.tool = Tool::Select;

        assert!(editor
            .begin_object_transform(Point::new(3.0, 3.0))
            .expect("drag preview should initialize"));
        assert!(editor.committed_preview.is_none());
        assert!(editor.manipulation_base.is_some());
        assert!(editor.update_gesture(Point::new(4.0, 4.0)));
        assert!(editor.commit_active_gesture().expect("drag should commit"));
        assert!(editor.manipulation_base.is_none());
        assert!(editor.committed_preview.is_some());
    }
}
