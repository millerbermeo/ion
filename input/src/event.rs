use ionconnect_protocol::MouseButton;
use ionconnect_shared::KeyModifiers;

/// Evento de entrada ya normalizado, sea capturado localmente o recibido de
/// un peer remoto para inyectar. Independiente de plataforma: cada backend
/// traduce hacia/desde su representación nativa a este tipo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturedEvent {
    /// Posición absoluta dentro del escritorio virtual (ver crate `screen`).
    /// Para inyección (`InputInjector`) esto es siempre lo que significa.
    /// Para captura, algunos backends (ver [`CapturedEvent::AbsolutePosition`])
    /// distinguen la posición real del cursor de una posición acumulada a
    /// partir de deltas — ver el backend `x11` para el porqué.
    MouseMove {
        x: i32,
        y: i32,
    },
    /// Solo emitido por backends de captura: la posición real y absoluta
    /// del cursor del sistema operativo en este instante, independiente de
    /// cualquier acumulación de deltas. Sirve para detectar que el cursor
    /// llegó al borde de la pantalla *mientras el sistema operativo todavía
    /// lo controla normalmente* (antes de agarrar el puntero).
    AbsolutePosition {
        x: i32,
        y: i32,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
    },
    Key {
        keycode: u32,
        modifiers: KeyModifiers,
        pressed: bool,
    },
}
