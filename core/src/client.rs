use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ionconnect_clipboard::{ArboardProvider, ClipboardWatcher};
use ionconnect_config::Settings;
use ionconnect_input::{CapturedEvent, InputInjector};
use ionconnect_network::{BackoffPolicy, Connection, connect_tls, connect_with_backoff};
use ionconnect_protocol::{Authentication, ClipboardMime, ClipboardSync, Message, MouseButton};
use ionconnect_shared::{DeviceId, KeyModifiers};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio_rustls::client::TlsStream;
use tracing::{info, warn};

use crate::error::CoreError;
use crate::identity::local_device_id;

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

    conn.send(&Message::Authentication(Authentication {
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

    let mut injector = create_injector().await?;
    let clipboard = Arc::new(AsyncMutex::new(ClipboardWatcher::new(
        ArboardProvider::new().map_err(|e| CoreError::Other(e.to_string()))?,
    )));

    session_loop(&mut conn, injector.as_mut(), &clipboard).await
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
) -> Result<(), CoreError> {
    let mut held = HeldInput::default();
    let result = session_loop_inner(conn, injector, clipboard, &mut held).await;
    held.release_all(injector);
    result
}

async fn session_loop_inner(
    conn: &mut ClientConnection,
    injector: &mut dyn InputInjector,
    clipboard: &Arc<AsyncMutex<ClipboardWatcher<ArboardProvider>>>,
    held: &mut HeldInput,
) -> Result<(), CoreError> {
    let mut clipboard_ticker = tokio::time::interval(Duration::from_millis(500));
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
            _ = clipboard_ticker.tick() => {
                let changed = clipboard.lock().await.poll_once();
                if let Ok(Some(text)) = changed {
                    conn.send(&Message::ClipboardSync(ClipboardSync {
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
