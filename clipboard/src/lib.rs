//! Sincronización de portapapeles de `IonConnect` — texto y listas de
//! archivos copiados — con prevención de bucles entre equipos.
//!
//! Este crate solo lee y escribe el portapapeles local y decide qué es un
//! cambio genuino; mover los bytes de los archivos es trabajo de `core`.
//! Imágenes quedan para una fase futura (ver roadmap).

mod arboard_provider;
mod error;
mod loop_guard;
mod provider;
mod watcher;
#[cfg(all(unix, not(target_os = "macos")))]
mod x11_files;

pub use arboard_provider::ArboardProvider;
pub use error::ClipboardError;
pub use loop_guard::LoopGuard;
pub use provider::ClipboardProvider;
pub use watcher::{ClipboardChange, ClipboardWatcher, FilesChange};
