use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ionconnect_clipboard::{ArboardProvider, ClipboardWatcher};
use ionconnect_config::Settings;
use ionconnect_input::{CapturedEvent, InputInjector};
use ionconnect_network::{
    BackoffPolicy, Connection, UdpKey, connect_tls, connect_with_backoff, is_newer,
    open_mouse_move,
};
use ionconnect_protocol::{
    Authentication, ClipboardMime, ClipboardSync, DisplayGeometry, Message, MouseButton, UdpHello,
};
use ionconnect_shared::{DeviceId, KeyModifiers};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Mutex as AsyncMutex;
use tokio_rustls::client::TlsStream;
use tracing::{info, warn};

use crate::error::CoreError;
use crate::identity::local_device_id;
use crate::udp_peers::UDP_KEY_LABEL;

type ClientConnection = Connection<TlsStream<TcpStream>>;

/// Corre este equipo como cliente: recibe entrada de un `Server` y la
/// inyecta localmente. Se reconecta indefinidamente con backoff
/// exponencial — pensado para correr como servicio de fondo de larga
/// duración, no para una sola sesión.
///
/// # Errors
///
/// Devuelve [`CoreError`] si la configuración de identidad/criptografía
/// falla al inicio (errores de conexión individuales se reintentan
/// internamente, no se propagan).
pub async fn run_client(settings: Settings, config_dir: &Path) -> Result<(), CoreError> {
    let identity = crate::identity::load_or_generate_identity(config_dir)?;
    let local_device = local_device_id(&identity);
    info!(device_id = %local_device, "identidad local cargada");

    let trust_store = Arc::new(crate::trust_store::FileTrustStore::load(
        config_dir.join("trusted_fingerprints"),
    )?);
    let pairing_mode = crate::server::to_crypto_pairing_mode(settings.pairing_mode);
    let client_config = ionconnect_crypto::client_config(&identity, trust_store, pairing_mode)?;

    let address: SocketAddr = settings
        .server_address
        .as_deref()
        .ok_or_else(|| {
            CoreError::Other(
                "se necesita server_address (el descubrimiento automático todavía no elige uno)"
                    .to_string(),
            )
        })?
        .parse()
        .map_err(|e| CoreError::Other(format!("server_address inválida: {e}")))?;

    connect_with_backoff(BackoffPolicy::default(), || {
        run_single_session(&settings, client_config.clone(), local_device, address)
    })
    .await;

    Ok(())
}

async fn run_single_session(
    settings: &Settings,
    client_config: Arc<rustls::ClientConfig>,
    local_device: DeviceId,
    address: SocketAddr,
) -> Result<(), CoreError> {
    let mut conn = connect_tls(address, client_config).await?;

    conn.send(Message::Authentication(Authentication {
        device_id: local_device,
        device_name: settings.device_name.clone(),
        protocol_version: 1,
        cert_fingerprint: [0u8; 32],
    }))
    .await?;
    let Some(Message::Authentication(server_auth)) = conn.recv().await? else {
        return Err(CoreError::Other(
            "el servidor no respondió con Authentication".to_string(),
        ));
    };
    info!(server = %server_auth.device_name, "conectado al servidor");

    if let Some(geometry) = local_display_geometry() {
        info!(
            width = geometry.width,
            height = geometry.height,
            "reportando resolución real al servidor"
        );
        conn.send(Message::DisplayGeometry(geometry)).await?;
    } else {
        warn!(
            "no se pudo detectar la resolución real de este equipo — el servidor va a asumir la suya propia"
        );
    }

    // Socket efímero para recibir los `MouseMove` continuos por UDP (ver
    // `core::udp_peers`) — se deriva una clave nueva por sesión a partir de
    // la conexión TLS ya autenticada, y se le avisa al servidor a qué
    // puerto mandar. Se re-arma en cada reconexión a propósito: `seq`
    // arranca de 0 de los dos lados, así que mezclarlo con el estado de una
    // sesión anterior rompería la comprobación de frescura.
    let udp_socket = UdpSocket::bind(("0.0.0.0", 0)).await?;
    let udp_port = udp_socket.local_addr()?.port();
    let udp_key = {
        let mut key_bytes = [0u8; 32];
        conn.get_ref()
            .get_ref()
            .1
            .export_keying_material(&mut key_bytes, UDP_KEY_LABEL, None)
            .map_err(|e| CoreError::Other(format!("no se pudo derivar la clave UDP: {e}")))?;
        UdpKey::new(&key_bytes)?
    };
    conn.send(Message::UdpHello(UdpHello { port: udp_port }))
        .await?;
    info!(port = udp_port, "puerto UDP anunciado al servidor");

    let mut injector = create_injector().await?;
    let clipboard = Arc::new(AsyncMutex::new(ClipboardWatcher::new(
        ArboardProvider::new().map_err(|e| CoreError::Other(e.to_string()))?,
    )));

    session_loop(&mut conn, injector.as_mut(), &clipboard, &udp_socket, &udp_key).await
}

