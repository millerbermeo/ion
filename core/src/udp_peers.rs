use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use ionconnect_network::{UdpKey, seal_mouse_move};
use ionconnect_shared::DeviceId;
use tokio::net::UdpSocket;
use tracing::warn;

/// Etiqueta de dominio para `export_keying_material` (RFC 5705) — deriva,
/// sin handshake ni RTT extra, la clave simétrica que cifra los datagramas
/// UDP de `MouseMove` a partir de la sesión TLS ya autenticada (TOFU). Fija
/// e igual en `core::server` y `core::client`; el mecanismo en sí vive en
/// `network::udp_codec`.
pub(crate) const UDP_KEY_LABEL: &[u8] = b"ionconnect-udp-mousemove-v1";

/// A qué dirección UDP y con qué clave mandarle los `MouseMove` continuos a
/// cada peer — transporte aparte de [`crate::routing::Routing`] (TCP+TLS,
/// confiable) a propósito: mezclar dos transportes en una sola tabla
/// ensuciaría una estructura que hoy es deliberadamente homogénea (un solo
/// tipo de canal por `DeviceId`). Vive en `core` y no en `network` porque
/// necesita `DeviceId` y la decisión de negocio de "solo el `MouseMove`
/// continuo va por acá, todo lo demás sigue por TCP" — `network` se
/// mantiene agnóstico de esa política.
///
/// Un solo `UdpSocket` compartido alcanza para todos los peers: UDP no
/// tiene conexión, cada envío elige la dirección de destino con
/// `send_to`/`try_send_to`.
pub struct UdpPeers {
    socket: Arc<UdpSocket>,
    peers: Mutex<HashMap<DeviceId, UdpPeer>>,
}

struct UdpPeer {
    addr: SocketAddr,
    key: Arc<UdpKey>,
    /// Contador de secuencia del *emisor* — arranca en 0 en cada sesión
    /// (nunca sobrevive a una reconexión, ver `register`), independiente
    /// del que lleva el receptor para descartar paquetes viejos.
    seq: AtomicU32,
}

impl UdpPeers {
    #[must_use]
    pub fn new(socket: Arc<UdpSocket>) -> Self {
        Self {
            socket,
            peers: Mutex::new(HashMap::new()),
        }
    }

    /// Registra (o reemplaza) el destino UDP de `device` — se llama al
    /// recibir su `Message::UdpHello`. Reemplazar de golpe en vez de
    /// acumular es intencional: cubre tanto el primer registro como una
    /// reconexión (nueva sesión TLS ⇒ nueva clave, nuevo contador de
    /// secuencia arrancando de 0 — no tendría sentido mezclarlo con el de
    /// una sesión anterior).
    pub fn register(&self, device: DeviceId, addr: SocketAddr, key: Arc<UdpKey>) {
        self.peers.lock().expect("el lock de udp_peers no debería estar envenenado").insert(
            device,
            UdpPeer {
                addr,
                key,
                seq: AtomicU32::new(0),
            },
        );
    }

    /// Se llama junto a `Routing::unregister` cuando el peer se desconecta
    /// — sin esto, una reconexión posterior podría convivir un instante con
    /// la entrada vieja mientras el `UdpHello` nuevo todavía no llegó.
    pub fn unregister(&self, device: DeviceId) {
        self.peers
            .lock()
            .expect("el lock de udp_peers no debería estar envenenado")
            .remove(&device);
    }

    /// Intenta mandar `(x, y)` por UDP a `device`. Devuelve `false` si no
    /// hay ningún peer UDP registrado para ese `device` (todavía no llegó
    /// su `UdpHello`, el cliente es una versión vieja que no lo manda, o ya
    /// se desconectó) — quien llama tiene que caer a TCP en ese caso, ver
    /// `input_session`. Un error de envío puntual (p. ej. `WouldBlock` bajo
    /// ráfaga) no cuenta como "sin peer": se descarta ese delta nada más,
    /// el próximo lo reemplaza — es exactamente la tolerancia a pérdida que
    /// justifica usar UDP acá.
    #[must_use]
    pub fn try_send_mouse_move(&self, device: DeviceId, x: i32, y: i32) -> bool {
        let peers = self
            .peers
            .lock()
            .expect("el lock de udp_peers no debería estar envenenado");
        let Some(peer) = peers.get(&device) else {
            return false;
        };
        let seq = peer.seq.fetch_add(1, Ordering::Relaxed);
        let datagram = seal_mouse_move(&peer.key, seq, x, y);
        if let Err(err) = self.socket.try_send_to(&datagram, peer.addr)
            && err.kind() != std::io::ErrorKind::WouldBlock
        {
            warn!(%err, %device, "no se pudo mandar MouseMove por UDP");
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn key() -> Arc<UdpKey> {
        Arc::new(UdpKey::new(&[9u8; 32]).expect("32 bytes es el largo correcto"))
    }

    #[tokio::test]
    async fn try_send_returns_false_without_a_registered_peer() {
        let socket = Arc::new(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .expect("bind debería funcionar"),
        );
        let peers = UdpPeers::new(socket);
        assert!(!peers.try_send_mouse_move(DeviceId::new(), 1, 2));
    }

    #[tokio::test]
    async fn registered_peer_receives_a_decryptable_datagram() {
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind debería funcionar");
        let receiver_addr = receiver.local_addr().expect("dirección local esperada");

        let sender_socket = Arc::new(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .expect("bind debería funcionar"),
        );
        // Un socket recién creado no tiene cacheado el estado "writable" de
        // tokio hasta que el reactor lo confirma — en el runtime
        // multi-hilo de producción (`Runtime::new()`, ver `core::main`) un
        // hilo de fondo lo resuelve solo; acá, con el runtime de un solo
        // hilo que usa `#[tokio::test]` por default, hace falta este
        // `.await` explícito antes de mandar de verdad. `Arc::clone` apunta
        // al mismo socket subyacente que va a usar `UdpPeers`, así que
        // "entibiarlo" acá alcanza.
        sender_socket
            .writable()
            .await
            .expect("el socket debería quedar listo para escribir");

        let peers = UdpPeers::new(sender_socket);
        let device = DeviceId::new();
        let key = key();
        peers.register(device, receiver_addr, key.clone());

        assert!(peers.try_send_mouse_move(device, 42, -7));

        let mut buf = [0u8; 64];
        let (len, _) = receiver
            .recv_from(&mut buf)
            .await
            .expect("debería recibir el datagrama");
        let (seq, x, y) = ionconnect_network::open_mouse_move(&key, &buf[..len])
            .expect("debería descifrar con la misma clave");
        assert_eq!((seq, x, y), (0, 42, -7));
    }

    #[tokio::test]
    async fn unregister_makes_try_send_fall_back_to_false() {
        let socket = Arc::new(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .expect("bind debería funcionar"),
        );
        let peers = UdpPeers::new(socket);
        let device = DeviceId::new();
        peers.register(device, "127.0.0.1:9".parse().unwrap(), key());
        assert!(peers.try_send_mouse_move(device, 1, 1));

        peers.unregister(device);
        assert!(!peers.try_send_mouse_move(device, 1, 1));
    }
}
