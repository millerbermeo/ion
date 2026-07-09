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

use crate::state::{AppState, ConnectedPeer};

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

/// Equipos que `core` reportó como conectados, extraídos en vivo de su log
/// (ver [`stream_output`]). Del lado servidor son los peers autenticados;
/// del lado cliente es el propio servidor una vez conectado.
#[tauri::command]
pub fn list_devices(state: State<AppState>) -> Vec<DeviceSummary> {
    state
        .core_peers
        .lock()
        .expect("el lock de peers no debería estar envenenado")
        .iter()
        .map(|p| DeviceSummary {
            name: p.name.clone(),
            connected: true,
            latency_ms: None,
        })
        .collect()
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

/// Saca los códigos de color ANSI (`\x1b[...m`) que `tracing_subscriber`
/// mete en cada línea cuando cree que escribe a una terminal. Necesario
/// tanto para que el panel de log se lea bien como para poder parsear
/// campos `clave=valor` de forma confiable (los códigos quedan pegados
/// entre la clave y el `=`, así que un `contains("device_id=")` ingenuo
/// no matchea si no se limpia antes).
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Extrae el valor de un campo `clave=valor` separado por espacios (el
/// formato que usa `tracing_subscriber` para campos estructurados).
fn extract_field(line: &str, prefix: &str) -> Option<String> {
    line.split_whitespace()
        .find_map(|tok| tok.strip_prefix(prefix))
        .map(str::to_string)
}

/// Lee `reader` línea a línea en un hilo dedicado, la limpia de ANSI, y
/// la vuelca en `AppState::core_log`/`core_status`/`core_peers` (fuente
/// de verdad, leída por *polling* desde la GUI) y además la emite como
/// evento (`core-log`, `core-status`) por si el frontend quiere
/// reaccionar al instante.
fn stream_output(app: AppHandle, reader: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        let buf = BufReader::new(reader);
        for raw_line in buf.lines().map_while(Result::ok) {
            let line = strip_ansi(&raw_line);
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

            if let Ok(mut peers) = state.core_peers.lock() {
                if line.contains("peer autenticado") {
                    if let (Some(device_id), Some(name)) = (
                        extract_field(&line, "device_id="),
                        extract_field(&line, "name="),
                    ) {
                        if !peers.iter().any(|p| p.device_id == device_id) {
                            peers.push(ConnectedPeer { device_id, name });
                        }
                    }
                } else if line.contains("peer desconectado") {
                    if let Some(device_id) = extract_field(&line, "device_id=") {
                        peers.retain(|p| p.device_id != device_id);
                    }
                } else if line.contains("conectado al servidor") {
                    if let Some(name) = extract_field(&line, "server=") {
                        peers.retain(|p| p.device_id != "server");
                        peers.push(ConnectedPeer {
                            device_id: "server".to_string(),
                            name,
                        });
                    }
                } else if line.contains("reintentando conexión") {
                    peers.retain(|p| p.device_id != "server");
                }
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
    state
        .core_peers
        .lock()
        .expect("el lock de peers no debería estar envenenado")
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
            state
                .core_peers
                .lock()
                .expect("el lock de peers no debería estar envenenado")
                .clear();
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
