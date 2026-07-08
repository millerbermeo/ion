use crate::error::ClipboardError;

/// Acceso de lectura/escritura al portapapeles del sistema operativo.
/// Puerto hexagonal — `ArboardProvider` es el único adaptador por ahora;
/// imágenes/archivos son fases futuras (ver roadmap, fuera de este alcance).
pub trait ClipboardProvider: Send {
    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si el backend nativo falla al leer.
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError>;

    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si el backend nativo falla al escribir.
    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError>;
}
