use crate::error::InputError;
use crate::event::CapturedEvent;

/// Inyecta eventos de entrada recibidos de un peer remoto en este equipo,
/// cuando este equipo es el que está siendo controlado.
pub trait InputInjector: Send {
    /// # Errors
    ///
    /// Devuelve [`InputError`] si el backend nativo rechaza o falla al
    /// inyectar el evento.
    fn inject(&mut self, event: &CapturedEvent) -> Result<(), InputError>;
}
