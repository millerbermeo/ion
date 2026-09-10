use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ionconnect_input::wayland::{BarrierSpec, WaylandCaptureEvent, WaylandCaptureSession};
use ionconnect_input::x11::{KeyRepeatSettings, SharedPosition, X11Capture, X11Control};
use ionconnect_input::{CapturedEvent, InputCapture as _, InputError};
use ionconnect_protocol::{
    KeyboardPress, KeyboardRelease, Message, MouseButton, MouseClick, MouseMove,
};
use ionconnect_shared::{DeviceId, KeyModifiers};
use tracing::{debug, info, warn};

use crate::handoff::{Active, HandoffAction, HandoffState};
use crate::key_repeat::KeyRepeater;
use crate::routing::Routing;
use crate::udp_peers::UdpPeers;

/// Intervalo mínimo entre dos `MouseMove` mandados al mismo peer (~250 Hz).
///
/// Un mouse gamer reporta hasta 1000 posiciones por segundo, y reenviar cada
/// una es contraproducente: satura el enlace justo cuando el `WiFi` anda mal
/// (que es cuando más se nota), sin que el usuario pueda percibir la
/// diferencia contra 250 Hz — muy por encima de los 60-144 Hz a los que
/// refresca la pantalla del equipo remoto. Los reportes que caen dentro del
/// intervalo no se tiran: se guardan como pendientes y se mandan en el
/// próximo hueco, así la posición final de un movimiento siempre llega
/// (ver [`SessionState::flush_pending_move`]).
const MOUSE_SEND_INTERVAL: Duration = Duration::from_millis(4);

/// Cada cuánto se comprueba si el peer que tiene el control sigue conectado.
/// Antes se hacía en cada evento capturado — hasta 1000 veces por segundo
/// para algo que solo cambia cuando alguien se desconecta.
const PEER_LIVENESS_CHECK_INTERVAL: Duration = Duration::from_millis(200);

/// Plazo con el que se arma la rama de repetición del `select!` de Wayland
/// cuando no hay ninguna tecla mantenida ni posición pendiente: lo bastante
/// lejano como para no dispararse nunca en la práctica, y así no tener que
/// construir el `select!` de dos formas distintas.
const IDLE_WAKEUP: Duration = Duration::from_hours(1);

/// Estado mutable de una sesión de captura: deduplicación, repetición de
/// teclas y limitación de la tasa de `MouseMove`. Agrupado en un struct para
/// no arrastrar media docena de parámetros sueltos por cada función del
/// camino caliente.
struct SessionState {
    held: HeldGuard,
    /// Teclas/botones que este servidor efectivamente reenvió como
    /// presionados al peer que tiene el control ahora — ver [`ForwardedHeld`].
    forwarded: ForwardedHeld,
    repeater: KeyRepeater,
    /// Última posición que quedó sin mandar por el límite de tasa.
    pending_move: Option<(DeviceId, i32, i32)>,
    last_move_sent: Instant,
    last_liveness_check: Instant,
}

impl SessionState {
    fn new(settings: KeyRepeatSettings) -> Self {
        // `Instant::now()` menos un intervalo: así el primer movimiento de
        // la sesión sale enseguida en vez de esperar el primer hueco. Si el
        // reloj monótono todavía no llegó a ese valor (arranque muy
        // temprano de la máquina), `now` sirve igual: solo demora el primer
        // envío unos milisegundos.
        let now = Instant::now();
        let past = now.checked_sub(MOUSE_SEND_INTERVAL).unwrap_or(now);
        Self {
            held: HeldGuard::default(),
            forwarded: ForwardedHeld::default(),
            repeater: KeyRepeater::new(settings.delay, settings.interval, move |keycode| {
                settings.repeats(keycode)
            }),
            pending_move: None,
            last_move_sent: past,
            last_liveness_check: past,
        }
    }

    /// Manda `(x, y)` al peer activo respetando [`MOUSE_SEND_INTERVAL`], o
    /// lo deja pendiente para el próximo hueco.
    fn send_move(
        &mut self,
        device: DeviceId,
        x: i32,
        y: i32,
        routing: &Routing,
        udp_peers: &UdpPeers,
    ) {
        let now = Instant::now();
        if now.duration_since(self.last_move_sent) < MOUSE_SEND_INTERVAL {
            self.pending_move = Some((device, x, y));
            return;
        }
        self.pending_move = None;
        self.last_move_sent = now;
        send_mouse_move(device, x, y, routing, udp_peers);
    }

    /// Manda la posición que haya quedado pendiente si ya pasó el intervalo.
    fn flush_pending_move(&mut self, routing: &Routing, udp_peers: &UdpPeers) {
        let Some((device, x, y)) = self.pending_move else {
            return;
        };
        let now = Instant::now();
        if now.duration_since(self.last_move_sent) < MOUSE_SEND_INTERVAL {
            return;
        }
        self.pending_move = None;
        self.last_move_sent = now;
        send_mouse_move(device, x, y, routing, udp_peers);
    }

    /// Descarta el estado que solo tiene sentido mientras el control está
    /// cedido a un remoto — llamar en cada cambio de dueño del control.
    fn on_control_changed(&mut self) {
        self.repeater.clear();
        self.pending_move = None;
    }

