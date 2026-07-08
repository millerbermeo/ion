use std::sync::mpsc::Sender;

use crate::error::InputError;
use crate::event::CapturedEvent;

/// Captura eventos de entrada globales (mouse+teclado) cuando este equipo
/// tiene el control físico.
///
/// `run` es **bloqueante**: los hooks nativos (`SetWindowsHookEx`, `XInput2`)
/// tienen su propio bucle de eventos síncrono, así que debe ejecutarse en un
/// hilo del sistema operativo dedicado (o `tokio::task::spawn_blocking`),
/// nunca directamente dentro de una tarea async.
pub trait InputCapture: Send {
    /// Corre el bucle de captura hasta error o hasta que `stop` lo señale.
    /// Cada evento capturado se entrega por `sink`.
    ///
    /// # Errors
    ///
    /// Devuelve [`InputError`] si el backend nativo falla al leer eventos.
    fn run(&mut self, sink: Sender<CapturedEvent>) -> Result<(), InputError>;

    /// Señala al bucle de `run` (corriendo en otro hilo) que debe terminar.
    fn stop(&mut self);
}
