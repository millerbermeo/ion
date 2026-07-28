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
use tokio::sync::{mpsc, watch};
use tokio_rustls::client::TlsStream;
use tracing::{debug, info, warn};

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

    let injector = create_injector().await?;
    let clipboard = Arc::new(AsyncMutex::new(ClipboardWatcher::new(
        ArboardProvider::new().map_err(|e| CoreError::Other(e.to_string()))?,
    )));

    session_loop(&mut conn, injector, &clipboard, &udp_socket, &udp_key).await
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
        // tiene `zones()`), pero casi toda sesión Wayland tiene un
        // `XWayland` detrás — el mismo que usa `create_injector` para
        // XTEST — así que se intenta igual por ahí en vez de rendirse de
        // entrada solo por ver `XDG_SESSION_TYPE=wayland`. Si no hay
        // XWayland (o no hay ningún X11 disponible), `root_geometry`
        // simplemente falla y se cae a `None`, igual que antes.
        return ionconnect_input::x11::X11Control::root_geometry()
            .ok()
            .map(|(width, height)| DisplayGeometry { width, height });
    }
    #[allow(unreachable_code)]
    None
}

/// Inyector ya elegido para esta sesión.
///
/// El backend del portal de Wayland vive en su propia variante en vez de
/// detrás del `dyn InputInjector` sincrónico a propósito: su implementación
/// sincrónica tiene que bloquear un hilo del runtime (`block_in_place`) en
/// **cada** evento para poder esperar la llamada D-Bus. A cientos de eventos
/// por segundo eso no solo cuesta el traspaso de hilo — impide que el bucle
/// de sesión vacíe el socket UDP mientras tanto, así que los movimientos se
/// acumulan y después se reproducen en ráfaga: se percibe como cortes y
/// arrastre. Esperando la llamada de forma async no se bloquea ningún hilo y
/// el bucle sigue atendiendo el resto.
enum ClientInjector {
    /// Backends nativamente sincrónicos y baratos (X11 vía XTEST, Windows
    /// vía `SendInput`): inyectar es una llamada local, no un round trip.
    Sync(Box<dyn InputInjector>),
    #[cfg(all(unix, not(target_os = "macos")))]
    WaylandPortal(Box<ionconnect_input::wayland::WaylandPortalInjector>),
}

impl ClientInjector {
    /// Un fallo puntual de inyección no corta la sesión: se descarta ese
    /// evento (mismo criterio que antes de existir este enum).
    async fn inject(&mut self, event: &CapturedEvent) {
        let result = match self {
            Self::Sync(injector) => injector.inject(event),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::WaylandPortal(injector) => injector.inject_async(event).await,
        };
        if let Err(err) = result {
            debug!(%err, "no se pudo inyectar un evento");
        }
    }
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

    async fn release_all(&mut self, injector: &mut ClientInjector) {
        for keycode in self.keys.drain() {
            warn!(keycode, "liberando tecla que quedó pegada al cortarse la sesión");
            injector
                .inject(&CapturedEvent::Key {
                    keycode,
                    modifiers: KeyModifiers::NONE,
                    pressed: false,
                })
                .await;
        }
        for button in self.buttons.drain() {
            warn!(?button, "liberando botón que quedó pegado al cortarse la sesión");
            injector
                .inject(&CapturedEvent::MouseButton {
                    button,
                    pressed: false,
                })
                .await;
        }
    }
}