    /// Reenvía al peer que acaba de perder el control la liberación de todo
    /// lo que le habíamos mandado como presionado y todavía no se soltó — si
    /// no, queda pegado a nivel SO del cliente (un Ctrl/Shift atascado hace
    /// que todo click/scroll/tecla siguiente se interprete como atajo). Ver
    /// [`ForwardedHeld`].
    fn release_forwarded_to(&mut self, device: DeviceId, routing: &Routing) {
        let releases = self.forwarded.drain_releases();
        if releases.is_empty() {
            return;
        }
        debug!(
            %device,
            count = releases.len(),
            "liberando teclas/botones mantenidos al ceder el control"
        );
        for message in releases {
            routing.send_to(device, message);
        }
    }

    /// Cuánto se puede dormir esperando el próximo evento capturado antes de
    /// tener trabajo propio que hacer (repetir una tecla o mandar la
    /// posición pendiente). `None` = no hay nada agendado.
    fn next_wakeup(&self) -> Option<Duration> {
        let now = Instant::now();
        let repeat = self
            .repeater
            .next_deadline()
            .map(|deadline| deadline.saturating_duration_since(now));
        let pending = self.pending_move.map(|_| {
            (self.last_move_sent + MOUSE_SEND_INTERVAL).saturating_duration_since(now)
        });
        match (repeat, pending) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (only, None) | (None, only) => only,
        }
    }
}

/// Manda un `MouseMove` continuo: por UDP si el peer ya se registró (tolera
/// perderse, el próximo lo reemplaza — ver `core::udp_peers`), cayendo a la
/// conexión TCP confiable si no.
fn send_mouse_move(
    device: DeviceId,
    x: i32,
    y: i32,
    routing: &Routing,
    udp_peers: &UdpPeers,
) {
    if !udp_peers.try_send_mouse_move(device, x, y) {
        routing.send_to(device, Message::MouseMove(MouseMove { x, y }));
    }
}

/// Filtra pulsaciones/clics duplicados que la captura del sistema operativo
/// pudiera reportar para una sola acción física real.
///
/// La duplicación que había en X11 ya no ocurre — venía de seleccionar los
/// eventos crudos sobre "todos los dispositivos" en vez de solo los
/// maestros, y se corrigió en el origen (ver
/// `ionconnect_input::x11::X11Capture`). Esto queda como red de seguridad
/// independiente de plataforma: no depende de la semántica fina de ningún
/// backend, así que sigue cubriendo a Wayland y a cualquier backend futuro.
///
/// Ojo con las repeticiones de tecla: **no** llegan por acá. El sistema no
/// las entrega por el camino crudo, se sintetizan aparte (ver
/// `crate::key_repeat`), así que este filtro nunca las ve y no puede
/// tragárselas por parecer "una pulsación repetida sin soltar".
#[derive(Default)]
struct HeldGuard {
    keys: std::collections::HashSet<u32>,
    buttons: std::collections::HashSet<MouseButton>,
}

impl HeldGuard {
    /// `true` si `event` es una transición de estado nueva y hay que
    /// procesarla; `false` si es un duplicado exacto del último reporte
    /// para esa tecla/botón (un `pressed: true` repetido sin soltar antes,
    /// o un `pressed: false` repetido sin haber estado presionado) y hay
    /// que descartarlo en silencio. Siempre `true` para eventos que no son
    /// de tecla/botón.
    fn accept(&mut self, event: &CapturedEvent) -> bool {
        match *event {
            CapturedEvent::Key {
                keycode,
                pressed: true,
                ..
            } => self.keys.insert(keycode),
            CapturedEvent::Key {
                keycode,
                pressed: false,
                ..
            } => self.keys.remove(&keycode),
            CapturedEvent::MouseButton {
                button,
                pressed: true,
            } => self.buttons.insert(button),
            CapturedEvent::MouseButton {
                button,
                pressed: false,
            } => self.buttons.remove(&button),
            CapturedEvent::AbsolutePosition { .. } | CapturedEvent::MouseMove { .. } => true,
        }
    }
}

/// Teclas y botones que este servidor ya reenvió al peer activo como
/// `pressed: true` y todavía no reenvió su liberación.
///
/// El cliente solo libera lo que quedó a medio presionar cuando la sesión
/// entera termina ([`crate::client`] `HeldInput::release_all`), **no** en un
/// hand-off. Así que si el control vuelve a local (cruce de borde de
/// regreso) con un modificador o un botón todavía mantenido, sin esto queda
/// pegado en el cliente hasta reconectar: un Ctrl/Shift atascado convierte
/// cada click en "abrir en ventana nueva", cada scroll en zoom y cada tecla
/// en un atajo. En cada cambio de dueño del control se drena este conjunto
/// mandándole al peer saliente el `pressed: false` que le falta.
///
/// Las variantes de scroll no se rastrean: el emisor las manda como un
/// par press+release instantáneo y el inyector ignora el `false`, así que un
/// release suelto no aporta nada y hasta podría interpretarse como una
/// muesca extra.
#[derive(Default)]
struct ForwardedHeld {
    keys: std::collections::HashSet<u32>,
    buttons: std::collections::HashSet<MouseButton>,
}

