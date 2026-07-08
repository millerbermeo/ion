/// Errores de transporte: E/S, framing/protocolo y descubrimiento mDNS.
/// Los errores de TLS/criptografía viven en `crypto`.
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("error de E/S: {0}")]
    Io(#[from] std::io::Error),

    #[error("error de protocolo: {0}")]
    Protocol(#[from] ionconnect_protocol::ProtocolError),

    #[error("error de descubrimiento mDNS: {0}")]
    Discovery(#[from] mdns_sd::Error),
}
