use bytes::{Buf, BufMut, BytesMut};
use ionconnect_shared::KeyModifiers;
use serde::Serialize;

use crate::error::ProtocolError;
use crate::message::{
    Heartbeat, KeyboardPress, KeyboardRelease, Message, MessageType, MouseButton, MouseClick,
    MouseMove,
};

/// Codifica un [`Message`] a su representación binaria de wire.
///
/// El framing (prefijo de longitud) lo añade `network` vía
/// `LengthDelimitedCodec`; esta función solo produce el payload, siempre
/// empezando por el byte de [`MessageType`]. Los mensajes de alta frecuencia
/// usan un layout fijo escrito a mano (sin reflection de serde) para
/// mantener el hot path libre de overhead innecesario.
///
/// # Errors
///
/// Devuelve [`ProtocolError::Postcard`] si la serialización de un mensaje de
/// control (`ClipboardSync`, `Authentication`, `Disconnect`, `Reconnect` o
/// `Version`) falla.
pub fn encode_message(message: &Message) -> Result<BytesMut, ProtocolError> {
    let mut buf = BytesMut::with_capacity(estimated_capacity(message));
    buf.put_u8(message.message_type() as u8);

    match message {
        Message::Heartbeat(Heartbeat { sequence }) => buf.put_u32_le(*sequence),
        Message::MouseMove(MouseMove { x, y }) => {
            buf.put_i32_le(*x);
            buf.put_i32_le(*y);
        }
        Message::MouseClick(MouseClick {
            button,
            pressed,
            x,
            y,
        }) => {
            buf.put_u8(*button as u8);
            buf.put_u8(u8::from(*pressed));
            buf.put_i32_le(*x);
            buf.put_i32_le(*y);
        }
        Message::KeyboardPress(KeyboardPress { keycode, modifiers })
        | Message::KeyboardRelease(KeyboardRelease { keycode, modifiers }) => {
            buf.put_u32_le(*keycode);
            buf.put_u8(modifiers.bits());
        }
        Message::ClipboardSync(payload) => encode_postcard(&mut buf, payload)?,
        Message::Authentication(payload) => encode_postcard(&mut buf, payload)?,
        Message::Disconnect(payload) => encode_postcard(&mut buf, payload)?,
        Message::Reconnect(payload) => encode_postcard(&mut buf, payload)?,
        Message::Version(payload) => encode_postcard(&mut buf, payload)?,
    }

    Ok(buf)
}

/// Decodifica el payload ya desenmarcado de un mensaje.
///
/// # Errors
///
/// Devuelve [`ProtocolError::Empty`] si `payload` está vacío,
/// [`ProtocolError::UnknownMessageType`] o [`ProtocolError::InvalidEnumValue`]
/// si el byte de tipo o algún discriminante interno no es válido,
/// [`ProtocolError::Truncated`] si faltan bytes del layout fijo esperado, o
/// [`ProtocolError::Postcard`] si falla la deserialización de un mensaje de
/// control.
pub fn decode_message(payload: &[u8]) -> Result<Message, ProtocolError> {
    if payload.is_empty() {
        return Err(ProtocolError::Empty);
    }
    let mut buf = payload;
    let message_type = MessageType::try_from(buf.get_u8())?;

    match message_type {
        MessageType::Heartbeat => {
            require(buf, 4)?;
            Ok(Message::Heartbeat(Heartbeat {
                sequence: buf.get_u32_le(),
            }))
        }
        MessageType::MouseMove => {
            require(buf, 8)?;
            Ok(Message::MouseMove(MouseMove {
                x: buf.get_i32_le(),
                y: buf.get_i32_le(),
            }))
        }
        MessageType::MouseClick => {
            require(buf, 10)?;
            let button = MouseButton::try_from(buf.get_u8())?;
            let pressed = buf.get_u8() != 0;
            let x = buf.get_i32_le();
            let y = buf.get_i32_le();
            Ok(Message::MouseClick(MouseClick {
                button,
                pressed,
                x,
                y,
            }))
        }
        MessageType::KeyboardPress => {
            require(buf, 5)?;
            let keycode = buf.get_u32_le();
            let modifiers = KeyModifiers::from_bits(buf.get_u8());
            Ok(Message::KeyboardPress(KeyboardPress { keycode, modifiers }))
        }
        MessageType::KeyboardRelease => {
            require(buf, 5)?;
            let keycode = buf.get_u32_le();
            let modifiers = KeyModifiers::from_bits(buf.get_u8());
            Ok(Message::KeyboardRelease(KeyboardRelease {
                keycode,
                modifiers,
            }))
        }
        MessageType::ClipboardSync => Ok(Message::ClipboardSync(postcard::from_bytes(buf)?)),
        MessageType::Authentication => Ok(Message::Authentication(postcard::from_bytes(buf)?)),
        MessageType::Disconnect => Ok(Message::Disconnect(postcard::from_bytes(buf)?)),
        MessageType::Reconnect => Ok(Message::Reconnect(postcard::from_bytes(buf)?)),
        MessageType::Version => Ok(Message::Version(postcard::from_bytes(buf)?)),
    }
}

fn require(buf: &[u8], expected: usize) -> Result<(), ProtocolError> {
    if buf.len() < expected {
        return Err(ProtocolError::Truncated {
            expected,
            remaining: buf.len(),
        });
    }
    Ok(())
}

fn encode_postcard<T: Serialize>(buf: &mut BytesMut, value: &T) -> Result<(), ProtocolError> {
    let bytes = postcard::to_allocvec(value)?;
    buf.extend_from_slice(&bytes);
    Ok(())
}

/// Capacidad inicial del buffer para evitar reallocs en el hot path; para
/// los mensajes de control es solo una estimación razonable.
const fn estimated_capacity(message: &Message) -> usize {
    match message {
        Message::Heartbeat(_) => 5,
        Message::MouseMove(_) | Message::Reconnect(_) => 9,
        Message::MouseClick(_) => 11,
        Message::KeyboardPress(_) | Message::KeyboardRelease(_) => 6,
        Message::ClipboardSync(_) => 64,
        Message::Authentication(_) => 96,
        Message::Disconnect(_) => 32,
        Message::Version(_) => 7,
    }
}
