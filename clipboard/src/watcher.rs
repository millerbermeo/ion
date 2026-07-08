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
}

impl<P: ClipboardProvider> ClipboardWatcher<P> {
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            guard: LoopGuard::new(),
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
    }

    impl ClipboardProvider for FakeProvider {
        fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
            Ok(self.reads.pop_front().flatten())
        }

        fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
            self.last_write = Some(text.to_string());
            Ok(())
        }
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
