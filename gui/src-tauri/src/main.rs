// Sin consola en Windows para el binario release; no afecta a Linux.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod identity;
mod state;

use tauri::tray::TrayIconBuilder;
use tauri::{Manager, RunEvent};

use state::AppState;

fn main() {
    let app = tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_device_id,
            commands::get_settings,
            commands::save_settings,
            commands::list_devices,
            commands::start_core,
            commands::stop_core,
        ])
        .setup(|app| {
            if let Some(icon) = app.default_window_icon().cloned() {
                TrayIconBuilder::new().icon(icon).build(app)?;
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error construyendo la aplicación IonConnect");

    app.run(|app_handle, event| {
        // Si el usuario deja `ionconnect-core` corriendo y cierra la GUI,
        // lo matamos acá — si no, queda huérfano escuchando el puerto.
        if let RunEvent::Exit = event {
            let state = app_handle.state::<AppState>();
            if let Ok(mut guard) = state.core_child.lock() {
                if let Some(mut child) = guard.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    });
}
