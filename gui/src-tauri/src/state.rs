use std::path::PathBuf;
use std::sync::Mutex;

use ionconnect_config::Settings;

use crate::identity;

/// Estado compartido de la app Tauri. Por ahora solo maneja la
/// configuración local; una vez exista el IPC con `core`, aquí vivirá
/// también el cliente de esa conexión.
pub struct AppState {
    pub config_path: PathBuf,
    pub settings: Mutex<Settings>,
    pub device_id_hex: String,
}

impl AppState {
    pub fn new() -> Self {
        let dir = default_config_dir();
        let config_path = dir.join("config.toml");
        let settings = Settings::load(&config_path).unwrap_or_default();

        let device_id_hex = identity::load_or_generate_identity(&dir)
            .map(|identity| identity::device_id_hex(&identity))
            .unwrap_or_else(|_| "????????????????????????????????".to_string());

        Self {
            config_path,
            settings: Mutex::new(settings),
            device_id_hex,
        }
    }
}

/// Ruta de configuración simple basada en `$HOME`. Una implementación
/// completa debería usar el crate `directories` para respetar XDG en Linux
/// y `%APPDATA%` en Windows — pendiente para cuando `core` decida dónde
/// vive el resto de su estado persistente (certificados, trust store).
fn default_config_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/ionconnect")
}
