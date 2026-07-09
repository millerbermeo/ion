mod client;
mod error;
mod handoff;
mod identity;
#[cfg(all(unix, not(target_os = "macos")))]
mod input_session;
mod peer_id;
mod routing;
mod server;
mod trust_store;

use std::path::PathBuf;

use ionconnect_config::{ConfigWatcher, Role, Settings};
use tracing::{error, info, warn};

use crate::error::CoreError;

fn config_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/ionconnect")
}

fn main() {
    tracing_subscriber::fmt::init();

    let dir = config_dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("no se pudo crear el directorio de configuración: {err}");
        std::process::exit(1);
    }
    let config_path = dir.join("config.toml");
    let settings = Settings::load(&config_path).unwrap_or_default();
    if let Err(err) = settings.save(&config_path) {
        warn!(%err, "no se pudo escribir la configuración inicial");
    }

    // TODO(fase futura): aplicar la nueva configuración al vuelo sin
    // reiniciar el proceso. Por ahora solo se avisa en el log — reiniciar
    // el servicio sigue siendo necesario para que un cambio tenga efecto.
    match ConfigWatcher::watch(&config_path) {
        Ok(watcher) => {
            std::thread::spawn(move || {
                while watcher.recv().is_some() {
                    warn!(
                        "configuración modificada en disco — reiniciá el servicio para aplicarla"
                    );
                }
            });
        }
        Err(err) => warn!(%err, "no se pudo observar cambios en la configuración"),
    }

    let runtime = tokio::runtime::Runtime::new().expect("no se pudo crear el runtime de tokio");
    runtime.block_on(async move {
        let result = match settings.role {
            Role::Server => run_server(settings, &dir).await,
            Role::Client => client::run_client(settings, &dir).await,
        };
        if let Err(err) = result {
            error!(%err, "ionconnect-core terminó con error");
            std::process::exit(1);
        }
    });
}

async fn run_server(settings: Settings, dir: &std::path::Path) -> Result<(), CoreError> {
    let geometry = local_screen_geometry();
    info!("iniciando como servidor");
    server::run_server(settings, dir, geometry).await
}

/// Ancho/alto reales de la pantalla local, para que el `Layout` de
/// hand-off detecte el borde donde de verdad termina el escritorio (antes
/// esto era un `1920x1080` fijo, incorrecto en cualquier monitor de otra
/// resolución o en multi-monitor).
#[cfg(all(unix, not(target_os = "macos")))]
fn local_screen_geometry() -> ionconnect_screen::MonitorGeometry {
    match ionconnect_input::x11::X11Control::root_geometry() {
        Ok((width, height)) => ionconnect_screen::MonitorGeometry::new(0, 0, width, height),
        Err(err) => {
            warn!(
                %err,
                "no se pudo consultar la geometría real de pantalla, usando 1920x1080 por defecto"
            );
            ionconnect_screen::MonitorGeometry::new(0, 0, 1920, 1080)
        }
    }
}

/// El rol Server todavía no captura entrada fuera de X11 (ver
/// `server::spawn_input_session`), así que en el resto de plataformas esta
/// geometría no se usa para nada funcional — solo hace falta un valor con
/// el que construir el `Layout`.
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn local_screen_geometry() -> ionconnect_screen::MonitorGeometry {
    ionconnect_screen::MonitorGeometry::new(0, 0, 1920, 1080)
}
