/// Errores de acceso al portapapeles del sistema operativo.
#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("error de portapapeles: {0}")]
    Backend(String),
}