impl ForwardedHeld {
    fn track_forwarded(&mut self, event: &CapturedEvent) {
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
            } if !is_scroll(button) => {
                self.buttons.insert(button);
            }
            CapturedEvent::MouseButton {
                button,
                pressed: false,
            } => {
                self.buttons.remove(&button);
            }
            _ => {}
        }
    }

    /// Vacía el conjunto y devuelve el `pressed: false` que le falta a cada
    /// entrada. Las coordenadas del click de release no importan (el cliente
    /// ya tiene el cursor donde toca), van en cero.
    fn drain_releases(&mut self) -> Vec<Message> {
        let mut out = Vec::with_capacity(self.keys.len() + self.buttons.len());
        for keycode in self.keys.drain() {
            out.push(Message::KeyboardRelease(KeyboardRelease {
                keycode,
                modifiers: KeyModifiers::NONE,
            }));
        }
        for button in self.buttons.drain() {
            out.push(Message::MouseClick(MouseClick {
                button,
                pressed: false,
                x: 0,
                y: 0,
            }));
        }
        out
    }

    fn clear(&mut self) {
        self.keys.clear();
        self.buttons.clear();
    }
}

const fn is_scroll(button: MouseButton) -> bool {
    matches!(
        button,
        MouseButton::ScrollUp
            | MouseButton::ScrollDown
            | MouseButton::ScrollLeft
            | MouseButton::ScrollRight
    )
}

/// Corre la sesión de captura de entrada X11 en el hilo actual —
/// **bloqueante**: llamar desde `tokio::task::spawn_blocking`, nunca
/// directamente dentro de una tarea async (igual que
/// [`ionconnect_input::InputCapture::run`], del que depende).
///
/// Alimenta [`HandoffState`] con cada posición reportada; cuando eso
/// dispara un hand-off, agarra/suelta el puntero real y reenvía
/// mouse/teclado al peer activo vía [`Routing`].
///
/// # Errors
///
/// Devuelve [`InputError`] si no se pudo abrir alguna de las dos
/// conexiones X11 que hacen falta (una para capturar eventos, otra para
/// las órdenes de control — ver [`ionconnect_input::x11::X11Control`]).
pub fn run_x11_input_session(
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Arc<Routing>,
    udp_peers: &Arc<UdpPeers>,
) -> Result<(), InputError> {
    let position = SharedPosition::new(0, 0);
    let mut capture = X11Capture::connect(position.clone())?;
    // La captura resincroniza la posición con el cursor real solo mientras
    // el control es local (ver `X11Capture::local_control`). Arranca en
    // `true` porque `HandoffState` arranca en `Active::Local`.
    let local_control = capture.local_control_flag();
    let control = X11Control::connect()?;
    let repeat = control.key_repeat_settings();
    info!(
        delay_ms = repeat.delay.as_millis(),
        interval_ms = repeat.interval.as_millis(),
        enabled = repeat.enabled,
        "captura de entrada X11 iniciada"
    );

    let (tx, rx) = std_mpsc::channel();
    let capture_thread = std::thread::spawn(move || {
        if let Err(err) = capture.run(tx) {
            warn!(%err, "el hilo de captura X11 terminó con error");
        }
    });

    let mut session = SessionState::new(repeat);
    loop {
        // Esperar sin plazo mientras no haya nada agendado (el caso normal:
        // ninguna tecla mantenida ni posición pendiente) mantiene este hilo
        // completamente dormido en vez de despertándolo a sondear.
        let event = match session.next_wakeup() {
            Some(timeout) => match rx.recv_timeout(timeout) {
                Ok(event) => Some(event),
                Err(std_mpsc::RecvTimeoutError::Timeout) => None,
                Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(event) => Some(event),
                Err(_) => break,
            },
        };

        if let Some(event) = event {
            handle_captured_event(
                event,
                handoff,
                &control,
                &position,
                routing,
                udp_peers,
                &mut session,
            );
        }
        emit_due_key_repeats(handoff, routing, &position, &mut session);
        session.flush_pending_move(routing, udp_peers);

        // Avisarle a la captura si el control sigue local o pasó a remoto,
        // para que resincronice (o no) con `query_pointer`.
        let is_local = matches!(
            handoff
                .lock()
                .expect("el lock de handoff no debería estar envenenado")
                .active(),
            Active::Local
        );
        local_control.store(is_local, Ordering::Relaxed);
    }

    let _ = capture_thread.join();
    Ok(())
}

/// Reenvía al equipo remoto activo las repeticiones de tecla que ya vencieron.
///
/// Estas repeticiones no vienen de la captura: el sistema operativo no las
/// entrega por el camino crudo que usa este proyecto, así que se sintetizan
/// acá (ver `crate::key_repeat` para el porqué y la medición). Si el control
/// volvió a ser local, no hay a quién mandarlas y se corta la repetición en
/// curso.
fn emit_due_key_repeats(
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Routing,
    position: &SharedPosition,
    session: &mut SessionState,
) {
    if session.repeater.next_deadline().is_none() {
        return;
    }
    let active = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .active();
    let Active::Remote(device) = active else {
        session.repeater.clear();
        return;
    };
    while let Some(keycode) = session.repeater.tick(Instant::now()) {
        forward_button_or_key(
            CapturedEvent::Key {
                keycode,
                modifiers: KeyModifiers::NONE,
                pressed: true,
            },
            device,
            position,
            routing,
            &mut session.forwarded,
        );
    }
}

