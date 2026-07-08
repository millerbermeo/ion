use std::path::PathBuf;
use std::sync::Mutex;

use ionconnect_config::Settings;

/// Estado compartido de la app Tauri. Por ahora solo maneja la
/// configuración local; una vez exista el IPC con `core` (fase 9), aquí
/// vivirá también el cliente de esa conexión.
pub struct AppState {
    pub config_path: PathBuf,
    pub settings: Mutex<Settings>,
}

impl AppState {
    pub fn new() -> Self {
        let config_path = default_config_path();
        let settings = Settings::load(&config_path).unwrap_or_default();
        Self {
            config_path,
            settings: Mutex::new(settings),
        }
    }
}

/// Ruta de configuración simple basada en `$HOME`. Una implementación
/// completa debería usar el crate `directories` para respetar XDG en Linux
/// y `%APPDATA%` en Windows — pendiente para cuando `core` decida dónde
/// vive el resto de su estado persistente (certificados, trust store).
fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/ionconnect/config.toml")
}
