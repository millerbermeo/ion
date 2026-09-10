// Sin consola en Windows para el binario release; no afecta a Linux.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod identity;
mod state;

use serde_json::json;
use tauri::{DragDropEvent, Manager, RunEvent, WindowEvent};

use state::AppState;

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_device_id,
            commands::get_settings,
            commands::save_settings,
            commands::list_devices,
            commands::start_core,
            commands::stop_core,
            commands::get_core_snapshot,
            commands::send_files,
            commands::send_files_dialog,
            commands::confirm_close,
        ])
        .on_window_event(|window, event| match event {
            // Cerrar la ventana corta las conexiones activas: se veta el
            // cierre y se le pide al frontend que muestre la confirmación.
            // "Cerrar" invoca `confirm_close`, que hace salir la app de
            // forma ordenada.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                if let Some(webview) = window.app_handle().get_webview_window(window.label()) {
                    let _ =
                        webview.eval("window.__ionCloseRequested && window.__ionCloseRequested()");
                }
            }
            // Arrastrar archivos a la ventana: el drop se captura acá (el
            // webview no lo ve con `dragDropEnabled`), y se le pasan las
            // rutas al frontend, que llama a `send_files` y muestra el
            // resultado como toast. `core` transfiere y el otro lado los
            // guarda en la carpeta `ionconnect` del Escritorio.
            WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                let paths: Vec<String> = paths
                    .iter()
                    .filter_map(|p| p.to_str().map(str::to_string))
                    .collect();
                if paths.is_empty() {
                    return;
                }
                if let Some(webview) = window.app_handle().get_webview_window(window.label()) {
                    let script = format!(
                        "window.__ionFilesDropped && window.__ionFilesDropped({})",
                        json!(paths)
                    );
                    let _ = webview.eval(&script);
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error construyendo la aplicación IonConnect");

    app.run(|app_handle, event| {
        // IonConnect solo funciona con la ventana abierta: no hay ícono de
        // bandeja ni ejecución en segundo plano. Al salir se apaga
        // `ionconnect-core` de forma ordenada — le llega SIGTERM, avisa a
        // los peers con `Disconnect` y recién ahí termina, así el otro
        // equipo no queda reintentando ni con el mouse agarrado.
        if let RunEvent::Exit = event {
            let state = app_handle.state::<AppState>();
            if let Ok(mut guard) = state.core_child.lock()
                && let Some(mut child) = guard.take()
            {
                commands::graceful_kill(&mut child);
            }
        }
    });
}
