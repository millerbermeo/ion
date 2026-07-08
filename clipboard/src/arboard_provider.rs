use arboard::{Clipboard, Error as ArboardError};

use crate::error::ClipboardError;
use crate::provider::ClipboardProvider;

/// Adaptador sobre `arboard`, que ya cubre Windows/X11/Wayland con una sola
/// API, evitando repetir por sistema operativo lo que ese crate resuelve
/// bien.
pub struct ArboardProvider {
    clipboard: Clipboard,
}

impl ArboardProvider {
    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si no se pudo abrir el portapapeles del
    /// sistema (por ejemplo, sin servidor de display disponible).
    pub fn new() -> Result<Self, ClipboardError> {
        let clipboard = Clipboard::new().map_err(|e| ClipboardError::Backend(e.to_string()))?;
        Ok(Self { clipboard })
    }
}

impl ClipboardProvider for ArboardProvider {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        match self.clipboard.get_text() {
            Ok(text) => Ok(Some(text)),
            // El portapapeles vacío o con contenido no textual (p. ej. una
            // imagen) no es un error de nuestro dominio, solo "no hay texto".
            Err(ArboardError::ContentNotAvailable) => Ok(None),
            Err(err) => Err(ClipboardError::Backend(err.to_string())),
        }
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.clipboard
            .set_text(text.to_string())
            .map_err(|e| ClipboardError::Backend(e.to_string()))
    }
}
