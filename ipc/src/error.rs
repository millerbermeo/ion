/// Errores del canal de IPC local entre la GUI y el core.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("error de E/S: {0}")]
    Io(#[from] std::io::Error),

    #[error("el archivo de token de IPC está corrupto o incompleto")]
    MalformedTokenFile,

    #[error("token de IPC inválido — la conexión no está autorizada")]
    Unauthorized,

    #[error("error de red: {0}")]
    Network(#[from] ionconnect_network::NetworkError),
}
