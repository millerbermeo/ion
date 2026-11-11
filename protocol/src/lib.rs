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
    Authentication, ClipboardFilesOffer, ClipboardFilesRequest, ClipboardMime, ClipboardSync,
    DISCONNECT_SHUTDOWN, Disconnect, DisplayGeometry, FileAbort, FileBatch, FileBatchEnd,
    FileBatchKind, FileChunk, FileEnd, FileOffer, Heartbeat, KeyboardPress, KeyboardRelease,
    MIME_DIRECTORY, Message, MessageType, MouseButton, MouseClick, MouseMove, Reconnect, UdpHello,
    Version,
};
