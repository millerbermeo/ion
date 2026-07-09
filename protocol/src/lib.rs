//! Protocolo binario de `IonConnect`: tipos de mensaje y su codificación de wire.
//!
//! Este crate no depende de red ni de sistema operativo — solo transforma
//! [`Message`] hacia/desde bytes. El framing (prefijo de longitud) y el
//! transporte viven en el crate `network`.

mod codec;
mod error;
mod message;

pub use codec::{decode_message, encode_message, encode_message_into};
pub use error::ProtocolError;
pub use message::{
    Authentication, ClipboardMime, ClipboardSync, Disconnect, DisplayGeometry, Heartbeat,
    KeyboardPress, KeyboardRelease, Message, MessageType, MouseButton, MouseClick, MouseMove,
    Reconnect, UdpHello, Version,
};