/// Cada cuánto se le pregunta al sistema si cambió el portapapeles local.
const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Sondea el portapapeles local y entrega por `sink` cada cambio genuino.
///
/// Corre en su propia tarea, y cada lectura va a un hilo bloqueante: leer el
/// portapapeles es una llamada síncrona que habla con el servidor
/// X/compositor y puede tardar cientos de milisegundos cuando la aplicación
/// que lo posee está ocupada (justo el caso de "muchas cosas abiertas").
/// Antes esto vivía dentro del mismo `select!` que la recepción de red, así
/// que cada una de esas lecturas lentas congelaba también la inyección de
/// mouse y teclado. Ahora el bucle de sesión solo recibe el resultado ya
/// listo por un canal, y `Connection` sigue teniendo un único dueño.
async fn poll_clipboard_changes(
    clipboard: Arc<AsyncMutex<ClipboardWatcher<ArboardProvider>>>,
    sink: mpsc::Sender<String>,
) {
    let mut ticker = tokio::time::interval(CLIPBOARD_POLL_INTERVAL);
    loop {
        ticker.tick().await;
        let clipboard = clipboard.clone();
        let polled =
            tokio::task::spawn_blocking(move || clipboard.blocking_lock().poll_once()).await;
        match polled {
            Ok(Ok(Some(text))) => {
                if sink.send(text).await.is_err() {
                    break;
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(err)) => warn!(%err, "error leyendo el portapapeles"),
            Err(err) => {
                warn!(%err, "la tarea de lectura del portapapeles terminó mal");
                break;
            }
        }
    }
}

/// Sea cual sea el motivo de salida (desconexión limpia o error de red vía
/// `?`), la tarea de inyección libera cualquier tecla/botón que haya quedado
/// a medio presionar — ver [`HeldInput`] — y se detiene el sondeo del
/// portapapeles de esta sesión.
async fn session_loop(
    conn: &mut ClientConnection,
    injector: ClientInjector,
    clipboard: &Arc<AsyncMutex<ClipboardWatcher<ArboardProvider>>>,
    udp_socket: &UdpSocket,
    udp_key: &UdpKey,
) -> Result<(), CoreError> {
    let (clipboard_tx, clipboard_rx) = mpsc::channel(4);
    let poller = tokio::spawn(poll_clipboard_changes(clipboard.clone(), clipboard_tx));

    // La inyección corre en su propia tarea, desacoplada del bucle de red
    // (ver `injection_task` para el porqué). El movimiento va por un `watch`
    // — coalesce solo, únicamente sobrevive la posición más fresca — y lo
    // discreto (teclas, clicks) por una cola acotada que preserva el orden.
    let (moves_tx, moves_rx) = watch::channel(None);
    let (discrete_tx, discrete_rx) = mpsc::channel(128);
    let injection = tokio::spawn(injection_task(injector, moves_rx, discrete_rx));

    let result = session_loop_inner(
        conn,
        &moves_tx,
        &discrete_tx,
        clipboard,
        udp_socket,
        udp_key,
        clipboard_rx,
    )
    .await;

    // Cada reconexión arranca su propio sondeo: sin esto se acumularía uno
    // por sesión, todos leyendo el mismo portapapeles.
    poller.abort();
    // Cerrar los dos canales le avisa a la tarea de inyección que la sesión
    // terminó: drena lo discreto pendiente, libera lo que quedó presionado
    // (`HeldInput::release_all`) y recién ahí termina — por eso se la espera
    // antes de devolver el control al bucle de reconexión.
    drop(moves_tx);
    drop(discrete_tx);
    if let Err(err) = injection.await {
        warn!(%err, "la tarea de inyección terminó mal");
    }
    result
}

/// Inyecta lo que llega por los canales, fuera del bucle de red.
///
/// El porqué de la tarea aparte: en el backend Wayland cada inyección es un
/// round trip D-Bus al portal (milisegundos), y cuando eso se esperaba
/// dentro del mismo `select!` que recibía la red, todo lo demás quedaba
/// detrás — el teclado se sentía trabado porque cada tecla hacía cola detrás
/// de una racha de movimientos, y cada movimiento pagaba su round trip
/// entero antes de mirar el siguiente. Acá el bucle de red nunca espera una
/// inyección: publica y sigue leyendo.
///
/// El orden se cuida en dos frentes:
/// - `biased` hacia la cola discreta: una tecla o click se inyecta apenas
///   llega, sin esperar a que se drene el movimiento (que coalesce solo en
///   el `watch`, así que nunca hay "racha" acumulada que drenar).
/// - Antes de cada evento discreto se aplica la posición más fresca aún no
///   inyectada: un click debe caer donde el cursor está *ahora*, no donde
///   estaba en la última inyección de movimiento.
async fn injection_task(
    mut injector: ClientInjector,
    mut moves: watch::Receiver<Option<(i32, i32)>>,
    mut discrete: mpsc::Receiver<CapturedEvent>,
) {
    let mut held = HeldInput::default();
    // Deduplicación por valor en vez de por la marca "visto" del `watch`:
    // `has_changed()` devuelve `Err` apenas el emisor se cierra aunque quede
    // un valor sin ver, y la marca tampoco distingue "cambió a lo mismo".
    // Como `MouseMove` es posición absoluta, comparar contra la última
    // inyectada es exacto y no depende del estado del canal.
    let mut last_injected: Option<(i32, i32)> = None;
    loop {
        tokio::select! {
            biased;
            event = discrete.recv() => {
                let Some(event) = event else { break };
                // Posición fresca primero — ver el comentario del doc.
                let position = *moves.borrow_and_update();
                inject_move_if_new(&mut injector, position, &mut last_injected).await;
                held.track(&event);
                injector.inject(&event).await;
            }
            changed = moves.changed() => {
                match changed {
                    Ok(()) => {
                        // `borrow` devuelve el más nuevo aunque haya vuelto
                        // a cambiar entre el aviso y esta lectura.
                        let position = *moves.borrow();
                        inject_move_if_new(&mut injector, position, &mut last_injected).await;
                    }
                    // El emisor se cerró: la sesión terminó.
                    Err(_) => break,
                }
            }
        }
    }
    // Drenar los release que hayan quedado encolados antes de liberar a
    // ciegas: así `HeldInput` ve la liberación real y `release_all` no
    // manda un release duplicado.
    while let Ok(event) = discrete.try_recv() {
        held.track(&event);
        injector.inject(&event).await;
    }
    held.release_all(&mut injector).await;
}

/// Inyecta `position` solo si difiere de la última ya inyectada — ver el
/// comentario sobre `last_injected` en [`injection_task`].
async fn inject_move_if_new(
    injector: &mut ClientInjector,
    position: Option<(i32, i32)>,
    last_injected: &mut Option<(i32, i32)>,
) {
    if position.is_none() || position == *last_injected {
        return;
    }
    let Some((x, y)) = position else { return };
    *last_injected = position;
    injector.inject(&CapturedEvent::MouseMove { x, y }).await;
}

/// Descifra un datagrama y devuelve su posición solo si es más nueva que la
/// última aceptada, actualizando `last_seen`. Un datagrama viejo, repetido o
/// inválido se descarta sin cortar nada — es exactamente la tolerancia a
/// pérdida y reordenamiento que justifica usar UDP acá.
fn accept_mouse_move(
    udp_key: &UdpKey,
    datagram: &[u8],
    last_seen: &mut Option<u32>,
) -> Option<(i32, i32)> {
    match open_mouse_move(udp_key, datagram) {
        Ok((seq, x, y)) => {
            if last_seen.is_none_or(|last| is_newer(seq, last)) {
                *last_seen = Some(seq);
                Some((x, y))
            } else {
                None
            }
        }
        Err(err) => {
            warn!(%err, "datagrama UDP inválido, descartado");
            None
        }
    }
}

/// Manda un evento discreto (tecla, click) a la tarea de inyección. Si la
/// cola está llena espera — a tasa humana de tecleo nunca pasa; si pasa es
/// que el backend de inyección está colgado y frenar acá es lo correcto.
/// `Err` = la tarea de inyección murió: se corta la sesión para reconectar
/// con un inyector nuevo en vez de seguir descartando entrada en silencio.
async fn send_discrete(
    discrete_tx: &mpsc::Sender<CapturedEvent>,
    event: CapturedEvent,
) -> Result<(), CoreError> {
    discrete_tx
        .send(event)
        .await
        .map_err(|_| CoreError::Other("la tarea de inyección terminó inesperadamente".to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn session_loop_inner(
    conn: &mut ClientConnection,
    moves_tx: &watch::Sender<Option<(i32, i32)>>,
    discrete_tx: &mpsc::Sender<CapturedEvent>,
    clipboard: &Arc<AsyncMutex<ClipboardWatcher<ArboardProvider>>>,
    udp_socket: &UdpSocket,
    udp_key: &UdpKey,
    mut clipboard_rx: mpsc::Receiver<String>,
) -> Result<(), CoreError> {
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
                        // Camino de respaldo TCP (antes del `UdpHello` del
                        // lado servidor) — mismo `watch` que el UDP: solo
                        // importa la posición más fresca.
                        let _ = moves_tx.send(Some((m.x, m.y)));
                    }
                    Some(Message::MouseClick(c)) => {
                        send_discrete(discrete_tx, CapturedEvent::MouseButton { button: c.button, pressed: c.pressed }).await?;
                    }
                    Some(Message::KeyboardPress(k)) => {
                        send_discrete(discrete_tx, CapturedEvent::Key { keycode: k.keycode, modifiers: k.modifiers, pressed: true }).await?;
                    }
                    Some(Message::KeyboardRelease(k)) => {
                        send_discrete(discrete_tx, CapturedEvent::Key { keycode: k.keycode, modifiers: k.modifiers, pressed: false }).await?;
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
                    Ok((len, _from)) => {
                        let mut freshest = accept_mouse_move(
                            udp_key, &udp_buf[..len], &mut udp_last_seen,
                        );

                        // Vaciar de una lo que ya esté encolado en el socket y
                        // quedarse solo con la posición más nueva: `MouseMove`
                        // lleva posición absoluta, así que las intermedias no
                        // aportan nada y publicarlas una por una solo haría
                        // girar este bucle de más. El `watch` hacia la tarea
                        // de inyección coalesce igual, pero drenar acá evita
                        // despertar el `select!` una vez por datagrama viejo.
                        let mut dropped = 0u32;
                        while let Ok((len, _from)) = udp_socket.try_recv_from(&mut udp_buf) {
                            if let Some(newer) = accept_mouse_move(
                                udp_key, &udp_buf[..len], &mut udp_last_seen,
                            ) {
                                if freshest.is_some() {
                                    dropped += 1;
                                }
                                freshest = Some(newer);
                            }
                        }
                        if dropped > 0 {
                            debug!(dropped, "posiciones intermedias descartadas al vaciar el socket UDP");
                        }

                        if let Some((x, y)) = freshest {
                            let _ = moves_tx.send(Some((x, y)));
                        }
                    }
                    Err(err) => warn!(%err, "error leyendo el socket UDP de MouseMove"),
                }
            }
            // Cambio local del portapapeles ya detectado por la tarea de
            // sondeo (ver `poll_clipboard_changes`) — acá solo queda
            // mandarlo, que es barato.
            Some(text) = clipboard_rx.recv() => {
                conn.send(Message::ClipboardSync(ClipboardSync {
                    mime: ClipboardMime::Text,
                    data: text.into_bytes(),
                })).await?;
            }
        }
    }
}