/// Resolución real del escritorio virtual de este equipo, para que el
/// servidor calcule bien dónde reaparece el cursor en vez de asumir que
/// el cliente tiene la misma resolución que él (ver
/// `ionconnect_screen`/`crate::handoff::HandoffState::clamp_to_active_desktop`
/// para el porqué importa). `None` si no se pudo detectar — el servidor
/// sigue funcionando igual que antes de que existiera este mensaje.
fn local_display_geometry() -> Option<DisplayGeometry> {
    #[cfg(windows)]
    {
        let (_, _, width, height) = ionconnect_input::win32::virtual_screen_geometry();
        if width <= 0 || height <= 0 {
            return None;
        }
        #[allow(clippy::cast_sign_loss)]
        return Some(DisplayGeometry {
            width: width as u32,
            height: height as u32,
        });
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // El portal `RemoteDesktop` que usa el cliente Wayland no expone
        // todavía una forma barata de consultar la geometría real (a
        // diferencia del portal `InputCapture` del lado servidor, que sí
        // tiene `zones()`) — se deja para cuando haga falta en vez de
        // pedir un permiso extra solo para esto.
        let is_wayland = std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland");
        if is_wayland {
            return None;
        }
        return ionconnect_input::x11::X11Control::root_geometry()
            .ok()
            .map(|(width, height)| DisplayGeometry { width, height });
    }
    #[allow(unreachable_code)]
    None
}

/// Teclas y botones que este equipo inyectó como presionados y todavía no
/// vio su liberación correspondiente. Si la sesión termina (conexión
/// cortada, servidor que se cayó con el control activo, error de red) a
/// mitad de una tecla u botón mantenido, sin esto se queda "pegado" a
/// nivel de sistema operativo — p. ej. un modificador atascado hace que
/// todo lo que se escriba después parezca no responder. `session_loop`
/// garantiza [`Self::release_all`] al salir, pase lo que pase.
#[derive(Default)]
struct HeldInput {
    keys: HashSet<u32>,
    buttons: HashSet<MouseButton>,
}

impl HeldInput {
    fn track(&mut self, event: &CapturedEvent) {
        match *event {
            CapturedEvent::Key {
                keycode,
                pressed: true,
                ..
            } => {
                self.keys.insert(keycode);
            }
            CapturedEvent::Key {
                keycode,
                pressed: false,
                ..
            } => {
                self.keys.remove(&keycode);
            }
            CapturedEvent::MouseButton {
                button,
                pressed: true,
            } => {
                self.buttons.insert(button);
            }
            CapturedEvent::MouseButton {
                button,
                pressed: false,
            } => {
                self.buttons.remove(&button);
            }
            CapturedEvent::MouseMove { .. } | CapturedEvent::AbsolutePosition { .. } => {}
        }
    }

