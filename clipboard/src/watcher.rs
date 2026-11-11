use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc::Sender;
use tracing::warn;

use crate::error::ClipboardError;
use crate::loop_guard::LoopGuard;
use crate::provider::ClipboardProvider;

/// Sincroniza el portapapeles local con los peers: detecta cambios propios
/// para reenviarlos, y aplica cambios remotos sin generar eco.
///
/// El sondeo periódico (en vez de un evento nativo de "portapapeles
/// cambió") es una excepción deliberada al principio general de "nada de
/// polling" del proyecto — no existe una API de cambio de portapapeles
/// verdaderamente uniforme entre X11/Wayland/Windows sin bindings nativos
/// adicionales por plataforma, y los cambios de portapapeles los origina un
/// humano copiando algo, nunca a una frecuencia donde el costo del sondeo
/// importe.
pub struct ClipboardWatcher<P: ClipboardProvider> {
    provider: P,
    guard: LoopGuard,
    files: FilesGuard,
}

/// Qué pasó con la lista de archivos del portapapeles desde el último sondeo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesChange {
    /// El usuario copió estos archivos/carpetas (rutas locales absolutas).
    Copied(Vec<PathBuf>),
    /// El portapapeles dejó de tener una lista de archivos.
    Cleared,
}

/// Resultado de un sondeo completo — ver [`ClipboardWatcher::poll_all`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipboardChange {
    pub text: Option<String>,
    pub files: Option<FilesChange>,
}

impl ClipboardChange {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.text.is_none() && self.files.is_none()
    }
}

/// Equivalente de [`LoopGuard`] para listas de archivos.
#[derive(Debug, Default)]
struct FilesGuard {
    /// `false` hasta el primer sondeo: lo que ya estaba copiado antes de
    /// arrancar no es una acción nueva del usuario y no se anuncia.
    primed: bool,
    /// Última lista observada (`None` = sin archivos).
    seen: Option<Vec<PathBuf>>,
    /// Última lista que escribimos nosotros a pedido de un peer.
    own: Option<Vec<PathBuf>>,
}

impl FilesGuard {
    fn observe(&mut self, current: Option<Vec<PathBuf>>) -> Option<FilesChange> {
        if !self.primed {
            self.primed = true;
            self.seen = current;
            return None;
        }
        if current == self.seen {
            return None;
        }
        self.seen.clone_from(&current);
        match current {
            // Eco de nuestra propia escritura (p. ej. tras un `Cleared`
            // transitorio mientras cambiaba el dueño de la selección).
            Some(files) if self.own.as_ref() == Some(&files) => None,
            Some(files) => Some(FilesChange::Copied(files)),
            None => Some(FilesChange::Cleared),
        }
    }

    fn record_own_write(&mut self, files: Vec<PathBuf>) {
        self.primed = true;
        self.seen = Some(files.clone());
        self.own = Some(files);
    }
}

