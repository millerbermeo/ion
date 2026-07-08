use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tracing::warn;

use crate::error::ConfigError;
use crate::settings::Settings;

/// Observa el archivo de configuración y recarga [`Settings`] cada vez que
/// cambia, sin reiniciar el proceso. Si el archivo queda momentáneamente
/// inválido a mitad de una escritura (p. ej. un editor externo guardando),
/// se registra una advertencia y se conserva la última configuración
/// válida — nunca se envía un `Settings` a medio escribir.
///
/// Bloqueante por diseño, igual que `InputCapture` en el crate `input`:
/// el callback nativo de `notify` corre en su propio hilo; quien use este
/// tipo debe correr [`ConfigWatcher::recv`] en un hilo dedicado o
/// `tokio::task::spawn_blocking`.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    receiver: Receiver<Settings>,
}

impl ConfigWatcher {
    /// # Errors
    ///
    /// Devuelve [`ConfigError::Watch`] si no se pudo iniciar la observación
    /// del archivo (por ejemplo, si el directorio padre no existe).
    pub fn watch(path: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let path = path.into();
        let (tx, rx) = mpsc::channel();
        let watched_path = path.clone();

        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            handle_event(result, &watched_path, &tx);
        })
        .map_err(|e| ConfigError::Watch(e.to_string()))?;

        watcher
            .watch(&path, RecursiveMode::NonRecursive)
            .map_err(|e| ConfigError::Watch(e.to_string()))?;

        Ok(Self {
            _watcher: watcher,
            receiver: rx,
        })
    }

    /// Bloquea hasta la próxima recarga válida, o `None` si el observador
    /// se cerró.
    #[must_use]
    pub fn recv(&self) -> Option<Settings> {
        self.receiver.recv().ok()
    }
}

fn handle_event(result: notify::Result<Event>, path: &Path, sink: &mpsc::Sender<Settings>) {
    let event = match result {
        Ok(event) => event,
        Err(err) => {
            warn!(%err, "error del observador de configuración");
            return;
        }
    };
    if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
        return;
    }
    match Settings::load(path) {
        Ok(settings) => {
            let _ = sink.send(settings);
        }
        Err(err) => warn!(%err, "configuración inválida tras recarga, se conserva la anterior"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn detects_a_reload_after_the_file_changes() {
        let dir =
            std::env::temp_dir().join(format!("ionconnect-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("crear el directorio temporal no debería fallar");
        let path = dir.join("config.toml");
        std::fs::write(&path, "device_name = \"inicial\"\n").expect("escribir el archivo inicial");

        let watcher = ConfigWatcher::watch(&path).expect("observar el archivo no debería fallar");

        // Dale tiempo al watcher nativo a registrarse antes de escribir.
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(&path, "device_name = \"actualizado\"\n").expect("reescribir el archivo");

        let reloaded = watcher
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("se esperaba una recarga dentro de 5s");
        assert_eq!(reloaded.device_name, "actualizado");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
