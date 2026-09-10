use ionconnect_shared::{DeviceId, KeyModifiers};
use serde::{Deserialize, Serialize};

use crate::error::ProtocolError;

/// Discriminante de un byte que identifica el tipo de mensaje en el wire.
///
/// El valor numérico es parte del protocolo de red — no reordenar ni
/// reutilizar variantes eliminadas en el futuro, rompería compatibilidad
/// entre versiones desplegadas.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    Heartbeat = 0,
    MouseMove = 1,
    MouseClick = 2,
    KeyboardPress = 3,
    KeyboardRelease = 4,
    ClipboardSync = 5,
    Authentication = 6,
    Disconnect = 7,
    Reconnect = 8,
    Version = 9,
    DisplayGeometry = 10,
    UdpHello = 11,
    FileOffer = 12,
    FileChunk = 13,
    FileEnd = 14,
    FileAbort = 15,
}

impl TryFrom<u8> for MessageType {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Heartbeat),
            1 => Ok(Self::MouseMove),
            2 => Ok(Self::MouseClick),
            3 => Ok(Self::KeyboardPress),
            4 => Ok(Self::KeyboardRelease),
            5 => Ok(Self::ClipboardSync),
            6 => Ok(Self::Authentication),
            7 => Ok(Self::Disconnect),
            8 => Ok(Self::Reconnect),
            9 => Ok(Self::Version),
            10 => Ok(Self::DisplayGeometry),
            11 => Ok(Self::UdpHello),
            12 => Ok(Self::FileOffer),
            13 => Ok(Self::FileChunk),
            14 => Ok(Self::FileEnd),
            15 => Ok(Self::FileAbort),
            other => Err(ProtocolError::UnknownMessageType(other)),
        }
    }
}

// ---- Mensajes de alta frecuencia (layout binario fijo, sin serde) ----

/// Señal periódica de vida de la conexión. `sequence` permite calcular RTT
/// para el indicador de latencia de la GUI (eco: el receptor puede responder
/// con el mismo número).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heartbeat {
    pub sequence: u32,
}

/// Posición absoluta del cursor dentro del escritorio virtual del equipo
/// receptor. Puede ser negativa: en Windows los monitores a la izquierda o
/// arriba del primario tienen coordenadas negativas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseMove {
    pub x: i32,
    pub y: i32,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left = 0,
    Right = 1,
    Middle = 2,
    Back = 3,
    Forward = 4,
    /// Una "muesca" (notch) de scroll, no un botón mantenido — igual que
    /// X11 (botones 4-7) y la mayoría de las APIs nativas la modelan como
    /// press+release instantáneo, no como un botón que se sostiene.
    ScrollUp = 5,
    ScrollDown = 6,
    ScrollLeft = 7,
    ScrollRight = 8,
}

impl TryFrom<u8> for MouseButton {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Left),
            1 => Ok(Self::Right),
            2 => Ok(Self::Middle),
            3 => Ok(Self::Back),
            4 => Ok(Self::Forward),
            5 => Ok(Self::ScrollUp),
            6 => Ok(Self::ScrollDown),
            7 => Ok(Self::ScrollLeft),
            8 => Ok(Self::ScrollRight),
            other => Err(ProtocolError::InvalidEnumValue(other)),
        }
    }
}

/// Incluye `x`/`y` para no depender del orden de llegada respecto a un
/// `MouseMove` previo bajo pérdida o reordenamiento de paquetes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseClick {
    pub button: MouseButton,
    pub pressed: bool,
    pub x: i32,
    pub y: i32,
}

/// `modifiers` viaja junto al keycode (en vez de inferirse solo del estado
/// del receptor) para tolerar la pérdida ocasional de algún evento sin que
/// Ctrl/Alt/Shift queden desincronizados entre equipos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardPress {
    pub keycode: u32,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardRelease {
    pub keycode: u32,
    pub modifiers: KeyModifiers,
}