impl<P: ClipboardProvider> ClipboardWatcher<P> {
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            guard: LoopGuard::new(),
            files: FilesGuard::default(),
        }
    }

    /// Aplica al portapapeles local un cambio recibido de un peer remoto,
    /// registrándolo para que el próximo sondeo no lo reenvíe como si fuera
    /// nuevo.
    ///
    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si falla la escritura nativa.
    pub fn apply_remote_change(&mut self, text: String) -> Result<(), ClipboardError> {
        self.provider.set_text(&text)?;
        self.guard.record_own_write(text);
        Ok(())
    }

    /// Un solo paso de sondeo: lee el portapapeles y decide si representa
    /// un cambio genuino a reenviar. Separado de `run` para poder probar la
    /// lógica de decisión sin depender de un temporizador async.
    ///
    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si falla la lectura nativa.
    pub fn poll_once(&mut self) -> Result<Option<String>, ClipboardError> {
        Ok(self
            .provider
            .get_text()?
            .and_then(|text| self.guard.observe(text)))
    }

    /// Publica en el portapapeles local los archivos recibidos de un peer
    /// (ya guardados en disco), registrándolos para que el próximo sondeo no
    /// los anuncie como una copia nueva del usuario.
    ///
    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si falla la escritura nativa.
    pub fn apply_remote_files(&mut self, files: Vec<PathBuf>) -> Result<(), ClipboardError> {
        self.provider.set_files(&files)?;
        self.files.record_own_write(files);
        Ok(())
    }

    /// Sondeo completo: archivos primero y, solo si el portapapeles no tiene
    /// archivos, texto. Cuando hay archivos copiados el explorador suele
    /// ofrecer también sus rutas como texto; sincronizar ese texto pisaría
    /// el portapapeles del otro equipo con rutas que allá no existen.
    ///
    /// # Errors
    ///
    /// Devuelve [`ClipboardError`] si falla la lectura nativa.
    pub fn poll_all(&mut self) -> Result<ClipboardChange, ClipboardError> {
        let current = self.provider.get_files()?;
        let has_files = current.is_some();
        let files = self.files.observe(current);
        let text = if has_files { None } else { self.poll_once()? };
        Ok(ClipboardChange { text, files })
    }

    /// Sondea el portapapeles cada `interval` hasta que `sink` se cierre.
    /// Cada cambio local genuino (no un eco de `apply_remote_change`) se
    /// entrega por `sink`.
    pub async fn run(&mut self, interval: Duration, sink: Sender<String>) {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            match self.poll_once() {
                Ok(Some(changed)) => {
                    if sink.send(changed).await.is_err() {
                        break;
                    }
                }
                Ok(None) => {}
                Err(err) => warn!(%err, "error leyendo el portapapeles"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    #[derive(Default)]
    struct FakeProvider {
        reads: VecDeque<Option<String>>,
        last_write: Option<String>,
        /// Lo que `get_files` devuelve ahora mismo.
        files: Option<Vec<PathBuf>>,
    }

    impl ClipboardProvider for FakeProvider {
        fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
            Ok(self.reads.pop_front().flatten())
        }

        fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
            self.last_write = Some(text.to_string());
            Ok(())
        }

        fn get_files(&mut self) -> Result<Option<Vec<PathBuf>>, ClipboardError> {
            Ok(self.files.clone())
        }

        fn set_files(&mut self, files: &[PathBuf]) -> Result<(), ClipboardError> {
            self.files = Some(files.to_vec());
            Ok(())
        }
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn files_already_copied_at_startup_are_not_announced() {
        let provider = FakeProvider {
            files: Some(paths(&["/viejo.txt"])),
            ..FakeProvider::default()
        };
        let mut watcher = ClipboardWatcher::new(provider);
        assert!(watcher.poll_all().unwrap().is_empty());
        assert!(watcher.poll_all().unwrap().is_empty());
    }

    #[test]
    fn copying_files_is_announced_once_and_clearing_too() {
        let mut watcher = ClipboardWatcher::new(FakeProvider::default());
        assert!(watcher.poll_all().unwrap().is_empty()); // sondeo inicial

        watcher.provider.files = Some(paths(&["/a.txt", "/carpeta"]));
        assert_eq!(
            watcher.poll_all().unwrap().files,
            Some(FilesChange::Copied(paths(&["/a.txt", "/carpeta"])))
        );
        assert!(watcher.poll_all().unwrap().is_empty(), "sin cambios, sin anuncio");

        watcher.provider.files = None;
        assert_eq!(watcher.poll_all().unwrap().files, Some(FilesChange::Cleared));
    }

    #[test]
    fn text_is_not_synced_while_the_clipboard_holds_files() {
        let mut watcher = ClipboardWatcher::new(FakeProvider::default());
        assert!(watcher.poll_all().unwrap().is_empty()); // sondeo inicial

        watcher.provider.files = Some(paths(&["/a.txt"]));
        watcher.provider.reads.push_back(Some("/a.txt".to_string()));
        let change = watcher.poll_all().unwrap();
        assert!(change.files.is_some());
        assert_eq!(change.text, None, "las rutas como texto no deben viajar");
        assert_eq!(
            watcher.provider.reads.len(),
            1,
            "con archivos en el portapapeles ni siquiera se lee el texto"
        );
    }

    #[test]
    fn files_applied_from_a_peer_do_not_echo_back() {
        let mut watcher = ClipboardWatcher::new(FakeProvider::default());
        assert!(watcher.poll_all().unwrap().is_empty());

        watcher
            .apply_remote_files(paths(&["/cache/1/a.txt"]))
            .expect("aplicar no debería fallar");
        assert!(watcher.poll_all().unwrap().is_empty());

        // El usuario copia otra cosa acá: eso sí es una novedad.
        watcher.provider.files = Some(paths(&["/local.txt"]));
        assert_eq!(
            watcher.poll_all().unwrap().files,
            Some(FilesChange::Copied(paths(&["/local.txt"])))
        );
    }

    #[test]
    fn poll_once_emits_only_genuine_external_changes() {
        let mut provider = FakeProvider::default();
        provider.reads.push_back(Some("uno".to_string()));
        provider.reads.push_back(Some("uno".to_string())); // repetido, no debería emitirse
        provider.reads.push_back(None); // sin texto en el portapapeles
        provider.reads.push_back(Some("dos".to_string()));

        let mut watcher = ClipboardWatcher::new(provider);
        assert_eq!(watcher.poll_once().unwrap(), Some("uno".to_string()));
        assert_eq!(watcher.poll_once().unwrap(), None);
        assert_eq!(watcher.poll_once().unwrap(), None);
        assert_eq!(watcher.poll_once().unwrap(), Some("dos".to_string()));
    }

    #[test]
    fn applying_a_remote_change_does_not_echo_back() {
        let provider = FakeProvider::default();
        let mut watcher = ClipboardWatcher::new(provider);
        watcher
            .apply_remote_change("del peer".to_string())
            .expect("aplicar no debería fallar");
        assert_eq!(watcher.provider.last_write.as_deref(), Some("del peer"));

        // El próximo sondeo local lee justo lo mismo que acabamos de
        // escribir: no debe considerarse un cambio nuevo.
        watcher
            .provider
            .reads
            .push_back(Some("del peer".to_string()));
        assert_eq!(watcher.poll_once().unwrap(), None);
    }

    #[tokio::test]
    async fn run_delivers_changes_through_the_channel() {
        let mut provider = FakeProvider::default();
        provider.reads.push_back(Some("uno".to_string()));
        provider.reads.push_back(Some("dos".to_string()));

        let mut watcher = ClipboardWatcher::new(provider);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);

        let run = tokio::spawn(async move {
            watcher.run(Duration::from_millis(1), tx).await;
        });

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("no debería tardar más de 5s en emitir el primer cambio"),
            Some("uno".to_string())
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("no debería tardar más de 5s en emitir el segundo cambio"),
            Some("dos".to_string())
        );

        // `run` sondea indefinidamente por diseño (es el bucle de fondo real);
        // una vez verificados los eventos que nos interesan, se aborta la
        // tarea en vez de esperar a que termine sola, porque no lo hace.
        run.abort();
    }
}
