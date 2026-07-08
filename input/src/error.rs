/// Errores de captura/inyección de entrada. Se mantiene agnóstico de
/// plataforma (sin tipos de `x11rb`/`windows` en la firma pública) para que
/// el crate compile en cualquier sistema aunque solo un backend esté activo;
/// cada backend convierte sus errores nativos a texto aquí.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("error de conexión X11: {0}")]
    X11Connection(String),

    #[error("la extensión X11 requerida ({0}) no está disponible en este servidor")]
    MissingX11Extension(&'static str),

    #[error("error de entrada en Windows: {0}")]
    Windows(String),

    #[error("error del portal de escritorio remoto (Wayland): {0}")]
    Portal(String),

    #[error("backend de entrada no soportado en este sistema: {0}")]
    Unsupported(&'static str),
}
