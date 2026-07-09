use std::path::PathBuf;
use std::process::Child;
use std::sync::Mutex;

use ionconnect_config::Settings;

use crate::identity;

/// Estado compartido de la app Tauri. Además de la configuración local,
/// trackea el proceso hijo `ionconnect-core` cuando el usuario lo arranca
/// desde el botón "Conectar" — vive solo en memoria, no sobrevive a un
/// reinicio de la GUI.
///
/// `core_log`/`core_status` son la fuente de verdad que lee la GUI por
/// *polling* (`get_core_snapshot`) en vez de depender únicamente de los
/// eventos que emiten los hilos lectores — así el estado mostrado es
/// correcto incluso si algo en el bus de eventos del webview falla.
pub struct AppState {
    pub config_path: PathBuf,
    pub settings: Mutex<Settings>,
    pub device_id_hex: String,
    pub core_child: Mutex<Option<Child>>,
    pub core_log: Mutex<Vec<String>>,
    pub core_status: Mutex<String>,
    pub core_peers: Mutex<Vec<ConnectedPeer>>,
}

/// Un equipo que `core` reportó como conectado, extraído en vivo de sus
/// líneas de log (`peer autenticado` del lado servidor, `conectado al
/// servidor` del lado cliente — ver `commands::stream_output`).
#[derive(Debug, Clone)]
pub struct ConnectedPeer {
    pub device_id: String,
    pub name: String,
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
            core_child: Mutex::new(None),
            core_log: Mutex::new(Vec::new()),
            core_status: Mutex::new("stopped".to_string()),
            core_peers: Mutex::new(Vec::new()),
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
