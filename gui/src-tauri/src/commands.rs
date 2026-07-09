use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use ionconnect_config::Settings;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

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

/// Ruta al binario `ionconnect-core`. Se asume instalado junto a la GUI
/// (así lo deja `install.sh`); si no está ahí, se cae a resolverlo por
/// `PATH` como último recurso.
fn core_binary_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| "no se pudo resolver el directorio del ejecutable".to_string())?;
    let name = if cfg!(windows) {
        "ionconnect-core.exe"
    } else {
        "ionconnect-core"
    };
    let candidate = dir.join(name);
    Ok(if candidate.exists() {
        candidate
    } else {
        PathBuf::from(name)
    })
}

/// Deriva un estado resumido a partir de una línea de log de `core`, para
/// que la GUI pueda reflejar progreso sin tener que parsear la línea
/// completa. `None` si la línea no aporta un cambio de estado.
fn classify_line(line: &str) -> Option<&'static str> {
    if line.contains("conectado al servidor") || line.contains("peer autenticado") {
        Some("connected")
    } else if line.contains("escuchando conexiones de peers") {
        Some("listening")
    } else if line.contains("reintentando conexión") {
        Some("retrying")
    } else if line.contains("rechazado") || line.contains("no está soportado") || line.contains("ERROR") {
        Some("error")
    } else if line.contains("identidad local cargada") {
        Some("starting")
    } else {
        None
    }
}

fn stream_output(app: AppHandle, reader: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        let buf = BufReader::new(reader);
        for line in buf.lines().map_while(Result::ok) {
            let _ = app.emit("core-log", &line);
            if let Some(status) = classify_line(&line) {
                let _ = app.emit("core-status", status);
            }
        }
    });
}

/// Arranca `ionconnect-core` como proceso hijo y transmite su salida a la
/// GUI vía eventos: `core-log` con cada línea cruda, `core-status` con un
/// estado corto derivado de esa línea (`starting`, `listening`,
/// `connected`, `retrying`, `error`). Falla si ya hay una instancia
/// corriendo, iniciada desde este mismo proceso de GUI.
#[tauri::command]
pub fn start_core(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let mut guard = state
        .core_child
        .lock()
        .expect("el lock del proceso core no debería estar envenenado");
    if guard.is_some() {
        return Err("ionconnect-core ya está corriendo".to_string());
    }

    let bin = core_binary_path()?;
    let mut child = Command::new(&bin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("no se pudo iniciar {}: {e}", bin.display()))?;

    if let Some(stdout) = child.stdout.take() {
        stream_output(app.clone(), stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        stream_output(app.clone(), stderr);
    }

    let _ = app.emit("core-status", "starting");
    *guard = Some(child);
    Ok(())
}

/// Mata el proceso `ionconnect-core` iniciado por [`start_core`], si hay
/// uno corriendo.
#[tauri::command]
pub fn stop_core(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let mut guard = state
        .core_child
        .lock()
        .expect("el lock del proceso core no debería estar envenenado");
    match guard.take() {
        Some(mut child) => {
            child.kill().map_err(|e| e.to_string())?;
            let _ = child.wait();
            let _ = app.emit("core-status", "stopped");
            Ok(())
        }
        None => Err("ionconnect-core no está corriendo".to_string()),
    }
}
