//! Window integration for Ruffle: cursor, fullscreen and device fonts.

use ruffle_core::backend::ui::{
    DialogResultFuture, FileDialogResult, FileFilter, FontDefinition, FullscreenError,
    LanguageIdentifier, MouseCursor, MultiDialogResultFuture, MultiFileDialogResult, UiBackend,
};
use ruffle_core::font::{FontFileData, FontQuery};
use std::sync::Arc;
use url::Url;
use winit::window::{CursorIcon, Fullscreen, Window};

/// Free stand-ins for the Windows fonts the 2002 game asks for, embedded so
/// text looks the same everywhere. Liberation Sans is metric-compatible with
/// Arial ("Arial CE" was its Central European build); DejaVu Sans shares
/// Verdana's wide Bitstream Vera design.
const ARIAL: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/LiberationSans-Regular.ttf"),
    include_bytes!("../assets/fonts/LiberationSans-Bold.ttf"),
    include_bytes!("../assets/fonts/LiberationSans-Italic.ttf"),
    include_bytes!("../assets/fonts/LiberationSans-BoldItalic.ttf"),
];
const VERDANA: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/DejaVuSans.ttf"),
    include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf"),
    include_bytes!("../assets/fonts/DejaVuSans-Oblique.ttf"),
    include_bytes!("../assets/fonts/DejaVuSans-BoldOblique.ttf"),
];

pub struct Ui {
    window: Arc<Window>,
    mouse_visible: bool,
}

impl Ui {
    pub fn new(window: Arc<Window>) -> Self {
        Self {
            window,
            mouse_visible: true,
        }
    }
}

impl UiBackend for Ui {
    fn mouse_visible(&self) -> bool {
        self.mouse_visible
    }

    fn set_mouse_visible(&mut self, visible: bool) {
        self.mouse_visible = visible;
        self.window.set_cursor_visible(visible);
    }

    fn set_mouse_cursor(&mut self, cursor: MouseCursor) {
        self.window.set_cursor(match cursor {
            MouseCursor::Arrow => CursorIcon::Default,
            MouseCursor::Hand => CursorIcon::Pointer,
            MouseCursor::IBeam => CursorIcon::Text,
            MouseCursor::Grab => CursorIcon::Grab,
        });
    }

    fn clipboard_content(&mut self) -> String {
        String::new()
    }

    fn set_clipboard_content(&mut self, _content: String) {}

    fn set_fullscreen(&mut self, is_full: bool) -> Result<(), FullscreenError> {
        self.window
            .set_fullscreen(is_full.then_some(Fullscreen::Borderless(None)));
        Ok(())
    }

    fn display_root_movie_download_failed_message(&self, _invalid_swf: bool, _fetch_error: String) {
    }

    fn message(&self, message: &str) {
        tracing::info!("{message}");
    }

    fn open_virtual_keyboard(&self) {}

    fn close_virtual_keyboard(&self) {}

    fn language(&self) -> LanguageIdentifier {
        "cs-CZ".parse().expect("valid language tag")
    }

    fn display_unsupported_video(&self, _url: Url) {}

    fn load_device_font(&self, query: &FontQuery, register: &mut dyn FnMut(FontDefinition)) {
        let family = if query.lowercase_name.starts_with("verdana") {
            VERDANA
        } else {
            ARIAL
        };
        let style = usize::from(query.is_bold) + 2 * usize::from(query.is_italic);
        register(FontDefinition::FontFile {
            name: query.name.clone(),
            is_bold: query.is_bold,
            is_italic: query.is_italic,
            data: FontFileData::new(family[style]),
            index: 0,
        });
    }

    fn sort_device_fonts(
        &self,
        _query: &FontQuery,
        _register: &mut dyn FnMut(FontDefinition),
    ) -> Vec<FontQuery> {
        Vec::new()
    }

    fn display_file_open_dialog(&mut self, _filters: Vec<FileFilter>) -> Option<DialogResultFuture> {
        Some(Box::pin(async { Ok(FileDialogResult::Canceled) }))
    }

    fn display_file_open_dialog_multiple(
        &mut self,
        _filters: Vec<FileFilter>,
    ) -> Option<MultiDialogResultFuture> {
        Some(Box::pin(async { Ok(MultiFileDialogResult::Canceled) }))
    }

    fn display_file_save_dialog(
        &mut self,
        _file_name: String,
        _domain: String,
    ) -> Option<DialogResultFuture> {
        None
    }

    fn close_file_dialog(&mut self) {}
}
