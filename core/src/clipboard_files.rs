//! Coordinación, del lado servidor, de los archivos que el usuario copia
//! (Ctrl+C) o arrastra de un equipo a otro.
//!
//! Los bytes nunca viajan al copiar: viajan cuando el cursor cruza al equipo
//! donde se van a pegar. Copiar es frecuentísimo y casi nunca termina en un
//! pegado del otro lado; cruzar el borde con algo copiado sí lo anticipa, y
//! el cruce le da a la transferencia el tiempo que el usuario tarda en
//! llegar a la ventana destino y apretar Ctrl+V.
//!
//! El servidor es el único que ve los cruces (es quien captura la entrada),
//! así que orquesta las dos direcciones:
//!
//! - **Servidor → cliente:** al ceder el control a un peer le manda lo que
//!   haya copiado localmente y ese peer todavía no tenga.
//! - **Cliente → servidor:** el cliente solo anuncia que copió algo
//!   (`ClipboardFilesOffer`); cuando el control vuelve de ese peer, el
//!   servidor se lo pide (`ClipboardFilesRequest`).
//! - **Cliente → otro cliente:** lo que llega de un cliente queda disponible
//!   para el próximo peer al que se le ceda el control.
//!
//! [`ClipboardFiles`] es solo el estado y sus transiciones — sin red, sin
//! disco — para poder probar esas reglas aisladas. [`run_control_events`] lo
//! conecta con la sesión de captura y con el envío real.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ionconnect_protocol::{ClipboardFilesRequest, FileBatchKind, Message};
use ionconnect_shared::DeviceId;
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::file_transfer::{FileSink, deliver_paths};
use crate::routing::Routing;

/// Cambios de dueño del control que le importan a la transferencia de
/// archivos. Los emite la sesión de captura (`input_session`), que corre en
/// su propio hilo y no puede hacer I/O de red ni de disco.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    /// El control pasó de este equipo a `device`.
    Entered(DeviceId),
    /// El control volvió de `device` a este equipo.
    Returned(DeviceId),
    /// El usuario cruzó a `device` arrastrando estos archivos.
    Dragged { device: DeviceId, paths: Vec<PathBuf> },
}

/// Extremo emisor de [`ControlEvent`], para la sesión de captura.
pub type ControlEvents = mpsc::UnboundedSender<ControlEvent>;

/// Archivos disponibles para pegar en otros equipos.
#[derive(Debug)]
struct Available {
    /// Cambia en cada copia nueva: distingue "ya te lo mandé" de "copiaron
    /// otra cosa desde entonces".
    generation: u64,
    paths: Vec<PathBuf>,
}

#[derive(Debug, Default)]
struct State {
    available: Option<Available>,
    next_generation: u64,
    /// Última generación que cada peer ya tiene (se la mandamos, o es quien
    /// la originó).
    delivered: HashMap<DeviceId, u64>,
    /// Lo que cada cliente anunció haber copiado y todavía no le pedimos.
    offered: HashMap<DeviceId, u64>,
}

impl State {
    fn set_available(&mut self, paths: Vec<PathBuf>) -> u64 {
        self.next_generation += 1;
        self.available = Some(Available {
            generation: self.next_generation,
            paths,
        });
        self.next_generation
    }
}

/// Estado compartido entre el sondeo del portapapeles, las conexiones de los
/// peers y el consumidor de [`ControlEvent`].
#[derive(Debug, Default)]
pub struct ClipboardFiles {
    state: Mutex<State>,
}

impl ClipboardFiles {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("el lock de clipboard_files no debería estar envenenado")
    }

    /// El usuario copió `paths` en este equipo.
    pub fn local_copied(&self, paths: Vec<PathBuf>) {
        self.lock().set_available(paths);
    }

    /// El portapapeles de este equipo ya no tiene archivos (se copió texto,
    /// o un peer lo pisó con el suyo): no queda nada para ofrecer.
    pub fn local_cleared(&self) {
        self.lock().available = None;
    }

    /// `device` anunció que copió archivos (`set_id != 0`) o que ya no los
    /// tiene (`set_id == 0`).
    pub fn remote_offered(&self, device: DeviceId, set_id: u64) {
        let mut state = self.lock();
        if set_id == 0 {
            state.offered.remove(&device);
        } else {
            state.offered.insert(device, set_id);
        }
    }

    /// Terminaron de llegar los archivos copiados en `device` y ya están en
    /// el portapapeles local: quedan disponibles para los demás peers, y
    /// `device` no los necesita de vuelta.
    pub fn received_from(&self, device: DeviceId, paths: Vec<PathBuf>) {
        let mut state = self.lock();
        let generation = state.set_available(paths);
        state.delivered.insert(device, generation);
    }

    /// El control pasa a `device`: devuelve los archivos que hay que
    /// mandarle para que pueda pegarlos, o `None` si no hay nada copiado o
    /// ya los tiene. Los marca como entregados.
    #[must_use]
    pub fn take_for_entering(&self, device: DeviceId) -> Option<Vec<PathBuf>> {
        let mut state = self.lock();
        let (generation, paths) = {
            let available = state.available.as_ref()?;
            (available.generation, available.paths.clone())
        };
        if state.delivered.get(&device) == Some(&generation) {
            return None;
        }
        state.delivered.insert(device, generation);
        Some(paths)
    }

    /// El control vuelve de `device` (o `device` anunció algo sin tener el
    /// control): devuelve el `set_id` que hay que pedirle, una sola vez.
    #[must_use]
    pub fn take_offer(&self, device: DeviceId) -> Option<u64> {
        self.lock().offered.remove(&device)
    }

    /// `device` se desconectó: su caché de portapapeles muere con la sesión,
    /// así que al reconectar hay que volver a mandarle lo que haga falta.
    pub fn forget(&self, device: DeviceId) {
        let mut state = self.lock();
        state.delivered.remove(&device);
        state.offered.remove(&device);
    }
}