    fn release_all(&mut self, injector: &mut dyn InputInjector) {
        for keycode in self.keys.drain() {
            warn!(keycode, "liberando tecla que quedó pegada al cortarse la sesión");
            let _ = injector.inject(&CapturedEvent::Key {
                keycode,
                modifiers: KeyModifiers::NONE,
                pressed: false,
            });
        }
        for button in self.buttons.drain() {
            warn!(?button, "liberando botón que quedó pegado al cortarse la sesión");
            let _ = injector.inject(&CapturedEvent::MouseButton {
                button,
                pressed: false,
            });
        }
    }
}

/// El sondeo de portapapeles vive en el mismo `select!` que la recepción de
/// red (en vez de una tarea aparte) porque ambos necesitan `conn`/`clipboard`
/// a la vez: una tarea separada obligaría a repartir el `Connection` entre
/// dos dueños, y `Connection` no está pensado para eso.
///
/// Sea cual sea el motivo de salida (desconexión limpia o error de red vía
/// `?`), libera cualquier tecla/botón que haya quedado a medio presionar —
/// ver [`HeldInput`].
async fn session_loop(
    conn: &mut ClientConnection,
    injector: &mut dyn InputInjector,
    clipboard: &Arc<AsyncMutex<ClipboardWatcher<ArboardProvider>>>,
    udp_socket: &UdpSocket,
    udp_key: &UdpKey,
) -> Result<(), CoreError> {
    let mut held = HeldInput::default();
    let result = session_loop_inner(conn, injector, clipboard, &mut held, udp_socket, udp_key).await;
    held.release_all(injector);
    result
}

#[allow(clippy::too_many_arguments)]
async fn session_loop_inner(
    conn: &mut ClientConnection,
    injector: &mut dyn InputInjector,
    clipboard: &Arc<AsyncMutex<ClipboardWatcher<ArboardProvider>>>,
    held: &mut HeldInput,
    udp_socket: &UdpSocket,
    udp_key: &UdpKey,
) -> Result<(), CoreError> {
    let mut clipboard_ticker = tokio::time::interval(Duration::from_millis(500));
    // `None` = todavía no se aceptó ningún `MouseMove` por UDP en esta
    // sesión — el primero siempre se acepta, después se exige que la
    // secuencia sea más nueva (ver `ionconnect_network::is_newer`).
    let mut udp_last_seen: Option<u32> = None;
    let mut udp_buf = [0u8; 64];
    loop {
        tokio::select! {
            incoming = conn.recv() => {
                match incoming? {
                    Some(Message::MouseMove(m)) => {
                        let _ = injector.inject(&CapturedEvent::MouseMove { x: m.x, y: m.y });
                    }
                    Some(Message::MouseClick(c)) => {
                        let event = CapturedEvent::MouseButton { button: c.button, pressed: c.pressed };
                        held.track(&event);
                        let _ = injector.inject(&event);
                    }
                    Some(Message::KeyboardPress(k)) => {
                        let event = CapturedEvent::Key { keycode: k.keycode, modifiers: k.modifiers, pressed: true };
                        held.track(&event);
                        let _ = injector.inject(&event);
                    }
                    Some(Message::KeyboardRelease(k)) => {
                        let event = CapturedEvent::Key { keycode: k.keycode, modifiers: k.modifiers, pressed: false };
                        held.track(&event);
                        let _ = injector.inject(&event);
                    }
                    Some(Message::ClipboardSync(sync)) => {
                        if let Ok(text) = String::from_utf8(sync.data) {
                            let mut guard = clipboard.lock().await;
                            let _ = guard.apply_remote_change(text);
                        }
                    }
                    Some(Message::Disconnect(_)) | None => return Ok(()),
                    _ => {}
                }
            }
            // Deltas continuos de MouseMove (ver `core::input_session` del
            // lado servidor) — pérdida/desorden es tolerable acá, por eso
            // van por UDP en vez de por `conn`. Un datagrama inválido o
            // viejo se descarta con un log, nunca corta la sesión (a
            // diferencia de `conn.recv()?` arriba, que si falla sí es
            // fatal para la conexión confiable).
            result = udp_socket.recv_from(&mut udp_buf) => {
                match result {
                    Ok((len, _from)) => match open_mouse_move(udp_key, &udp_buf[..len]) {
                        Ok((seq, x, y)) => {
                            let accept = udp_last_seen.is_none_or(|last| is_newer(seq, last));
                            if accept {
                                udp_last_seen = Some(seq);
                                let _ = injector.inject(&CapturedEvent::MouseMove { x, y });
                            }
                        }
                        Err(err) => warn!(%err, "datagrama UDP inválido, descartado"),
                    },
                    Err(err) => warn!(%err, "error leyendo el socket UDP de MouseMove"),
                }
            }
            _ = clipboard_ticker.tick() => {
                let changed = clipboard.lock().await.poll_once();
                if let Ok(Some(text)) = changed {
                    conn.send(Message::ClipboardSync(ClipboardSync {
                        mime: ClipboardMime::Text,
                        data: text.into_bytes(),
                    })).await?;
                }
            }
        }
    }
}

