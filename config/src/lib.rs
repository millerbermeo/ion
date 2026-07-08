//! Configuración persistente de `IonConnect`: TOML con recarga en caliente.
//!
//! Un TOML parcial o de una versión anterior sigue cargando (valores por
//! defecto para los campos faltantes); un TOML inválido durante un
//! hot-reload se descarta con una advertencia, conservando la
//! configuración anterior en vez de interrumpir el servicio.

mod error;
mod settings;
mod watcher;

pub use error::ConfigError;
pub use settings::{PairingModePreference, Settings};
pub use watcher::ConfigWatcher;
