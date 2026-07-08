/// Errores del binario `core`: agrega los de cada crate que orquesta.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("error de E/S: {0}")]
    Io(#[from] std::io::Error),

    #[error("error de criptografía: {0}")]
    Crypto(#[from] ionconnect_crypto::CryptoError),

    #[error("error de red: {0}")]
    Network(#[from] ionconnect_network::NetworkError),

    #[error("error de configuración: {0}")]
    Config(#[from] ionconnect_config::ConfigError),

    #[error("error de entrada: {0}")]
    Input(#[from] ionconnect_input::InputError),

    #[error("error de IPC local: {0}")]
    Ipc(#[from] ionconnect_ipc::IpcError),

    #[error("{0}")]
    Other(String),
}
