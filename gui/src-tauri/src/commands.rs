use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use ionconnect_config::Settings;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// Cuántas líneas de log de `core` se retienen en memoria para
/// `get_core_snapshot` — suficiente para diagnosticar sin crecer sin límite
/// en una sesión larga.
const CORE_LOG_CAPACITY: usize = 500;

use crate::state::{AppState, ConnectedPeer, SentFile};

/// Valor de un campo `clave=valor` de `tracing` que está **al final de la
/// línea** — devuelve todo lo que sigue a `key=`, así soporta valores con
/// espacios (nombres/rutas de archivo). Ver los `warn!/info!` de
/// `core::file_transfer`, que ponen `name=`/`path=` último a propósito.
fn trailing_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_once(key).map(|(_, rest)| rest.trim())
}

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

/// Apaga `ionconnect-core` de forma ordenada: en Unix le manda SIGTERM
/// (que `core` intercepta para avisar a los peers con `Disconnect` antes de
/// salir) y espera hasta 2 s; si no terminó, SIGKILL. En Windows no hay
/// SIGTERM, así que `kill()` es lo único disponible.
pub fn graceful_kill(child: &mut Child) {
    #[cfg(unix)]
    {
        // SAFETY: `kill(2)` con una señal válida sobre un pid que este
        // proceso creó y todavía no cosechó.
        if let Ok(pid) = libc::pid_t::try_from(child.id()) {
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        for _ in 0..40 {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Traduce el valor del select de nivel de registro de la GUI al filtro
/// `RUST_LOG` que entiende `tracing`. "Todos" (`all`) sube al máximo detalle
/// **de los crates de `IonConnect`** y deja las dependencias (`rustls`,
/// `tokio`, `mio`) en `info` — un `trace` global las convierte en una manguera.
fn log_level_to_rust_log(level: &str) -> String {
    match level {
        "all" | "todos" => concat!(
            "info,",
            "ionconnect_core=trace,ionconnect_network=trace,ionconnect_input=trace,",
            "ionconnect_crypto=debug,ionconnect_protocol=debug,ionconnect_ipc=trace,",
            "ionconnect_clipboard=debug,ionconnect_config=debug,ionconnect_screen=debug"
        )
        .to_string(),
        "error" | "warn" | "info" | "debug" | "trace" => level.to_string(),
        _ => "info".to_string(),
    }
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
    } else if line.contains("Address already in use") || line.contains("address in use") {
        // Otro `ionconnect-core` (servicio systemd viejo, o instancia
        // manual) ya tiene el puerto. La GUI muestra un texto accionable.
        Some("port_busy")
    } else if line.contains("rechazado")
        || line.contains("no está soportado")
        || line.contains("ERROR")
    {
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

/// Extrae el valor de un campo `clave=valor` de una línea de `tracing`.
/// `tracing` escribe el valor **entre comillas** cuando tiene espacios
/// (`server="mi pc"`) y sin comillas cuando no (`server=cli`); esta función
/// maneja los dos casos — un `split_whitespace` ingenuo partía el valor con
/// espacios y devolvía basura (o nada), y por eso el equipo remoto no
/// aparecía en la lista de la GUI.
fn extract_field(line: &str, prefix: &str) -> Option<String> {
    // `tracing` siempre separa los campos con un espacio (` clave=valor`);
    // buscar ` prefix` evita que `name=` matchee dentro de `device_name=`.
    let spaced = format!(" {prefix}");
    let after = line.splitn(2, spaced.as_str()).nth(1)?;
    let value = if let Some(rest) = after.strip_prefix('"') {
        // Valor citado: hasta la comilla de cierre.
        rest.split('"').next().unwrap_or(rest)
    } else {
        // Sin comillas: hasta el próximo espacio.
        after.split_whitespace().next().unwrap_or(after)
    };
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
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

            // `archivo recibido path=<ruta>` — `path=` es el último campo.
            if line.contains("archivo recibido")
                && let Some(path) = trailing_field(&line, "path=")
                && !path.is_empty()
            {
                let path = path.to_string();
                if let Ok(mut files) = state.core_received_files.lock()
                    && !files.iter().any(|p| p == &path)
                {
                    files.push(path);
                }
            }

            // Progreso de envíos salientes — `name=` es el último campo en
            // esas líneas de `core::file_transfer::send_file`.
            if let Ok(mut sent) = state.core_sent_files.lock() {
                if line.contains("enviando archivo")
                    && let Some(name) = trailing_field(&line, "name=")
                    && !name.is_empty()
                {
                    match sent.iter_mut().find(|s| s.name == name) {
                        Some(existing) => {
                            existing.done = false;
                            existing.failed = false;
                        }
                        None => sent.push(SentFile {
                            name: name.to_string(),
                            done: false,
                            failed: false,
                        }),
                    }
                } else if line.contains("archivo enviado")
                    && let Some(name) = trailing_field(&line, "name=")
                {
                    if let Some(existing) = sent.iter_mut().find(|s| s.name == name) {
                        existing.done = true;
                    }
                } else if (line.contains("error leyendo el archivo")
                    || line.contains("no se pudo abrir el archivo a enviar"))
                    && let Some(name) = trailing_field(&line, "name=")
                {
                    if let Some(existing) = sent.iter_mut().find(|s| s.name == name) {
                        existing.failed = true;
                    } else {
                        sent.push(SentFile {
                            name: name.to_string(),
                            done: false,
                            failed: true,
                        });
                    }
                }
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
                    // Siempre agregar la fila, aunque el nombre no se pueda
                    // parsear — lo importante es que el usuario vea que está
                    // conectado.
                    let name = extract_field(&line, "server=")
                        .unwrap_or_else(|| "Servidor".to_string());
                    peers.retain(|p| p.device_id != "server");
                    peers.push(ConnectedPeer {
                        device_id: "server".to_string(),
                        name,
                    });
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
    let mut command = Command::new(&bin);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    // Nivel de registro elegido en la GUI → `RUST_LOG` del hijo (que
    // `tracing_subscriber::fmt::init` ya honra). "Todos" = `trace`. Si el
    // usuario ya trae `RUST_LOG` en el entorno, se respeta.
    if std::env::var_os("RUST_LOG").is_none() {
        let level = state
            .settings
            .lock()
            .expect("el lock de configuración no debería estar envenenado")
            .log_level
            .clone();
        command.env("RUST_LOG", log_level_to_rust_log(&level));
    }

    // Si la GUI muere (cierre normal o crash), el kernel le manda SIGTERM a
    // `core` en vez de dejarlo huérfano escuchando el puerto — la GUI es la
    // única forma de tenerlo corriendo, así no quedan procesos de fondo.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `pre_exec` solo llama funciones async-signal-safe
        // (`prctl`, `getppid`, `raise`).
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Carrera: si la GUI ya murió entre fork y prctl, reparent a
                // init (pid 1) — pedir el apagado de inmediato.
                if libc::getppid() == 1 {
                    libc::raise(libc::SIGTERM);
                }
                Ok(())
            });
        }
    }

    let mut child = command
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
    state
        .core_received_files
        .lock()
        .expect("el lock de archivos recibidos no debería estar envenenado")
        .clear();

    state
        .core_sent_files
        .lock()
        .expect("el lock de archivos enviados no debería estar envenenado")
        .clear();
    *state
        .core_status
        .lock()
        .expect("el lock de estado no debería estar envenenado") = "starting".to_string();
    let _ = app.emit("core-status", "starting");
    *guard = Some(child);
    Ok(())
}

/// Manda `paths` al otro equipo por el canal IPC local de `core`. Devuelve un
/// mensaje para mostrarle al usuario (éxito o el motivo del fallo) — sin
/// feedback, un drop que no llega a ningún lado se ve como "no funciona".
///
/// Precondiciones que se chequean antes de intentar el IPC:
/// - el servicio (`ionconnect-core`) tiene que estar corriendo;
/// - tiene que haber al menos un equipo conectado (si no, no hay a quién
///   mandárselo y los mensajes se quedarían en un buffer sin drenar).
async fn send_files_impl(state: &AppState, paths: Vec<String>) -> Result<String, String> {
    if paths.is_empty() {
        return Err("No se seleccionó ningún archivo.".to_string());
    }
    if state
        .core_child
        .lock()
        .expect("el lock del proceso core no debería estar envenenado")
        .is_none()
    {
        return Err("El servicio no está corriendo. Tocá «Conectar» primero.".to_string());
    }
    let peers = state
        .core_peers
        .lock()
        .expect("el lock de peers no debería estar envenenado")
        .len();
    if peers == 0 {
        return Err(
            "No hay ningún equipo conectado todavía. Esperá a que aparezca en «Equipos conectados»."
                .to_string(),
        );
    }

    let token_file = state.config_path.with_file_name("ipc.token");
    let mut conn = ionconnect_ipc::IpcClient::connect(&token_file)
        .await
        .map_err(|e| format!("No se pudo contactar al servicio ({e})."))?;

    let count = paths.len();
    for path in paths {
        let message = ionconnect_protocol::Message::FileOffer(ionconnect_protocol::FileOffer {
            transfer_id: 0,
            name: path,
            total_size: 0,
            mime: String::new(),
        });
        conn.send(message)
            .await
            .map_err(|e| format!("Fallo enviando un archivo ({e})."))?;
    }
    Ok(format!(
        "Enviando {count} archivo{} al otro equipo…",
        if count == 1 { "" } else { "s" }
    ))
}

/// Mandar archivos ya elegidos (arrastrados a la ventana, o desde el picker).
#[tauri::command]
pub async fn send_files(state: State<'_, AppState>, paths: Vec<String>) -> Result<String, String> {
    send_files_impl(&state, paths).await
}

/// Abre el diálogo nativo de selección de archivos y manda lo elegido.
/// Alternativa al drag&drop.
#[tauri::command]
pub async fn send_files_dialog(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Elegí archivos para enviar")
        .pick_files(move |selected| {
            let _ = tx.send(selected);
        });
    let selected = rx
        .await
        .map_err(|_| "no se pudo abrir el diálogo de archivos".to_string())?;
    let Some(selected) = selected else {
        return Ok("Selección cancelada.".to_string());
    };
    let paths: Vec<String> = selected
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .filter_map(|p| p.to_str().map(str::to_string))
        .collect();
    send_files_impl(&state, paths).await
}

/// El usuario confirmó en el modal que quiere cerrar. Sale de la app de
/// forma ordenada: dispara `RunEvent::Exit`, que apaga `ionconnect-core` con
/// SIGTERM (`graceful_kill`), y este a su vez avisa a los peers con
/// `Disconnect` antes de terminar.
#[tauri::command]
pub fn confirm_close(app: AppHandle) {
    app.exit(0);
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
            graceful_kill(&mut child);
            *state
                .core_status
                .lock()
                .expect("el lock de estado no debería estar envenenado") = "stopped".to_string();
            state
                .core_peers
                .lock()
                .expect("el lock de peers no debería estar envenenado")
                .clear();
            state
                .core_received_files
                .lock()
                .expect("el lock de archivos recibidos no debería estar envenenado")
                .clear();

            state
                .core_sent_files
                .lock()
                .expect("el lock de archivos enviados no debería estar envenenado")
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
    pub received_files: Vec<String>,
    pub sent_files: Vec<SentFile>,
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
    let received_files = state
        .core_received_files
        .lock()
        .expect("el lock de archivos recibidos no debería estar envenenado")
        .clone();
    let sent_files = state
        .core_sent_files
        .lock()
        .expect("el lock de archivos enviados no debería estar envenenado")
        .clone();
    CoreSnapshot {
        running,
        status,
        log,
        received_files,
        sent_files,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_line, extract_field, strip_ansi};

    #[test]
    fn extract_field_handles_unquoted_and_quoted_values() {
        assert_eq!(
            extract_field("... conectado al servidor server=cli", "server="),
            Some("cli".to_string())
        );
        // `tracing` cita los valores con espacios — este era el caso que
        // dejaba al servidor sin aparecer en la lista de la GUI.
        assert_eq!(
            extract_field("... conectado al servidor server=\"mi pc\"", "server="),
            Some("mi pc".to_string())
        );
        assert_eq!(
            extract_field("peer autenticado device_id=abc123 name=laptop", "name="),
            Some("laptop".to_string())
        );
        // `name=` no debe matchear dentro de `device_name=`.
        assert_eq!(
            extract_field("x device_name=foo name=bar", "name="),
            Some("bar".to_string())
        );
        assert_eq!(extract_field("sin el campo", "server="), None);
    }

    #[test]
    fn extract_field_works_after_stripping_ansi() {
        let raw = "\x1b[2m2026\x1b[0m INFO x: conectado al servidor \x1b[3mserver\x1b[0m\x1b[2m=\x1b[0mcli";
        assert_eq!(
            extract_field(&strip_ansi(raw), "server="),
            Some("cli".to_string())
        );
    }

    #[test]
    fn classify_line_prefers_connected_over_generic_error() {
        assert_eq!(
            classify_line("... conectado al servidor server=cli"),
            Some("connected")
        );
        assert_eq!(
            classify_line("ERROR ...: Address already in use (os error 98)"),
            Some("port_busy")
        );
    }
}