// ---- Mensajes de control (raros/grandes, codificados con postcard+serde) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardMime {
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardSync {
    pub mime: ClipboardMime,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authentication {
    pub device_id: DeviceId,
    pub device_name: String,
    pub protocol_version: u32,
    /// SHA-256 del certificado TLS del emisor, para verificación TOFU.
    pub cert_fingerprint: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disconnect {
    pub code: u8,
    pub reason: String,
}

/// Código de [`Disconnect::code`] para un apagado voluntario del otro
/// extremo (el usuario cerró la ventana de la GUI). El receptor debe
/// terminar **sin reintentar** — a diferencia de una caída de red, donde sí
/// corresponde reconectar con backoff.
pub const DISCONNECT_SHUTDOWN: u8 = 1;

/// Permite correlacionar una reconexión con la sesión previa (p. ej. tras
/// una renegociación de claves) sin reautenticar desde cero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reconnect {
    pub session_nonce: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

/// Tamaño real del escritorio virtual del emisor — lo manda el cliente al
/// conectarse (y de nuevo si cambia, p. ej. un monitor externo que se
/// conecta/desconecta) para que el servidor deje de asumir que tiene la
/// misma resolución que él al calcular dónde reaparece el cursor en un
/// hand-off. Origen siempre `(0, 0)`: alcanza para el caso común de un
/// cliente de un solo monitor; multi-monitor en el cliente queda para
/// cuando haga falta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayGeometry {
    pub width: u32,
    pub height: u32,
}

/// Anuncia el comienzo de una transferencia de archivo por el canal TCP+TLS
/// confiable (los `FileChunk` que siguen dependen del orden, así que no
/// pueden ir por UDP). `transfer_id` es único por emisor mientras la
/// transferencia está viva; `name` es solo el nombre del archivo (sin ruta),
/// el receptor lo materializa en su carpeta de descargas. `total_size` es
/// informativo (barra de progreso); el fin real lo marca [`FileEnd`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOffer {
    pub transfer_id: u64,
    pub name: String,
    pub total_size: u64,
    pub mime: String,
}

/// Un trozo de una transferencia en curso. Los trozos llegan en orden (TCP)
/// y se van anexando; el tamaño lo decide el emisor (ver
/// `core::file_transfer::CHUNK_SIZE`), siempre bastante por debajo del límite
/// de frame del códec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChunk {
    pub transfer_id: u64,
    pub data: Vec<u8>,
}

/// Marca que ya se enviaron todos los trozos de `transfer_id`: el receptor
/// cierra el archivo y lo mueve de `<nombre>.part` a su nombre definitivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEnd {
    pub transfer_id: u64,
}

/// Aborta `transfer_id` (error de lectura en el emisor, cancelación): el
/// receptor descarta el `.part` a medias.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAbort {
    pub transfer_id: u64,
    pub reason: String,
}

/// El cliente lo manda por el canal TCP ya confiable, una vez por sesión
/// (justo después de `DisplayGeometry`), para avisarle al servidor a qué
/// puerto UDP local mandarle los `MouseMove` continuos — el servidor ya
/// conoce la IP del cliente por la conexión TCP, le falta el puerto porque
/// el cliente bindea el socket UDP en uno efímero. Ver `core::udp_peers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UdpHello {
    pub port: u16,
}

/// Envoltura de todos los tipos de mensaje del protocolo `IonConnect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Heartbeat(Heartbeat),
    MouseMove(MouseMove),
    MouseClick(MouseClick),
    KeyboardPress(KeyboardPress),
    KeyboardRelease(KeyboardRelease),
    ClipboardSync(ClipboardSync),
    Authentication(Authentication),
    Disconnect(Disconnect),
    Reconnect(Reconnect),
    Version(Version),
    DisplayGeometry(DisplayGeometry),
    UdpHello(UdpHello),
    FileOffer(FileOffer),
    FileChunk(FileChunk),
    FileEnd(FileEnd),
    FileAbort(FileAbort),
}

impl Message {
    #[must_use]
    pub const fn message_type(&self) -> MessageType {
        match self {
            Self::Heartbeat(_) => MessageType::Heartbeat,
            Self::MouseMove(_) => MessageType::MouseMove,
            Self::MouseClick(_) => MessageType::MouseClick,
            Self::KeyboardPress(_) => MessageType::KeyboardPress,
            Self::KeyboardRelease(_) => MessageType::KeyboardRelease,
            Self::ClipboardSync(_) => MessageType::ClipboardSync,
            Self::Authentication(_) => MessageType::Authentication,
            Self::Disconnect(_) => MessageType::Disconnect,
            Self::Reconnect(_) => MessageType::Reconnect,
            Self::Version(_) => MessageType::Version,
            Self::DisplayGeometry(_) => MessageType::DisplayGeometry,
            Self::UdpHello(_) => MessageType::UdpHello,
            Self::FileOffer(_) => MessageType::FileOffer,
            Self::FileChunk(_) => MessageType::FileChunk,
            Self::FileEnd(_) => MessageType::FileEnd,
            Self::FileAbort(_) => MessageType::FileAbort,
        }
    }
}
