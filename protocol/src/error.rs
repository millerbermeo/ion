/// Errores de codificación/decodificación del protocolo binario de `IonConnect`.
///
/// No incluye errores de red ni de criptografía — esos viven en `network` y
/// `crypto` respectivamente. Este crate solo conoce bytes de un mensaje ya
/// desenmarcado (framing lo hace `network` con `LengthDelimitedCodec`).
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("mensaje vacío: no se pudo leer el byte de tipo")]
    Empty,

    #[error("tipo de mensaje desconocido: {0}")]
    UnknownMessageType(u8),

    #[error("valor de enum inválido: {0}")]
    InvalidEnumValue(u8),

    #[error("payload truncado: se esperaban al menos {expected} bytes, quedaban {remaining}")]
    Truncated { expected: usize, remaining: usize },

    #[error("error de (de)serialización postcard: {0}")]
    Postcard(#[from] postcard::Error),
}