/// Cuántos reportes de posición saltear entre cada línea de log — a la
/// tasa normal de un mouse (cientos de eventos/s) loguear todos inundaría
/// el panel de la GUI; cada ~30 alcanza para confirmar en vivo que la
/// captura sigue viva sin ahogar el resto del log.
const POSITION_LOG_SAMPLE_RATE: u32 = 30;
static POSITION_LOG_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Si el control está cedido a un peer que ya no tiene conexión registrada
/// en `routing` (se desconectó, crasheó, o se le mató el proceso), lo
/// recupera localmente — sin esto, un peer que desaparece mientras tiene
/// el control deja el mouse/teclado local muertos para siempre, porque
/// nada vuelve a disparar un cruce de borde. Se llama en cada evento
/// capturado, así que la detección es prácticamente instantánea mientras
/// haya algo de actividad (mover el mouse alcanza).
fn reclaim_if_peer_gone(
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
    session: &mut SessionState,
) {
    let now = Instant::now();
    if now.duration_since(session.last_liveness_check) < PEER_LIVENESS_CHECK_INTERVAL {
        return;
    }
    session.last_liveness_check = now;

    let device = match handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .active()
    {
        Active::Remote(device) => device,
        Active::Local => return,
    };
    if routing.is_connected(device) {
        return;
    }
    warn!(
        %device,
        "peer remoto desconectado con el control activo — recuperando control local"
    );
    let reclaimed = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .reclaim_if_remote(device);
    if reclaimed {
        session.on_control_changed();
        // El peer se fue: mandarle releases es inútil (no hay conexión) y su
        // propio cliente ya libera todo al terminar la sesión. Solo se
        // descarta el registro para no arrastrarlo si vuelve a conectar.
        session.forwarded.clear();
        let (x, y) = position.get();
        apply_handoff_action(
            HandoffAction::ReturnLocal { x, y },
            handoff,
            control,
            position,
            routing,
            (x, y),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_captured_event(
    event: CapturedEvent,
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
    udp_peers: &UdpPeers,
    session: &mut SessionState,
) {
    reclaim_if_peer_gone(handoff, control, position, routing, session);
    match event {
        CapturedEvent::AbsolutePosition { x, y } | CapturedEvent::MouseMove { x, y } => {
            // A nivel `debug` a propósito: con el mouse en movimiento esto
            // se dispara decenas de veces por segundo, y cada línea de log
            // es un viaje a journald que compite con el camino caliente.
            if POSITION_LOG_COUNTER.fetch_add(1, Ordering::Relaxed) % POSITION_LOG_SAMPLE_RATE == 0
            {
                debug!(x, y, ?event, "posición de mouse capturada");
            }
            handle_position_report(event, x, y, handoff, control, position, routing, udp_peers, session);
        }
        CapturedEvent::MouseButton { .. } | CapturedEvent::Key { .. } => {
            if !session.held.accept(&event) {
                return;
            }
            info!(?event, "botón/tecla capturado");
            // El estado de repetición se alimenta con la pulsación física
            // aunque el control sea local: así, si el hand-off ocurre con
            // una tecla ya mantenida, la repetición arranca con el mismo
            // retardo que tendría localmente.
            match event {
                CapturedEvent::Key {
                    keycode,
                    pressed: true,
                    ..
                } => session.repeater.on_press(keycode, Instant::now()),
                CapturedEvent::Key {
                    keycode,
                    pressed: false,
                    ..
                } => session.repeater.on_release(keycode),
                _ => {}
            }
            let active = handoff
                .lock()
                .expect("el lock de handoff no debería estar envenenado")
                .active();
            if let Active::Remote(device) = active {
                forward_button_or_key(event, device, position, routing, &mut session.forwarded);
            } else {
                info!("botón/tecla no reenviado: control sigue local");
            }
        }
    }
}

/// Solo el tipo de reporte que corresponde al estado actual es relevante:
/// posición absoluta (no cruda) mientras `Local` — el sistema operativo
/// sigue moviendo el cursor real, y es lo único fiable ahí; posición
/// acumulada (cruda) mientras `Remote` — ver la documentación de
/// [`ionconnect_input::x11::X11Capture`] para el porqué.
#[allow(clippy::too_many_arguments)]
fn handle_position_report(
    event: CapturedEvent,
    x: i32,
    y: i32,
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
    udp_peers: &UdpPeers,
    session: &mut SessionState,
) {
    let mut state = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado");

    // Mientras el control es local, X11 clava el cursor real en el último
    // píxel válido de la pantalla (nunca reporta `x >= width` ni `x < 0`),
    // así que `AbsolutePosition` sola nunca dispara un cruce de borde acá
    // — el usuario queda con el mouse pegado al borde para siempre.
    // Resincronizar `position` en cada reporte absoluto deja que, apenas
    // el cursor visual queda pinneado contra el borde, los deltas crudos
    // de `MouseMove` que siguen llegando (sin ese clamp, ver
    // `X11Capture`) se acumulen *desde ahí* y sí superen el límite.
    if state.active() == Active::Local
        && let CapturedEvent::AbsolutePosition { .. } = event
    {
        position.reset(x, y);
    }

    let relevant = matches!(
        (state.active(), event),
        (
            Active::Local,
            CapturedEvent::AbsolutePosition { .. } | CapturedEvent::MouseMove { .. }
        ) | (Active::Remote(_), CapturedEvent::MouseMove { .. })
    );
    if !relevant {
        return;
    }

    let previous_active = state.active();
    if let Some(action) = state.on_position(x, y) {
        drop(state);
        // Cambia el dueño del control: lo que quedara pendiente pertenece a
        // la etapa anterior y ya no corresponde mandarlo.
        session.on_control_changed();
        // ...y el peer que lo tenía necesita el `pressed: false` de todo lo
        // que le habíamos reenviado y sigue sin soltarse, antes de que el
        // nuevo dueño (local u otro peer) empiece a recibir.
        if let Active::Remote(previous_device) = previous_active {
            session.release_forwarded_to(previous_device, routing);
        }
        apply_handoff_action(action, handoff, control, position, routing, (x, y));
    } else if let Active::Remote(device) = state.active() {
        // Sin vecino enlazado en el borde que se acaba de cruzar (o
        // todavía dentro de límites): igual hay que pegar la posición al
        // escritorio del remoto, si no la acumulada sigue alejándose sin
        // límite — ver la documentación de `clamp_to_active_desktop`.
        let (x, y) = state.clamp_to_active_desktop(x, y);
        drop(state);
        position.reset(x, y);
        // Delta continuo (no el primer `MouseMove` de un hand-off, que va
        // por `apply_handoff_action` y siempre por TCP): tolera perderse y
        // se limita a [`MOUSE_SEND_INTERVAL`], el próximo lo reemplaza.
        session.send_move(device, x, y, routing, udp_peers);
    }
}

/// `local_xy` es la posición real en la pantalla *local* en el instante del
/// hand-off (no confundir con `x, y` dentro de `HandoffAction::ForwardTo`,
/// que ya están expresados en el escritorio del equipo remoto) — se usa
/// para clavar ahí el cursor real vía [`X11Control::grab`] y que no se
/// pasee por toda la pantalla local mientras el control ya es del remoto.
fn apply_handoff_action(
    action: HandoffAction,
    handoff: &Arc<Mutex<HandoffState>>,
    control: &X11Control,
    position: &SharedPosition,
    routing: &Routing,
    local_xy: (i32, i32),
) {
    match action {
        HandoffAction::ForwardTo { device, x, y } => {
            info!(%device, x, y, "hand-off: cediendo control a equipo remoto");
            if let Err(err) = control.grab(local_xy.0, local_xy.1) {
                // `HandoffState::on_position` ya marcó el estado como
                // `Remote` antes de esto (necesita decidir el hand-off
                // antes de saber si el grab real va a funcionar). Si el
                // grab falla — típicamente porque otro cliente X11 (p. ej.
                // GNOME Shell con su vista de actividades abierta) ya tiene
                // el puntero agarrado — el cursor real queda sin confinar
                // pero el estado interno sigue pensando que el control es
                // remoto, y arranca a reenviar movimiento igual: se ve
                // como si el mouse se moviera en las dos pantallas a la
                // vez. Sin este `reclaim_if_remote`, ese estado fantasma
                // queda pegado hasta el próximo cruce de borde real.
                warn!(%err, "no se pudo agarrar el puntero para el hand-off, revirtiendo a control local");
                handoff
                    .lock()
                    .expect("el lock de handoff no debería estar envenenado")
                    .reclaim_if_remote(device);
                return;
            }
            position.reset(x, y);
            if !routing.send_to(device, Message::MouseMove(MouseMove { x, y })) {
                warn!(%device, "hand-off disparado pero el peer no está conectado en routing");
            }
        }
        HandoffAction::ReturnLocal { x, y } => {
            info!(x, y, "hand-off: recuperando control local");
            if let Err(err) = control.ungrab() {
                warn!(%err, "no se pudo soltar el puntero al devolver el control");
            }
            if let Err(err) = control.warp_to(x, y) {
                warn!(%err, "no se pudo mover el cursor real al devolver el control");
            }
        }
    }
}

fn forward_button_or_key(
    event: CapturedEvent,
    device: DeviceId,
    position: &SharedPosition,
    routing: &Routing,
    forwarded: &mut ForwardedHeld,
) {
    let (x, y) = position.get();
    let message = match event {
        CapturedEvent::MouseButton { button, pressed } => Some(Message::MouseClick(MouseClick {
            button,
            pressed,
            x,
            y,
        })),
        CapturedEvent::Key {
            keycode,
            modifiers,
            pressed: true,
        } => Some(Message::KeyboardPress(KeyboardPress { keycode, modifiers })),
        CapturedEvent::Key {
            keycode,
            modifiers,
            pressed: false,
        } => Some(Message::KeyboardRelease(KeyboardRelease {
            keycode,
            modifiers,
        })),
        CapturedEvent::AbsolutePosition { .. } | CapturedEvent::MouseMove { .. } => None,
    };
    if let Some(message) = message {
        forwarded.track_forwarded(&event);
        routing.send_to(device, message);
    }
}

/// Corre la sesión de captura Wayland (portal `InputCapture` + `libei`) —
/// async, a diferencia de la de X11: acá es el compositor el que decide
/// cuándo se cruzó un borde (vía las barreras que configuramos), no
/// nosotros sondeando posición contra nuestra propia geometría.
///
/// Reutiliza la misma [`HandoffState`] que la sesión X11: en la
/// activación, alimenta la posición real del cursor que reporta el portal
/// a `on_position` — misma lógica pura de `screen::Layout`, solo cambia
/// quién dispara la primera llamada.
///
/// # Errors
///
/// Devuelve [`InputError`] si registrar las barreras, habilitar la
/// captura, o el stream de eventos EIS fallan.
pub async fn run_wayland_input_session(
    mut session: WaylandCaptureSession,
    barriers: Vec<BarrierSpec>,
    handoff: Arc<Mutex<HandoffState>>,
    routing: Arc<Routing>,
    udp_peers: Arc<UdpPeers>,
) -> Result<(), InputError> {
    let failed = session.set_barriers(&barriers).await?;
    if !failed.is_empty() {
        warn!(
            ?failed,
            "algunas barreras de hand-off fueron rechazadas por el compositor"
        );
    }
    session.enable().await?;
    info!("captura de entrada Wayland habilitada");

    // Handle separado (misma conexión D-Bus, no una nueva) para no pelear
    // por el préstamo de `&mut session` que hace falta más abajo — ver
    // la documentación de `WaylandCaptureSession::activation_watcher`.
    let watcher = session.activation_watcher().await?;
    let mut activated_stream = watcher.receive_activated().await?;
    let mut deactivated_stream = watcher.receive_deactivated().await?;
    let mut current_activation: Option<u32> = None;
    // El portal no expone el retardo/ritmo de repetición que tiene
    // configurado el usuario (a diferencia de X11, ver
    // `X11Control::key_repeat_settings`), así que acá se usan los valores
    // estándar.
    let mut session_state = SessionState::new(KeyRepeatSettings::default());

    // Red de seguridad para cuando el peer que tiene el control desaparece
    // (crash, se le mata el proceso, se corta la red) sin que eso dispare
    // un evento de captura — p. ej. si nadie está tocando el mouse en ese
    // momento. Sin este tick, `next_event` se queda esperando para siempre
    // y el control local nunca vuelve. Cada evento capturado también
    // dispara el mismo chequeo (ver `handle_wayland_input`), así que en la
    // práctica la recuperación es casi instantánea apenas hay actividad;
    // esto solo cubre el caso de inactividad total.
    let mut watchdog = tokio::time::interval(Duration::from_secs(1));

    loop {
        let repeat_deadline = session_state
            .next_wakeup()
            .unwrap_or(IDLE_WAKEUP);

        tokio::select! {
            _ = watchdog.tick() => {
                reclaim_if_peer_gone_wayland(
                    &handoff,
                    &routing,
                    &mut session,
                    &mut current_activation,
                    &mut session_state,
                )
                .await;
            }
            () = tokio::time::sleep(repeat_deadline) => {
                emit_due_key_repeats_wayland(&handoff, &routing, &mut session_state);
                session_state.flush_pending_move(&routing, &udp_peers);
            }
            event = session.next_event(&mut activated_stream, &mut deactivated_stream) => {
                let event = event?;
                handle_wayland_event(
                    event,
                    &mut session,
                    &handoff,
                    &routing,
                    &udp_peers,
                    &mut current_activation,
                    &mut session_state,
                )
                .await;
            }
        }
    }
}

/// Igual que [`emit_due_key_repeats`] pero para la sesión Wayland: las
/// repeticiones de teclado no llevan posición, así que no hace falta el
/// [`SharedPosition`] que sí usa el camino X11.
fn emit_due_key_repeats_wayland(
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Routing,
    state: &mut SessionState,
) {
    if state.repeater.next_deadline().is_none() {
        return;
    }
    let active = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .active();
    let Active::Remote(device) = active else {
        state.repeater.clear();
        return;
    };
    while let Some(keycode) = state.repeater.tick(Instant::now()) {
        state.forwarded.track_forwarded(&CapturedEvent::Key {
            keycode,
            modifiers: KeyModifiers::NONE,
            pressed: true,
        });
        routing.send_to(
            device,
            Message::KeyboardPress(KeyboardPress {
                keycode,
                modifiers: KeyModifiers::NONE,
            }),
        );
    }
}

async fn reclaim_if_peer_gone_wayland(
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Routing,
    session: &mut WaylandCaptureSession,
    current_activation: &mut Option<u32>,
    state: &mut SessionState,
) {
    let device = match handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .active()
    {
        Active::Remote(device) => device,
        Active::Local => return,
    };
    if routing.is_connected(device) {
        return;
    }
    warn!(
        %device,
        "peer remoto desconectado con el control activo (Wayland) — recuperando control local"
    );
    let reclaimed = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .reclaim_if_remote(device);
    if reclaimed {
        state.on_control_changed();
        // El peer se fue: sin conexión no hay a quién mandarle los releases,
        // y su cliente ya libera todo al terminar la sesión.
        state.forwarded.clear();
        let _ = session.release(*current_activation, None).await;
        *current_activation = None;
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_wayland_event(
    event: WaylandCaptureEvent,
    session: &mut WaylandCaptureSession,
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Routing,
    udp_peers: &UdpPeers,
    current_activation: &mut Option<u32>,
    state: &mut SessionState,
) {
    match event {
        WaylandCaptureEvent::Activated {
            activation_id,
            cursor,
            ..
        } => {
            *current_activation = activation_id;
            let Some((x, y)) = cursor else {
                warn!("activación sin posición de cursor reportada, ignorando");
                return;
            };
            #[allow(clippy::cast_possible_truncation)]
            let (x, y) = (x as i32, y as i32);
            info!(x, y, "captura Wayland activada");

            let action = handoff
                .lock()
                .expect("el lock de handoff no debería estar envenenado")
                .on_position(x, y);

            match action {
                Some(HandoffAction::ForwardTo { device, x, y }) => {
                    info!(%device, x, y, "hand-off: cediendo control a equipo remoto");
                    state.on_control_changed();
                    session.reset_position(x, y);
                    if !routing.send_to(device, Message::MouseMove(MouseMove { x, y })) {
                        warn!(%device, "hand-off disparado pero el peer no está conectado en routing");
                    }
                }
                Some(HandoffAction::ReturnLocal { .. }) | None => {
                    warn!(
                        "activación sin hand-off válido (¿barrera sin vecino configurado en ese borde?), liberando"
                    );
                    let _ = session.release(*current_activation, None).await;
                    *current_activation = None;
                }
            }
        }
        WaylandCaptureEvent::Deactivated { .. } => {
            info!("captura Wayland desactivada");
            *current_activation = None;
        }
        WaylandCaptureEvent::Input(captured) => {
            handle_wayland_input(
                captured,
                session,
                handoff,
                routing,
                udp_peers,
                current_activation,
                state,
            )
            .await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_wayland_input(
    event: CapturedEvent,
    session: &mut WaylandCaptureSession,
    handoff: &Arc<Mutex<HandoffState>>,
    routing: &Routing,
    udp_peers: &UdpPeers,
    current_activation: &mut Option<u32>,
    state: &mut SessionState,
) {
    reclaim_if_peer_gone_wayland(handoff, routing, session, current_activation, state).await;

    if matches!(
        event,
        CapturedEvent::MouseButton { .. } | CapturedEvent::Key { .. }
    ) && !state.held.accept(&event)
    {
        return;
    }

    // Ver el comentario equivalente en `handle_captured_event` (X11): el
    // estado de repetición se alimenta con la pulsación física.
    match event {
        CapturedEvent::Key {
            keycode,
            pressed: true,
            ..
        } => state.repeater.on_press(keycode, Instant::now()),
        CapturedEvent::Key {
            keycode,
            pressed: false,
            ..
        } => state.repeater.on_release(keycode),
        _ => {}
    }

    let active = handoff
        .lock()
        .expect("el lock de handoff no debería estar envenenado")
        .active();
    let Active::Remote(device) = active else {
        return;
    };

    match event {
        CapturedEvent::MouseMove { x, y } => {
            // A nivel `debug`: ver el comentario equivalente en el camino X11.
            if POSITION_LOG_COUNTER.fetch_add(1, Ordering::Relaxed) % POSITION_LOG_SAMPLE_RATE == 0
            {
                debug!(x, y, "posición de mouse capturada (Wayland)");
            }
            let mut guard = handoff
                .lock()
                .expect("el lock de handoff no debería estar envenenado");
            let action = guard.on_position(x, y);
            if let Some(HandoffAction::ReturnLocal { x, y }) = action {
                drop(guard);
                info!(x, y, "hand-off: recuperando control local");
                state.on_control_changed();
                // Soltar en el peer todo lo que le habíamos mandado como
                // presionado antes de devolver el control — si no, queda
                // pegado en el cliente (ver `ForwardedHeld`).
                state.release_forwarded_to(device, routing);
                let _ = session
                    .release(*current_activation, Some((f64::from(x), f64::from(y))))
                    .await;
                *current_activation = None;
            } else {
                // Sin vecino enlazado en ese borde (o todavía dentro de
                // límites): pegar la posición al escritorio del remoto
                // para que la acumulada no se aleje sin límite — ver
                // `HandoffState::clamp_to_active_desktop`.
                let (x, y) = guard.clamp_to_active_desktop(x, y);
                drop(guard);
                session.reset_position(x, y);
                // Delta continuo — tolera perderse y va limitado a
                // [`MOUSE_SEND_INTERVAL`], ver el comentario equivalente en
                // `handle_position_report` (X11).
                state.send_move(device, x, y, routing, udp_peers);
            }
        }
        CapturedEvent::MouseButton { button, pressed } => {
            info!(?button, pressed, "botón capturado (Wayland)");
            state.forwarded.track_forwarded(&event);
            let (x, y) = session.position();
            routing.send_to(
                device,
                Message::MouseClick(MouseClick {
                    button,
                    pressed,
                    x,
                    y,
                }),
            );
        }
        CapturedEvent::Key {
            keycode,
            modifiers,
            pressed: true,
        } => {
            info!(keycode, "tecla capturada (Wayland)");
            state.forwarded.track_forwarded(&event);
            routing.send_to(
                device,
                Message::KeyboardPress(KeyboardPress { keycode, modifiers }),
            );
        }
        CapturedEvent::Key {
            keycode,
            modifiers,
            pressed: false,
        } => {
            state.forwarded.track_forwarded(&event);
            routing.send_to(
                device,
                Message::KeyboardRelease(KeyboardRelease { keycode, modifiers }),
            );
        }
        CapturedEvent::AbsolutePosition { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ionconnect_shared::KeyModifiers;

    fn key(keycode: u32, pressed: bool) -> CapturedEvent {
        CapturedEvent::Key {
            keycode,
            modifiers: KeyModifiers::NONE,
            pressed,
        }
    }

    fn button(button: MouseButton, pressed: bool) -> CapturedEvent {
        CapturedEvent::MouseButton { button, pressed }
    }

    #[test]
    fn forwarded_held_emits_the_missing_release_for_everything_still_down() {
        let mut fwd = ForwardedHeld::default();
        fwd.track_forwarded(&key(42, true)); // Shift
        fwd.track_forwarded(&button(MouseButton::Left, true));
        fwd.track_forwarded(&key(30, true));
        fwd.track_forwarded(&key(30, false)); // esta ya se soltó

        let mut releases = fwd.drain_releases();
        releases.sort_by_key(|m| format!("{m:?}"));
        assert_eq!(
            releases,
            {
                let mut expected = vec![
                    Message::KeyboardRelease(KeyboardRelease {
                        keycode: 42,
                        modifiers: KeyModifiers::NONE,
                    }),
                    Message::MouseClick(MouseClick {
                        button: MouseButton::Left,
                        pressed: false,
                        x: 0,
                        y: 0,
                    }),
                ];
                expected.sort_by_key(|m| format!("{m:?}"));
                expected
            },
            "solo lo que sigue presionado, con su pressed:false"
        );
        assert!(
            fwd.drain_releases().is_empty(),
            "drenar vacía el conjunto — un segundo hand-off no re-libera"
        );
    }

    #[test]
    fn forwarded_held_ignores_scroll_notches() {
        let mut fwd = ForwardedHeld::default();
        fwd.track_forwarded(&button(MouseButton::ScrollUp, true));
        fwd.track_forwarded(&button(MouseButton::ScrollDown, true));
        assert!(
            fwd.drain_releases().is_empty(),
            "el scroll es una muesca instantánea, no algo que quede mantenido"
        );
    }

    #[test]
    fn drops_a_press_reported_twice_without_a_release_in_between() {
        let mut held = HeldGuard::default();
        assert!(held.accept(&key(30, true)), "primera pulsación: nueva");
        assert!(
            !held.accept(&key(30, true)),
            "segunda pulsación sin soltar antes: duplicado, se descarta"
        );
    }

    #[test]
    fn drops_a_release_reported_twice_without_a_press_in_between() {
        let mut held = HeldGuard::default();
        assert!(held.accept(&key(30, true)));
        assert!(held.accept(&key(30, false)), "primer release: nuevo");
        assert!(
            !held.accept(&key(30, false)),
            "segundo release sin volver a presionar: duplicado, se descarta"
        );
    }

    #[test]
    fn accepts_legitimate_repeated_taps_of_the_same_key() {
        let mut held = HeldGuard::default();
        // Tap, tap, tap — cada uno con su release intermedio, como escribir
        // la misma letra varias veces seguidas. No debería filtrarse nada.
        for _ in 0..3 {
            assert!(held.accept(&key(30, true)));
            assert!(held.accept(&key(30, false)));
        }
    }

    #[test]
    fn tracks_keys_and_buttons_independently() {
        let mut held = HeldGuard::default();
        assert!(held.accept(&key(30, true)));
        assert!(held.accept(&CapturedEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        }));
        // El botón repetido se descarta; la tecla, ya soltada aparte, sigue
        // pudiendo volver a presionarse — estados independientes.
        assert!(!held.accept(&CapturedEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        }));
        assert!(held.accept(&key(30, false)));
        assert!(held.accept(&key(30, true)));
    }

    #[test]
    fn always_accepts_motion_events() {
        let mut held = HeldGuard::default();
        assert!(held.accept(&CapturedEvent::MouseMove { x: 1, y: 1 }));
        assert!(held.accept(&CapturedEvent::MouseMove { x: 1, y: 1 }));
        assert!(held.accept(&CapturedEvent::AbsolutePosition { x: 1, y: 1 }));
    }
}
