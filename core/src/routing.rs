use std::collections::HashMap;
use std::sync::Mutex;

use ionconnect_protocol::Message;
use ionconnect_shared::DeviceId;
use tokio::sync::mpsc::UnboundedSender;

/// Tabla de a qué canal enviarle un `Message` para que llegue a cada peer
/// conectado. Cada conexión aceptada registra su propio canal al
/// autenticarse y lo retira al desconectarse; el hilo de captura de
/// entrada (que no sabe nada de sockets) solo necesita esto para reenviar
/// eventos al equipo activo.
#[derive(Default)]
pub struct Routing {
    senders: Mutex<HashMap<DeviceId, UnboundedSender<Message>>>,
}

impl Routing {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, device: DeviceId, sender: UnboundedSender<Message>) {
        self.senders
            .lock()
            .expect("el lock de routing no debería estar envenenado")
            .insert(device, sender);
    }

    pub fn unregister(&self, device: DeviceId) {
        self.senders
            .lock()
            .expect("el lock de routing no debería estar envenenado")
            .remove(&device);
    }

    /// `true` si `device` tiene una conexión registrada en este momento —
    /// usado por la sesión de captura para detectar que el peer al que le
    /// cedió el control se desconectó, y así recuperar el control local en
    /// vez de quedarse esperando para siempre a un peer que ya no existe.
    #[must_use]
    pub fn is_connected(&self, device: DeviceId) -> bool {
        self.senders
            .lock()
            .expect("el lock de routing no debería estar envenenado")
            .contains_key(&device)
    }

    /// Intenta enviar `message` a `device`. `false` si no hay ninguna
    /// conexión activa para ese peer en este momento (ya se desconectó, o
    /// todavía no terminó de autenticarse).
    pub fn send_to(&self, device: DeviceId, message: Message) -> bool {
        let senders = self
            .senders
            .lock()
            .expect("el lock de routing no debería estar envenenado");
        senders
            .get(&device)
            .is_some_and(|sender| sender.send(message).is_ok())
    }

    /// Envía `message` a todos los peers conectados — usado para
    /// propagar cambios de portapapeles, que no dependen de cuál pantalla
    /// está activa.
    pub fn broadcast(&self, message: &Message) {
        let senders = self
            .senders
            .lock()
            .expect("el lock de routing no debería estar envenenado");
        for sender in senders.values() {
            let _ = sender.send(message.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use ionconnect_protocol::{Heartbeat, Message};

    use super::*;

    #[test]
    fn send_to_unknown_device_returns_false() {
        let routing = Routing::new();
        assert!(!routing.send_to(
            DeviceId::new(),
            Message::Heartbeat(Heartbeat { sequence: 0 })
        ));
    }

    #[test]
    fn send_to_registered_device_delivers_the_message() {
        let routing = Routing::new();
        let device = DeviceId::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        routing.register(device, tx);

        assert!(routing.send_to(device, Message::Heartbeat(Heartbeat { sequence: 7 })));
        assert_eq!(
            rx.try_recv(),
            Ok(Message::Heartbeat(Heartbeat { sequence: 7 }))
        );
    }

    #[test]
    fn unregister_stops_delivery() {
        let routing = Routing::new();
        let device = DeviceId::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        routing.register(device, tx);
        routing.unregister(device);

        assert!(!routing.send_to(device, Message::Heartbeat(Heartbeat { sequence: 1 })));
    }

    #[test]
    fn broadcast_reaches_every_registered_device() {
        let routing = Routing::new();
        let (tx_a, mut rx_a) = tokio::sync::mpsc::unbounded_channel();
        let (tx_b, mut rx_b) = tokio::sync::mpsc::unbounded_channel();
        routing.register(DeviceId::new(), tx_a);
        routing.register(DeviceId::new(), tx_b);

        routing.broadcast(&Message::Heartbeat(Heartbeat { sequence: 3 }));

        assert_eq!(
            rx_a.try_recv(),
            Ok(Message::Heartbeat(Heartbeat { sequence: 3 }))
        );
        assert_eq!(
            rx_b.try_recv(),
            Ok(Message::Heartbeat(Heartbeat { sequence: 3 }))
        );
    }
}