/// Elige con qué backend inyectar la entrada recibida.
///
/// En Linux se intentan **los dos** backends, empezando por el que
/// corresponde al tipo de sesión: que `XDG_SESSION_TYPE` diga `wayland` no
/// garantiza que el portal `RemoteDesktop` esté disponible (falta
/// `xdg-desktop-portal`, el usuario rechaza el permiso, o el servicio corre
/// sin bus de sesión accesible), y en una sesión Wayland casi siempre hay un
/// `XWayland` al que XTEST sí puede llegar. Al revés vale lo mismo: un
/// servicio de sistema con `XDG_SESSION_TYPE` sin definir no debería
/// quedarse sin inyectar solo porque `DISPLAY` no estaba puesto. Sin esta
/// cadena, un cliente Linux que fallara en su backend preferido terminaba el
/// proceso en vez de conectarse igual.
async fn create_injector() -> Result<ClientInjector, CoreError> {
    #[cfg(windows)]
    {
        return Ok(ClientInjector::Sync(Box::new(
            ionconnect_input::win32::WindowsInjector::new(),
        )));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Escotilla de escape para comparar backends sin recompilar:
        // `IONCONNECT_INJECT_BACKEND=x11` fuerza XTEST incluso en una sesión
        // Wayland. Inyectar por XTEST es una llamada local, mientras que el
        // portal cuesta un round trip D-Bus por evento, así que en
        // compositores que reenvían XTEST desde XWayland al sistema entero
        // (GNOME/mutter, por ejemplo) forzar x11 puede ser bastante más
        // fluido. En los que no lo hacen, la inyección solo llegaría a las
        // aplicaciones X11, y por eso no es el valor por defecto.
        let forced = std::env::var("IONCONNECT_INJECT_BACKEND").ok();
        if let Some(forced) = forced.as_deref() {
            let backend = match forced {
                "x11" => Some(Backend::X11),
                "wayland" => Some(Backend::Wayland),
                other => {
                    warn!(backend = other, "IONCONNECT_INJECT_BACKEND desconocido, se ignora");
                    None
                }
            };
            if let Some(backend) = backend {
                info!(backend = backend.name(), "backend de inyección forzado por entorno");
                return connect_backend(backend).await;
            }
        }

        // GNOME/mutter reenvía la entrada XTEST de XWayland al sistema
        // entero, así que ahí XTEST inyecta igual de completo que el portal
        // pero con una llamada local en vez de un round trip D-Bus por
        // evento — diferencia enorme de fluidez a cientos de eventos por
        // segundo. En GNOME se prefiere x11 aun en sesión Wayland; si
        // XWayland no está, `connect_backend` falla y la cadena cae al
        // portal igual que siempre. Otros compositores no garantizan ese
        // reenvío, por eso la preferencia es solo para GNOME.
        let desktop_relays_xtest = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| {
            desktop
                .split(':')
                .any(|part| part.eq_ignore_ascii_case("gnome"))
        });

        let prefers_wayland = !desktop_relays_xtest
            && (std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland")
                || (std::env::var_os("WAYLAND_DISPLAY").is_some()
                    && std::env::var_os("DISPLAY").is_none()));

        let (first, second) = if prefers_wayland {
            (Backend::Wayland, Backend::X11)
        } else {
            (Backend::X11, Backend::Wayland)
        };

        let first_error = match connect_backend(first).await {
            Ok(injector) => {
                info!(backend = first.name(), "backend de inyección listo");
                return Ok(injector);
            }
            Err(err) => err,
        };
        warn!(
            backend = first.name(),
            %first_error,
            fallback = second.name(),
            "el backend de inyección preferido no está disponible, probando el otro"
        );
        return match connect_backend(second).await {
            Ok(injector) => {
                info!(backend = second.name(), "backend de inyección listo");
                Ok(injector)
            }
            Err(second_error) => Err(CoreError::Other(format!(
                "ningún backend de inyección disponible en este equipo — {}: {first_error} / {}: {second_error}",
                first.name(),
                second.name()
            ))),
        };
    }
    #[allow(unreachable_code)]
    {
        warn!("sistema operativo sin backend de inyección conocido");
        Err(CoreError::Other(
            "sistema operativo no soportado".to_string(),
        ))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
#[derive(Debug, Clone, Copy)]
enum Backend {
    X11,
    Wayland,
}

#[cfg(all(unix, not(target_os = "macos")))]
impl Backend {
    const fn name(self) -> &'static str {
        match self {
            Self::X11 => "x11",
            Self::Wayland => "wayland",
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
async fn connect_backend(backend: Backend) -> Result<ClientInjector, CoreError> {
    match backend {
        Backend::X11 => ionconnect_input::x11::X11Injector::connect()
            .map(|injector| ClientInjector::Sync(Box::new(injector)))
            .map_err(CoreError::Input),
        Backend::Wayland => ionconnect_input::wayland::WaylandPortalInjector::connect()
            .await
            .map(|injector| ClientInjector::WaylandPortal(Box::new(injector)))
            .map_err(CoreError::Input),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealed(key: &UdpKey, seq: u32, x: i32, y: i32) -> Vec<u8> {
        ionconnect_network::seal_mouse_move(key, seq, x, y)
    }

    #[test]
    fn accepts_the_first_datagram_and_then_only_newer_ones() {
        let key = UdpKey::new(&[7u8; 32]).expect("32 bytes es el largo correcto");
        let mut last_seen = None;

        assert_eq!(
            accept_mouse_move(&key, &sealed(&key, 0, 10, 20), &mut last_seen),
            Some((10, 20)),
            "el primero siempre se acepta"
        );
        assert_eq!(
            accept_mouse_move(&key, &sealed(&key, 1, 11, 21), &mut last_seen),
            Some((11, 21))
        );
        // Reordenado: llega uno viejo después del nuevo.
        assert_eq!(
            accept_mouse_move(&key, &sealed(&key, 0, 99, 99), &mut last_seen),
            None,
            "un datagrama viejo no debería moverse el cursor hacia atrás"
        );
        // Repetido exacto del último aceptado.
        assert_eq!(
            accept_mouse_move(&key, &sealed(&key, 1, 11, 21), &mut last_seen),
            None
        );
    }

    #[test]
    fn a_corrupt_datagram_is_discarded_without_touching_the_freshness_state() {
        let key = UdpKey::new(&[7u8; 32]).expect("32 bytes es el largo correcto");
        let mut last_seen = None;
        assert_eq!(
            accept_mouse_move(&key, &sealed(&key, 5, 1, 2), &mut last_seen),
            Some((1, 2))
        );

        assert_eq!(accept_mouse_move(&key, b"basura", &mut last_seen), None);
        // El estado quedó intacto: el siguiente legítimo sigue entrando.
        assert_eq!(
            accept_mouse_move(&key, &sealed(&key, 6, 3, 4), &mut last_seen),
            Some((3, 4))
        );
    }

    /// Registra lo inyectado en un `Vec` compartido: `ClientInjector` toma
    /// el inyector por `Box`, así que el test necesita seguir viendo lo que
    /// pasó desde afuera.
    #[derive(Clone, Default)]
    struct RecordingInjector {
        injected: Arc<std::sync::Mutex<Vec<CapturedEvent>>>,
    }

    impl RecordingInjector {
        fn events(&self) -> Vec<CapturedEvent> {
            self.injected.lock().expect("lock sano").clone()
        }
    }

    impl InputInjector for RecordingInjector {
        fn inject(&mut self, event: &CapturedEvent) -> Result<(), ionconnect_input::InputError> {
            self.injected.lock().expect("lock sano").push(*event);
            Ok(())
        }
    }

    fn recording() -> (RecordingInjector, ClientInjector) {
        let recorder = RecordingInjector::default();
        let injector = ClientInjector::Sync(Box::new(recorder.clone()));
        (recorder, injector)
    }

    #[tokio::test]
    async fn release_all_is_a_noop_when_nothing_is_held() {
        let mut held = HeldInput::default();
        let (recorder, mut injector) = recording();
        held.release_all(&mut injector).await;
        assert!(recorder.events().is_empty());
    }

    #[tokio::test]
    async fn tracks_and_releases_a_key_left_pressed() {
        let mut held = HeldInput::default();
        held.track(&CapturedEvent::Key {
            keycode: 30,
            modifiers: KeyModifiers::NONE,
            pressed: true,
        });

        let (recorder, mut injector) = recording();
        held.release_all(&mut injector).await;

        assert_eq!(
            recorder.events(),
            vec![CapturedEvent::Key {
                keycode: 30,
                modifiers: KeyModifiers::NONE,
                pressed: false,
            }]
        );
        // Ya se liberó — un segundo `release_all` no debería mandar nada de nuevo.
        let (recorder, mut injector) = recording();
        held.release_all(&mut injector).await;
        assert!(recorder.events().is_empty());
    }

    #[tokio::test]
    async fn a_matching_release_clears_the_held_key_before_disconnect() {
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

        let (recorder, mut injector) = recording();
        held.release_all(&mut injector).await;
        assert!(recorder.events().is_empty());
    }

    /// Todo lo encolado antes de arrancar la tarea se procesa en un orden
    /// determinista (el `select!` es `biased` hacia lo discreto), así que se
    /// puede afirmar la secuencia exacta: la posición más fresca coalescida
    /// se inyecta antes del click, y las intermedias nunca se inyectan.
    #[tokio::test]
    async fn injection_task_applies_the_freshest_move_before_a_click() {
        let (moves_tx, moves_rx) = watch::channel(None);
        let (discrete_tx, discrete_rx) = mpsc::channel(8);

        moves_tx.send(Some((1, 1))).expect("receiver vivo");
        moves_tx.send(Some((5, 7))).expect("receiver vivo");
        discrete_tx
            .send(CapturedEvent::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            })
            .await
            .expect("receiver vivo");
        discrete_tx
            .send(CapturedEvent::MouseButton {
                button: MouseButton::Left,
                pressed: false,
            })
            .await
            .expect("receiver vivo");
        drop(moves_tx);
        drop(discrete_tx);

        let (recorder, injector) = recording();
        tokio::spawn(injection_task(injector, moves_rx, discrete_rx))
            .await
            .expect("la tarea no debería entrar en panic");

        assert_eq!(
            recorder.events(),
            vec![
                // La (1, 1) intermedia coalesció: nunca se inyecta.
                CapturedEvent::MouseMove { x: 5, y: 7 },
                CapturedEvent::MouseButton {
                    button: MouseButton::Left,
                    pressed: true,
                },
                CapturedEvent::MouseButton {
                    button: MouseButton::Left,
                    pressed: false,
                },
                // El release ya llegó por la cola — `release_all` no duplica.
            ]
        );
    }

    /// Si la sesión se corta con una tecla a medio presionar, la propia
    /// tarea la libera al terminar (antes esto vivía en `session_loop`).
    #[tokio::test]
    async fn injection_task_releases_held_keys_on_shutdown() {
        let (moves_tx, moves_rx) = watch::channel(None);
        let (discrete_tx, discrete_rx) = mpsc::channel(8);

        discrete_tx
            .send(CapturedEvent::Key {
                keycode: 30,
                modifiers: KeyModifiers::NONE,
                pressed: true,
            })
            .await
            .expect("receiver vivo");
        drop(moves_tx);
        drop(discrete_tx);

        let (recorder, injector) = recording();
        tokio::spawn(injection_task(injector, moves_rx, discrete_rx))
            .await
            .expect("la tarea no debería entrar en panic");

        assert_eq!(
            recorder.events(),
            vec![
                CapturedEvent::Key {
                    keycode: 30,
                    modifiers: KeyModifiers::NONE,
                    pressed: true,
                },
                CapturedEvent::Key {
                    keycode: 30,
                    modifiers: KeyModifiers::NONE,
                    pressed: false,
                },
            ]
        );
    }

    #[tokio::test]
    async fn tracks_and_releases_a_held_mouse_button() {
        let mut held = HeldInput::default();
        held.track(&CapturedEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        });

        let (recorder, mut injector) = recording();
        held.release_all(&mut injector).await;

        assert_eq!(
            recorder.events(),
            vec![CapturedEvent::MouseButton {
                button: MouseButton::Left,
                pressed: false,
            }]
        );
    }
}
