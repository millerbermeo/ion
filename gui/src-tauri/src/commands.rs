use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use ionconnect_config::Settings;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// Cuántas líneas de log de `core` se retienen en memoria para
/// `get_core_snapshot` — suficiente para diagnosticar sin crecer sin límite
/// en una sesión larga.
const CORE_LOG_CAPACITY: usize = 500;

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

/// Lee `reader` línea a línea en un hilo dedicado y la vuelca en
/// `AppState::core_log`/`core_status` (fuente de verdad, leída por
/// polling desde la GUI) y además la emite como evento (`core-log`,
/// `core-status`) por si el frontend quiere reaccionar al instante.
fn stream_output(app: AppHandle, reader: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        let buf = BufReader::new(reader);
        for line in buf.lines().map_while(Result::ok) {
            let state = app.state::<AppState>();
            if let Ok(mut log) = state.core_log.lock() {
                log.push(line.clone());
                let overflow = log.len().saturating_sub(CORE_LOG_CAPACITY);
                if overflow > 0 {
                    log.drain(0..overflow);
                }
            }
            if let Some(status) = classify_line(&line) {
                if let Ok(mut current) = state.core_status.lock() {
                    *current = status.to_string();
                }
                let _ = app.emit("core-status", status);
            }
            let _ = app.emit("core-log", &line);
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

    state
        .core_log
        .lock()
        .expect("el lock del log no debería estar envenenado")
        .clear();
    *state
        .core_status
        .lock()
        .expect("el lock de estado no debería estar envenenado") = "starting".to_string();
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
            *state
                .core_status
                .lock()
                .expect("el lock de estado no debería estar envenenado") = "stopped".to_string();
            let _ = app.emit("core-status", "stopped");
            Ok(())
        }
        None => Err("ionconnect-core no está corriendo".to_string()),
    }
}

/// Foto del estado actual de `ionconnect-core`: si está corriendo, su
/// último estado derivado, y el log acumulado desde el último
/// [`start_core`]. Pensado para *polling* desde la GUI — es la fuente de
/// verdad, no depende de que los eventos hayan llegado bien al frontend.
#[derive(Debug, Clone, Serialize)]
pub struct CoreSnapshot {
    pub running: bool,
    pub status: String,
    pub log: Vec<String>,
}

#[tauri::command]
pub fn get_core_snapshot(state: State<AppState>) -> CoreSnapshot {
    let running = state
        .core_child
        .lock()
        .expect("el lock del proceso core no debería estar envenenado")
        .is_some();
    let status = state
        .core_status
        .lock()
        .expect("el lock de estado no debería estar envenenado")
        .clone();
    let log = state
        .core_log
        .lock()
        .expect("el lock del log no debería estar envenenado")
        .clone();
    CoreSnapshot {
        running,
        status,
        log,
    }
}
