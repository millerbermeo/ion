use bytes::BytesMut;
use ionconnect_protocol::{Message, decode_message, encode_message_into};
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

use crate::error::NetworkError;

/// Ningún mensaje legítimo de `IonConnect` se acerca a este tamaño (el más
/// grande es `ClipboardSync`); un prefijo de longitud mayor que esto solo
/// puede venir de un peer malicioso o corrupto intentando agotar memoria.
const MAX_FRAME_LEN: usize = 1024 * 1024;

/// Codec de `tokio_util` que enmarca (`LengthDelimitedCodec`) y (de)serializa
/// mensajes `IonConnect` en una sola pieza, lista para usarse con `Framed`.
#[derive(Debug)]
pub struct MessageCodec {
    framing: LengthDelimitedCodec,
    /// Buffer de trabajo reutilizado en cada [`Encoder::encode`] — sin
    /// esto, cada `MouseMove`/clic/tecla saliente (cientos por segundo
    /// mientras el mouse está en movimiento) le pediría al heap una
    /// allocación nueva solo para el payload. `BytesMut::split` devuelve
    /// lo escrito sin copiarlo y deja la capacidad sobrante lista para el
    /// próximo mensaje, así que tras los primeros envíos deja de allocar.
    scratch: BytesMut,
}

impl Default for MessageCodec {
    fn default() -> Self {
        Self {
            framing: LengthDelimitedCodec::builder()
                .max_frame_length(MAX_FRAME_LEN)
                .new_codec(),
            scratch: BytesMut::new(),
        }
    }
}

impl Encoder<Message> for MessageCodec {
    type Error = NetworkError;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        encode_message_into(&mut self.scratch, &item)?;
        let payload = self.scratch.split().freeze();
        self.framing.encode(payload, dst).map_err(NetworkError::Io)
    }
}

impl Decoder for MessageCodec {
    type Item = Message;
    type Error = NetworkError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.framing.decode(src)? {
            Some(frame) => Ok(Some(decode_message(&frame)?)),
            None => Ok(None),
        }
    }
}
