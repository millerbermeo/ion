use ionconnect_config::Settings;
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

/// El `device_id` derivado de la identidad TLS de este equipo, en el mismo
/// formato hexadecimal que espera `PeerConfig::device_id` — para que el
/// usuario lo copie al configurar este equipo como peer en otra máquina.
#[tauri::command]
pub fn get_device_id(state: State<AppState>) -> String {
    state.device_id_hex.clone()
}

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Settings {
    state
        .settings
        .lock()
        .expect("el lock de configuración no debería estar envenenado")
        .clone()
}

#[tauri::command]
pub fn save_settings(state: State<AppState>, settings: Settings) -> Result<(), String> {
    settings
        .save(&state.config_path)
        .map_err(|e| e.to_string())?;
    *state
        .settings
        .lock()
        .expect("el lock de configuración no debería estar envenenado") = settings;
    Ok(())
}

/// Resumen de un equipo para la lista de la GUI.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceSummary {
    pub name: String,
    pub connected: bool,
    pub latency_ms: Option<u32>,
}

/// Sin el IPC hacia `core` (fase 9) todavía no hay un servicio real del que
/// listar equipos conectados. Devuelve una lista vacía por ahora; la GUI ya
/// puede construirse contra esta forma y solo cambiar la fuente de datos
/// cuando el IPC exista.
#[tauri::command]
pub fn list_devices() -> Vec<DeviceSummary> {
    Vec::new()
}
