use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ionconnect_clipboard::{ArboardProvider, ClipboardWatcher};
use ionconnect_config::Settings;
use ionconnect_crypto::PairingMode;
use ionconnect_network::{Discovery, accept_tls};
use ionconnect_protocol::{Authentication, ClipboardMime, ClipboardSync, Message};
use ionconnect_screen::{Layout, MonitorGeometry, VirtualDesktop};
use ionconnect_shared::DeviceId;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::error::CoreError;
use crate::identity::local_device_id;
use crate::peer_id;
use crate::routing::Routing;
use crate::trust_store::FileTrustStore;

/// Geometría de pantalla asumida para cada peer cuando no hay un mecanismo
/// de intercambio de geometría real (fase futura): la misma resolución que
/// la pantalla local. Los cruces de borde siguen funcionando correctamente
/// como máquina de estados; lo único que puede no ser exacto es el punto
/// preciso de reingreso si el equipo remoto tiene una resolución distinta.
fn peer_desktop(local_geometry: MonitorGeometry) -> VirtualDesktop {
    VirtualDesktop::new(vec![local_geometry])
}

/// Corre este equipo como servidor: el que tiene el mouse/teclado físico.
/// Acepta conexiones de los equipos configurados en `settings.peers`,
/// autentica cada una, y expone un [`Routing`] + [`Layout`] para que el
/// llamador conecte la sesión de captura de entrada (específica de SO,
/// ver `input_session`).
///
/// # Errors
///
/// Devuelve [`CoreError`] si falla el bind del listener, la identidad TLS,
/// o el trust store.
pub async fn run_server(
    settings: Settings,
    config_dir: &Path,
    local_geometry: MonitorGeometry,
) -> Result<(), CoreError> {
    let identity = crate::identity::load_or_generate_identity(config_dir)?;
    let local_device = local_device_id(&identity);
    info!(device_id = %local_device, "identidad local cargada");

    let trust_store = Arc::new(FileTrustStore::load(
        config_dir.join("trusted_fingerprints"),
    )?);
    let pairing_mode = to_crypto_pairing_mode(settings.pairing_mode);

    let mut layout = Layout::new();
    layout.set_desktop(local_device, peer_desktop(local_geometry));
    let mut known_peers: HashMap<DeviceId, String> = HashMap::new();
    for peer in &settings.peers {
        let Some(peer_device) = peer_id::from_hex(&peer.device_id) else {
            warn!(device_id = %peer.device_id, "device_id de peer mal formado, se ignora");
            continue;
        };
        layout.set_desktop(peer_device, peer_desktop(local_geometry));
        layout.link_mirrored(local_device, peer.edge, peer_device);
        known_peers.insert(peer_device, peer.name.clone());
    }

    let server_config = ionconnect_crypto::server_config(&identity, trust_store, pairing_mode)?;
    let listener = TcpListener::bind(("0.0.0.0", settings.listen_port)).await?;
    info!(
        port = settings.listen_port,
        "escuchando conexiones de peers"
    );

    // Se mantiene viva mientras dure el servidor: al soltarla, `mdns-sd`
    // deja de anunciar el servicio.
    let _discovery = if settings.discovery_enabled {
        let discovery = Discovery::new()?;
        discovery.advertise(
            &settings.device_name,
            &peer_id::to_hex(local_device),
            settings.listen_port,
        )?;
        Some(discovery)
    } else {
        None
    };

    let routing = Arc::new(Routing::new());
    let clipboard = Arc::new(AsyncMutex::new(ClipboardWatcher::new(
        ArboardProvider::new().map_err(|e| CoreError::Other(e.to_string()))?,
    )));

    tokio::spawn(broadcast_local_clipboard_changes(
        clipboard.clone(),
        routing.clone(),
    ));

    let handoff = Arc::new(std::sync::Mutex::new(crate::handoff::HandoffState::new(
        layout,
        local_device,
    )));
    spawn_input_session(handoff, routing.clone());

    let known_peers = Arc::new(known_peers);
    loop {
        let (tcp, addr) = listener.accept().await?;
        let server_config = server_config.clone();
        let known_peers = known_peers.clone();
        let routing = routing.clone();
        let clipboard = clipboard.clone();
        let device_name = settings.device_name.clone();

        tokio::spawn(async move {
            let result = handle_peer_connection(
                tcp,
                server_config,
                local_device,
                device_name,
                known_peers,
                routing,
                clipboard,
            )
            .await;
            if let Err(err) = result {
                warn!(%addr, %err, "conexión de peer terminó con error");
            }
        });
    }
}