async fn create_injector() -> Result<Box<dyn InputInjector>, CoreError> {
    #[cfg(windows)]
    {
        return Ok(Box::new(ionconnect_input::win32::WindowsInjector::new()));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let is_wayland = std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland");
        if is_wayland {
            let injector = ionconnect_input::wayland::WaylandPortalInjector::connect()
                .await
                .map_err(CoreError::Input)?;
            return Ok(Box::new(injector));
        }
        let injector = ionconnect_input::x11::X11Injector::connect().map_err(CoreError::Input)?;
        return Ok(Box::new(injector));
    }
    #[allow(unreachable_code)]
    {
        warn!("sistema operativo sin backend de inyección conocido");
        Err(CoreError::Other(
            "sistema operativo no soportado".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingInjector {
        injected: Vec<CapturedEvent>,
    }

    impl InputInjector for RecordingInjector {
        fn inject(&mut self, event: &CapturedEvent) -> Result<(), ionconnect_input::InputError> {
            self.injected.push(*event);
            Ok(())
        }
    }

    #[test]
    fn release_all_is_a_noop_when_nothing_is_held() {
        let mut held = HeldInput::default();
        let mut injector = RecordingInjector::default();
        held.release_all(&mut injector);
        assert!(injector.injected.is_empty());
    }

    #[test]
    fn tracks_and_releases_a_key_left_pressed() {
        let mut held = HeldInput::default();
        held.track(&CapturedEvent::Key {
            keycode: 30,
            modifiers: KeyModifiers::NONE,
            pressed: true,
        });

        let mut injector = RecordingInjector::default();
        held.release_all(&mut injector);

        assert_eq!(
            injector.injected,
            vec![CapturedEvent::Key {
                keycode: 30,
                modifiers: KeyModifiers::NONE,
                pressed: false,
            }]
        );
        // Ya se liberó — un segundo `release_all` no debería mandar nada de nuevo.
        let mut injector = RecordingInjector::default();
        held.release_all(&mut injector);
        assert!(injector.injected.is_empty());
    }

    #[test]
    fn a_matching_release_clears_the_held_key_before_disconnect() {
        let mut held = HeldInput::default();
        held.track(&CapturedEvent::Key {
            keycode: 30,
            modifiers: KeyModifiers::NONE,
            pressed: true,
        });
        held.track(&CapturedEvent::Key {
            keycode: 30,
            modifiers: KeyModifiers::NONE,
            pressed: false,
        });

        let mut injector = RecordingInjector::default();
        held.release_all(&mut injector);
        assert!(injector.injected.is_empty());
    }

    #[test]
    fn tracks_and_releases_a_held_mouse_button() {
        let mut held = HeldInput::default();
        held.track(&CapturedEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        });

        let mut injector = RecordingInjector::default();
        held.release_all(&mut injector);

        assert_eq!(
            injector.injected,
            vec![CapturedEvent::MouseButton {
                button: MouseButton::Left,
                pressed: false,
            }]
        );
    }
}
