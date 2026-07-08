//! Sincronización de portapapeles (texto) de `IonConnect`, con prevención
//! de bucles entre equipos.
//!
//! Imágenes y archivos son fases futuras fuera de este alcance (ver
//! roadmap).

mod arboard_provider;
mod error;
mod loop_guard;
mod provider;
mod watcher;

pub use arboard_provider::ArboardProvider;
pub use error::ClipboardError;
pub use loop_guard::LoopGuard;
pub use provider::ClipboardProvider;
pub use watcher::ClipboardWatcher;
