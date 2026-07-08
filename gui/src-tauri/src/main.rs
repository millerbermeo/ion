// Sin consola en Windows para el binario release; no afecta a Linux.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod identity;
mod state;

use tauri::Manager;
use tauri::tray::TrayIconBuilder;

use state::AppState;

fn main() {
    tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_device_id,
            commands::get_settings,
            commands::save_settings,
            commands::list_devices,
        ])
        .setup(|app| {
            if let Some(icon) = app.default_window_icon().cloned() {
                TrayIconBuilder::new().icon(icon).build(app)?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error corriendo la aplicación IonConnect");
}
