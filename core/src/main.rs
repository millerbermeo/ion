mod client;
mod display;
mod error;
mod file_transfer;
mod handoff;
mod identity;
#[cfg(all(unix, not(target_os = "macos")))]
mod input_session;
mod key_repeat;
mod peer_id;
mod routing;
mod server;
mod trust_store;
mod udp_peers;

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
    // `RUST_LOG` manda (lo pone la GUI según el select "Nivel de registro");
    // si no está, INFO.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

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
        // Apagado voluntario: la GUI manda SIGTERM al cerrar su ventana (y
        // `PR_SET_PDEATHSIG` lo hace también si la GUI crashea). Al recibirlo
        // se le avisa al otro extremo con `Disconnect` antes de salir, para
        // que su cliente/servidor no quede reintentando o con el mouse
        // agarrado. Ver `server::run_server`/`client::run_client`.
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            wait_for_shutdown_signal().await;
            info!("señal de apagado recibida, cerrando de forma ordenada");
            let _ = shutdown_tx.send(true);
        });

        let result = match settings.role {
            Role::Server => run_server(settings, &dir, shutdown_rx).await,
            Role::Client => client::run_client(settings, &dir, shutdown_rx).await,
        };
        if let Err(err) = result {
            error!(%err, "ionconnect-core terminó con error");
            std::process::exit(1);
        }
        // Salir ya, sin dejar que `Runtime::drop` espere: la captura X11
        // corre en un `spawn_blocking` cuyo bucle de eventos nunca retorna
        // (no hay forma de cancelarlo desde afuera), así que dropear el
        // runtime colgaría el proceso para siempre en un apagado limpio.
        info!("ionconnect-core detenido");
        std::process::exit(0);
    });
}

/// Se completa cuando llega SIGTERM o SIGINT (Ctrl-C) en Unix, o Ctrl-C en
/// Windows.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term =
            signal(SignalKind::terminate()).expect("no se pudo instalar el manejador de SIGTERM");
        let mut int =
            signal(SignalKind::interrupt()).expect("no se pudo instalar el manejador de SIGINT");
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn run_server(
    settings: Settings,
    dir: &std::path::Path,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<(), CoreError> {
    let local_display = display::detect_local_display().await;
    info!("iniciando como servidor");
    server::run_server(settings, dir, local_display, shutdown).await
}