/// La sesión de captura real solo existe en X11 (ver `input_session`, que a
/// su vez depende de `ionconnect_input::x11`, gateado a
/// `cfg(all(unix, not(target_os = "macos")))`). En cualquier otra
/// plataforma, elegir `Role::Server` deja el servicio escuchando y
/// autenticando peers con normalidad, pero sin nada que capturar ni
/// reenviar — se loguea una sola vez en vez de fallar la compilación o el
/// arranque entero.
#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_input_session(
    handoff: Arc<std::sync::Mutex<crate::handoff::HandoffState>>,
    routing: Arc<Routing>,
) {
    tokio::task::spawn_blocking(move || {
        if let Err(err) = crate::input_session::run_x11_input_session(&handoff, &routing) {
            tracing::error!(%err, "la sesión de captura de entrada terminó con error");
        }
    });
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn spawn_input_session(
    _handoff: Arc<std::sync::Mutex<crate::handoff::HandoffState>>,
    _routing: Arc<Routing>,
) {
    tracing::error!(
        "el rol Server todavía no está soportado en este sistema operativo (solo Linux/X11 por ahora, ver README) — este equipo no va a capturar ni reenviar entrada"
    );
}

#[allow(clippy::too_many_arguments)]
async fn handle_peer_connection(
    tcp: TcpStream,
    server_config: Arc<rustls::ServerConfig>,
    local_device: DeviceId,
    device_name: String,
    known_peers: Arc<HashMap<DeviceId, String>>,
    routing: Arc<Routing>,
    clipboard: Arc<AsyncMutex<ClipboardWatcher<ArboardProvider>>>,
) -> Result<(), CoreError> {
    let mut conn = accept_tls(tcp, server_config).await?;

    let Some(Message::Authentication(auth)) = conn.recv().await? else {
        return Err(CoreError::Other(
            "se esperaba Authentication como primer mensaje".to_string(),
        ));
    };
    if !known_peers.contains_key(&auth.device_id) {
        return Err(CoreError::Other(format!(
            "peer no configurado: {}",
            auth.device_id
        )));
    }

    conn.send(&Message::Authentication(Authentication {
        device_id: local_device,
        device_name,
        protocol_version: 1,
        cert_fingerprint: [0u8; 32],
    }))
    .await?;

    info!(device_id = %auth.device_id, name = %auth.device_name, "peer autenticado");

    let (tx, mut rx) = mpsc::unbounded_channel();
    routing.register(auth.device_id, tx);

    loop {
        tokio::select! {
            incoming = conn.recv() => {
                match incoming? {
                    Some(Message::ClipboardSync(sync)) => {
                        if let Ok(text) = String::from_utf8(sync.data) {
                            let mut guard = clipboard.lock().await;
                            let _ = guard.apply_remote_change(text);
                        }
                    }
                    Some(Message::Disconnect(_)) | None => break,
                    _ => {}
                }
            }
            Some(outgoing) = rx.recv() => {
                conn.send(&outgoing).await?;
            }
        }
    }

    routing.unregister(auth.device_id);
    info!(device_id = %auth.device_id, "peer desconectado");
    Ok(())
}

async fn broadcast_local_clipboard_changes(
    clipboard: Arc<AsyncMutex<ClipboardWatcher<ArboardProvider>>>,
    routing: Arc<Routing>,
) {
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    loop {
        ticker.tick().await;
        let changed = clipboard.lock().await.poll_once();
        if let Ok(Some(text)) = changed {
            routing.broadcast(&Message::ClipboardSync(ClipboardSync {
                mime: ClipboardMime::Text,
                data: text.into_bytes(),
            }));
        }
    }
}

pub(crate) const fn to_crypto_pairing_mode(
    preference: ionconnect_config::PairingModePreference,
) -> PairingMode {
    match preference {
        ionconnect_config::PairingModePreference::AutoTrustOnFirstUse => {
            PairingMode::AutoTrustOnFirstUse
        }
        ionconnect_config::PairingModePreference::RejectUnknown => PairingMode::RejectUnknown,
    }
}

#[cfg(test)]
mod tests {
    use ionconnect_crypto::{Identity, InMemoryTrustStore, TrustStore};
    use ionconnect_protocol::MouseMove;
    use tokio::net::TcpListener;

    use super::*;

    /// Ejercita `handle_peer_connection` de punta a punta sobre un
    /// loopback TCP+TLS real: handshake de autenticación, registro en
    /// `Routing`, y reenvío efectivo de un `MouseMove` a través de la
    /// conexión ya autenticada. No pasa por `input_session` (eso necesita
    /// un servidor X real, ver `input` crate) — lo que se prueba acá es la
    /// plomería de red/enrutamiento que sí es nueva de este crate.
    #[tokio::test]
    async fn authenticated_peer_receives_routed_mouse_move() {
        let server_identity = Identity::generate().expect("generar no debería fallar");
        let client_identity = Identity::generate().expect("generar no debería fallar");
        let server_device = local_device_id(&server_identity);
        let client_device = local_device_id(&client_identity);

        let server_trust = Arc::new(InMemoryTrustStore::new());
        server_trust.trust(client_identity.fingerprint());
        let client_trust = Arc::new(InMemoryTrustStore::new());
        client_trust.trust(server_identity.fingerprint());

        let server_config = ionconnect_crypto::server_config(
            &server_identity,
            server_trust,
            PairingMode::RejectUnknown,
        )
        .expect("configuración de servidor válida");
        let client_config = ionconnect_crypto::client_config(
            &client_identity,
            client_trust,
            PairingMode::RejectUnknown,
        )
        .expect("configuración de cliente válida");

        let mut known_peers = HashMap::new();
        known_peers.insert(client_device, "cliente-de-prueba".to_string());
        let routing = Arc::new(Routing::new());
        let clipboard = Arc::new(AsyncMutex::new(ClipboardWatcher::new(
            ArboardProvider::new().expect("abrir el portapapeles no debería fallar"),
        )));

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind no debería fallar");
        let addr = listener.local_addr().expect("dirección local esperada");

        let server_task = tokio::spawn({
            let routing = routing.clone();
            async move {
                let (tcp, _) = listener.accept().await.expect("accept no debería fallar");
                handle_peer_connection(
                    tcp,
                    server_config,
                    server_device,
                    "servidor-de-prueba".to_string(),
                    Arc::new(known_peers),
                    routing,
                    clipboard,
                )
                .await
            }
        });

        let mut client_conn = ionconnect_network::connect_tls(addr, client_config)
            .await
            .expect("el handshake TLS del cliente debería completarse");
        client_conn
            .send(&Message::Authentication(Authentication {
                device_id: client_device,
                device_name: "cliente-de-prueba".to_string(),
                protocol_version: 1,
                cert_fingerprint: [0u8; 32],
            }))
            .await
            .expect("enviar Authentication no debería fallar");
        let Some(Message::Authentication(_)) =
            client_conn.recv().await.expect("recv no debería fallar")
        else {
            panic!("se esperaba Authentication del servidor");
        };

        // Simula lo que haría `input_session` tras un hand-off: enrutar un
        // MouseMove al peer recién autenticado.
        assert!(routing.send_to(client_device, Message::MouseMove(MouseMove { x: 42, y: 7 })));

        let received = client_conn
            .recv()
            .await
            .expect("recv no debería fallar")
            .expect("se esperaba un mensaje reenviado");
        assert_eq!(received, Message::MouseMove(MouseMove { x: 42, y: 7 }));

        drop(client_conn);
        let _ = server_task.await;
    }
}