/// Consume los [`ControlEvent`] de la sesión de captura y dispara las
/// transferencias que correspondan. Corre hasta que la sesión de captura
/// termina (se cierra el canal).
pub async fn run_control_events(
    mut events: mpsc::UnboundedReceiver<ControlEvent>,
    files: Arc<ClipboardFiles>,
    routing: Arc<Routing>,
) {
    while let Some(event) = events.recv().await {
        match event {
            ControlEvent::Entered(device) => {
                let Some(paths) = files.take_for_entering(device) else {
                    continue;
                };
                info!(%device, count = paths.len(), "enviando archivos copiados al equipo que recibe el control");
                spawn_delivery(paths, FileBatchKind::Clipboard, routing.clone(), device);
            }
            ControlEvent::Returned(device) => request_offered(&files, &routing, device),
            ControlEvent::Dragged { device, paths } => {
                info!(%device, count = paths.len(), "enviando archivos arrastrados al otro equipo");
                spawn_delivery(paths, FileBatchKind::Downloads, routing.clone(), device);
            }
        }
    }
}

/// Le pide a `device` los archivos que anunció haber copiado, si hay alguno
/// pendiente.
pub fn request_offered(files: &ClipboardFiles, routing: &Routing, device: DeviceId) {
    let Some(set_id) = files.take_offer(device) else {
        return;
    };
    debug!(%device, set_id, "pidiendo los archivos copiados en el peer");
    routing.send_to(
        device,
        Message::ClipboardFilesRequest(ClipboardFilesRequest { set_id }),
    );
}

fn spawn_delivery(
    paths: Vec<PathBuf>,
    kind: FileBatchKind,
    routing: Arc<Routing>,
    device: DeviceId,
) {
    tokio::spawn(async move {
        deliver_paths(paths, Some(kind), &FileSink::Peer(routing, device)).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn nothing_is_sent_when_nothing_was_copied() {
        let files = ClipboardFiles::new();
        assert_eq!(files.take_for_entering(DeviceId::new()), None);
    }

    #[test]
    fn copied_files_are_sent_once_per_peer_until_something_new_is_copied() {
        let files = ClipboardFiles::new();
        let (a, b) = (DeviceId::new(), DeviceId::new());

        files.local_copied(paths(&["/uno.txt"]));
        assert_eq!(files.take_for_entering(a), Some(paths(&["/uno.txt"])));
        assert_eq!(files.take_for_entering(a), None, "volver a cruzar no reenvía");
        assert_eq!(
            files.take_for_entering(b),
            Some(paths(&["/uno.txt"])),
            "cada peer lleva su propia cuenta"
        );

        files.local_copied(paths(&["/dos.txt"]));
        assert_eq!(files.take_for_entering(a), Some(paths(&["/dos.txt"])));
    }

    #[test]
    fn copying_the_same_files_again_resends_them() {
        // El usuario pudo haber borrado lo pegado, o copiado texto en el
        // medio del otro lado: un Ctrl+C nuevo es una intención nueva.
        let files = ClipboardFiles::new();
        let peer = DeviceId::new();
        files.local_copied(paths(&["/uno.txt"]));
        assert!(files.take_for_entering(peer).is_some());
        files.local_copied(paths(&["/uno.txt"]));
        assert!(files.take_for_entering(peer).is_some());
    }

    #[test]
    fn clearing_the_clipboard_withdraws_the_files() {
        let files = ClipboardFiles::new();
        files.local_copied(paths(&["/uno.txt"]));
        files.local_cleared();
        assert_eq!(files.take_for_entering(DeviceId::new()), None);
    }

    #[test]
    fn a_peer_offer_is_requested_exactly_once() {
        let files = ClipboardFiles::new();
        let peer = DeviceId::new();
        files.remote_offered(peer, 7);
        assert_eq!(files.take_offer(peer), Some(7));
        assert_eq!(files.take_offer(peer), None);
    }

    #[test]
    fn a_newer_offer_replaces_the_previous_and_zero_withdraws_it() {
        let files = ClipboardFiles::new();
        let peer = DeviceId::new();
        files.remote_offered(peer, 7);
        files.remote_offered(peer, 8);
        assert_eq!(files.take_offer(peer), Some(8));

        files.remote_offered(peer, 9);
        files.remote_offered(peer, 0);
        assert_eq!(files.take_offer(peer), None);
    }

    #[test]
    fn files_received_from_a_peer_are_relayed_to_the_others_but_not_back() {
        let files = ClipboardFiles::new();
        let (origin, other) = (DeviceId::new(), DeviceId::new());
        files.received_from(origin, paths(&["/cache/1/foto.png"]));

        assert_eq!(files.take_for_entering(origin), None, "no se devuelven al origen");
        assert_eq!(
            files.take_for_entering(other),
            Some(paths(&["/cache/1/foto.png"]))
        );
    }

    #[test]
    fn a_reconnected_peer_gets_the_files_again() {
        let files = ClipboardFiles::new();
        let peer = DeviceId::new();
        files.local_copied(paths(&["/uno.txt"]));
        assert!(files.take_for_entering(peer).is_some());

        files.remote_offered(peer, 3);
        files.forget(peer);
        assert!(files.take_for_entering(peer).is_some(), "su caché murió con la sesión");
        assert_eq!(files.take_offer(peer), None, "su oferta vieja ya no vale");
    }
}
