use ionconnect_protocol::MouseButton;
use ionconnect_shared::KeyModifiers;

/// Evento de entrada ya normalizado, sea capturado localmente o recibido de
/// un peer remoto para inyectar. Independiente de plataforma: cada backend
/// traduce hacia/desde su representación nativa a este tipo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturedEvent {
    /// Posición absoluta dentro del escritorio virtual (ver crate `screen`).
    MouseMove {
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
