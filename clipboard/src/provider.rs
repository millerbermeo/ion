use std::path::PathBuf;

use crate::error::ClipboardError;

/// Acceso de lectura/escritura al portapapeles del sistema operativo.
/// Puerto hexagonal — `ArboardProvider` es el único adaptador por ahora;
/// imágenes son una fase futura (ver roadmap, fuera de este alcance).
pub trait ClipboardProvider: Send {
    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si el backend nativo falla al leer.
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError>;

    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si el backend nativo falla al escribir.
    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError>;

    /// Archivos que el usuario copió en su explorador (Ctrl+C), como rutas
    /// locales absolutas. `None` si el portapapeles no tiene una lista de
    /// archivos.
    ///
    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si el backend nativo falla al leer.
    fn get_files(&mut self) -> Result<Option<Vec<PathBuf>>, ClipboardError>;

    /// Deja `files` en el portapapeles como si el usuario los hubiera
    /// copiado acá, para que Ctrl+V en un explorador los pegue.
    ///
    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si el backend nativo falla al escribir.
    fn set_files(&mut self, files: &[PathBuf]) -> Result<(), ClipboardError>;
}
